const TARGET_WINDOW_TITLE: &str = "原神";
const SCREENSHOT_DIR_PATH: &str = "screenshots";
const TRASH_DIR_PATH: &str = "trashed";

fn take_window_screenshot(window_title: &str) -> xcap::XCapResult<image::RgbaImage> {
    let window_title_lower = window_title.to_lowercase();
    let all_windows = xcap::Window::all()?;
    let target_window = all_windows
        .iter()
        .find(|w| {
            if let Ok(title) = w.title() {
                title.to_lowercase().contains(&window_title_lower)
            } else {
                false
            }
        })
        .ok_or_else(|| {
            eprintln!("Window with title '{}' not found", window_title);
            format!("Window '{}' not found", window_title)
        })
        .map_err(|_| xcap::XCapError::new("Window not found"))?;
    target_window.capture_image()
}

enum FileLocation {
    Local,
    Trash,
}

struct ScreenshotState {
    pub capturing: bool,
    pub writing: bool,
    pub moving: bool,
    pub failed: bool,
}

impl ScreenshotState {
    pub fn new() -> Self {
        Self {
            capturing: false,
            writing: false,
            moving: false,
            failed: false,
        }
    }
}

#[derive(Clone)]
struct ScreenshotEntry {
    pub texture_handle: std::sync::Arc<tokio::sync::Mutex<Option<eframe::epaint::TextureHandle>>>,
    pub filename: std::sync::Arc<String>,
    pub file_location: std::sync::Arc<tokio::sync::Mutex<FileLocation>>,
    pub file_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    pub state: std::sync::Arc<tokio::sync::Mutex<ScreenshotState>>,
}

impl ScreenshotEntry {
    pub fn new(filename: std::sync::Arc<String>, file_location: FileLocation) -> Self {
        Self {
            texture_handle: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            filename,
            file_location: std::sync::Arc::new(tokio::sync::Mutex::new(file_location)),
            file_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            state: std::sync::Arc::new(tokio::sync::Mutex::new(ScreenshotState::new())),
        }
    }
}

#[derive(Clone)]
struct RayshotState {
    pub screenshot_entries: std::sync::Arc<tokio::sync::Mutex<Vec<ScreenshotEntry>>>,
    pub cur_screenshot_idx: std::sync::Arc<tokio::sync::Mutex<usize>>,
    pub error_messages: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
}

