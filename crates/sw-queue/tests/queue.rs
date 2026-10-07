use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sw_queue::{EncodeSpec, Queue, State};

fn have_ffmpeg() -> bool {
    sw_backend_ffmpeg::find_ffmpeg().is_some()
}

/// Write a mono 24-bit WAV of deterministic pseudo-noise-plus-tone.
fn write_wav(path: &Path, rate: u32, secs: f64, seed: u32) {
    let n = (rate as f64 * secs) as usize;
    let mut data = Vec::with_capacity(n * 3);
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    for i in 0..n {
        x ^= x << 13; x ^= x >> 17; x ^= x << 5;
        let noise = ((x >> 8) as i32 - (1 << 23)) / 40; // quiet noise
        let tone = ((i as f64 * 2.0 * std::f64::consts::PI * (200.0 + seed as f64 * 90.0) / rate as f64).sin() * 2_000_000.0) as i32;
        let v = (noise + tone).clamp(-8_388_608, 8_388_607);
        data.extend_from_slice(&v.to_le_bytes()[..3]);
    }
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"RIFF").unwrap();
    f.write_all(&((36 + data.len()) as u32).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&(rate * 3).to_le_bytes()).unwrap();
    f.write_all(&3u16.to_le_bytes()).unwrap();
    f.write_all(&24u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
    f.write_all(&data).unwrap();
}

fn stems(dir: &Path, secs: f64) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for (i, ch) in ["L", "R", "C", "LFE", "Ls", "Rs"].iter().enumerate() {
        let p = dir.join(format!("S_{ch}.wav"));
        write_wav(&p, 48_000, secs, i as u32 + 1);
        m.insert(ch.to_string(), p.to_string_lossy().into_owned());
    }
    m
}

fn spec(dir: &Path, name: &str, stream: &str, inputs: &HashMap<String, String>) -> EncodeSpec {
    EncodeSpec {
        dest: "generic".into(), stream: stream.into(), layout: "5.1".into(), sample_rate: 48_000,
        bitrate: if stream == "ac3" { Some(448) } else { None }, dialnorm: Some(31), start: None, duration: None,
        inputs: inputs.clone(), output: dir.join(name).to_string_lossy().into_owned(), unlocked: false, engine: String::new(),
    }
}

