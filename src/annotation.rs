use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke};
use image::{Rgba, RgbaImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationTool {
    Pen,
    Highlighter,
    Rectangle,
    Arrow,
}

impl AnnotationTool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pen => "✏ Freehand Pen",
            Self::Highlighter => "🖍 Highlighter",
            Self::Rectangle => "▭ Rectangle",
            Self::Arrow => "↗ Arrow",
        }
    }
}

/// Represents a single user annotation stored in native image pixel coordinates.
#[derive(Debug, Clone)]
pub struct AnnotationStroke {
    pub tool: AnnotationTool,
    pub color: [u8; 4],
    /// Stroke width in native image pixels.
    pub thickness: f32,
    /// Coordinates in native image pixel space (`0..width`, `0..height`).
    pub points: Vec<[f32; 2]>,
}

impl AnnotationStroke {
    pub fn effective_color(&self) -> [u8; 4] {
        match self.tool {
            AnnotationTool::Highlighter => [
                self.color[0],
                self.color[1],
                self.color[2],
                // Ensure highlighter stays translucent even if user picked an opaque swatch
                self.color[3].min(105).max(55),
            ],
            _ => self.color,
        }
    }

    pub fn effective_thickness(&self) -> f32 {
        match self.tool {
            AnnotationTool::Highlighter => (self.thickness * 3.0).max(12.0),
            _ => self.thickness.max(1.0),
        }
    }
}

/// Projects and renders an annotation onto an `egui::Painter` canvas.
pub fn paint_stroke_on_egui(
    painter: &Painter,
    stroke: &AnnotationStroke,
    img_rect_on_screen: Rect,
    img_size: [f32; 2],
) {
    if stroke.points.is_empty() || img_size[0] <= 0.0 || img_size[1] <= 0.0 {
        return;
    }

    let scale_x = img_rect_on_screen.width() / img_size[0];
    let scale_y = img_rect_on_screen.height() / img_size[1];
    let scale = (scale_x + scale_y) * 0.5;

    let to_screen = |pt: [f32; 2]| -> Pos2 {
        Pos2::new(
            img_rect_on_screen.min.x + pt[0] * scale_x,
            img_rect_on_screen.min.y + pt[1] * scale_y,
        )
    };

    let [r, g, b, a] = stroke.effective_color();
    let color = Color32::from_rgba_unmultiplied(r, g, b, a);
    let screen_width = (stroke.effective_thickness() * scale).max(1.0);
    let egui_stroke = Stroke::new(screen_width, color);

    match stroke.tool {
        AnnotationTool::Pen | AnnotationTool::Highlighter => {
            if stroke.points.len() == 1 {
                let center = to_screen(stroke.points[0]);
                painter.circle_filled(center, screen_width * 0.5, color);
            } else {
                let pts: Vec<Pos2> = stroke.points.iter().copied().map(to_screen).collect();
                painter.add(Shape::line(pts, egui_stroke));
            }
        }
        AnnotationTool::Rectangle => {
            if let (Some(&start), Some(&end)) = (stroke.points.first(), stroke.points.last()) {
                let r_screen = Rect::from_two_pos(to_screen(start), to_screen(end));
                painter.rect_stroke(r_screen, 0.0, egui_stroke, egui::StrokeKind::Middle);
            }
        }
        AnnotationTool::Arrow => {
            if let (Some(&start), Some(&end)) = (stroke.points.first(), stroke.points.last()) {
                let p0 = to_screen(start);
                let p1 = to_screen(end);
                let dir = p1 - p0;
                let len = dir.length();
                if len < 1.0 {
                    painter.circle_filled(p0, screen_width * 0.5, color);
                } else {
                    let unit = dir / len;
                    let head_len = (screen_width * 4.0).max(12.0).min(len * 0.75);
                    let head_half_w = head_len * 0.55;
                    let shaft_end = p1 - unit * (head_len * 0.7);
                    painter.line_segment([p0, shaft_end], egui_stroke);

                    let perp = egui::Vec2::new(-unit.y, unit.x);
                    let left = p1 - unit * head_len + perp * head_half_w;
                    let right = p1 - unit * head_len - perp * head_half_w;
                    painter.add(Shape::convex_polygon(
                        vec![p1, left, right],
                        color,
                        Stroke::NONE,
                    ));
                }
            }
        }
    }
}

