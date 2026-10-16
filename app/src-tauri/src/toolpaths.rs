//! The FFmpeg and DTS-HD Master Audio Suite paths the user can point the app at, and what is found there.

use crate::blocking;
use sw_backend_ffmpeg as ff;

#[derive(serde::Serialize)]
pub struct ToolStatus {
    ffmpeg: Option<String>,
    ffmpeg_version: Option<String>,
    mas_dir: Option<String>,
    mas_files: Vec<(String, bool)>,
}

/// Use the ffmpeg and the DTS-HD Master Audio Suite folder the user chose (empty means look in the usual places).
#[tauri::command]
pub fn set_tool_paths(ffmpeg: String, mas_dir: String) {
    let opt = |s: String| (!s.trim().is_empty()).then(|| std::path::PathBuf::from(s.trim()));
    ff::set_ffmpeg_override(opt(ffmpeg));
    sw_verify::set_mas_dir(opt(mas_dir));
}

/// What the app finds with the current paths, for the Settings page.
#[tauri::command]
pub async fn tool_status() -> ToolStatus {
    blocking(move || {
        let ffmpeg = ff::find_ffmpeg();
        let version = ffmpeg.as_ref().and_then(|p| {
            let mut c = sw_core::process::command(p);
            c.arg("-version").stdin(std::process::Stdio::null());
            c.output()
                .ok()
                .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().map(str::to_string))
        });
        let dir = sw_verify::mas_dir();
        let files = ["DTSHDVerify.exe", "DtsJobQueue.exe", "dtshd.exe"]
            .iter()
            .map(|n| (n.to_string(), dir.as_ref().is_some_and(|d| d.join(n).is_file())))
            .collect();
        Ok(ToolStatus {
            ffmpeg: ffmpeg.map(|p| p.display().to_string()),
            ffmpeg_version: version,
            mas_dir: dir.map(|d| d.display().to_string()),
            mas_files: files,
        })
    })
    .await
    .unwrap_or(ToolStatus {
        ffmpeg: None,
        ffmpeg_version: None,
        mas_dir: None,
        mas_files: vec![],
    })
}

/// The download script: next to the program when installed, or in the source tree when run from a build folder.
fn fetch_script() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().skip(1).find_map(|dir| {
        [
            dir.join("fetch-ffmpeg.ps1"),
            dir.join("app/src-tauri/installer/fetch-ffmpeg.ps1"),
        ]
        .into_iter()
        .find(|p| p.is_file())
    })
}

/// Downloads FFmpeg into Subwoofa's own folder (the same step the installer offers) and says how it went.
#[tauri::command]
pub async fn fetch_ffmpeg() -> Result<String, String> {
    crate::blocking(|| {
        let script = fetch_script().ok_or("The download script was not found next to the program.")?;
        let mut cmd = sw_core::process::command("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script);
        let out = cmd
            .output()
            .map_err(|e| format!("PowerShell could not be started: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() {
            Ok(text.lines().last().unwrap_or("FFmpeg is ready.").to_string())
        } else {
            Err(text.lines().last().unwrap_or("FFmpeg could not be set up.").to_string())
        }
    })
    .await
}
