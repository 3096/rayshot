const TARGET_WINDOW_TITLE: &str = "原神";
const SCREENSHOT_DIR_PATH: &str = "screenshots";
const TRASH_DIR_PATH: &str = "trashed";

// UI sizing constants
const THUMBNAIL_SIZE: f32 = 400.0;
const MAIN_IMAGE_WIDTH_RATIO: f32 = 1.0;
const MAIN_IMAGE_HEIGHT_RATIO: f32 = 1.0;
const HORIZONTAL_LIST_HEIGHT: f32 = 200.0;
const LOADING_PLACEHOLDER_SIZE: f32 = 200.0;

// Texture cache constants
const MAX_LOADED_TEXTURES: usize = 32;

// UI spacing constants
const WELCOME_SECTION_TOP_SPACING: f32 = 50.0;
const WELCOME_SECTION_MIDDLE_SPACING: f32 = 20.0;
const WELCOME_SECTION_BOTTOM_SPACING: f32 = 10.0;
const SCREENSHOT_INFO_SPACING: f32 = 10.0;
const THUMBNAIL_SPACING: f32 = 10.0;
const SECTION_SEPARATOR_SPACING: f32 = 20.0;
const ERROR_LIST_ITEM_SPACING: f32 = 5.0;

// Error window constants
const ERROR_WINDOW_DEFAULT_WIDTH: f32 = 400.0;

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

