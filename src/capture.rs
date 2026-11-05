use egui::{Pos2, Rect, Vec2};
use image::RgbaImage;
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;

pub struct CapturedDisplay {
    pub monitor_handle: Option<MonitorHandle>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub image: RgbaImage,
}

/// Captures screenshots of all connected displays before opening the selection overlay.
pub fn capture_all_displays(event_loop: &ActiveEventLoop) -> Result<Vec<CapturedDisplay>, String> {
    let xcap_monitors =
        xcap::Monitor::all().map_err(|e| format!("Failed to detect display monitors: {e}"))?;
    if xcap_monitors.is_empty() {
        return Err("No monitors detected.".to_string());
    }

    let winit_monitors: Vec<MonitorHandle> = event_loop.available_monitors().collect();
    let mut used_winit = vec![false; winit_monitors.len()];
    let mut results = Vec::with_capacity(xcap_monitors.len());

    for xm in xcap_monitors {
        let img = xm
            .capture_image()
            .map_err(|e| format!("Failed to capture screen image: {e}"))?;
        let mx = xm.x().unwrap_or(0);
        let my = xm.y().unwrap_or(0);
        let mw = xm.width().unwrap_or(img.width());
        let mh = xm.height().unwrap_or(img.height());
        let mscale = xm.scale_factor().unwrap_or(1.0);

        // Match with corresponding winit MonitorHandle for exact borderless fullscreen placement
        let matched_handle = match_winit_monitor(
            &winit_monitors,
            &mut used_winit,
            mx,
            my,
            img.width(),
            img.height(),
            mscale,
        );

        results.push(CapturedDisplay {
            monitor_handle: matched_handle,
            x: mx,
            y: my,
            width: mw,
            height: mh,
            image: img,
        });
    }

    Ok(results)
}

fn match_winit_monitor(
    winit_monitors: &[MonitorHandle],
    used: &mut [bool],
    mx: i32,
    my: i32,
    img_w: u32,
    img_h: u32,
    mscale: f32,
) -> Option<MonitorHandle> {
    if winit_monitors.len() == 1 && !used[0] {
        used[0] = true;
        return Some(winit_monitors[0].clone());
    }

    let mut best_idx: Option<usize> = None;
    let mut best_score = i64::MAX;

    for (idx, handle) in winit_monitors.iter().enumerate() {
        if used[idx] {
            continue;
        }
        let pos = handle.position();
        let size = handle.size();

        // Check both raw physical coordinates (Windows) and scaled coordinates (macOS)
        let dist_raw = i64::from(pos.x - mx).abs() + i64::from(pos.y - my).abs();
        let scaled_mx = (mx as f32 * mscale).round() as i32;
        let scaled_my = (my as f32 * mscale).round() as i32;
        let dist_scaled = i64::from(pos.x - scaled_mx).abs() + i64::from(pos.y - scaled_my).abs();
        let pos_dist = dist_raw.min(dist_scaled);

        let size_diff =
            (i64::from(size.width) - i64::from(img_w)).abs()
                + (i64::from(size.height) - i64::from(img_h)).abs();

        let score = pos_dist * 4 + size_diff;
        if score < best_score {
            best_score = score;
            best_idx = Some(idx);
        }
    }

    if let Some(idx) = best_idx {
        used[idx] = true;
        Some(winit_monitors[idx].clone())
    } else {
        None
    }
}

