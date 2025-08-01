use super::compression::{compress_files, decompress_files_with_progress, CompressionType};
use anyhow::Result;
use std::ffi::c_void;
use std::path::{PathBuf, Path};
use std::thread;
use std::time::Duration;
use std::sync::{Arc, Mutex};
use tauri::{Manager, App, AppHandle, generate_context, WebviewWindow, Emitter, Runtime, Window, Listener};
use serde::{Serialize, Deserialize};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
//use tauri_plugin_cli::CliExt;
//use tauri_plugin_shell::ShellExt;
use sysinfo::{System, Process, Signal};
use crate::GuiState;

#[derive(Clone, Serialize)]
pub struct CompressionProgressUpdate {
    progress: f64,
    current_file: String,
    total_files: usize,
    current_file_index: usize,
    operation: String, // "compressing" or "extracting"
}

fn count_processes_by_name(name: &str) -> usize {
    let mut sys = System::new_all();
    sys.refresh_processes();

    sys.processes()
        .values()
        .filter(|proc| proc.name().eq_ignore_ascii_case(name))
        .count()
}

fn kill_processes_by_name(name: &str) {
    let mut sys = System::new_all();
    sys.refresh_processes();

    for process in sys.processes().values() {
        if process.name().eq_ignore_ascii_case(name) {
            println!("Killing process: {} (PID: {})", process.name(), process.pid());
            let _ = process.kill_with(Signal::Kill); // or Signal::Term
        }
    }
}


#[tauri::command]
async fn close_all() {
	kill_processes_by_name("TauZip.exe");
}

#[tauri::command]
async fn count_now(count: usize, state: tauri::State<'_, Arc<GuiState>>) -> Result<(), String> {
	*state.count_now.lock().unwrap() = count;
	return Ok(());
}

#[tauri::command]
async fn compress_files_command(
    window: tauri::Window,
    files: Vec<String>, 
    outputfile: String, 
    compressiontype: String,
	state: tauri::State<'_, Arc<GuiState>>
) -> Result<String, String> {
    println!("Compression request received - files: {:?}, output: {}, type: {}", 
             files, outputfile, compressiontype);
    
	// let current = state.fetch_add(0, Ordering::SeqCst);
	let count = count_processes_by_name("TauZip.exe");
	if count > 1 {
		return Err("multiple instance of apps detected".to_string());
	}
	
    // Convert string to CompressionType enum
    let compression_enum = match compressiontype.as_str() {
        "Zip" => CompressionType::Zip,
        "TarGz" => CompressionType::TarGz,
        "TarBr" => CompressionType::TarBr,
        "Gz" => CompressionType::Gz,
        "Br" => CompressionType::Br,
        "Gzip" => CompressionType::Gzip,
        "Bzip2" => CompressionType::Bzip2,
        _ => return Err(format!("Unsupported compression type: {}", compressiontype)),
    };
    
    // Convert string paths back to PathBuf
    let file_paths: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(f)).collect();
    
    // Construct the full output path
    let output_path = if std::path::Path::new(&outputfile).is_absolute() {
        // If it's already an absolute path, use it as-is
        PathBuf::from(&outputfile)
    } else {
        // If it's a relative path, use the directory of the first file
        if !file_paths.is_empty() {
            let first_file_dir = file_paths[0]
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            first_file_dir.join(&outputfile)
        } else {
            PathBuf::from(&outputfile)
        }
    };
    
    println!("Output path resolved to: {}", output_path.display());
    
    // Use the new progress version
    use super::compression::compress_files_with_progress;
    
    compress_files_with_progress(&file_paths, &output_path, compression_enum, |progress, current_filename| {
        let progress_update = CompressionProgressUpdate {
            progress,
            current_file: current_filename,
            total_files: file_paths.len(),
            current_file_index: 1,
            operation: "compressing".to_string(),
        };
        let _ = window.app_handle().emit("compression-progress", &progress_update);
    })
    .await
    .map_err(|e| {
        let error_msg = format!("Compression failed: {}", e);
        println!("{}", error_msg);
        error_msg
    })?;
    
    // Final progress update
    let final_progress = CompressionProgressUpdate {
        progress: 100.0,
        current_file: "Complete".to_string(),
        total_files: 1,
        current_file_index: 1,
        operation: "compressing".to_string(),
    };
    let _ = window.emit("compression-progress", &final_progress);
    
    let success_msg = format!("Files compressed successfully to: {}", output_path.display());
    println!("{}", success_msg);
    Ok(success_msg)
}