fn wait_until(q: &Queue, f: impl Fn(&[sw_queue::JobView]) -> bool) -> Vec<sw_queue::JobView> {
    let end = Instant::now() + Duration::from_secs(60);
    loop {
        let s = q.snapshot();
        if f(&s) { return s; }
        assert!(Instant::now() < end, "timed out: {s:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn tmpdir(n: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sw_q_{n}_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn runs_jobs_verifies_lossless_and_writes_logs() {
    if !have_ffmpeg() { eprintln!("ffmpeg missing, skipping"); return; }
    let dir = tmpdir("run");
    let inputs = stems(&dir, 3.0);
    let updates = Arc::new(Mutex::new(0usize));
    let u = updates.clone();
    let q = Queue::new(2, Arc::new(move |_| *u.lock().unwrap() += 1));
    q.enqueue(spec(&dir, "a.ac3", "ac3", &inputs), false).unwrap();
    q.enqueue(spec(&dir, "b.thd", "truehd", &inputs), true).unwrap();
    // a job that must fail validation (AC-3 cannot carry 7.1)
    let mut bad = spec(&dir, "c.ac3", "ac3", &inputs);
    bad.layout = "7.1-side-rear".into();
    q.enqueue(bad, false).unwrap();

    let s = wait_until(&q, |s| s.len() == 3 && s.iter().all(|j| j.state.is_final()));
    assert_eq!(s[0].state, State::Completed, "{:?}", s[0]);
    assert_eq!(s[1].state, State::Passed, "{:?}", s[1]);
    assert!(s[1].message.contains("lossless verified"), "{}", s[1].message);
    assert_eq!(s[2].state, State::Failed);
    assert!(s[2].message.contains("invalid combination"), "{}", s[2].message);
    assert!(dir.join("a.ac3").metadata().unwrap().len() > 1000);
    assert!(std::fs::read_to_string(dir.join("b_log.txt")).unwrap().contains("lossless verified"));
    assert!(std::fs::read_to_string(dir.join("c_log.txt")).unwrap().contains("FAILED"));
    assert!(*updates.lock().unwrap() > 6, "queue should push updates");
    q.shutdown();
}

#[test]
fn native_engine_runs_in_the_queue() {
    if !have_ffmpeg() { eprintln!("ffmpeg missing, skipping"); return; }
    let dir = tmpdir("native");
    let inputs = stems(&dir, 4.0);
    let q = Queue::new(1, Arc::new(|_| {}));
    let mut s = spec(&dir, "n.ac3", "ac3", &inputs);
    s.engine = "native".into();
    s.bitrate = Some(448);
    q.enqueue(s, true).unwrap();
    let snap = wait_until(&q, |s| s.iter().all(|j| j.state.is_final()));
    assert_eq!(snap[0].state, State::Passed, "{:?}", snap[0]);
    assert!(std::fs::read_to_string(dir.join("n_log.txt")).unwrap().contains("Subwoofa native encoder"));
    // E-AC-3 through the native engine, at a bit rate AC-3 cannot express
    let mut e = spec(&dir, "n.eac3", "eac3", &inputs);
    e.engine = "native".into();
    e.bitrate = Some(300);
    q.enqueue(e, true).unwrap();
    let snap = wait_until(&q, |s| s.len() == 2 && s.iter().all(|j| j.state.is_final()));
    assert_eq!(snap[1].state, State::Passed, "{:?}", snap[1]);
    // lossless TrueHD through the native engine: verification compares MD5 against the source
    let mut t = spec(&dir, "n.thd", "truehd", &inputs);
    t.engine = "native".into();
    q.enqueue(t, true).unwrap();
    let snap = wait_until(&q, |s| s.len() == 3 && s.iter().all(|j| j.state.is_final()));
    assert_eq!(snap[2].state, State::Passed, "{:?}", snap[2]);
    assert!(snap[2].message.contains("lossless verified"), "{}", snap[2].message);
    // native engine refuses formats it cannot make
    let mut bad = spec(&dir, "m.mlp", "mlp", &inputs);
    bad.engine = "native".into();
    q.enqueue(bad, false).unwrap();
    let snap = wait_until(&q, |s| s.len() == 4 && s.iter().all(|j| j.state.is_final()));
    assert_eq!(snap[3].state, State::Failed);
    assert!(snap[3].message.contains("native engine"), "{}", snap[3].message);
    q.shutdown();
}

#[test]
fn cancel_pending_and_running() {
    if !have_ffmpeg() { eprintln!("ffmpeg missing, skipping"); return; }
    let dir = tmpdir("cancel");
    let inputs = stems(&dir, 40.0); // long enough that TrueHD takes a moment
    let q = Queue::new(1, Arc::new(|_| {}));
    let first = q.enqueue(spec(&dir, "long.thd", "truehd", &inputs), false).unwrap();
    let second = q.enqueue(spec(&dir, "never.ac3", "ac3", &inputs), false).unwrap();
    wait_until(&q, |s| s[0].state == State::Running);
    q.cancel(second);               // still pending: concurrency is 1
    q.cancel(first);                // running: ffmpeg must be killed
    let s = wait_until(&q, |s| s.iter().all(|j| j.state.is_final()));
    assert_eq!(s[0].state, State::Canceled);
    assert_eq!(s[1].state, State::Canceled);
    q.shutdown();
}

#[test]
fn concurrency_limit_is_respected() {
    if !have_ffmpeg() { eprintln!("ffmpeg missing, skipping"); return; }
    let dir = tmpdir("conc");
    let inputs = stems(&dir, 20.0);
    let q = Queue::new(1, Arc::new(|_| {}));
    for i in 0..3 {
        q.enqueue(spec(&dir, &format!("j{i}.thd"), "truehd", &inputs), false).unwrap();
    }
    let end = Instant::now() + Duration::from_secs(90);
    let mut max_running = 0;
    loop {
        let s = q.snapshot();
        max_running = max_running.max(s.iter().filter(|j| matches!(j.state, State::Running | State::Verifying)).count());
        if s.iter().all(|j| j.state.is_final()) { break; }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(max_running, 1);
    q.shutdown();
}
