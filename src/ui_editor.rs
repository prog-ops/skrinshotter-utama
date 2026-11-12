use crate::annotation::{
    bake_annotations, paint_stroke_on_egui, AnnotationStroke, AnnotationTool,
};
use crate::gpu::{EguiWindowSurface, SharedGpu};
use crate::metadata::{save_image_with_metadata, ScreenshotMetadata};
use crate::settings::{AppSettings, SaveFormat};
use egui::{Color32, CursorIcon, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use image::RgbaImage;
use std::borrow::Cow;
use std::sync::Arc;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

pub enum EditorAction {
    None,
    CloseWindow,
}

const QUICK_COLORS: [[u8; 4]; 9] = [
    [239, 68, 68, 255],   // Red
    [250, 204, 21, 255],  // Yellow
    [34, 197, 94, 255],   // Green
    [59, 130, 246, 255],  // Blue
    [6, 182, 212, 255],   // Cyan
    [249, 115, 22, 255],  // Orange
    [168, 85, 247, 255],  // Purple
    [255, 255, 255, 255], // White
    [15, 23, 42, 255],    // Black
];

pub struct EditorWindow {
    pub surface: EguiWindowSurface,
    base_image: RgbaImage,
    texture: egui::TextureHandle,
    strokes: Vec<AnnotationStroke>,
    active_stroke: Option<AnnotationStroke>,
    current_tool: AnnotationTool,
    brush_color: Color32,
    brush_thickness: f32,
    metadata: ScreenshotMetadata,
    save_format: SaveFormat,
    jpeg_quality: u8,
    status_message: Option<(String, bool)>,
}

impl EditorWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        shared_gpu: &mut Option<Arc<SharedGpu>>,
        captured_image: RgbaImage,
        settings: &AppSettings,
    ) -> Result<(Self, Arc<SharedGpu>), String> {
        let win_w = f64::from(captured_image.width()).clamp(520.0, 1020.0) + 350.0;
        let win_h = f64::from(captured_image.height()).clamp(440.0, 760.0) + 110.0;

        let attrs = Window::default_attributes()
            .with_title(format!(
                "Skrinshotter — Capture Preview ({} × {} px)",
                captured_image.width(),
                captured_image.height()
            ))
            .with_inner_size(LogicalSize::new(win_w, win_h))
            .with_min_inner_size(LogicalSize::new(780.0, 500.0));

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("Failed to open capture preview window: {e}"))?,
        );
        window.focus_window();

        let (surface, gpu) = EguiWindowSurface::new(window, shared_gpu)?;

        let color_img = egui::ColorImage::from_rgba_unmultiplied(
            [
                captured_image.width() as usize,
                captured_image.height() as usize,
            ],
            captured_image.as_raw(),
        );
        let texture = surface.egui_ctx.load_texture(
            "captured_result_preview",
            color_img,
            egui::TextureOptions::LINEAR,
        );

        Ok((
            Self {
                surface,
                base_image: captured_image,
                texture,
                strokes: Vec::new(),
                active_stroke: None,
                current_tool: AnnotationTool::Pen,
                brush_color: Color32::from_rgb(239, 68, 68),
                brush_thickness: 4.0,
                metadata: ScreenshotMetadata::new(&settings.default_author),
                save_format: settings.default_format,
                jpeg_quality: settings.jpeg_quality,
                status_message: None,
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
    ) -> EditorAction {
        let _ = self.surface.handle_event(event);

        match event {
            WindowEvent::CloseRequested => return EditorAction::CloseWindow,
            WindowEvent::Resized(size) => {
                self.surface.resize(gpu, size.width, size.height);
                self.surface.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                return self.draw(gpu, settings);
            }
            _ => {}
        }

        EditorAction::None
    }

    pub fn draw(&mut self, gpu: &SharedGpu, settings: &mut AppSettings) -> EditorAction {
        let mut action = EditorAction::None;
        let mut trigger_save_dialog = false;
        let mut trigger_copy_clipboard = false;

        let img_w = self.base_image.width() as f32;
        let img_h = self.base_image.height() as f32;
        let tex_id = self.texture.id();

        let mut current_tool = self.current_tool;
        let mut brush_color = self.brush_color;
        let mut brush_thickness = self.brush_thickness;
        let mut strokes = std::mem::take(&mut self.strokes);
        let mut active_stroke = self.active_stroke.take();
        let mut metadata = self.metadata.clone();
        let mut save_format = self.save_format;
        let mut jpeg_quality = self.jpeg_quality;
        let mut status_message = self.status_message.clone();

        self.surface.render(gpu, |ctx| {
            // Keyboard shortcuts (Ctrl/Cmd+S to save; Ctrl/Cmd+Z to undo stroke when no text input is focused)
            let any_text_focused = ctx.memory(|m| m.focused().is_some());
            ctx.input(|i| {
                if i.modifiers.command && i.key_pressed(egui::Key::S) {
                    trigger_save_dialog = true;
                }
                if !any_text_focused && i.modifiers.command && i.key_pressed(egui::Key::Z) {
                    strokes.pop();
                }
            });

            // 1. Top Annotation Toolbar (Tools + Any-Color Picker + Swatches + Thickness + Undo/Clear)
            egui::TopBottomPanel::top("editor_annotation_toolbar")
                .exact_height(48.0)
                .show(ctx, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;

                        for tool in [
                            AnnotationTool::Pen,
                            AnnotationTool::Highlighter,
                            AnnotationTool::Rectangle,
                            AnnotationTool::Arrow,
                        ] {
                            let selected = current_tool == tool;
                            if ui.selectable_label(selected, tool.label()).clicked() {
                                current_tool = tool;
                            }
                        }

                        ui.separator();
                        ui.label("Color:");
                        // Full RGBA/HSV/Hex color picker allowing ANY color
                        ui.color_edit_button_srgba(&mut brush_color)
                            .on_hover_text("Click to pick any custom color (RGB / HSV / Hex)");

                        // Quick color preset swatches
                        for &[r, g, b, a] in &QUICK_COLORS {
                            let swatch_col = Color32::from_rgba_unmultiplied(r, g, b, a);
                            let (rect, resp) =
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                            ui.painter().rect_filled(rect, 4.0, swatch_col);
                            let is_active = brush_color.r() == r
                                && brush_color.g() == g
                                && brush_color.b() == b;
                            let border_col = if is_active {
                                Color32::WHITE
                            } else {
                                Color32::from_gray(90)
                            };
                            ui.painter().rect_stroke(
                                rect,
                                4.0,
                                Stroke::new(if is_active { 2.0_f32 } else { 1.0_f32 }, border_col),
                                egui::StrokeKind::Inside,
                            );
                            if resp.clicked() {
                                brush_color = swatch_col;
                            }
                        }

                        ui.separator();
                        ui.label("Thickness:");
                        ui.add(
                            egui::Slider::new(&mut brush_thickness, 1.0..=32.0)
                                .suffix(" px")
                                .show_value(true),
                        );

                        ui.separator();
                        if ui
                            .add_enabled(!strokes.is_empty(), egui::Button::new("↩ Undo"))
                            .on_hover_text("Undo last annotation stroke (Ctrl+Z)")
                            .clicked()
                        {
                            strokes.pop();
                        }
                        if ui
                            .add_enabled(!strokes.is_empty(), egui::Button::new("🗑 Clear Canvas"))
                            .on_hover_text("Clear all annotations")
                            .clicked()
                        {
                            strokes.clear();
                        }
                    });
                });

            // 2. Right Side Panel: Metadata Form Fields & Save Controls
            egui::SidePanel::right("editor_metadata_panel")
                .resizable(false)
                .exact_width(330.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add_space(6.0);
                        ui.heading("Metadata & Save");
                        ui.label(
                            RichText::new(format!(
                                "Resolution: {} × {} px  •  {}",
                                img_w as u32,
                                img_h as u32,
                                metadata.captured_at.format("%d %b %Y %H:%M:%S")
                            ))
                                .small()
                                .color(Color32::from_gray(165)),
                        );
                        ui.separator();

                        // Field 1: Title
                        ui.label(RichText::new("Title").strong());
                        ui.add_sized(
                            [ui.available_width(), 26.0],
                            egui::TextEdit::singleline(&mut metadata.title)
                                .hint_text("e.g., Login Page Bug"),
                        );
                        ui.add_space(8.0);

                        // Field 2: Description / Comment
                        ui.label(RichText::new("Description / Comment").strong());
                        ui.add(
                            egui::TextEdit::multiline(&mut metadata.description)
                                .desired_width(f32::INFINITY)
                                .desired_rows(4)
                                .hint_text("Write a detailed description or note for this screenshot..."),
                        );
                        ui.add_space(8.0);

                        // Field 3: Author (+ Save as Default in Settings)
                        ui.label(RichText::new("Author").strong());
                        ui.add_sized(
                            [ui.available_width(), 26.0],
                            egui::TextEdit::singleline(&mut metadata.author)
                                .hint_text("Screenshot author name"),
                        );
                        ui.horizontal(|ui| {
                            let trimmed_curr = metadata.author.trim();
                            let trimmed_def = settings.default_author.trim();
                            let is_same_default =
                                !trimmed_curr.is_empty() && trimmed_curr == trimmed_def;

                            if is_same_default {
                                ui.label(
                                    RichText::new("✓ Saved as Default Author")
                                        .small()
                                        .color(Color32::from_rgb(74, 222, 128)),
                                );
                            } else if ui
                                .small_button("📌 Save as Default in Settings")
                                .on_hover_text(
                                    "Save this Author name to auto-fill on future captures",
                                )
                                .clicked()
                            {
                                settings.default_author = trimmed_curr.to_string();
                                status_message = match settings.save() {
                                    Ok(()) => Some((
                                        "Default Author saved to Settings.".to_string(),
                                        false,
                                    )),
                                    Err(e) => Some((e, true)),
                                };
                            }
                        });
                        ui.add_space(8.0);

                        // Field 4: Tags / Keywords (comma-separated)
                        ui.label(RichText::new("Tags / Keywords (comma-separated)").strong());
                        ui.add_sized(
                            [ui.available_width(), 26.0],
                            egui::TextEdit::singleline(&mut metadata.tags_raw)
                                .hint_text("e.g., bug, checkout, ui"),
                        );

                        let parsed_tags = metadata.parsed_tags();
                        if !parsed_tags.is_empty() {
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                for tag in &parsed_tags {
                                    egui::Frame::NONE
                                        .fill(Color32::from_rgb(30, 58, 95))
                                        .corner_radius(4.0)
                                        .inner_margin(egui::Margin::symmetric(6, 2))
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(format!("#{tag}"))
                                                    .small()
                                                    .color(Color32::from_rgb(147, 197, 253)),
                                            );
                                        });
                                }
                            });
                        }

                        ui.add_space(10.0);
                        ui.separator();

                        // Field 5: Save as PNG or JPEG
                        ui.label(RichText::new("File Format (Save As)").strong());
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut save_format, SaveFormat::Png, "PNG (Lossless)");
                            ui.radio_value(&mut save_format, SaveFormat::Jpeg, "JPEG");
                        });

                        match save_format {
                            SaveFormat::Png => {
                                ui.label(
                                    RichText::new(
                                        "Sharp lossless quality & preserves transparency outside Freeform cutouts. Metadata via PNG iTXt + XMP + EXIF.",
                                    )
                                        .small()
                                        .color(Color32::from_gray(155)),
                                );
                            }
                            SaveFormat::Jpeg => {
                                ui.horizontal(|ui| {
                                    ui.label("Quality:");
                                    ui.add(
                                        egui::Slider::new(&mut jpeg_quality, 50..=100).suffix("%"),
                                    );
                                });
                                ui.label(
                                    RichText::new(
                                        "Freeform transparent areas are composited over white. Metadata via EXIF + Windows Details + XMP.",
                                    )
                                        .small()
                                        .color(Color32::from_gray(155)),
                                );
                            }
                        }

                        ui.add_space(14.0);

                        // Primary Save Button
                        let save_btn = egui::Button::new(
                            RichText::new(format!(
                                "💾 Save as {}...",
                                save_format.extension().to_ascii_uppercase()
                            ))
                                .strong()
                                .size(15.0)
                                .color(Color32::WHITE),
                        )
                            .fill(Color32::from_rgb(37, 99, 235))
                            .min_size(Vec2::new(ui.available_width(), 38.0));

                        if ui
                            .add(save_btn)
                            .on_hover_text("Save screenshot with annotations & metadata (Ctrl+S)")
                            .clicked()
                        {
                            trigger_save_dialog = true;
                        }

                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let half_w = (ui.available_width() - 8.0) * 0.5;
                            if ui
                                .add_sized(
                                    [half_w, 30.0],
                                    egui::Button::new("📋 Copy Image"),
                                )
                                .on_hover_text("Copy image with annotations to Clipboard")
                                .clicked()
                            {
                                trigger_copy_clipboard = true;
                            }

                            if ui
                                .add_sized([half_w, 30.0], egui::Button::new("✖ Close"))
                                .on_hover_text(
                                    "Close this window (app continues running in System Tray / Status Bar)",
                                )
                                .clicked()
                            {
                                action = EditorAction::CloseWindow;
                            }
                        });

                        if let Some((msg, is_err)) = &status_message {
                            ui.add_space(10.0);
                            let bg = if *is_err {
                                Color32::from_rgb(95, 25, 25)
                            } else {
                                Color32::from_rgb(20, 75, 45)
                            };
                            let fg = if *is_err {
                                Color32::from_rgb(254, 202, 202)
                            } else {
                                Color32::from_rgb(187, 247, 208)
                            };
                            egui::Frame::NONE
                                .fill(bg)
                                .corner_radius(6.0)
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    ui.label(RichText::new(msg).small().color(fg));
                                });
                        }
                    });
                });

            // 3. Central Canvas: Interactive Screenshot Preview + Freehand/Shape Annotation
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::NONE
                        .fill(Color32::from_rgb(16, 18, 23))
                        .inner_margin(egui::Margin::same(14)),
                )
                .show(ctx, |ui| {
                    let avail = ui.available_rect_before_wrap();
                    if avail.width() <= 4.0 || avail.height() <= 4.0 || img_w <= 0.0 || img_h <= 0.0
                    {
                        return;
                    }

                    // Fit image inside available canvas while preserving aspect ratio
                    let scale = (avail.width() / img_w)
                        .min(avail.height() / img_h)
                        .min(2.0);
                    let disp_size = Vec2::new(img_w * scale, img_h * scale);
                    let img_rect = Rect::from_center_size(avail.center(), disp_size);

                    let resp = ui.allocate_rect(img_rect, Sense::drag());
                    if resp.hovered() || resp.is_pointer_button_down_on() {
                        ctx.set_cursor_icon(CursorIcon::Crosshair);
                    }

                    let painter = ui.painter();
                    // Subtle backdrop border around canvas
                    painter.rect_stroke(
                        img_rect.expand(1.5),
                        2.0,
                        Stroke::new(1.5_f32, Color32::from_gray(55)),
                        egui::StrokeKind::Outside,
                    );

                    // Checkerboard background so Freeform transparent cutouts are clearly visible
                    paint_transparency_checkerboard(&painter.with_clip_rect(img_rect), img_rect);

                    // Base screenshot texture
                    painter.image(
                        tex_id,
                        img_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );

                    let to_img_coords = |pos: Pos2| -> [f32; 2] {
                        let x = ((pos.x - img_rect.min.x) / img_rect.width().max(1.0)) * img_w;
                        let y = ((pos.y - img_rect.min.y) / img_rect.height().max(1.0)) * img_h;
                        [x.clamp(0.0, img_w), y.clamp(0.0, img_h)]
                    };

                    let [r, g, b, a] = brush_color.to_srgba_unmultiplied();

                    // Zero-deadzone drawing while respecting egui popups (e.g. open Color Picker)
                    if resp.is_pointer_button_down_on() {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let pt = to_img_coords(pos);
                            if let Some(stroke) = active_stroke.as_mut() {
                                match stroke.tool {
                                    AnnotationTool::Pen | AnnotationTool::Highlighter => {
                                        let should_push = stroke
                                            .points
                                            .last()
                                            .map(|last| {
                                                let dx = pt[0] - last[0];
                                                let dy = pt[1] - last[1];
                                                (dx * dx + dy * dy) >= 0.5
                                            })
                                            .unwrap_or(true);
                                        if should_push {
                                            stroke.points.push(pt);
                                        }
                                    }
                                    AnnotationTool::Rectangle | AnnotationTool::Arrow => {
                                        if stroke.points.len() >= 2 {
                                            stroke.points[1] = pt;
                                        } else {
                                            stroke.points.push(pt);
                                        }
                                    }
                                }
                            } else {
                                let pts = match current_tool {
                                    AnnotationTool::Pen | AnnotationTool::Highlighter => vec![pt],
                                    AnnotationTool::Rectangle | AnnotationTool::Arrow => {
                                        vec![pt, pt]
                                    }
                                };
                                active_stroke = Some(AnnotationStroke {
                                    tool: current_tool,
                                    color: [r, g, b, a],
                                    thickness: brush_thickness,
                                    points: pts,
                                });
                            }
                        }
                    } else if let Some(finished) = active_stroke.take() {
                        if !finished.points.is_empty() {
                            strokes.push(finished);
                        }
                    }

                    // Render committed + active strokes clipped cleanly to the image bounds
                    let clipped = painter.with_clip_rect(img_rect);
                    for s in &strokes {
                        paint_stroke_on_egui(&clipped, s, img_rect, [img_w, img_h]);
                    }
                    if let Some(s) = &active_stroke {
                        paint_stroke_on_egui(&clipped, s, img_rect, [img_w, img_h]);
                    }
                });
        });

        self.current_tool = current_tool;
        self.brush_color = brush_color;
        self.brush_thickness = brush_thickness;
        self.strokes = strokes;
        self.active_stroke = active_stroke;
        self.metadata = metadata;
        self.save_format = save_format;
        self.jpeg_quality = jpeg_quality;
        self.status_message = status_message;

        if trigger_copy_clipboard {
            let final_img = bake_annotations(&self.base_image, &self.strokes);
            match arboard::Clipboard::new().and_then(|mut cb| {
                cb.set_image(arboard::ImageData {
                    width: final_img.width() as usize,
                    height: final_img.height() as usize,
                    bytes: Cow::Borrowed(final_img.as_raw()),
                })
            }) {
                Ok(()) => {
                    self.status_message = Some((
                        "✓ Image with annotations copied to Clipboard.".to_string(),
                        false,
                    ));
                }
                Err(e) => {
                    self.status_message =
                        Some((format!("Failed to copy to Clipboard: {e}"), true));
                }
            }
            self.surface.window.request_redraw();
        }

        if trigger_save_dialog {
            self.execute_save_dialog(settings);
            self.surface.window.request_redraw();
        }

        action
    }

    fn execute_save_dialog(&mut self, settings: &mut AppSettings) {
        let suggested = self.metadata.suggested_filename(self.save_format);
        let start_dir = settings.preferred_save_dir();

        let dialog = match self.save_format {
            SaveFormat::Png => rfd::FileDialog::new()
                .set_directory(&start_dir)
                .set_file_name(&suggested)
                .add_filter("PNG Image (*.png)", SaveFormat::Png.filter_extensions())
                .add_filter(
                    "JPEG Image (*.jpg, *.jpeg)",
                    SaveFormat::Jpeg.filter_extensions(),
                ),
            SaveFormat::Jpeg => rfd::FileDialog::new()
                .set_directory(&start_dir)
                .set_file_name(&suggested)
                .add_filter(
                    "JPEG Image (*.jpg, *.jpeg)",
                    SaveFormat::Jpeg.filter_extensions(),
                )
                .add_filter("PNG Image (*.png)", SaveFormat::Png.filter_extensions()),
        };

        let Some(mut target_path) = dialog.save_file() else {
            return;
        };

        // Respect explicit extension typed by user, otherwise append selected format extension
        let chosen_format = if let Some(fmt) = SaveFormat::from_path(&target_path) {
            self.save_format = fmt;
            fmt
        } else {
            target_path.set_extension(self.save_format.extension());
            self.save_format
        };

        let final_image = bake_annotations(&self.base_image, &self.strokes);

        match save_image_with_metadata(
            &target_path,
            &final_image,
            chosen_format,
            self.jpeg_quality,
            &self.metadata,
        ) {
            Ok(()) => {
                if let Some(parent) = target_path.parent() {
                    settings.last_save_dir = Some(parent.to_path_buf());
                }
                settings.default_format = chosen_format;
                settings.jpeg_quality = self.jpeg_quality;
                let _ = settings.save();

                self.status_message = Some((
                    format!("✓ Successfully saved to:\n{}", target_path.display()),
                    false,
                ));
            }
            Err(err) => {
                self.status_message = Some((err, true));
            }
        }
    }
}

fn paint_transparency_checkerboard(painter: &egui::Painter, rect: Rect) {
    let dark_tile = Color32::from_rgb(34, 38, 46);
    let light_tile = Color32::from_rgb(49, 55, 66);
    painter.rect_filled(rect, 0.0, dark_tile);

    let tile = 14.0f32;
    let cols = (rect.width() / tile).ceil() as usize;
    let rows = (rect.height() / tile).ceil() as usize;

    for r in 0..rows {
        let y0 = rect.min.y + r as f32 * tile;
        let y1 = (y0 + tile).min(rect.max.y);
        for c in ((r & 1)..cols).step_by(2) {
            let x0 = rect.min.x + c as f32 * tile;
            let x1 = (x0 + tile).min(rect.max.x);
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1)),
                0.0,
                light_tile,
            );
        }
    }
}