fn get_window_app_name(window_title: &str) -> xcap::XCapResult<String> {
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

    let app_path = target_window.app_name()?;

    // Extract just the filename from the full path
    let app_name = std::path::Path::new(&app_path)
        .file_stem() // Gets filename without extension
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown")
        .to_string();

    Ok(app_name)
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
    pub state: std::sync::Arc<tokio::sync::Mutex<ScreenshotState>>,
    pub texture_handle: std::sync::Arc<tokio::sync::Mutex<Option<eframe::epaint::TextureHandle>>>,
    pub demension: std::sync::Arc<tokio::sync::Mutex<Option<(usize, usize)>>>,
    pub filename: std::sync::Arc<String>,
    pub file_location: std::sync::Arc<tokio::sync::Mutex<FileLocation>>,
    pub file_size: std::sync::Arc<tokio::sync::Mutex<Option<usize>>>,
    pub file_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl ScreenshotEntry {
    pub fn new(filename: std::sync::Arc<String>, file_location: FileLocation) -> Self {
        Self {
            texture_handle: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            demension: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            filename,
            file_location: std::sync::Arc::new(tokio::sync::Mutex::new(file_location)),
            file_size: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
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

    pub async fn manage_texture_cache(&self) {
        let entries = self.screenshot_entries.lock().await;
        for entry in entries
            .iter()
            .take(entries.len().saturating_sub(MAX_LOADED_TEXTURES))
            .rev()
        {
            if let Ok(mut texture_lock) = entry.texture_handle.try_lock() {
                if texture_lock.is_some() {
                    *texture_lock = None; // the TextureHandle drop will handle the freeing
                } else {
                    break;
                }
            }
        }
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
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default().with_maximized(true),
            ..Default::default()
        },
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
                                    get_window_app_name(TARGET_WINDOW_TITLE)
                                        .unwrap_or_else(|_| "Unknown".to_string()),
                                    chrono::Local::now().format("%Y%m%d_%H%M%S.%f")
                                ));
                                let screenshot_file_path =
                                    std::path::Path::new(SCREENSHOT_DIR_PATH)
                                        .join(screenshot_file_name.as_str());
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
                                rayshot_state.manage_texture_cache().await;
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
                                *screenshot_entry.demension.lock().await = Some((
                                    image_buffer.width() as usize,
                                    image_buffer.height() as usize,
                                ));

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
                                            .save(screenshot_file_path.clone())
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
                                        if let Ok(metadata) =
                                            std::fs::metadata(&screenshot_file_path)
                                        {
                                            screenshot_entry
                                                .file_size
                                                .blocking_lock()
                                                .replace(metadata.len() as usize);
                                        }
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

// the ui code below is vibe coded
impl eframe::App for RayshotApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        // Copy data needed for UI without holding locks
        let entries: Vec<_> = {
            if let Ok(entries_guard) = self.rayshot_state.screenshot_entries.try_lock() {
                entries_guard.clone()
            } else {
                Vec::new() // Return empty if we can't get lock
            }
        };
        let current_idx = {
            if let Ok(idx_guard) = self.rayshot_state.cur_screenshot_idx.try_lock() {
                *idx_guard
            } else {
                0
            }
        };
        let errors: Vec<String> = {
            if let Ok(errors_guard) = self.rayshot_state.error_messages.try_lock() {
                errors_guard.clone()
            } else {
                Vec::new() // Return empty if we can't get lock
            }
        };

        // Top panel with controls
        eframe::egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🎯 rayshot");
                ui.separator();

                if ui.button("🔄 Refresh").clicked() {
                    ctx.request_repaint();
                }

                if !errors.is_empty() && ui.button("🗑 Clear Errors").clicked() {
                    if let Ok(mut errors_guard) = self.rayshot_state.error_messages.try_lock() {
                        errors_guard.clear();
                    }
                }

                ui.separator();
                ui.label("📸 Hotkey: Ctrl+Shift+P");

                ui.with_layout(
                    eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                    |ui| {
                        if !entries.is_empty() {
                            ui.label(format!("Current: {}/{}", current_idx + 1, entries.len()));
                            ui.separator();
                        }
                        ui.label(format!("Screenshots: {}", entries.len()));
                    },
                );
            });
        });

        // Main content area
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            if entries.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(WELCOME_SECTION_TOP_SPACING);
                    ui.heading("Welcome to rayshot!");
                    ui.add_space(WELCOME_SECTION_MIDDLE_SPACING);
                    ui.label("Press Ctrl+Shift+P to capture a screenshot of the target window");
                    ui.add_space(WELCOME_SECTION_BOTTOM_SPACING);
                    ui.label(format!("🎮 Current target: {}", TARGET_WINDOW_TITLE));
                });
            } else {
                // Get current screenshot (current_idx is in natural order)
                let current_entry = if current_idx < entries.len() {
                    Some(&entries[current_idx])
                } else {
                    entries.last()
                };

                ui.vertical(|ui| {
                    // Current screenshot center stage
                    if let Some(entry) = current_entry {
                        ui.group(|ui| {
                            ui.vertical_centered(|ui| {
                                ui.heading(format!(
                                    "📷 Current Screenshot ({}/{})",
                                    current_idx + 1,
                                    entries.len()
                                ));
                                ui.add_space(SCREENSHOT_INFO_SPACING);

                                // Large screenshot display
                                if let Ok(img_lock) = entry.texture_handle.try_lock() {
                                    if let Some(tex) = &*img_lock {
                                        let available_rect = ui.available_rect_before_wrap();
                                        let max_width =
                                            available_rect.width() * MAIN_IMAGE_WIDTH_RATIO;
                                        let max_height =
                                            available_rect.height() * MAIN_IMAGE_HEIGHT_RATIO;

                                        let tex_size = tex.size_vec2();
                                        let scale = (max_width / tex_size.x)
                                            .min(max_height / tex_size.y)
                                            .min(1.0);
                                        let display_size = tex_size * scale;

                                        ui.image((tex.id(), display_size));
                                    } else {
                                        ui.add_space(LOADING_PLACEHOLDER_SIZE);
                                        ui.label("🖼 Texture unloaded (memory limit)");
                                    }
                                } else {
                                    ui.add_space(LOADING_PLACEHOLDER_SIZE);
                                    ui.label("🖼 Loading...");
                                }

                                ui.add_space(SCREENSHOT_INFO_SPACING);

                                // Current screenshot info
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label("📁 Filename:");
                                        ui.label(
                                            eframe::egui::RichText::new(entry.filename.as_str())
                                                .monospace(),
                                        );

                                        ui.add_space(5.0);

                                        // File location status
                                        if let Ok(location) = entry.file_location.try_lock() {
                                            match *location {
                                                FileLocation::Local => {
                                                    ui.colored_label(
                                                        eframe::egui::Color32::GREEN,
                                                        "📂 Local",
                                                    );
                                                }
                                                FileLocation::Trash => {
                                                    ui.colored_label(
                                                        eframe::egui::Color32::RED,
                                                        "🗑 Trashed",
                                                    );
                                                }
                                            }
                                        } else {
                                            ui.label("📍 Location: Loading...");
                                        }
                                    });

                                    ui.separator();

                                    ui.vertical(|ui| {
                                        // File size
                                        ui.label("📏 File Size:");
                                        if let Ok(file_size) = entry.file_size.try_lock() {
                                            if let Some(size) = *file_size {
                                                let size_str = if size >= 1_048_576 {
                                                    format!("{:.2} MB", size as f64 / 1_048_576.0)
                                                } else if size >= 1024 {
                                                    format!("{:.2} KB", size as f64 / 1024.0)
                                                } else {
                                                    format!("{} bytes", size)
                                                };
                                                ui.label(
                                                    eframe::egui::RichText::new(size_str)
                                                        .monospace(),
                                                );
                                            } else {
                                                ui.label("Unknown");
                                            }
                                        } else {
                                            ui.label("Loading...");
                                        }

                                        ui.add_space(5.0);

                                        // Screenshot dimensions
                                        ui.label("📐 Dimensions:");
                                        if let Ok(dimensions) = entry.demension.try_lock() {
                                            if let Some((width, height)) = *dimensions {
                                                ui.label(
                                                    eframe::egui::RichText::new(format!(
                                                        "{}×{} px",
                                                        width, height
                                                    ))
                                                    .monospace(),
                                                );
                                            } else {
                                                ui.label("Unknown");
                                            }
                                        } else {
                                            ui.label("Loading...");
                                        }
                                    });

                                    ui.separator();

                                    ui.vertical(|ui| {
                                        // Status indicators
                                        if let Ok(state) = entry.state.try_lock() {
                                            ui.label("📊 Status:");
                                            if state.failed {
                                                ui.colored_label(
                                                    eframe::egui::Color32::RED,
                                                    "❌ Failed",
                                                );
                                            } else if state.capturing {
                                                ui.colored_label(
                                                    eframe::egui::Color32::YELLOW,
                                                    "📸 Capturing...",
                                                );
                                            } else if state.moving {
                                                ui.colored_label(
                                                    eframe::egui::Color32::BLUE,
                                                    "📦 Moving...",
                                                );
                                            } else if state.writing {
                                                ui.colored_label(
                                                    eframe::egui::Color32::YELLOW,
                                                    "💾 Writing to disk...",
                                                );
                                            } else {
                                                ui.colored_label(
                                                    eframe::egui::Color32::GREEN,
                                                    "✅ Saved",
                                                );
                                            }
                                        } else {
                                            ui.label("📊 Status: Loading...");
                                        }
                                    });

                                    ui.separator();

                                    ui.vertical(|ui| {
                                        // Navigation info
                                        ui.label("Navigation:");
                                        ui.label("Left/Right Arrow keys to navigate");
                                        ui.label("Delete key to trash/restore");
                                    });
                                });

                                ui.add_space(SCREENSHOT_INFO_SPACING);

                                // Action buttons
                                ui.horizontal(|ui| {
                                    if ui.button("📂 Open Folder").clicked() {
                                        let entry_path = match entry.file_location.try_lock() {
                                            Ok(location) => match *location {
                                                FileLocation::Local => SCREENSHOT_DIR_PATH,
                                                FileLocation::Trash => TRASH_DIR_PATH,
                                            },
                                            Err(_) => SCREENSHOT_DIR_PATH, // Fallback to local
                                        };
                                        let _ = std::process::Command::new("explorer")
                                            .arg(entry_path)
                                            .spawn();
                                    }

                                    if ui.button("📋 Copy Path").clicked() {
                                        ctx.copy_text(entry.filename.as_str().to_string());
                                    }
                                });
                            });
                        });

                        ui.add_space(SECTION_SEPARATOR_SPACING);
                    }

                    // All screenshots list below
                    ui.separator();
                    ui.heading("📸 All Screenshots");
                    ui.add_space(SCREENSHOT_INFO_SPACING);

                    let scroll_area = eframe::egui::ScrollArea::horizontal()
                        .auto_shrink([false; 2])
                        .max_height(HORIZONTAL_LIST_HEIGHT);

                    scroll_area.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (index, entry) in entries.iter().enumerate() {
                                let is_current = index == current_idx;

                                let response = if is_current {
                                    // Current screenshot with yellow outline
                                    ui.scope(|ui| {
                                        ui.visuals_mut().widgets.noninteractive.bg_stroke.color =
                                            eframe::egui::Color32::YELLOW;
                                        ui.visuals_mut().widgets.noninteractive.bg_stroke.width =
                                            2.0;
                                        ui.group(|ui| {
                                            ui.vertical(|ui| {
                                                // Thumbnail at the top
                                                if let Ok(img_lock) =
                                                    entry.texture_handle.try_lock()
                                                {
                                                    if let Some(tex) = &*img_lock {
                                                        let max_size = THUMBNAIL_SIZE;
                                                        let tex_size = tex.size_vec2();
                                                        let scale = (max_size
                                                            / tex_size.x.max(tex_size.y))
                                                        .min(1.0);
                                                        let display_size = tex_size * scale;

                                                        ui.image((tex.id(), display_size));
                                                    } else {
                                                        ui.add_space(THUMBNAIL_SIZE);
                                                        ui.label("🖼");
                                                    }
                                                } else {
                                                    ui.add_space(THUMBNAIL_SIZE);
                                                    ui.label("🖼");
                                                }

                                                // All labels below in horizontal layout
                                                ui.horizontal(|ui| {
                                                    // Screenshot number
                                                    ui.label(
                                                        eframe::egui::RichText::new(format!(
                                                            "#{}",
                                                            index + 1
                                                        ))
                                                        .strong()
                                                        .small(),
                                                    );

                                                    // File location indicator
                                                    if let Ok(location) =
                                                        entry.file_location.try_lock()
                                                    {
                                                        match *location {
                                                            FileLocation::Local => {
                                                                ui.colored_label(
                                                                    eframe::egui::Color32::GREEN,
                                                                    "📂",
                                                                );
                                                            }
                                                            FileLocation::Trash => {
                                                                ui.colored_label(
                                                                    eframe::egui::Color32::RED,
                                                                    "🗑",
                                                                );
                                                            }
                                                        }
                                                    }

                                                    // Compact status
                                                    if let Ok(state) = entry.state.try_lock() {
                                                        if state.failed {
                                                            ui.colored_label(
                                                                eframe::egui::Color32::RED,
                                                                "❌",
                                                            );
                                                        } else if state.capturing {
                                                            ui.colored_label(
                                                                eframe::egui::Color32::YELLOW,
                                                                "📸",
                                                            );
                                                        } else if state.moving {
                                                            ui.colored_label(
                                                                eframe::egui::Color32::BLUE,
                                                                "📦",
                                                            );
                                                        } else if state.writing {
                                                            ui.colored_label(
                                                                eframe::egui::Color32::YELLOW,
                                                                "💾",
                                                            );
                                                        } else {
                                                            ui.colored_label(
                                                                eframe::egui::Color32::GREEN,
                                                                "✅",
                                                            );
                                                        }
                                                    }

                                                    // Current indicator as the last label
                                                    if is_current {
                                                        ui.colored_label(
                                                            eframe::egui::Color32::YELLOW,
                                                            "▶",
                                                        );
                                                    }
                                                });
                                            });
                                        })
                                    })
                                    .inner
                                } else {
                                    // Regular screenshot with default outline
                                    ui.group(|ui| {
                                        ui.vertical(|ui| {
                                            // Thumbnail at the top
                                            if let Ok(img_lock) = entry.texture_handle.try_lock() {
                                                if let Some(tex) = &*img_lock {
                                                    let max_size = THUMBNAIL_SIZE;
                                                    let tex_size = tex.size_vec2();
                                                    let scale = (max_size
                                                        / tex_size.x.max(tex_size.y))
                                                    .min(1.0);
                                                    let display_size = tex_size * scale;

                                                    ui.image((tex.id(), display_size));
                                                } else {
                                                    ui.add_space(THUMBNAIL_SIZE);
                                                    ui.label("🖼");
                                                }
                                            } else {
                                                ui.add_space(THUMBNAIL_SIZE);
                                                ui.label("🖼");
                                            }

                                            // All labels below in horizontal layout
                                            ui.horizontal(|ui| {
                                                // Screenshot number
                                                ui.label(
                                                    eframe::egui::RichText::new(format!(
                                                        "#{}",
                                                        index + 1
                                                    ))
                                                    .strong()
                                                    .small(),
                                                );

                                                // File location indicator
                                                if let Ok(location) = entry.file_location.try_lock()
                                                {
                                                    match *location {
                                                        FileLocation::Local => {
                                                            ui.colored_label(
                                                                eframe::egui::Color32::GREEN,
                                                                "📂",
                                                            );
                                                        }
                                                        FileLocation::Trash => {
                                                            ui.colored_label(
                                                                eframe::egui::Color32::RED,
                                                                "🗑",
                                                            );
                                                        }
                                                    }
                                                }

                                                // Compact status
                                                if let Ok(state) = entry.state.try_lock() {
                                                    if state.failed {
                                                        ui.colored_label(
                                                            eframe::egui::Color32::RED,
                                                            "❌",
                                                        );
                                                    } else if state.capturing {
                                                        ui.colored_label(
                                                            eframe::egui::Color32::YELLOW,
                                                            "📸",
                                                        );
                                                    } else if state.moving {
                                                        ui.colored_label(
                                                            eframe::egui::Color32::BLUE,
                                                            "📦",
                                                        );
                                                    } else if state.writing {
                                                        ui.colored_label(
                                                            eframe::egui::Color32::YELLOW,
                                                            "💾",
                                                        );
                                                    } else {
                                                        ui.colored_label(
                                                            eframe::egui::Color32::GREEN,
                                                            "✅",
                                                        );
                                                    }
                                                }

                                                // Current indicator as the last label
                                                if is_current {
                                                    ui.colored_label(
                                                        eframe::egui::Color32::YELLOW,
                                                        "▶",
                                                    );
                                                }
                                            });
                                        });
                                    })
                                };

                                // Scroll to current item
                                if is_current {
                                    ui.scroll_to_rect(
                                        response.response.rect,
                                        Some(eframe::egui::Align::Center),
                                    );
                                }

                                ui.add_space(THUMBNAIL_SPACING);
                            }
                        });
                    });
                });
            }
        });

        // Error window (if there are errors)
        if !errors.is_empty() {
            eframe::egui::Window::new("⚠ Errors")
                .collapsible(true)
                .resizable(true)
                .default_width(ERROR_WINDOW_DEFAULT_WIDTH)
                .show(ctx, |ui| {
                    ui.label(format!("Found {} error(s):", errors.len()));
                    ui.separator();

                    eframe::egui::ScrollArea::vertical().show(ui, |ui| {
                        for (i, err) in errors.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}.", i + 1));
                                ui.colored_label(eframe::egui::Color32::RED, err);
                            });
                            ui.add_space(ERROR_LIST_ITEM_SPACING);
                        }
                    });

                    ui.separator();
                    if ui.button("Clear All Errors").clicked() {
                        if let Ok(mut errors_guard) = self.rayshot_state.error_messages.try_lock() {
                            errors_guard.clear();
                        }
                    }
                });
        }
    }
}
