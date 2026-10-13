//! The job queue and the log files of finished jobs.

use sw_queue::{EncodeSpec, Queue};

#[tauri::command]
pub fn queue_add(queue: tauri::State<Queue>, spec: EncodeSpec, verify: bool) -> Result<u64, String> {
    queue.enqueue(spec, verify)
}
#[tauri::command]
pub fn queue_list(queue: tauri::State<Queue>) -> Vec<sw_queue::JobView> {
    queue.snapshot()
}
#[tauri::command]
pub fn queue_cancel(queue: tauri::State<Queue>, id: u64) {
    queue.cancel(id)
}
#[tauri::command]
pub fn queue_remove(queue: tauri::State<Queue>, id: u64) {
    queue.remove(id)
}
#[tauri::command]
pub fn queue_clear(queue: tauri::State<Queue>) {
    queue.clear_finished()
}
#[tauri::command]
pub fn queue_set_concurrency(queue: tauri::State<Queue>, n: usize) {
    queue.set_concurrency(n)
}
#[tauri::command]
pub fn write_text(path: String, text: String) -> Result<(), String> {
    std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))
}

#[tauri::command]
pub fn read_log(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))
}