#[tauri::command]
async fn decompress_files_command(
    window: tauri::Window,
    files: Vec<String>
) -> Result<String, String> {
    println!("Decompression request received - files: {:?}", files);
    
    let file_paths: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(f)).collect();
    let total_files = file_paths.len();
    
    let mut decompressed_to = Vec::new();
    
    for (index, file_path) in file_paths.iter().enumerate() {
        // Generate output directory for this file
        let output_dir = generate_output_dir(file_path);
        
        // Update progress
        let progress = CompressionProgressUpdate {
            progress: (index as f64 / total_files as f64) * 100.0,
            current_file: file_path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            total_files,
            current_file_index: index + 1,
            operation: "extracting".to_string(),
        };
        
        let _ = window.emit("compression-progress", &progress);
        
        // Decompress the file
        match decompress_files_with_progress(file_path, &output_dir, |file_progress, current_filename| {
            // Create a more detailed progress update
            let detailed_progress = CompressionProgressUpdate {
                progress: ((index as f64 + file_progress / 100.0) / total_files as f64) * 100.0,
                current_file: current_filename,
                total_files,
                current_file_index: index + 1,
                operation: "extracting".to_string(),
            };
            let _ = window.emit("compression-progress", &detailed_progress);
        }).await {
            Ok(_) => {
                decompressed_to.push(output_dir.display().to_string());
                println!("File decompressed to: {}", output_dir.display());
            },
            Err(e) => {
                let error_msg = format!("Failed to decompress '{}': {}", file_path.display(), e);
                println!("{}", error_msg);
                return Err(error_msg);
            }
        }
    }
    
    // Final progress update
    let final_progress = CompressionProgressUpdate {
        progress: 100.0,
        current_file: "Complete".to_string(),
        total_files,
        current_file_index: total_files,
        operation: "extracting".to_string(),
    };
    let _ = window.app_handle().emit("compression-progress", &final_progress);
    
    let success_msg = if decompressed_to.len() == 1 {
        format!("File decompressed successfully to: {}", decompressed_to[0])
    } else {
        format!("Files decompressed successfully. {} archives processed.", decompressed_to.len())
    };
    
    println!("{}", success_msg);
    Ok(success_msg)
}

#[tauri::command]
async fn get_compression_types() -> Vec<String> {
    vec![
        "Zip".to_string(),
        "TarGz".to_string(),
        "TarBr".to_string(),
        "Gz".to_string(),
        "Br".to_string(),
        "Gzip".to_string(),
        "Bzip2".to_string(),
    ]
}

#[tauri::command]
async fn validate_compression_type(files: Vec<String>, compressiontype: String) -> Result<bool, String> {
    // Convert string to CompressionType enum
    let compression_enum = match compressiontype.as_str() {
        "Zip" => CompressionType::Zip,
        "TarGz" => CompressionType::TarGz,
        "TarBr" => CompressionType::TarBr,
        "Gz" => CompressionType::Gz,
        "Br" => CompressionType::Br,
        "Gzip" => CompressionType::Gzip,
        "Bzip2" => CompressionType::Bzip2,
        _ => return Err(format!("Unsupported compression type: {}", compressiontype)),
    };
    
    if !compression_enum.supports_multiple_files() && files.len() > 1 {
        return Ok(false);
    }
    Ok(true)
}

#[tauri::command]
fn close(app: tauri::AppHandle) -> Result<(), String> {
	let count = count_processes_by_name("TauZip.exe");
	//if count > 1 {
		kill_processes_by_name("TauZip.exe");
		return Ok(());
	//}
    //if let Some(window) = app.get_webview_window("main") {
    //    let _ = window.close(); // or .unwrap() if you want to panic on error
	//	return Ok(());
    //}
	//return Err("Unable to close window".to_string());
}

