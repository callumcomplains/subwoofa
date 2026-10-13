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
            let mut c = std::process::Command::new(p);
            c.arg("-version").stdin(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0800_0000);
            }
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
