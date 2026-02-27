use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use walkdir::WalkDir;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Serialize, Deserialize)]
pub struct FolderInfo {
    pub name: String,
    pub path: String,
    pub size: String,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
    pub duration: f64,
    pub output_size: String,
}

fn find_zstar_exe() -> String {
    // 1. 先找当前 exe 所在目录
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(dir) = exe_path.parent() {
            let zstar_in_dir = dir.join("zstar.exe");
            if zstar_in_dir.exists() {
                return zstar_in_dir.to_string_lossy().to_string();
            }
        }
    }

    // 2. 找同目录 (GUI 启动目录)
    if Path::new("zstar.exe").exists() {
        return "zstar.exe".to_string();
    }

    // 3. 从 PATH 环境变量中查找
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(';') {
            let p = Path::new(dir).join("zstar.exe");
            if p.exists() {
                return p.to_string_lossy().to_string();
            }
        }
    }

    // 4. 都找不到，返回默认路径
    "zstar.exe".to_string()
}

fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let k = 1024.0;
    let sizes = ["B", "KB", "MB", "GB", "TB"];
    let i = (bytes as f64).log(k).floor() as usize;
    let size = bytes as f64 / k.powi(i as i32);
    format!("{:.2} {}", size, sizes[i.min(sizes.len() - 1)])
}

fn get_folder_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

#[tauri::command]
async fn check_zstar() -> Result<serde_json::Value, String> {
    let exe_path = find_zstar_exe();
    let exists = Path::new(&exe_path).exists();
    Ok(serde_json::json!({
        "path": exe_path,
        "exists": exists
    }))
}

#[tauri::command]
async fn get_folder_info(path: String) -> Result<FolderInfo, String> {
    let path_obj = Path::new(&path);

    if !path_obj.exists() {
        return Err("Path does not exist".to_string());
    }

    if !path_obj.is_dir() {
        return Err("Path is not a directory".to_string());
    }

    let name = path_obj
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let size_bytes = get_folder_size(path_obj);
    let size = format_size(size_bytes);

    Ok(FolderInfo {
        name,
        path: path.clone(),
        size,
        size_bytes,
    })
}

#[tauri::command]
async fn pack_folder(
    source_path: String,
    output_path: String,
    level: Option<u32>,
    threads: Option<u32>,
    ignore_failed_read: Option<bool>,
    no_long: Option<bool>,
) -> Result<PackResult, String> {
    let zstar_exe = find_zstar_exe();

    if !Path::new(&zstar_exe).exists() {
        return Err(format!("zstar.exe not found at: {}", zstar_exe));
    }

    if !Path::new(&source_path).exists() {
        return Err("Source path does not exist".to_string());
    }

    let mut args = vec!["pack".to_string(), source_path.clone(), "-o".to_string(), output_path.clone()];

    if let Some(l) = level {
        args.push("--level".to_string());
        args.push(l.to_string());
    }

    if let Some(t) = threads {
        args.push("--threads".to_string());
        args.push(t.to_string());
    }

    if ignore_failed_read.unwrap_or(false) {
        args.push("--ignore-failed-read".to_string());
    }

    if no_long.unwrap_or(false) {
        args.push("--no-long".to_string());
    }

    let start = std::time::Instant::now();

    let mut cmd = tokio::process::Command::new(&zstar_exe);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    let mut stdout = String::new();
    let mut stderr = String::new();

    if let Some(ref mut out) = child.stdout {
        out.read_to_string(&mut stdout).await.map_err(|e| e.to_string())?;
    }

    if let Some(ref mut err) = child.stderr {
        err.read_to_string(&mut stderr).await.map_err(|e| e.to_string())?;
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    let duration = start.elapsed().as_secs_f64();

    let output_size = if Path::new(&output_path).exists() {
        let size = std::fs::metadata(&output_path)
            .map(|m| m.len())
            .unwrap_or(0);
        format_size(size)
    } else {
        "N/A".to_string()
    };

    if status.success() {
        Ok(PackResult {
            success: true,
            output: stdout,
            error: None,
            duration,
            output_size,
        })
    } else {
        Ok(PackResult {
            success: false,
            output: stdout,
            error: Some(stderr),
            duration,
            output_size,
        })
    }
}

#[tauri::command]
async fn unpack_folder(
    archive_path: String,
    output_path: String,
    threads: Option<u32>,
) -> Result<PackResult, String> {
    let zstar_exe = find_zstar_exe();

    if !Path::new(&zstar_exe).exists() {
        return Err(format!("zstar.exe not found at: {}", zstar_exe));
    }

    if !Path::new(&archive_path).exists() {
        return Err("Archive path does not exist".to_string());
    }

    let mut args = vec!["unpack".to_string(), archive_path.clone(), "-o".to_string(), output_path.clone()];

    if let Some(t) = threads {
        args.push("--threads".to_string());
        args.push(t.to_string());
    }

    let start = std::time::Instant::now();

    let mut cmd = tokio::process::Command::new(&zstar_exe);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    let mut stdout = String::new();
    let mut stderr = String::new();

    if let Some(ref mut out) = child.stdout {
        out.read_to_string(&mut stdout).await.map_err(|e| e.to_string())?;
    }

    if let Some(ref mut err) = child.stderr {
        err.read_to_string(&mut stderr).await.map_err(|e| e.to_string())?;
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    let duration = start.elapsed().as_secs_f64();

    if status.success() {
        Ok(PackResult {
            success: true,
            output: stdout,
            error: None,
            duration,
            output_size: "N/A".to_string(),
        })
    } else {
        Ok(PackResult {
            success: false,
            output: stdout,
            error: Some(stderr),
            duration,
            output_size: "N/A".to_string(),
        })
    }
}

#[tauri::command]
async fn minimize_window(window: tauri::Window) -> Result<(), String> {
    window.minimize().map_err(|e| e.to_string())
}

#[tauri::command]
async fn maximize_window(window: tauri::Window) -> Result<(), String> {
    if window.is_maximized().unwrap_or(false) {
        window.unmaximize().map_err(|e| e.to_string())
    } else {
        window.maximize().map_err(|e| e.to_string())
    }
}

#[tauri::command]
async fn close_window(window: tauri::Window) -> Result<(), String> {
    window.close().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            check_zstar,
            get_folder_info,
            pack_folder,
            unpack_folder,
            minimize_window,
            maximize_window,
            close_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