#[tauri::command]
async fn open_file_location(file_path: String) -> Result<(), String> {
    let path = PathBuf::from(&file_path);
    
    println!("Opening file location for: {}", file_path);
    
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg("/select,")
            .arg(&file_path)
            .spawn()
            .map_err(|e| format!("Failed to open explorer: {}", e))?;
    }
    
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&file_path)
            .spawn()
            .map_err(|e| format!("Failed to open finder: {}", e))?;
    }
    
    #[cfg(target_os = "linux")]
    {
        let parent_dir = path.parent()
            .ok_or_else(|| "Could not determine parent directory".to_string())?;
        
        // Try different file managers
        let file_managers = ["nautilus", "dolphin", "thunar", "nemo", "pcmanfm"];
        let mut opened = false;
        
        for fm in &file_managers {
            if let Ok(_) = std::process::Command::new(fm)
                .arg(parent_dir)
                .spawn()
            {
                opened = true;
                break;
            }
        }
        
        if !opened {
            return Err("No supported file manager found".to_string());
        }
    }
    
    Ok(())
}

pub fn run_app(app: &AppHandle, mut file_strings2: Vec<String>, argv: Vec<String>, gui_state: Arc<GuiState>) {
	let log = false;
	if log { std::fs::write("aa.txt", format!("run_app")); }
	
	let file_args: Vec<String> = argv.into_iter().skip(2).collect();
	file_strings2.extend(file_args);
	
	if log { std::fs::write("aa.txt", format!("file_strings2 {:?}", file_strings2)); }
	
	let c = *&file_strings2.len();
	let app2 = app.clone();
	let app3 = app.clone();
	let window = gui_state.window_count.clone();
	let item2 = gui_state.item_count.clone();
	let item3 = gui_state.item_count.clone();
	let count_now_clone = gui_state.count_now.clone();
	let count_now_clone2 = gui_state.count_now.clone();
	let arg_received_clone = gui_state.arg_received.clone();
	let arg_received_clone2 = gui_state.arg_received.clone();
	let arg_received_clone3 = gui_state.arg_received.clone();
	let mut total_arg = 0;
	{
		*arg_received_clone3.lock().unwrap() += c;
		total_arg = *arg_received_clone3.lock().unwrap();
	}
	
	thread::spawn(move || {
		let mut count = window.fetch_add(0, Ordering::SeqCst);
		while count <= 0 {
			count = window.fetch_add(0, Ordering::SeqCst);
			thread::sleep(Duration::from_millis(100));
		}
		count = window.fetch_add(0, Ordering::SeqCst);
		if count <= 3 {
			thread::sleep(Duration::from_millis(500));
		}
		
		match app2.emit("files-selected", file_strings2) {
			Ok(_) => {
				item2.fetch_add(c, Ordering::SeqCst);
				//let s = Path::new(&c);
				//if log { std::fs::write(format!("aa {}.txt", s.file_name().unwrap().to_string_lossy()), format!("emit success {:?}", s)); }
				//println!("Successfully emitted files-selected event with {} files", file_strings3.len())
			},
			Err(e) => {
				//let s = Path::new(&c);
				//if log { std::fs::write(format!("aa {}.txt", s.file_name().unwrap().to_string_lossy()), format!("emit fail {:?}", file_strings4)); }
				//println!("Failed to emit files-selected event: {}", e)
			},
		}
		// let mut count1 = item3.fetch_add(0, Ordering::SeqCst);
		// while true {
			// thread::sleep(Duration::from_millis(1200));
			// let count2 = item3.fetch_add(0, Ordering::SeqCst);
			// if count1 == count2 {
				// app2.emit("enable-ok", "");
				// break;
			// }
			// count1 = count2;
		// }
	});
	
	//if enable_thread 
	// {
		// thread::spawn(move || {
			// while true {
				// let mut c = 0;
				// {
					// c = *count_now_clone2.lock().unwrap();
				// }
				// thread::sleep(Duration::from_millis(1000));
				// if total_arg == c {
					// app3.emit("enable-ok", "");
					// break;
				// }
			// }
		// });
	// }
}