/// Crops the captured physical image using logical selection coordinates from the overlay window.
/// Dividing physical image dimensions directly by the logical overlay screen size ensures
/// pixel-exact cropping across 100%, 125%, 150%, and 200% Retina/HiDPI displays.
pub fn crop_captured_area(
    image: &RgbaImage,
    selection_rect: Rect,
    logical_screen_size: Vec2,
) -> Option<RgbaImage> {
    if logical_screen_size.x <= 1.0 || logical_screen_size.y <= 1.0 {
        return None;
    }

    let scale_x = image.width() as f32 / logical_screen_size.x;
    let scale_y = image.height() as f32 / logical_screen_size.y;

    let min_x = (selection_rect.min.x.min(selection_rect.max.x) * scale_x)
        .round()
        .clamp(0.0, image.width() as f32) as u32;
    let min_y = (selection_rect.min.y.min(selection_rect.max.y) * scale_y)
        .round()
        .clamp(0.0, image.height() as f32) as u32;
    let max_x = (selection_rect.min.x.max(selection_rect.max.x) * scale_x)
        .round()
        .clamp(0.0, image.width() as f32) as u32;
    let max_y = (selection_rect.min.y.max(selection_rect.max.y) * scale_y)
        .round()
        .clamp(0.0, image.height() as f32) as u32;

    let crop_w = max_x.saturating_sub(min_x);
    let crop_h = max_y.saturating_sub(min_y);

    if crop_w < 5 || crop_h < 5 {
        return None;
    }

    Some(image::imageops::crop_imm(image, min_x, min_y, crop_w, crop_h).to_image())
}

