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

struct ScreenshotEntry {
    pub image: Option<std::sync::Arc<image::RgbaImage>>,
    pub filename: String,
    pub file_location: FileLocation,
    pub file_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    pub state: ScreenshotState,
}

impl ScreenshotEntry {
    pub fn new(filename: String, file_location: FileLocation) -> Self {
        Self {
            image: None,
            filename,
            file_location,
            file_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            state: ScreenshotState {
                writing: false,
                moving: false,
            },
        }
    }

    pub fn new_with_image(
        image: image::RgbaImage,
        filename: String,
        file_location: FileLocation,
    ) -> Self {
        Self {
            image: Some(std::sync::Arc::new(image)),
            filename,
            file_location,
            file_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            state: ScreenshotState {
                writing: false,
                moving: false,
            },
        }
    }
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

    let screenshot_entries: std::sync::Arc<tokio::sync::Mutex<Vec<ScreenshotEntry>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let screenshot_entries_gui = screenshot_entries.clone();

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
                            tokio::task::spawn_blocking(move || {
                                let window_title = "原神";
                                let screenshot_file_name = format!(
                                    "{}_{}.png",
                                    window_title.replace(" ", "_"),
                                    chrono::Local::now().format("%Y%m%d_%H%M%S.%f")
                                );
                                match take_window_screenshot(window_title) {
                                    Ok(image_buffer) => {
                                        print!("Captured screenshot: {}", screenshot_file_name);
                                        if let Err(e) = image_buffer.save(&screenshot_file_name) {
                                            eprintln!("Error saving screenshot: {}", e);
                                        } else {
                                            println!(
                                                "Screenshot saved as: {}",
                                                screenshot_file_name
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Error capturing screenshot: {}", e);
                                    }
                                }
                            });
                            egui_ctx.request_repaint();
                        }
                    }
                }
            });

            Ok(Box::new(RayshotApp::new(screenshot_entries_gui)))
        }),
    )
    .unwrap();
}

#[derive(Debug, Clone)]
enum RayshotHotkey {
    CaptureScreenshot,
}

struct RayshotApp {
    screenshot_entries: std::sync::Arc<tokio::sync::Mutex<Vec<ScreenshotEntry>>>,
}

impl RayshotApp {
    fn new(screenshot_entries: std::sync::Arc<tokio::sync::Mutex<Vec<ScreenshotEntry>>>) -> Self {
        Self { screenshot_entries }
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
