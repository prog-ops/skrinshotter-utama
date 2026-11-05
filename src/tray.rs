use crate::settings::CaptureMode;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use winit::event_loop::EventLoopProxy;

#[derive(Debug, Clone)]
pub enum UserEvent {
    StartCapture(Option<CaptureMode>),
    OpenSettings,
    Quit,
}

pub struct TrayController {
    _tray_icon: TrayIcon,
    _hotkey_manager: Option<GlobalHotKeyManager>,
    _capture_hotkey: Option<HotKey>,
    pub hotkey_active: bool,
}

impl TrayController {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Result<Self, String> {
        let menu = Menu::new();
        let capture_rect_item = MenuItem::new("Capture Rectangle (Ctrl+Shift+S)", true, None);
        let capture_free_item = MenuItem::new("Capture Freeform (Lasso)", true, None);
        let settings_item = MenuItem::new("Settings (Default Author)...", true, None);
        let separator = PredefinedMenuItem::separator();
        let quit_item = MenuItem::new("Quit", true, None);

        menu.append(&capture_rect_item)
            .map_err(|e| format!("Failed to append rectangle capture menu item: {e}"))?;
        menu.append(&capture_free_item)
            .map_err(|e| format!("Failed to append freeform capture menu item: {e}"))?;
        menu.append(&settings_item)
            .map_err(|e| format!("Failed to append settings menu item: {e}"))?;
        menu.append(&separator)
            .map_err(|e| format!("Failed to append menu separator: {e}"))?;
        menu.append(&quit_item)
            .map_err(|e| format!("Failed to append quit menu item: {e}"))?;

        let capture_rect_id = capture_rect_item.id().clone();
        let capture_free_id = capture_free_item.id().clone();
        let settings_id = settings_item.id().clone();
        let quit_id = quit_item.id().clone();

        let menu_proxy = proxy.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == capture_rect_id {
                let _ = menu_proxy.send_event(UserEvent::StartCapture(Some(CaptureMode::Rectangle)));
            } else if event.id == capture_free_id {
                let _ = menu_proxy.send_event(UserEvent::StartCapture(Some(CaptureMode::Freeform)));
            } else if event.id == settings_id {
                let _ = menu_proxy.send_event(UserEvent::OpenSettings);
            } else if event.id == quit_id {
                let _ = menu_proxy.send_event(UserEvent::Quit);
            }
        }));

        // On Windows, left-clicking the tray icon triggers area capture immediately (using preferred mode),
        // while right-clicking opens the tray context menu.
        // On macOS, clicking the Status Bar icon opens the native menu bar dropdown.
        if !cfg!(target_os = "macos") {
            let click_proxy = proxy.clone();
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    let _ = click_proxy.send_event(UserEvent::StartCapture(None));
                }
            }));
        }

        let icon = build_tray_icon()?;
        let is_macos = cfg!(target_os = "macos");

        let mut tray_builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(is_macos)
            .with_tooltip("Skrinshotter — Click or press Ctrl+Shift+S to Capture Area");

        #[cfg(target_os = "macos")]
        {
            tray_builder = tray_builder.with_icon_templated(icon);
        }
        #[cfg(not(target_os = "macos"))]
        {
            tray_builder = tray_builder.with_icon(icon);
        }

        let tray_icon = tray_builder
            .build()
            .map_err(|e| format!("Failed to create System Tray / Status Bar icon: {e}"))?;

        // Register optional global shortcut: Ctrl+Shift+S (works safely on both Windows and macOS)
        let mut hotkey_manager_opt = None;
        let mut hotkey_opt = None;
        let mut hotkey_active = false;

        if let Ok(manager) = GlobalHotKeyManager::new() {
            let hk = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS);
            if manager.register(hk).is_ok() {
                let hk_id = hk.id();
                let hk_proxy = proxy;
                GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                    if event.id == hk_id && event.state == HotKeyState::Pressed {
                        let _ = hk_proxy.send_event(UserEvent::StartCapture(None));
                    }
                }));
                hotkey_active = true;
                hotkey_opt = Some(hk);
                hotkey_manager_opt = Some(manager);
            }
        }

        Ok(Self {
            _tray_icon: tray_icon,
            _hotkey_manager: hotkey_manager_opt,
            _capture_hotkey: hotkey_opt,
            hotkey_active,
        })
    }
}

/// Generates a crisp 32x32 RGBA camera/viewfinder icon.
/// On macOS (`template = true`), draws clean monochrome alpha geometry so macOS automatically
/// adapts the status bar icon to Dark Mode and Light Mode menu bars.
/// On Windows, draws a vibrant teal-blue rounded badge with white viewfinder corners.
pub fn build_tray_icon() -> Result<Icon, String> {
    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let is_macos = cfg!(target_os = "macos");

    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;

            if is_macos {
                // Monochrome template viewfinder frame + center dot
                let in_frame_x = (5..=26).contains(&x);
                let in_frame_y = (6..=25).contains(&y);
                let on_border = (in_frame_x && (y == 6 || y == 7 || y == 24 || y == 25))
                    || (in_frame_y && (x == 5 || x == 6 || x == 25 || x == 26));
                let corner_gap =
                    (12..=19).contains(&x) && (y <= 7 || y >= 24)
                        || (12..=19).contains(&y) && (x <= 6 || x >= 25);

                let dist_center = ((fx - 16.0).powi(2) + (fy - 16.0).powi(2)).sqrt();
                let in_lens = (dist_center >= 3.2 && dist_center <= 5.2) || dist_center <= 1.6;

                if (on_border && !corner_gap) || in_lens {
                    rgba[idx] = 0;
                    rgba[idx + 1] = 0;
                    rgba[idx + 2] = 0;
                    rgba[idx + 3] = 255;
                }
            } else {
                // Rounded square background badge [2..30, 2..30], radius 6
                let cx = fx.clamp(8.0, 24.0);
                let cy = fy.clamp(8.0, 24.0);
                let dist_round = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();

                if dist_round <= 6.2 {
                    let edge_alpha = ((6.2 - dist_round) / 1.0).clamp(0.0, 1.0);
                    // Gradient teal/indigo background
                    let t = (fx + fy) / 64.0;
                    let mut r = (18.0 + t * 25.0) as u8;
                    let mut g = (135.0 + t * 55.0) as u8;
                    let mut b = (225.0 - t * 20.0) as u8;

                    // White viewfinder brackets & center shutter circle
                    let in_box_x = (7..=24).contains(&x);
                    let in_box_y = (8..=23).contains(&y);
                    let on_bracket_border = (in_box_x && (y == 8 || y == 9 || y == 22 || y == 23))
                        || (in_box_y && (x == 7 || x == 8 || x == 23 || x == 24));
                    let bracket_gap = ((13..=18).contains(&x) && (y <= 9 || y >= 22))
                        || ((13..=18).contains(&y) && (x <= 8 || x >= 23));

                    let dist_center = ((fx - 16.0).powi(2) + (fy - 16.0).powi(2)).sqrt();
                    let in_center_ring = dist_center <= 3.3;

                    if (on_bracket_border && !bracket_gap) || in_center_ring {
                        r = 255;
                        g = 255;
                        b = 255;
                    }

                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = (edge_alpha * 255.0).round() as u8;
                }
            }
        }
    }

    Icon::from_rgba(rgba, size, size).map_err(|e| format!("Failed to create RGBA Icon: {e}"))
}
