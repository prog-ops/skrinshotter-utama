use crate::gpu::{EguiWindowSurface, SharedGpu};
use crate::settings::{AppSettings, CaptureMode, SaveFormat};
use egui::{Color32, RichText, Vec2};
use std::sync::Arc;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

pub enum SettingsAction {
    None,
    CloseWindow,
}

pub struct SettingsWindow {
    pub surface: EguiWindowSurface,
    draft_author: String,
    draft_capture_mode: CaptureMode,
    draft_format: SaveFormat,
    draft_jpeg_quality: u8,
    hotkey_active: bool,
    status_note: Option<(String, bool)>,
}

impl SettingsWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        shared_gpu: &mut Option<Arc<SharedGpu>>,
        settings: &AppSettings,
        hotkey_active: bool,
    ) -> Result<(Self, Arc<SharedGpu>), String> {
        let attrs = Window::default_attributes()
            .with_title("Skrinshotter — Settings")
            .with_inner_size(LogicalSize::new(450.0, 370.0))
            .with_resizable(false);

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("Failed to open Settings window: {e}"))?,
        );
        window.focus_window();

        let (surface, gpu) = EguiWindowSurface::new(window, shared_gpu)?;

        Ok((
            Self {
                surface,
                draft_author: settings.default_author.clone(),
                draft_capture_mode: settings.default_capture_mode,
                draft_format: settings.default_format,
                draft_jpeg_quality: settings.jpeg_quality,
                hotkey_active,
                status_note: None,
            },
            gpu,
        ))
    }

    pub fn window_id(&self) -> WindowId {
        self.surface.window.id()
    }

    pub fn handle_event(
        &mut self,
        gpu: &SharedGpu,
        settings: &mut AppSettings,
        event: &WindowEvent,
    ) -> SettingsAction {
        let _ = self.surface.handle_event(event);

        match event {
            WindowEvent::CloseRequested => return SettingsAction::CloseWindow,
            WindowEvent::Resized(size) => {
                self.surface.resize(gpu, size.width, size.height);
                self.surface.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                return self.draw(gpu, settings);
            }
            _ => {}
        }

        SettingsAction::None
    }

    pub fn draw(&mut self, gpu: &SharedGpu, settings: &mut AppSettings) -> SettingsAction {
        let mut action = SettingsAction::None;
        let mut draft_author = self.draft_author.clone();
        let mut draft_capture_mode = self.draft_capture_mode;
        let mut draft_format = self.draft_format;
        let mut draft_quality = self.draft_jpeg_quality;
        let hotkey_active = self.hotkey_active;
        let mut status_note = self.status_note.clone();

        self.surface.render(gpu, |ctx| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::NONE
                        .fill(Color32::from_rgb(24, 26, 32))
                        .inner_margin(egui::Margin::same(18)),
                )
                .show(ctx, |ui| {
                    ui.heading("Skrinshotter Settings");
                    ui.add_space(4.0);
                    let hk_text = if hotkey_active {
                        "Global Shortcut Active: Ctrl + Shift + S  (Press Tab during capture to switch mode)"
                    } else {
                        "Use the System Tray / Status Bar icon to start a Capture"
                    };
                    ui.label(
                        RichText::new(hk_text)
                            .small()
                            .color(Color32::from_rgb(125, 211, 252)),
                    );
                    ui.separator();
                    ui.add_space(6.0);

                    ui.label(RichText::new("Default Author").strong());
                    ui.label(
                        RichText::new(
                            "Automatically fills the Author field after every screenshot capture.",
                        )
                        .small()
                        .color(Color32::from_gray(160)),
                    );
                    ui.add_sized(
                        [ui.available_width(), 28.0],
                        egui::TextEdit::singleline(&mut draft_author)
                            .hint_text("e.g., Your Name / Team Name"),
                    );

                    ui.add_space(12.0);
                    ui.label(RichText::new("Default Capture Mode").strong());
                    ui.horizontal(|ui| {
                        ui.radio_value(
                            &mut draft_capture_mode,
                            CaptureMode::Rectangle,
                            CaptureMode::Rectangle.label(),
                        );
                        ui.radio_value(
                            &mut draft_capture_mode,
                            CaptureMode::Freeform,
                            CaptureMode::Freeform.label(),
                        );
                    });

                    ui.add_space(12.0);
                    ui.label(RichText::new("Default Save Format").strong());
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut draft_format, SaveFormat::Png, SaveFormat::Png.label());
                        ui.radio_value(
                            &mut draft_format,
                            SaveFormat::Jpeg,
                            SaveFormat::Jpeg.label(),
                        );
                    });

                    if draft_format == SaveFormat::Jpeg {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("Default JPEG Quality:");
                            ui.add(egui::Slider::new(&mut draft_quality, 50..=100).suffix("%"));
                        });
                    }

                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        let btn_save = egui::Button::new(
                            RichText::new("💾 Save Settings")
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(37, 99, 235))
                        .min_size(Vec2::new(165.0, 32.0));

                        if ui.add(btn_save).clicked() {
                            settings.default_author = draft_author.trim().to_string();
                            settings.default_capture_mode = draft_capture_mode;
                            settings.default_format = draft_format;
                            settings.jpeg_quality = draft_quality;
                            status_note = match settings.save() {
                                Ok(()) => {
                                    Some(("✓ Settings saved successfully.".to_string(), false))
                                }
                                Err(e) => Some((e, true)),
                            };
                        }

                        if ui
                            .add_sized([100.0, 32.0], egui::Button::new("Close"))
                            .clicked()
                        {
                            action = SettingsAction::CloseWindow;
                        }
                    });

                    if let Some((msg, is_err)) = &status_note {
                        ui.add_space(8.0);
                        let color = if *is_err {
                            Color32::from_rgb(248, 113, 113)
                        } else {
                            Color32::from_rgb(74, 222, 128)
                        };
                        ui.label(RichText::new(msg).small().color(color));
                    }
                });
        });

        self.draft_author = draft_author;
        self.draft_capture_mode = draft_capture_mode;
        self.draft_format = draft_format;
        self.draft_jpeg_quality = draft_quality;
        self.status_note = status_note;

        action
    }
}

