fn take_window_screenshot(window_title: &str) {
    let windows = xcap::Window::all().unwrap();
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

    eframe::run_native(
        "rayshot",
        eframe::NativeOptions::default(),
        Box::new(|creation_context| {
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(4));
                let Ok(event) = global_hotkey::GlobalHotKeyEvent::receiver().recv() else {
                    continue;
                };

                if event.state != global_hotkey::HotKeyState::Pressed {
                    continue;
                }

                let Some(hotkey) = hotkey_map.get(&event.id) else {
                    panic!("Unknown hotkey ID: {}", event.id);
                };

                match hotkey {
                    RayshotHotkey::CaptureScreenshot => {
                        take_window_screenshot("原神");
                    }
                }
            });

            Ok(Box::new(RayshotApp::default()))
        }),
    )
    .unwrap();
}

#[derive(Debug)]
enum RayshotHotkey {
    CaptureScreenshot,
}

struct RayshotApp {
    // hotkey_map: std::collections::HashMap<u32, RayshotHotkey>,
    // global_hotkey_manager: global_hotkey::GlobalHotKeyManager,
}

impl Default for RayshotApp {
    fn default() -> Self {
        Self {
            // hotkey_map,
            // global_hotkey_manager,
        }
    }
}

impl eframe::App for RayshotApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Hello, egui!");
        });
    }
}
