use crate::capture::{crop_captured_area, crop_freeform_area, CapturedDisplay};
use crate::gpu::{EguiWindowSurface, SharedGpu};
use crate::settings::CaptureMode;
use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Order, Painter, Pos2, Rect, RichText, Sense, Shape,
    Stroke, Vec2,
};
use image::RgbaImage;
use std::sync::Arc;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

pub enum OverlayAction {
    None,
    Cancel,
    SwitchMode(CaptureMode),
    Captured {
        image: RgbaImage,
        mode: CaptureMode,
    },
}

pub struct OverlayWindow {
    pub surface: EguiWindowSurface,
    raw_image: RgbaImage,
    texture: egui::TextureHandle,
    mode: CaptureMode,
    drag_start: Option<Pos2>,
    drag_current: Option<Pos2>,
    lasso_points: Vec<Pos2>,
    last_toolbar_rect: Option<Rect>,
}

impl OverlayWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        shared_gpu: &mut Option<Arc<SharedGpu>>,
        display: CapturedDisplay,
        initial_mode: CaptureMode,
    ) -> Result<(Self, Arc<SharedGpu>), String> {
        let mut attrs = Window::default_attributes()
            .with_title("Skrinshotter — Select Area")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop);

        if let Some(monitor_handle) = display.monitor_handle {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(Some(monitor_handle))));
        } else {
            attrs = attrs
                .with_position(PhysicalPosition::new(display.x, display.y))
                .with_inner_size(PhysicalSize::new(display.width, display.height));
        }

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("Failed to create capture overlay window: {e}"))?,
        );
        window.focus_window();

        let (surface, gpu) = EguiWindowSurface::new(window, shared_gpu)?;

        let color_img = egui::ColorImage::from_rgba_unmultiplied(
            [
                display.image.width() as usize,
                display.image.height() as usize,
            ],
            display.image.as_raw(),
        );
        let texture = surface.egui_ctx.load_texture(
            "frozen_monitor_capture",
            color_img,
            egui::TextureOptions::LINEAR,
        );

        Ok((
            Self {
                surface,
                raw_image: display.image,
                texture,
                mode: initial_mode,
                drag_start: None,
                drag_current: None,
                lasso_points: Vec::new(),
                last_toolbar_rect: None,
            },
            gpu,
        ))
    }

    pub fn window_id(&self) -> WindowId {
        self.surface.window.id()
    }

    pub fn set_mode(&mut self, mode: CaptureMode) {
        if self.mode != mode {
            self.mode = mode;
            self.drag_start = None;
            self.drag_current = None;
            self.lasso_points.clear();
            self.surface.window.request_redraw();
        }
    }

    pub fn handle_event(&mut self, gpu: &SharedGpu, event: &WindowEvent) -> OverlayAction {
        let _ = self.surface.handle_event(event);

        match event {
            WindowEvent::CloseRequested => return OverlayAction::Cancel,
            WindowEvent::Resized(size) => {
                self.surface.resize(gpu, size.width, size.height);
                self.surface.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                return self.draw(gpu);
            }
            _ => {}
        }

        OverlayAction::None
    }

    pub fn draw(&mut self, gpu: &SharedGpu) -> OverlayAction {
        let mut action = OverlayAction::None;
        let tex_id = self.texture.id();
        let img_w = self.raw_image.width();
        let img_h = self.raw_image.height();

        let mut mode = self.mode;
        let mut requested_mode: Option<CaptureMode> = None;
        let mut drag_start = self.drag_start;
        let mut drag_current = self.drag_current;
        let mut lasso_points = std::mem::take(&mut self.lasso_points);
        let mut last_toolbar_rect = self.last_toolbar_rect;

        let mut completed_rect: Option<(Rect, Vec2)> = None;
        let mut completed_freeform: Option<(Vec<Pos2>, Vec2)> = None;

        self.surface.render(gpu, |ctx| {
            let cancel_pressed = ctx
                .input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.secondary_clicked());
            if cancel_pressed {
                action = OverlayAction::Cancel;
                return;
            }

            // Keyboard shortcuts to switch between Rectangle and Freeform selection modes
            let is_actively_dragging = drag_start.is_some() || !lasso_points.is_empty();
            if !is_actively_dragging {
                ctx.input(|i| {
                    if i.key_pressed(egui::Key::Tab) || i.key_pressed(egui::Key::F) {
                        let next = match mode {
                            CaptureMode::Rectangle => CaptureMode::Freeform,
                            CaptureMode::Freeform => CaptureMode::Rectangle,
                        };
                        mode = next;
                        requested_mode = Some(next);
                    } else if i.key_pressed(egui::Key::Num1) {
                        if mode != CaptureMode::Rectangle {
                            mode = CaptureMode::Rectangle;
                            requested_mode = Some(CaptureMode::Rectangle);
                        }
                    } else if i.key_pressed(egui::Key::Num2) {
                        if mode != CaptureMode::Freeform {
                            mode = CaptureMode::Freeform;
                            requested_mode = Some(CaptureMode::Freeform);
                        }
                    }
                });
            }

            let (primary_pressed, primary_down, primary_released, pointer_pos) = ctx.input(|i| {
                (
                    i.pointer.primary_pressed(),
                    i.pointer.primary_down(),
                    i.pointer.primary_released(),
                    i.pointer.interact_pos().or_else(|| i.pointer.hover_pos()),
                )
            });

            let pointer_on_toolbar = match (pointer_pos, last_toolbar_rect) {
                (Some(p), Some(r)) if !is_actively_dragging => r.expand(4.0).contains(p),
                _ => false,
            };

            if !pointer_on_toolbar {
                ctx.set_cursor_icon(CursorIcon::Crosshair);
            }

            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let screen_size = ui.available_size();
                    let (screen_rect, _resp) =
                        ui.allocate_exact_size(screen_size, Sense::hover());
                    let painter = ui.painter_at(screen_rect);

                    // 1. Frozen monitor screenshot background
                    painter.image(
                        tex_id,
                        screen_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );

                    // 2. Process pointer selection according to active CaptureMode
                    match mode {
                        CaptureMode::Rectangle => {
                            if primary_pressed && !pointer_on_toolbar {
                                if let Some(pos) = pointer_pos {
                                    if screen_rect.contains(pos) {
                                        drag_start = Some(pos);
                                        drag_current = Some(pos);
                                    }
                                }
                            } else if primary_down {
                                if drag_start.is_some() {
                                    if let Some(pos) = pointer_pos {
                                        drag_current =
                                            Some(pos.clamp(screen_rect.min, screen_rect.max));
                                    }
                                }
                            } else if primary_released {
                                if let (Some(start), Some(end)) =
                                    (drag_start, pointer_pos.or(drag_current))
                                {
                                    let clamped_end =
                                        end.clamp(screen_rect.min, screen_rect.max);
                                    let sel = Rect::from_two_pos(start, clamped_end);
                                    let local_sel = sel.translate(-screen_rect.min.to_vec2());
                                    completed_rect = Some((local_sel, screen_rect.size()));
                                }
                                drag_start = None;
                                drag_current = None;
                            }
                        }
                        CaptureMode::Freeform => {
                            if primary_pressed && !pointer_on_toolbar {
                                if let Some(pos) = pointer_pos {
                                    if screen_rect.contains(pos) {
                                        lasso_points.clear();
                                        lasso_points.push(pos);
                                    }
                                }
                            } else if primary_down {
                                if !lasso_points.is_empty() {
                                    if let Some(pos) = pointer_pos {
                                        let clamped =
                                            pos.clamp(screen_rect.min, screen_rect.max);
                                        let should_add = lasso_points
                                            .last()
                                            .map(|last| last.distance_sq(clamped) >= 2.25)
                                            .unwrap_or(true);
                                        if should_add {
                                            lasso_points.push(clamped);
                                        }
                                    }
                                }
                            } else if primary_released && !lasso_points.is_empty() {
                                if let Some(pos) = pointer_pos {
                                    let clamped = pos.clamp(screen_rect.min, screen_rect.max);
                                    if lasso_points
                                        .last()
                                        .map(|l| l.distance_sq(clamped) >= 1.0)
                                        .unwrap_or(false)
                                    {
                                        lasso_points.push(clamped);
                                    }
                                }
                                let offset = screen_rect.min.to_vec2();
                                let local_pts: Vec<Pos2> =
                                    lasso_points.iter().map(|&p| p - offset).collect();
                                completed_freeform = Some((local_pts, screen_rect.size()));
                                lasso_points.clear();
                            }
                        }
                    }

                    // 3. Paint visual overlay for active selection or idle state
                    let dim_color = Color32::from_black_alpha(130);

                    if let (CaptureMode::Rectangle, Some(start), Some(curr)) =
                        (mode, drag_start, drag_current)
                    {
                        let sel = Rect::from_two_pos(start, curr).intersect(screen_rect);
                        paint_dimmed_surround(&painter, screen_rect, sel, dim_color);

                        painter.rect_stroke(
                            sel.expand(1.0),
                            0.0,
                            Stroke::new(1.0_f32, Color32::from_black_alpha(190)),
                            egui::StrokeKind::Outside,
                        );
                        let accent = Color32::from_rgb(56, 189, 248);
                        painter.rect_stroke(
                            sel,
                            0.0,
                            Stroke::new(2.0_f32, accent),
                            egui::StrokeKind::Middle,
                        );

                        for corner in [
                            sel.left_top(),
                            sel.right_top(),
                            sel.left_bottom(),
                            sel.right_bottom(),
                        ] {
                            let handle = Rect::from_center_size(corner, Vec2::splat(7.0));
                            painter.rect_filled(handle, 1.5, Color32::WHITE);
                            painter.rect_stroke(
                                handle,
                                1.5,
                                Stroke::new(1.0_f32, accent),
                                egui::StrokeKind::Middle,
                            );
                        }

                        let px_w = ((sel.width() / screen_rect.width().max(1.0)) * img_w as f32)
                            .round() as u32;
                        let px_h = ((sel.height() / screen_rect.height().max(1.0)) * img_h as f32)
                            .round() as u32;
                        paint_dimension_badge(
                            &painter,
                            screen_rect,
                            sel,
                            &format!("{px_w} × {px_h} px"),
                        );
                    } else if mode == CaptureMode::Freeform && !lasso_points.is_empty() {
                        let mut path = lasso_points.clone();
                        if let Some(curr) = pointer_pos {
                            let clamped = curr.clamp(screen_rect.min, screen_rect.max);
                            if path
                                .last()
                                .map(|l| l.distance_sq(clamped) >= 0.25)
                                .unwrap_or(false)
                            {
                                path.push(clamped);
                            }
                        }

                        // Compute bounding box of current freeform lasso
                        let mut bounds = Rect::from_min_max(path[0], path[0]);
                        for &pt in &path[1..] {
                            bounds.extend_with(pt);
                        }
                        let focus_box = bounds.intersect(screen_rect);

                        // Dim outside bounding box strongly, and lightly inside so the bright lasso line pops
                        paint_dimmed_surround(&painter, screen_rect, focus_box, dim_color);
                        painter.rect_filled(focus_box, 0.0, Color32::from_black_alpha(35));

                        let accent = Color32::from_rgb(52, 211, 153); // Emerald-cyan highlight for Freeform
                        if path.len() >= 2 {
                            // Automatic dashed closing preview line back to start vertex
                            let first = path[0];
                            let last = *path.last().unwrap();
                            if first.distance(last) > 4.0 {
                                draw_dashed_line(
                                    &painter,
                                    last,
                                    first,
                                    6.0,
                                    5.0,
                                    Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(52, 211, 153, 185)),
                                );
                            }

                            // Crisp dual-stroke lasso trail (dark outer halo + bright inner line)
                            painter.add(Shape::line(
                                path.clone(),
                                Stroke::new(4.0_f32, Color32::from_black_alpha(200)),
                            ));
                            painter.add(Shape::line(path, Stroke::new(2.2_f32, accent)));
                        }

                        // Anchor dot at start point
                        let start_pt = lasso_points[0];
                        painter.circle_filled(start_pt, 5.0, Color32::WHITE);
                        painter.circle_stroke(start_pt, 5.0, Stroke::new(2.0_f32, accent));

                        let px_w = ((focus_box.width() / screen_rect.width().max(1.0))
                            * img_w as f32)
                            .round() as u32;
                        let px_h = ((focus_box.height() / screen_rect.height().max(1.0))
                            * img_h as f32)
                            .round() as u32;
                        paint_dimension_badge(
                            &painter,
                            screen_rect,
                            focus_box,
                            &format!("{px_w} × {px_h} px • Freeform"),
                        );
                    } else {
                        // Idle state before dragging starts
                        painter.rect_filled(screen_rect, 0.0, Color32::from_black_alpha(95));

                        if let Some(pos) = pointer_pos {
                            if screen_rect.contains(pos) && !pointer_on_toolbar {
                                let guide_color = match mode {
                                    CaptureMode::Rectangle => Color32::from_white_alpha(90),
                                    CaptureMode::Freeform => {
                                        Color32::from_rgba_unmultiplied(52, 211, 153, 110)
                                    }
                                };
                                let guide_stroke = Stroke::new(1.0_f32, guide_color);
                                painter.line_segment(
                                    [
                                        Pos2::new(screen_rect.min.x, pos.y),
                                        Pos2::new(screen_rect.max.x, pos.y),
                                    ],
                                    guide_stroke,
                                );
                                painter.line_segment(
                                    [
                                        Pos2::new(pos.x, screen_rect.min.y),
                                        Pos2::new(pos.x, screen_rect.max.y),
                                    ],
                                    guide_stroke,
                                );
                            }
                        }

                        // Top-center interactive mode selector & hint bar
                        let area_resp = egui::Area::new(Id::new("capture_mode_toolbar"))
                            .order(Order::Foreground)
                            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 24.0))
                            .show(ctx, |ui| {
                                let border_col = match mode {
                                    CaptureMode::Rectangle => Color32::from_rgb(56, 189, 248),
                                    CaptureMode::Freeform => Color32::from_rgb(52, 211, 153),
                                };
                                egui::Frame::NONE
                                    .fill(Color32::from_rgba_unmultiplied(15, 23, 42, 238))
                                    .stroke(Stroke::new(1.2_f32, border_col))
                                    .corner_radius(10.0)
                                    .inner_margin(egui::Margin::symmetric(14, 9))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 8.0;

                                            // Mode toggle buttons
                                            let is_rect = mode == CaptureMode::Rectangle;
                                            let rect_btn = egui::Button::new(
                                                RichText::new("▭ Rectangle (1)")
                                                    .strong()
                                                    .color(if is_rect {
                                                        Color32::WHITE
                                                    } else {
                                                        Color32::from_gray(185)
                                                    }),
                                            )
                                                .fill(if is_rect {
                                                    Color32::from_rgb(2, 132, 199)
                                                } else {
                                                    Color32::from_rgb(30, 41, 59)
                                                })
                                                .corner_radius(6.0);

                                            if ui.add(rect_btn).clicked() && !is_rect {
                                                mode = CaptureMode::Rectangle;
                                                requested_mode = Some(CaptureMode::Rectangle);
                                            }

                                            let is_free = mode == CaptureMode::Freeform;
                                            let free_btn = egui::Button::new(
                                                RichText::new("〰 Freeform / Lasso (2)")
                                                    .strong()
                                                    .color(if is_free {
                                                        Color32::from_rgb(15, 23, 42)
                                                    } else {
                                                        Color32::from_gray(185)
                                                    }),
                                            )
                                                .fill(if is_free {
                                                    Color32::from_rgb(52, 211, 153)
                                                } else {
                                                    Color32::from_rgb(30, 41, 59)
                                                })
                                                .corner_radius(6.0);

                                            if ui.add(free_btn).clicked() && !is_free {
                                                mode = CaptureMode::Freeform;
                                                requested_mode = Some(CaptureMode::Freeform);
                                            }

                                            ui.separator();

                                            let hint = match mode {
                                                CaptureMode::Rectangle => {
                                                    "Drag to select area  •  Tab: Freeform Mode  •  Esc: Cancel"
                                                }
                                                CaptureMode::Freeform => {
                                                    "Draw around any custom area  •  Tab: Rectangle Mode  •  Esc: Cancel"
                                                }
                                            };
                                            ui.label(
                                                RichText::new(hint)
                                                    .color(Color32::from_gray(225))
                                                    .size(13.0),
                                            );
                                        });
                                    });
                            });

                        last_toolbar_rect = Some(area_resp.response.rect);
                    }
                });
        });

        self.mode = mode;
        self.drag_start = drag_start;
        self.drag_current = drag_current;
        self.lasso_points = lasso_points;
        self.last_toolbar_rect = last_toolbar_rect;

        if matches!(action, OverlayAction::Cancel) {
            return OverlayAction::Cancel;
        }

        if let Some((local_sel, logical_size)) = completed_rect {
            if let Some(cropped) = crop_captured_area(&self.raw_image, local_sel, logical_size) {
                return OverlayAction::Captured {
                    image: cropped,
                    mode: CaptureMode::Rectangle,
                };
            }
        }

        if let Some((local_pts, logical_size)) = completed_freeform {
            if let Some(cropped) = crop_freeform_area(&self.raw_image, &local_pts, logical_size) {
                return OverlayAction::Captured {
                    image: cropped,
                    mode: CaptureMode::Freeform,
                };
            }
        }

        if let Some(new_mode) = requested_mode {
            return OverlayAction::SwitchMode(new_mode);
        }

        OverlayAction::None
    }
}