impl RayshotState {
    pub fn new() -> Self {
        Self {
            screenshot_entries: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            cur_screenshot_idx: std::sync::Arc::new(tokio::sync::Mutex::new(0)),
            error_messages: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    pub async fn try_increment_screenshot_index(&self) -> usize {
        let mut idx = self.cur_screenshot_idx.lock().await;
        *idx += 1;
        {
            let entries = self.screenshot_entries.lock().await;
            if *idx >= entries.len() {
                *idx = entries.len() - 1;
            }
        }
        *idx
    }

    pub async fn try_decrement_screenshot_index(&self) -> usize {
        let mut idx = self.cur_screenshot_idx.lock().await;
        if *idx > 0 {
            *idx -= 1;
        }
        *idx
    }

    pub async fn get_current_screenshot(&self) -> Option<ScreenshotEntry> {
        let idx = *self.cur_screenshot_idx.lock().await;
        let entries = self.screenshot_entries.lock().await;
        entries.get(idx).cloned()
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
enum RayshotHotkey {
    CaptureScreenshot,
    Left,
    Right,
    Trash,
}

async fn report_error(
    rayshot_state: &RayshotState,
    egui_ctx: &eframe::egui::Context,
    error_msg: String,
) {
    eprintln!("{}", error_msg);
    rayshot_state.error_messages.lock().await.push(error_msg);
    egui_ctx.request_repaint();
}

#[tokio::main]
async fn main() {
    // Ensure required directories exist
    let screenshot_dir = std::path::Path::new(SCREENSHOT_DIR_PATH);
    let trash_dir = std::path::Path::new(TRASH_DIR_PATH);

    if !screenshot_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(screenshot_dir) {
            eprintln!(
                "Failed to create screenshot directory '{}': {}",
                SCREENSHOT_DIR_PATH, e
            );
            std::process::exit(1);
        }
        println!("Created screenshot directory: {}", SCREENSHOT_DIR_PATH);
    }

    if !trash_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(trash_dir) {
            eprintln!(
                "Failed to create trash directory '{}': {}",
                TRASH_DIR_PATH, e
            );
            std::process::exit(1);
        }
        println!("Created trash directory: {}", TRASH_DIR_PATH);
    }

    let hotkey_definitions = std::collections::HashMap::from([
        (
            RayshotHotkey::CaptureScreenshot,
            (
                Some(
                    global_hotkey::hotkey::Modifiers::CONTROL
                        | global_hotkey::hotkey::Modifiers::SHIFT,
                ),
                global_hotkey::hotkey::Code::KeyP,
            ),
        ),
        (
            RayshotHotkey::Left,
            (None, global_hotkey::hotkey::Code::ArrowLeft),
        ),
        (
            RayshotHotkey::Right,
            (None, global_hotkey::hotkey::Code::ArrowRight),
        ),
        (
            RayshotHotkey::Trash,
            (None, global_hotkey::hotkey::Code::Delete),
        ),
    ]);

    let hotkeys: Vec<_> = hotkey_definitions
        .iter()
        .map(|(rayshot_hotkey, (modifiers, code))| {
            let hotkey = global_hotkey::hotkey::HotKey::new(*modifiers, *code);
            (hotkey, rayshot_hotkey.clone())
        })
        .collect();

    let global_hotkey_manager = global_hotkey::GlobalHotKeyManager::new().unwrap();
    for (hotkey, _) in &hotkeys {
        global_hotkey_manager.register(*hotkey).unwrap();
    }

    let hotkey_map: std::collections::HashMap<_, _> = hotkeys
        .into_iter()
        .map(|(hotkey, rayshot_hotkey)| (hotkey.id(), rayshot_hotkey))
        .collect();

    let global_hotkey_receiver = global_hotkey::GlobalHotKeyEvent::receiver();

    let rayshot_state = RayshotState::new();
    let rayshot_state_gui = rayshot_state.clone();

    let (hotkey_tx, mut hotkey_rx) = tokio::sync::mpsc::unbounded_channel::<RayshotHotkey>();
    std::thread::spawn(move || loop {
        let Ok(event) = global_hotkey_receiver.recv() else {
            continue;
        };

        if event.state != global_hotkey::HotKeyState::Pressed {
            continue;
        }

        let Some(hotkey) = hotkey_map.get(&event.id) else {
            panic!("Unknown hotkey ID: {}", event.id);
        };

        hotkey_tx.send(hotkey.clone()).unwrap_or_else(|e| {
            eprintln!("Failed to send hotkey: {}", e);
        });
    });

    eframe::run_native(
        "rayshot",
        eframe::NativeOptions::default(),
        Box::new(|creation_context| {
            let egui_ctx = creation_context.egui_ctx.clone();
            tokio::task::spawn(async move {
                loop {
                    let Some(hotkey) = hotkey_rx.recv().await else {
                        continue;
                    };

                    let rayshot_state = rayshot_state.clone();
                    let egui_ctx = egui_ctx.clone();
                    match hotkey {
                        RayshotHotkey::CaptureScreenshot => {
                            println!("Hotkey event detected: {:?}", hotkey);

                            // immediately capture the screenshot
                            let screenshot_task = tokio::task::spawn_blocking(|| {
                                take_window_screenshot(TARGET_WINDOW_TITLE)
                            });

                            tokio::task::spawn(async move {
                                // prepare the screenshot entry to signal the UI we have a new screenshot
                                let screenshot_file_name = std::sync::Arc::new(format!(
                                    "{}_{}.png",
                                    TARGET_WINDOW_TITLE,
                                    chrono::Local::now().format("%Y%m%d_%H%M%S.%f")
                                ));
                                let screenshot_entry = ScreenshotEntry::new(
                                    screenshot_file_name.clone(),
                                    FileLocation::Local,
                                );
                                screenshot_entry.state.lock().await.capturing = true;
                                let screenshot_entry_idx;
                                {
                                    let mut entries = rayshot_state.screenshot_entries.lock().await;
                                    screenshot_entry_idx = entries.len();
                                    entries.push(screenshot_entry.clone());
                                }
                                egui_ctx.request_repaint();

                                let handle_error = |error_msg: String| async {
                                    screenshot_entry.state.lock().await.failed = true;
                                    report_error(&rayshot_state, &egui_ctx, error_msg).await;
                                };

                                // receive the screenshot
                                let image_buffer = match screenshot_task.await {
                                    Ok(Ok(buffer)) => buffer,
                                    Ok(Err(error)) => {
                                        handle_error(format!(
                                            "Failed to capture screenshot for window '{}': {}",
                                            TARGET_WINDOW_TITLE, error
                                        ))
                                        .await;
                                        return;
                                    }
                                    Err(error) => {
                                        handle_error(format!(
                                            "Task failed for window '{}': {}",
                                            TARGET_WINDOW_TITLE, error
                                        ))
                                        .await;
                                        return;
                                    }
                                };

                                // write the screenshot to gpu for UI display, then to file
                                tokio::task::spawn_blocking(move || {
                                    let texture_handle = egui_ctx.load_texture(
                                        screenshot_file_name.as_str(),
                                        eframe::epaint::ColorImage::from_rgba_unmultiplied(
                                            [
                                                image_buffer.width() as usize,
                                                image_buffer.height() as usize,
                                            ],
                                            image_buffer.as_raw(),
                                        ),
                                        Default::default(),
                                    );
                                    screenshot_entry
                                        .texture_handle
                                        .blocking_lock()
                                        .replace(texture_handle);
                                    *rayshot_state.cur_screenshot_idx.blocking_lock() =
                                        screenshot_entry_idx;
                                    {
                                        let mut screenshot_state =
                                            screenshot_entry.state.blocking_lock();
                                        screenshot_state.capturing = false;
                                        screenshot_state.writing = true;
                                    }
                                    {
                                        let _file_lock = screenshot_entry.file_lock.blocking_lock();
                                        egui_ctx.request_repaint();
                                        image_buffer
                                            .save(
                                                std::path::Path::new(SCREENSHOT_DIR_PATH)
                                                    .join(screenshot_file_name.as_str()),
                                            )
                                            .unwrap_or_else(|e| {
                                                let err_str =
                                                    format!("Failed to save screenshot: {}", e);
                                                eprintln!("{}", err_str);
                                                rayshot_state
                                                    .error_messages
                                                    .blocking_lock()
                                                    .push(err_str);
                                                screenshot_entry.state.blocking_lock().failed =
                                                    true;
                                            });
                                    }
                                    screenshot_entry.state.blocking_lock().writing = false;
                                    egui_ctx.request_repaint();
                                });
                            });
                        }
                        RayshotHotkey::Left => {
                            tokio::task::spawn(async move {
                                rayshot_state.try_decrement_screenshot_index().await;
                                egui_ctx.request_repaint();
                            });
                        }
                        RayshotHotkey::Right => {
                            tokio::task::spawn(async move {
                                rayshot_state.try_increment_screenshot_index().await;
                                egui_ctx.request_repaint();
                            });
                        }
                        RayshotHotkey::Trash => {
                            tokio::task::spawn(async move {
                                let Some(current_entry) =
                                    rayshot_state.get_current_screenshot().await
                                else {
                                    return report_error(
                                        &rayshot_state,
                                        &egui_ctx,
                                        "No current screenshot to move to trash".to_string(),
                                    )
                                    .await;
                                };

                                current_entry.state.lock().await.moving = true;
                                {
                                    let _file_lock = current_entry.file_lock.lock().await;
                                    egui_ctx.request_repaint();
                                    let (current_dir, target_location, target_dir) =
                                        match *current_entry.file_location.lock().await {
                                            FileLocation::Local => (
                                                SCREENSHOT_DIR_PATH,
                                                FileLocation::Trash,
                                                TRASH_DIR_PATH,
                                            ),
                                            FileLocation::Trash => (
                                                TRASH_DIR_PATH,
                                                FileLocation::Local,
                                                SCREENSHOT_DIR_PATH,
                                            ),
                                        };
                                    let target_path = std::path::Path::new(target_dir)
                                        .join(current_entry.filename.as_str());
                                    let current_path = std::path::Path::new(current_dir)
                                        .join(current_entry.filename.as_str());
                                    if let Err(e) = std::fs::rename(&current_path, &target_path) {
                                        current_entry.state.lock().await.failed = true;
                                        return report_error(
                                            &rayshot_state,
                                            &egui_ctx,
                                            format!(
                                                "Failed to move screenshot '{}' to '{}': {}",
                                                current_entry.filename, target_dir, e
                                            ),
                                        )
                                        .await;
                                    }
                                    *current_entry.file_location.lock().await = target_location;
                                }
                                current_entry.state.lock().await.moving = false;
                                egui_ctx.request_repaint();
                            });
                        }
                    }
                }
            });

            Ok(Box::new(RayshotApp::new(rayshot_state_gui)))
        }),
    )
    .unwrap();
}

struct RayshotApp {
    rayshot_state: RayshotState,
}

impl RayshotApp {
    fn new(rayshot_state: RayshotState) -> Self {
        Self { rayshot_state }
    }
}

impl eframe::App for RayshotApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!(
                "time now is: {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            ));
        });
    }
}