pub fn run_decom_app(app: &AppHandle, mut file_strings2: Vec<String>, argv: Vec<String>, gui_state: Arc<GuiState>) {
	let log = false;
	if log { std::fs::write("aa.txt", "got main decom"); }
		
	let file_args: Vec<String> = argv.into_iter().skip(2).collect();
	file_strings2.extend(file_args);
		
	if log { std::fs::write("b.txt", format!("file_strings2 decom 3 {:?}", &file_strings2)); }
	
	let c = *&file_strings2.len();
	let app2 = app.clone();
	let app3 = app.clone();
	let window = gui_state.window_count.clone();
	let item2 = gui_state.item_count.clone();
	let item3 = gui_state.item_count.clone();
	let count_now_clone = gui_state.count_now.clone();
	let count_now_clone2 = gui_state.count_now.clone();
	let arg_received_clone = gui_state.arg_received.clone();
	let arg_received_clone2 = gui_state.arg_received.clone();
	let arg_received_clone3 = gui_state.arg_received.clone();
	let mut total_arg = 0;
	{
		*arg_received_clone3.lock().unwrap() += c;
		total_arg = *arg_received_clone3.lock().unwrap();
	}
	
	thread::spawn(move || {
		let mut count = window.fetch_add(0, Ordering::SeqCst);;
		while count <= 0 {
			count = window.fetch_add(0, Ordering::SeqCst);
			thread::sleep(Duration::from_millis(100));
		}
		count = window.fetch_add(0, Ordering::SeqCst);
		if count <= 3 {
			thread::sleep(Duration::from_millis(500));
		}
		
		match app2.emit("set-mode", "decompression") {
			Ok(_) => println!("Successfully set decompression mode"),
			Err(e) => println!("Failed to set decompression mode: {}", e),
		}
		match app2.emit("archives-selected", file_strings2) {
			Ok(_) => {
				item2.fetch_add(c, Ordering::SeqCst);
				//let s = Path::new(&c);
				//if log { std::fs::write(format!("ba {}.txt", s.file_name().unwrap().to_string_lossy()), format!("emit decom success {:?}", s)); }
				//println!("Successfully emitted files-selected event with {} files", file_strings3.len())
			},
			Err(e) => {
				//let s = Path::new(&c);
				//if log { std::fs::write(format!("ba {}.txt", s.file_name().unwrap().to_string_lossy()), format!("emit decom fail {:?}", file_strings4)); }
				//println!("Failed to emit files-selected event: {}", e)
			},
		}
	});
}


pub async fn run_compression_dialog(file_strings: Vec<String>, files: Vec<PathBuf>, gui_state: Arc<GuiState>) -> Result<()> {
    println!("Starting Tauri compression app with {} files", files.len());
    
	let log = false;
	let file_strings2 = file_strings.clone();
	let file_strings2b = file_strings.clone();
    if log { std::fs::write("a.txt", "before"); }
	
	let item_clone = gui_state.item_count.clone();
	let item_clone2 = gui_state.item_count.clone();
	let item_clone3 = gui_state.item_count.clone();
	let window_count_clone = gui_state.window_count.clone();
	let window_count_clone2 = gui_state.window_count.clone();
	let window_count_clone3 = gui_state.window_count.clone();
	let count_now_clone = gui_state.count_now.clone();
	let count_now_clone2 = gui_state.count_now.clone();
	let count_now_clone3 = gui_state.count_now.clone();
	let arg_received_clone = gui_state.arg_received.clone();
	let arg_received_clone2 = gui_state.arg_received.clone();
	let arg_received_clone3 = gui_state.arg_received.clone();
	
	tauri::Builder::default()
		.invoke_handler(tauri::generate_handler![
            compress_files_command,
            get_compression_types,
            validate_compression_type,
            open_file_location,
			close,
			count_now
        ])
		.manage(gui_state.clone()) // store it in Tauri state
		//.manage(item_clone.clone()) // store it in Tauri state
		//.plugin(tauri_plugin_shell::init())
		//.plugin(tauri_plugin_cli::init())
        .plugin(tauri_plugin_single_instance::init(move |app, argv, _cwd| {
			//println!("Tauri compression app setup started");
			if log { std::fs::write("abc.txt", format!("{:?}", argv.clone())); }
            run_app(app, file_strings2.clone(), argv.clone(), Arc::new(GuiState { window_count: window_count_clone2.clone(), item_count: item_clone.clone(), count_now: count_now_clone.clone(), arg_received: arg_received_clone.clone() }));
			//return Ok(());
		}))
		.setup(move |app| {
			let appx = app.app_handle().clone();
			let count = window_count_clone.fetch_add(1, Ordering::SeqCst);
			if let Some(window) = app.get_webview_window("main") {
				let _ = window.center();
			}
			let mut fb = vec![];
			for x in files {
				fb.push(x.display().to_string());
			}
			run_app(&app.app_handle(), file_strings2b.clone(), fb.clone(), Arc::new(GuiState { window_count: window_count_clone3.clone(), item_count: item_clone2.clone(), count_now: count_now_clone2.clone(), arg_received: arg_received_clone2.clone() }));
			
			let app3 = appx.clone();
			{
				tokio::spawn(async move {
					while true {
						let mut c = 0;
						{
							c = *count_now_clone3.lock().unwrap();
						}
						
						tokio::time::sleep(Duration::from_millis(1000));
						
						let itemc = item_clone3.fetch_add(0, Ordering::SeqCst);
						let total_arg = *arg_received_clone3.lock().unwrap();
						if ((total_arg == itemc && itemc != 0)){
							app3.emit("enable-ok", "");
							break;
						}
					}
				});
			}
			return Ok(());
		}
		)
		.run(tauri::generate_context!())
        .expect("error while running tauri application");
    
	Ok(())
}