/// Renders all annotations onto a full-resolution copy of the base `RgbaImage`
/// using anti-aliased per-stroke coverage masks (preventing alpha double-blending at joints).
pub fn bake_annotations(base: &RgbaImage, strokes: &[AnnotationStroke]) -> RgbaImage {
    let mut canvas = base.clone();
    let w = canvas.width() as usize;
    let h = canvas.height() as usize;
    if w == 0 || h == 0 || strokes.is_empty() {
        return canvas;
    }

    let mut coverage_buf = vec![0.0f32; w * h];

    for stroke in strokes {
        if stroke.points.is_empty() {
            continue;
        }
        let color = stroke.effective_color();
        if color[3] == 0 {
            continue;
        }
        let thickness = stroke.effective_thickness();
        let radius = thickness * 0.5;

        let dirty_bounds = match stroke.tool {
            AnnotationTool::Pen | AnnotationTool::Highlighter => {
                if stroke.points.len() == 1 {
                    let p = stroke.points[0];
                    rasterize_capsule(&mut coverage_buf, w, h, p, p, radius)
                } else {
                    let mut bounds: Option<[usize; 4]> = None;
                    for pair in stroke.points.windows(2) {
                        let b =
                            rasterize_capsule(&mut coverage_buf, w, h, pair[0], pair[1], radius);
                        bounds = merge_bounds(bounds, b);
                    }
                    bounds
                }
            }
            AnnotationTool::Rectangle => {
                if let (Some(&p0), Some(&p1)) = (stroke.points.first(), stroke.points.last()) {
                    let min_x = p0[0].min(p1[0]);
                    let max_x = p0[0].max(p1[0]);
                    let min_y = p0[1].min(p1[1]);
                    let max_y = p0[1].max(p1[1]);
                    let corners = [
                        [min_x, min_y],
                        [max_x, min_y],
                        [max_x, max_y],
                        [min_x, max_y],
                    ];
                    let mut bounds = None;
                    for i in 0..4 {
                        let b = rasterize_capsule(
                            &mut coverage_buf,
                            w,
                            h,
                            corners[i],
                            corners[(i + 1) % 4],
                            radius,
                        );
                        bounds = merge_bounds(bounds, b);
                    }
                    bounds
                } else {
                    None
                }
            }
            AnnotationTool::Arrow => {
                if let (Some(&p0), Some(&p1)) = (stroke.points.first(), stroke.points.last()) {
                    let dx = p1[0] - p0[0];
                    let dy = p1[1] - p0[1];
                    let len = (dx * dx + dy * dy).sqrt();
                    if len < 1.0 {
                        rasterize_capsule(&mut coverage_buf, w, h, p0, p0, radius)
                    } else {
                        let ux = dx / len;
                        let uy = dy / len;
                        let head_len = (thickness * 4.0).max(12.0).min(len * 0.75);
                        let head_half_w = head_len * 0.55;
                        let shaft_end = [
                            p1[0] - ux * (head_len * 0.7),
                            p1[1] - uy * (head_len * 0.7),
                        ];
                        let b1 = rasterize_capsule(&mut coverage_buf, w, h, p0, shaft_end, radius);

                        let px = -uy;
                        let py = ux;
                        let left = [
                            p1[0] - ux * head_len + px * head_half_w,
                            p1[1] - uy * head_len + py * head_half_w,
                        ];
                        let right = [
                            p1[0] - ux * head_len - px * head_half_w,
                            p1[1] - uy * head_len - py * head_half_w,
                        ];
                        let b2 = rasterize_triangle(&mut coverage_buf, w, h, p1, left, right);
                        merge_bounds(b1, b2)
                    }
                } else {
                    None
                }
            }
        };

        if let Some([min_x, min_y, max_x, max_y]) = dirty_bounds {
            let src_a_norm = f32::from(color[3]) / 255.0;
            for y in min_y..=max_y {
                let row_offset = y * w;
                for x in min_x..=max_x {
                    let idx = row_offset + x;
                    let cov = coverage_buf[idx];
                    if cov > 0.0 {
                        coverage_buf[idx] = 0.0;
                        let alpha = (src_a_norm * cov).clamp(0.0, 1.0);
                        let dst = canvas.get_pixel(x as u32, y as u32).0;
                        let blended = blend_rgba(dst, color, alpha);
                        canvas.put_pixel(x as u32, y as u32, Rgba(blended));
                    }
                }
            }
        }
    }

    canvas
}

