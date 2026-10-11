//! The Purpamail application icon, drawn procedurally.

/// A 128×128 RGBA icon: a white envelope on a purple rounded square.
pub fn app_icon() -> egui::IconData {
    let n = 128usize;
    let mut rgba = vec![0u8; n * n * 4];
    let purple = [0x7b, 0x4f, 0xd6];
    let purple2 = [0x9d, 0x6b, 0xf0];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // rounded square mask
            let r = 26.0;
            let cx = fx.clamp(r + 4.0, n as f32 - r - 4.0);
            let cy = fy.clamp(r + 4.0, n as f32 - r - 4.0);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let alpha = (r - d + 0.5).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let t = fy / n as f32;
            let mut c = [
                (purple2[0] as f32 * (1.0 - t) + purple[0] as f32 * t) as u8,
                (purple2[1] as f32 * (1.0 - t) + purple[1] as f32 * t) as u8,
                (purple2[2] as f32 * (1.0 - t) + purple[2] as f32 * t) as u8,
            ];
            // envelope body
            let (l, rr, top, bot) = (28.0, 100.0, 40.0, 90.0);
            if fx >= l && fx <= rr && fy >= top && fy <= bot {
                c = [255, 255, 255];
                // flap lines (a V from the top corners to the center)
                let flap_y = top + (fx - l).min(rr - fx) * 0.62;
                if (fy - flap_y).abs() < 2.2 && fy < top + 26.0 {
                    c = purple;
                }
            }
            let i = (y * n + x) * 4;
            rgba[i] = c[0];
            rgba[i + 1] = c[1];
            rgba[i + 2] = c[2];
            rgba[i + 3] = (alpha * 255.0) as u8;
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}