pub async fn run_decompression_dialog(file_strings: Vec<String>, files: Vec<PathBuf>, gui_state: Arc<GuiState>) -> Result<()> {
    println!("Starting Tauri decompression app with {} files", files.len());
    
	let log = false;
    let file_strings2 = file_strings.clone();
	let file_strings2b = file_strings.clone();
	
	let item_clone = gui_state.item_count.clone();
	let item_clone2 = gui_state.item_count.clone();
	let item_clone3 = gui_state.item_count.clone();
	let window_count_clone = gui_state.window_count.clone();
	let window_count_clone2 = gui_state.window_count.clone();
	let window_count_clone3 = gui_state.window_count.clone();
	let count_now_clone = gui_state.count_now.clone();
	let count_now_clone2 = gui_state.count_now.clone();
	let count_now_clone3 = gui_state.count_now.clone();
	let arg_received_clone = gui_state.arg_received.clone();
	let arg_received_clone2 = gui_state.arg_received.clone();
	let arg_received_clone3 = gui_state.arg_received.clone();
	
	tauri::Builder::default()
		.invoke_handler(tauri::generate_handler![
            decompress_files_command,
            open_file_location,
			close,
			count_now
        ])
		.manage(gui_state.clone()) // store it in Tauri state
		//.manage(item_clone.clone()) // store it in Tauri state
		//.plugin(tauri_plugin_shell::init())
		//.plugin(tauri_plugin_cli::init())
        .plugin(tauri_plugin_single_instance::init(move |app, argv, _cwd| {
			if log { std::fs::write("def.txt", format!("{:?}", argv.clone())); }
			run_decom_app(app, file_strings2.clone(), argv.clone(), Arc::new(GuiState { window_count: window_count_clone2.clone(), item_count: item_clone.clone(), count_now: count_now_clone.clone(), arg_received: arg_received_clone.clone()}));
        }))
		.setup(move |app| {
			let appx = app.app_handle().clone();
			let count = window_count_clone.fetch_add(1, Ordering::SeqCst);
			if let Some(window) = app.get_webview_window("main") {
				let _ = window.center();
			}
			let mut fb = vec![];
			for x in files {
				fb.push(x.display().to_string());
			}
			run_decom_app(&app.app_handle(), file_strings2b.clone(), fb.clone(), Arc::new(GuiState { window_count: window_count_clone3.clone(), item_count: item_clone2.clone(), count_now: count_now_clone2.clone(), arg_received: arg_received_clone2.clone()}));
			
			let app3 = appx.clone();
			{
				tokio::spawn(async move {
					while true {
						let mut c = 0;
						{
							c = *count_now_clone3.lock().unwrap();
						}
						
						tokio::time::sleep(Duration::from_millis(1000));
						
						let itemc = item_clone3.fetch_add(0, Ordering::SeqCst);
						let total_arg = *arg_received_clone3.lock().unwrap();
						if ((total_arg == itemc && total_arg != 0)){
							app3.emit("enable-ok", "");
							break;
						}
					}
				});
			}
			return Ok(());
		}
        )
		.run(tauri::generate_context!())
        .expect("error while running tauri application");
		
	Ok(())
}

fn generate_output_dir(file: &PathBuf) -> PathBuf {
    let base_name = file.file_stem().unwrap_or_default().to_string_lossy();
    let parent = file.parent().unwrap_or_else(|| std::path::Path::new("."));
    
    let mut counter = 1;
    let mut output_dir = parent.join(base_name.as_ref());
    
    while output_dir.exists() {
        counter += 1;
        output_dir = parent.join(format!("{} ({})", base_name, counter));
    }
    
    output_dir
}