fn merge_bounds(a: Option<[usize; 4]>, b: Option<[usize; 4]>) -> Option<[usize; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn rasterize_capsule(
    buf: &mut [f32],
    w: usize,
    h: usize,
    a: [f32; 2],
    b: [f32; 2],
    radius: f32,
) -> Option<[usize; 4]> {
    let pad = radius + 1.5;
    let min_x_f = a[0].min(b[0]) - pad;
    let max_x_f = a[0].max(b[0]) + pad;
    let min_y_f = a[1].min(b[1]) - pad;
    let max_y_f = a[1].max(b[1]) + pad;

    if max_x_f < 0.0 || max_y_f < 0.0 || min_x_f >= w as f32 || min_y_f >= h as f32 {
        return None;
    }

    let min_x = min_x_f.floor().max(0.0) as usize;
    let max_x = (max_x_f.ceil() as usize).min(w - 1);
    let min_y = min_y_f.floor().max(0.0) as usize;
    let max_y = (max_y_f.ceil() as usize).min(h - 1);

    let abx = b[0] - a[0];
    let aby = b[1] - a[1];
    let ab_len_sq = abx * abx + aby * aby;

    for y in min_y..=max_y {
        let py = y as f32 + 0.5;
        let row = y * w;
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let t = if ab_len_sq > 1e-6 {
                (((px - a[0]) * abx + (py - a[1]) * aby) / ab_len_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let cx = a[0] + t * abx;
            let cy = a[1] + t * aby;
            let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let cov = (radius + 0.5 - dist).clamp(0.0, 1.0);
            if cov > buf[row + x] {
                buf[row + x] = cov;
            }
        }
    }

    Some([min_x, min_y, max_x, max_y])
}

fn rasterize_triangle(
    buf: &mut [f32],
    w: usize,
    h: usize,
    v0: [f32; 2],
    v1: [f32; 2],
    v2: [f32; 2],
) -> Option<[usize; 4]> {
    let min_x_f = v0[0].min(v1[0]).min(v2[0]) - 1.5;
    let max_x_f = v0[0].max(v1[0]).max(v2[0]) + 1.5;
    let min_y_f = v0[1].min(v1[1]).min(v2[1]) - 1.5;
    let max_y_f = v0[1].max(v1[1]).max(v2[1]) + 1.5;

    if max_x_f < 0.0 || max_y_f < 0.0 || min_x_f >= w as f32 || min_y_f >= h as f32 {
        return None;
    }

    let min_x = min_x_f.floor().max(0.0) as usize;
    let max_x = (max_x_f.ceil() as usize).min(w - 1);
    let min_y = min_y_f.floor().max(0.0) as usize;
    let max_y = (max_y_f.ceil() as usize).min(h - 1);

    // 2x2 supersampling inside triangle bounding box for crisp arrowhead edges
    let offsets = [0.25f32, 0.75f32];
    for y in min_y..=max_y {
        let row = y * w;
        for x in min_x..=max_x {
            let mut hits = 0u32;
            for &oy in &offsets {
                for &ox in &offsets {
                    if point_in_triangle([x as f32 + ox, y as f32 + oy], v0, v1, v2) {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                let cov = hits as f32 * 0.25;
                if cov > buf[row + x] {
                    buf[row + x] = cov;
                }
            }
        }
    }

    Some([min_x, min_y, max_x, max_y])
}

fn point_in_triangle(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let edge = |p1: [f32; 2], p2: [f32; 2], p3: [f32; 2]| -> f32 {
        (p1[0] - p3[0]) * (p2[1] - p3[1]) - (p2[0] - p3[0]) * (p1[1] - p3[1])
    };
    let d1 = edge(p, a, b);
    let d2 = edge(p, b, c);
    let d3 = edge(p, c, a);
    let has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    let has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    !(has_neg && has_pos)
}

fn blend_rgba(dst: [u8; 4], src: [u8; 4], src_a: f32) -> [u8; 4] {
    let dst_a = f32::from(dst[3]) / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 1e-6 {
        return [0, 0, 0, 0];
    }
    let blend_ch = |s: u8, d: u8| -> u8 {
        let sf = f32::from(s);
        let df = f32::from(d);
        ((sf * src_a + df * dst_a * (1.0 - src_a)) / out_a)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [
        blend_ch(src[0], dst[0]),
        blend_ch(src[1], dst[1]),
        blend_ch(src[2], dst[2]),
        (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bake_pen_and_out_of_bounds_safety() {
        let base = RgbaImage::from_pixel(50, 50, Rgba([255, 255, 255, 255]));
        let stroke = AnnotationStroke {
            tool: AnnotationTool::Pen,
            color: [220, 20, 60, 255],
            thickness: 6.0,
            points: vec![[-10.0, 25.0], [25.0, 25.0], [60.0, 25.0]],
        };
        let out = bake_annotations(&base, &[stroke]);
        assert_eq!(out.get_pixel(25, 25).0, [220, 20, 60, 255]);
        assert_eq!(out.get_pixel(25, 5).0, [255, 255, 255, 255]);
    }
}

