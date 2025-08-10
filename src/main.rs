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
    pub writing: bool,
    pub moving: bool,
}

#[derive(Clone)]
struct ScreenshotEntry {
    pub image: Option<std::sync::Arc<image::RgbaImage>>,
    pub filename: std::sync::Arc<String>,
    pub file_location: std::sync::Arc<tokio::sync::Mutex<FileLocation>>,
    pub file_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    pub state: std::sync::Arc<tokio::sync::Mutex<ScreenshotState>>,
}

impl ScreenshotEntry {
    pub fn new(filename: String, file_location: FileLocation) -> Self {
        Self {
            image: None,
            filename: std::sync::Arc::new(filename),
            file_location: std::sync::Arc::new(tokio::sync::Mutex::new(file_location)),
            file_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            state: std::sync::Arc::new(tokio::sync::Mutex::new(ScreenshotState {
                writing: false,
                moving: false,
            })),
        }
    }

    pub fn new_with_image(
        image: std::sync::Arc<image::RgbaImage>,
        filename: String,
        file_location: FileLocation,
    ) -> Self {
        Self {
            image: Some(image),
            filename: std::sync::Arc::new(filename),
            file_location: std::sync::Arc::new(tokio::sync::Mutex::new(file_location)),
            file_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            state: std::sync::Arc::new(tokio::sync::Mutex::new(ScreenshotState {
                writing: false,
                moving: false,
            })),
        }
    }
}

#[derive(Clone)]
struct RayshotState {
    pub screenshot_entries: std::sync::Arc<tokio::sync::Mutex<Vec<ScreenshotEntry>>>,
    pub error_messages: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
}

impl RayshotState {
    pub fn new() -> Self {
        Self {
            screenshot_entries: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            error_messages: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }
}

#[derive(Debug, Clone)]
enum RayshotHotkey {
    CaptureScreenshot,
}

#[tokio::main]
async fn main() {
    let capture_hotkey = global_hotkey::hotkey::HotKey::new(
        Some(global_hotkey::hotkey::Modifiers::CONTROL | global_hotkey::hotkey::Modifiers::SHIFT),
        global_hotkey::hotkey::Code::KeyP,
    );

    let mut hotkey_map = std::collections::HashMap::new();
    hotkey_map.insert(capture_hotkey.id(), RayshotHotkey::CaptureScreenshot);

    let global_hotkey_manager = global_hotkey::GlobalHotKeyManager::new().unwrap();
    global_hotkey_manager.register(capture_hotkey).unwrap();
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

                    match hotkey {
                        RayshotHotkey::CaptureScreenshot => {
                            println!("Hotkey event detected: {:?}", hotkey);
                            let rayshot_state = rayshot_state.clone();
                            let egui_ctx = egui_ctx.clone();
                            tokio::task::spawn_blocking(move || {
                                let window_title = "原神";
                                let screenshot_file_name = format!(
                                    "{}_{}.png",
                                    window_title.replace(" ", "_"),
                                    chrono::Local::now().format("%Y%m%d_%H%M%S.%f")
                                );
                                let Ok(image_buffer) = take_window_screenshot(window_title) else {
                                    let err_str = format!(
                                        "Failed to capture screenshot for window '{}'",
                                        window_title
                                    );
                                    eprintln!("{}", err_str);
                                    rayshot_state.error_messages.blocking_lock().push(err_str);
                                    egui_ctx.request_repaint();
                                    return;
                                };
                                let image_buffer = std::sync::Arc::new(image_buffer);

                                let screenshot_entry = ScreenshotEntry::new_with_image(
                                    image_buffer.clone(),
                                    screenshot_file_name,
                                    FileLocation::Local,
                                );

                                screenshot_entry.state.blocking_lock().writing = true;
                                {
                                    let _file_lock = screenshot_entry.file_lock.blocking_lock();
                                    rayshot_state
                                        .screenshot_entries
                                        .blocking_lock()
                                        .push(screenshot_entry.clone());
                                    egui_ctx.request_repaint();

                                    image_buffer
                                        .save(screenshot_entry.filename.as_str())
                                        .unwrap_or_else(|e| {
                                            let err_str =
                                                format!("Failed to save screenshot: {}", e);
                                            eprintln!("{}", err_str);
                                            rayshot_state
                                                .error_messages
                                                .blocking_lock()
                                                .push(err_str);
                                        });
                                }
                                screenshot_entry.state.blocking_lock().writing = false;
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