fn paint_dimmed_surround(painter: &Painter, screen_rect: Rect, clear_rect: Rect, dim_color: Color32) {
    if clear_rect.min.y > screen_rect.min.y {
        painter.rect_filled(
            Rect::from_min_max(screen_rect.min, Pos2::new(screen_rect.max.x, clear_rect.min.y)),
            0.0,
            dim_color,
        );
    }
    if clear_rect.max.y < screen_rect.max.y {
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(screen_rect.min.x, clear_rect.max.y), screen_rect.max),
            0.0,
            dim_color,
        );
    }
    if clear_rect.min.x > screen_rect.min.x {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(screen_rect.min.x, clear_rect.min.y),
                Pos2::new(clear_rect.min.x, clear_rect.max.y),
            ),
            0.0,
            dim_color,
        );
    }
    if clear_rect.max.x < screen_rect.max.x {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(clear_rect.max.x, clear_rect.min.y),
                Pos2::new(screen_rect.max.x, clear_rect.max.y),
            ),
            0.0,
            dim_color,
        );
    }
}

fn paint_dimension_badge(painter: &Painter, screen_rect: Rect, anchor_rect: Rect, label: &str) {
    let badge_y = if anchor_rect.min.y > 34.0 {
        anchor_rect.min.y - 26.0
    } else {
        (anchor_rect.max.y + 8.0).min(screen_rect.max.y - 28.0)
    };
    let badge_pos = Pos2::new(anchor_rect.min.x.max(8.0), badge_y);
    let galley = painter.layout_no_wrap(
        label.to_string(),
        FontId::proportional(13.0),
        Color32::WHITE,
    );
    let pill_rect = Rect::from_min_size(badge_pos, galley.size() + Vec2::new(14.0, 8.0));
    painter.rect_filled(
        pill_rect,
        5.0,
        Color32::from_rgba_unmultiplied(15, 23, 42, 228),
    );
    painter.galley(badge_pos + Vec2::new(7.0, 4.0), galley, Color32::WHITE);
}

fn draw_dashed_line(
    painter: &Painter,
    start: Pos2,
    end: Pos2,
    dash_len: f32,
    gap_len: f32,
    stroke: Stroke,
) {
    let delta = end - start;
    let total_len = delta.length();
    if total_len < 1.0 {
        return;
    }
    let dir = delta / total_len;
    let step = (dash_len + gap_len).max(2.0);
    let mut dist = 0.0f32;
    while dist < total_len {
        let seg_end = (dist + dash_len).min(total_len);
        painter.line_segment([start + dir * dist, start + dir * seg_end], stroke);
        dist += step;
    }
}