/// Crops and masks an arbitrary freeform (lasso) polygon selection using a 4x vertical sub-row
/// scanline rasterizer with exact horizontal sub-pixel coverage for smooth anti-aliased edges.
/// Pixels outside the freeform lasso path become transparent (`[0, 0, 0, 0]`).
pub fn crop_freeform_area(
    image: &RgbaImage,
    lasso_points: &[Pos2],
    logical_screen_size: Vec2,
) -> Option<RgbaImage> {
    if lasso_points.len() < 3 || logical_screen_size.x <= 1.0 || logical_screen_size.y <= 1.0 {
        return None;
    }

    let img_w_f = image.width() as f32;
    let img_h_f = image.height() as f32;
    let scale_x = img_w_f / logical_screen_size.x;
    let scale_y = img_h_f / logical_screen_size.y;

    let mut poly_px: Vec<[f32; 2]> = Vec::with_capacity(lasso_points.len());
    let mut min_xf = f32::INFINITY;
    let mut min_yf = f32::INFINITY;
    let mut max_xf = f32::NEG_INFINITY;
    let mut max_yf = f32::NEG_INFINITY;

    for pt in lasso_points {
        let px = (pt.x * scale_x).clamp(0.0, img_w_f);
        let py = (pt.y * scale_y).clamp(0.0, img_h_f);
        min_xf = min_xf.min(px);
        min_yf = min_yf.min(py);
        max_xf = max_xf.max(px);
        max_yf = max_yf.max(py);
        poly_px.push([px, py]);
    }

    let min_x = min_xf.floor().clamp(0.0, img_w_f) as u32;
    let min_y = min_yf.floor().clamp(0.0, img_h_f) as u32;
    let max_x = max_xf.ceil().clamp(0.0, img_w_f) as u32;
    let max_y = max_yf.ceil().clamp(0.0, img_h_f) as u32;

    let crop_w = max_x.saturating_sub(min_x);
    let crop_h = max_y.saturating_sub(min_y);
    if crop_w < 5 || crop_h < 5 {
        return None;
    }

    // Translate polygon vertices into local cropped image coordinates (0..crop_w, 0..crop_h)
    let off_x = min_x as f32;
    let off_y = min_y as f32;
    for pt in &mut poly_px {
        pt[0] -= off_x;
        pt[1] -= off_y;
    }

    let mut cropped = image::imageops::crop_imm(image, min_x, min_y, crop_w, crop_h).to_image();
    let w_usize = crop_w as usize;
    let mut row_coverage = vec![0.0f32; w_usize];
    let mut intersections = Vec::with_capacity(32);
    const SUB_ROWS: usize = 4;
    let inv_sub = 1.0 / (SUB_ROWS as f32);
    let mut has_any_visible_pixel = false;

    for y in 0..crop_h {
        row_coverage.fill(0.0);

        for sy in 0..SUB_ROWS {
            let scan_y = y as f32 + (sy as f32 + 0.5) * inv_sub;
            intersections.clear();

            let n = poly_px.len();
            for i in 0..n {
                let p0 = poly_px[i];
                let p1 = poly_px[(i + 1) % n];

                // Half-open y-interval prevents double-counting shared polygon vertices
                if (p0[1] <= scan_y && p1[1] > scan_y) || (p1[1] <= scan_y && p0[1] > scan_y) {
                    let t = (scan_y - p0[1]) / (p1[1] - p0[1]);
                    let x_int = p0[0] + t * (p1[0] - p0[0]);
                    intersections.push(x_int);
                }
            }

            intersections.sort_by(|a, b| a.total_cmp(b));

            for span in intersections.chunks_exact(2) {
                let x_start = span[0].clamp(0.0, crop_w as f32);
                let x_end = span[1].clamp(0.0, crop_w as f32);
                if x_end <= x_start {
                    continue;
                }

                let ix_start = (x_start.floor() as usize).min(w_usize.saturating_sub(1));
                let ix_end = ((x_end.ceil() as usize).min(w_usize)).saturating_sub(1);

                for ix in ix_start..=ix_end {
                    let px_left = ix as f32;
                    let px_right = (ix + 1) as f32;
                    let overlap = (x_end.min(px_right) - x_start.max(px_left)).max(0.0);
                    row_coverage[ix] += overlap * inv_sub;
                }
            }
        }

        for x in 0..w_usize {
            let cov = row_coverage[x].clamp(0.0, 1.0);
            let px = cropped.get_pixel_mut(x as u32, y);
            let out_alpha = (f32::from(px.0[3]) * cov).round().clamp(0.0, 255.0) as u8;
            if out_alpha == 0 {
                px.0 = [0, 0, 0, 0];
            } else {
                px.0[3] = out_alpha;
                has_any_visible_pixel = true;
            }
        }
    }

    if !has_any_visible_pixel {
        return None;
    }

    Some(cropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn test_hidpi_crop_captured_area_accuracy() {
        // Simulate a 2x Retina / 200% DPI display: physical 200x100, logical 100x50
        let mut img = RgbaImage::from_pixel(200, 100, Rgba([0, 0, 0, 255]));
        for y in 20..60 {
            for x in 40..120 {
                img.put_pixel(x, y, Rgba([10, 200, 90, 255]));
            }
        }

        let sel_logical = Rect::from_min_max(Pos2::new(20.0, 10.0), Pos2::new(60.0, 30.0));
        let cropped = crop_captured_area(&img, sel_logical, Vec2::new(100.0, 50.0))
            .expect("Valid selection should produce cropped image");

        assert_eq!(cropped.width(), 80);
        assert_eq!(cropped.height(), 40);
        assert_eq!(cropped.get_pixel(0, 0).0, [10, 200, 90, 255]);
        assert_eq!(cropped.get_pixel(79, 39).0, [10, 200, 90, 255]);
    }

    #[test]
    fn test_freeform_concave_lasso_crop_and_transparency() {
        // Physical 100x100 opaque blue image, logical 100x100
        let img = RgbaImage::from_pixel(100, 100, Rgba([30, 140, 250, 255]));

        // Draw an L-shaped concave polygon from (10, 10) to (70, 70):
        // Top-right notch [40..70, 10..40] is OUTSIDE the L-shape!
        let lasso = vec![
            Pos2::new(10.0, 10.0),
            Pos2::new(40.0, 10.0),
            Pos2::new(40.0, 40.0),
            Pos2::new(70.0, 40.0),
            Pos2::new(70.0, 70.0),
            Pos2::new(10.0, 70.0),
        ];

        let cropped = crop_freeform_area(&img, &lasso, Vec2::new(100.0, 100.0))
            .expect("Valid L-shape freeform selection should succeed");

        assert_eq!(cropped.width(), 60);
        assert_eq!(cropped.height(), 60);

        // Inside vertical bar of L -> local (10, 10) corresponds to original (20, 20): opaque blue
        assert_eq!(cropped.get_pixel(10, 10).0, [30, 140, 250, 255]);
        // Inside horizontal foot of L -> local (45, 45) corresponds to original (55, 55): opaque blue
        assert_eq!(cropped.get_pixel(45, 45).0, [30, 140, 250, 255]);
        // Inside concave cutout notch -> local (45, 10) corresponds to original (55, 20): transparent!
        assert_eq!(cropped.get_pixel(45, 10).0, [0, 0, 0, 0]);
    }
}
