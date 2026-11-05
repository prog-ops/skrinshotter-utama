#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotation;
mod capture;
mod gpu;
mod metadata;
mod settings;
mod tray;
mod ui_editor;
mod ui_overlay;
mod ui_settings;

use crate::capture::capture_all_displays;
use crate::gpu::SharedGpu;
use crate::settings::{AppSettings, CaptureMode};
use crate::tray::{TrayController, UserEvent};
use crate::ui_editor::{EditorAction, EditorWindow};
use crate::ui_overlay::{OverlayAction, OverlayWindow};
use crate::ui_settings::{SettingsAction, SettingsWindow};
use std::sync::Arc;
use std::time::Duration;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

struct SkrinshotterApp {
    proxy: EventLoopProxy<UserEvent>,
    tray: Option<TrayController>,
    shared_gpu: Option<Arc<SharedGpu>>,
    settings: AppSettings,
    overlays: Vec<OverlayWindow>,
    editor: Option<EditorWindow>,
    settings_window: Option<SettingsWindow>,
}

impl SkrinshotterApp {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            proxy,
            tray: None,
            shared_gpu: None,
            settings: AppSettings::load(),
            overlays: Vec::new(),
            editor: None,
            settings_window: None,
        }
    }

    fn begin_area_capture(
        &mut self,
        event_loop: &ActiveEventLoop,
        mode_override: Option<CaptureMode>,
    ) {
        let initial_mode = mode_override.unwrap_or(self.settings.default_capture_mode);

        if !self.overlays.is_empty() {
            for ov in &mut self.overlays {
                if let Some(m) = mode_override {
                    ov.set_mode(m);
                }
                ov.surface.window.focus_window();
            }
            return;
        }

        // Temporarily hide any open Skrinshotter window so it won't obstruct the screenshot
        let mut hid_any = false;
        if let Some(ed) = &self.editor {
            ed.surface.window.set_visible(false);
            hid_any = true;
        }
        if let Some(sw) = &self.settings_window {
            sw.surface.window.set_visible(false);
            hid_any = true;
        }
        if hid_any {
            std::thread::sleep(Duration::from_millis(90));
        }

        match capture_all_displays(event_loop) {
            Ok(displays) => {
                for disp in displays {
                    match OverlayWindow::new(event_loop, &mut self.shared_gpu, disp, initial_mode) {
                        Ok((ov, _gpu)) => {
                            ov.surface.window.request_redraw();
                            self.overlays.push(ov);
                        }
                        Err(err) => {
                            eprintln!("Failed to create monitor overlay: {err}");
                        }
                    }
                }

                if self.overlays.is_empty() {
                    self.restore_hidden_windows();
                }
            }
            Err(err) => {
                eprintln!("Failed to capture screen: {err}");
                self.restore_hidden_windows();
            }
        }
    }

    fn restore_hidden_windows(&self) {
        if let Some(ed) = &self.editor {
            ed.surface.window.set_visible(true);
        }
        if let Some(sw) = &self.settings_window {
            sw.surface.window.set_visible(true);
        }
    }

    fn open_editor_with_capture(
        &mut self,
        event_loop: &ActiveEventLoop,
        captured: image::RgbaImage,
    ) {
        self.overlays.clear();
        // Drop previous editor window (if any) before opening the fresh capture result box
        self.editor = None;
        if let Some(sw) = &self.settings_window {
            sw.surface.window.set_visible(true);
        }

        match EditorWindow::new(event_loop, &mut self.shared_gpu, captured, &self.settings) {
            Ok((ed, _gpu)) => {
                ed.surface.window.request_redraw();
                self.editor = Some(ed);
            }
            Err(err) => {
                eprintln!("Failed to open capture preview window: {err}");
            }
        }
    }

    fn open_settings_dialog(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(existing) = &self.settings_window {
            existing.surface.window.set_visible(true);
            existing.surface.window.focus_window();
            existing.surface.window.request_redraw();
            return;
        }

        let hotkey_active = self.tray.as_ref().map(|t| t.hotkey_active).unwrap_or(false);
        match SettingsWindow::new(
            event_loop,
            &mut self.shared_gpu,
            &self.settings,
            hotkey_active,
        ) {
            Ok((win, _gpu)) => {
                win.surface.window.request_redraw();
                self.settings_window = Some(win);
            }
            Err(err) => {
                eprintln!("Failed to open Settings window: {err}");
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for SkrinshotterApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);

        if self.tray.is_none() {
            match TrayController::new(self.proxy.clone()) {
                Ok(tray) => {
                    println!(
                        "Skrinshotter is running in the background (System Tray / Status Bar). Shortcut: {}",
                        if tray.hotkey_active {
                            "Ctrl+Shift+S"
                        } else {
                            "Click Tray/Status Bar icon"
                        }
                    );
                    self.tray = Some(tray);
                }
                Err(err) => {
                    eprintln!("Failed to initialize System Tray / Status Bar: {err}");
                }
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        event_loop.set_control_flow(ControlFlow::Wait);
        match event {
            UserEvent::StartCapture(mode_opt) => self.begin_area_capture(event_loop, mode_opt),
            UserEvent::OpenSettings => self.open_settings_dialog(event_loop),
            UserEvent::Quit => {
                self.overlays.clear();
                self.editor = None;
                self.settings_window = None;
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        event_loop.set_control_flow(ControlFlow::Wait);
        let Some(gpu) = self.shared_gpu.clone() else {
            return;
        };

        // 1. Check active capture overlays first
        if let Some(idx) = self
            .overlays
            .iter()
            .position(|ov| ov.window_id() == window_id)
        {
            match self.overlays[idx].handle_event(&gpu, &event) {
                OverlayAction::None => {}
                OverlayAction::Cancel => {
                    self.overlays.clear();
                    self.restore_hidden_windows();
                }
                OverlayAction::SwitchMode(new_mode) => {
                    self.settings.default_capture_mode = new_mode;
                    let _ = self.settings.save();
                    for ov in &mut self.overlays {
                        ov.set_mode(new_mode);
                    }
                }
                OverlayAction::Captured {
                    image: cropped_img,
                    mode,
                } => {
                    if self.settings.default_capture_mode != mode {
                        self.settings.default_capture_mode = mode;
                        let _ = self.settings.save();
                    }
                    self.open_editor_with_capture(event_loop, cropped_img);
                }
            }
            return;
        }

        // 2. Check Capture Result / Editor Window
        if let Some(editor) = &mut self.editor {
            if editor.window_id() == window_id {
                if let EditorAction::CloseWindow =
                    editor.handle_event(&gpu, &mut self.settings, &event)
                {
                    // Drop only the editor window; app stays alive in System Tray / Status Bar!
                    self.editor = None;
                }
                return;
            }
        }

        // 3. Check Settings Window
        if let Some(sw) = &mut self.settings_window {
            if sw.window_id() == window_id {
                if let SettingsAction::CloseWindow =
                    sw.handle_event(&gpu, &mut self.settings, &event)
                {
                    self.settings_window = None;
                }
            }
        }
    }
}

fn main() {
    let mut builder = EventLoop::<UserEvent>::with_user_event();

    // On macOS, run as an Accessory (Menu Bar / Status Bar) app without cluttering the Dock.
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }

    let event_loop = builder
        .build()
        .expect("Failed to create main Skrinshotter EventLoop");
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = SkrinshotterApp::new(proxy);

    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("EventLoop exited with error: {err}");
    }
}
