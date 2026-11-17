//! Small NV12 helpers: rotate/mirror, preview conversion, image → NV12, the built-in cover card.

/// Rotates (clockwise) and/or mirrors an NV12 frame. Returns the new size.
pub fn transform(src: &[u8], w: usize, h: usize, rotation: u32, mirror: bool, out: &mut Vec<u8>) -> (usize, usize) {
    let rot = rotation % 360;
    let (dw, dh) = if rot == 90 || rot == 270 { (h, w) } else { (w, h) };
    out.resize(dw * dh * 3 / 2, 0);
    // Maps a pixel of the output (after mirroring) to the source pixel it shows.
    let map = |x: usize, y: usize, sw: usize, sh: usize, dw: usize| -> (usize, usize) {
        let x = if mirror { dw - 1 - x } else { x };
        match rot {
            90 => (y, sh - 1 - x),
            180 => (sw - 1 - x, sh - 1 - y),
            270 => (sw - 1 - y, x),
            _ => (x, y),
        }
    };
    let (ys, yd) = src.split_at(w * h);
    let (out_y, out_uv) = out.split_at_mut(dw * dh);
    for y in 0..dh {
        let row = &mut out_y[y * dw..(y + 1) * dw];
        for (x, px) in row.iter_mut().enumerate() {
            let (sx, sy) = map(x, y, w, h, dw);
            *px = ys[sy * w + sx];
        }
    }
    let (cw, ch, cdw, cdh) = (w / 2, h / 2, dw / 2, dh / 2);
    for y in 0..cdh {
        for x in 0..cdw {
            let (sx, sy) = map(x, y, cw, ch, cdw);
            let s = sy * w + sx * 2;
            let d = y * dw + x * 2;
            out_uv[d] = yd[s];
            out_uv[d + 1] = yd[s + 1];
        }
    }
    (dw, dh)
}

/// NV12 → RGBA, scaled down (nearest) to at most `max_w` wide. For the preview in the window.
pub fn preview_rgba(src: &[u8], w: usize, h: usize, max_w: usize, out: &mut Vec<u8>) -> (usize, usize) {
    let tw = w.min(max_w).max(2);
    let th = (h * tw / w).max(2);
    out.resize(tw * th * 4, 0);
    let uv = &src[w * h..];
    for ty in 0..th {
        let sy = ty * h / th;
        for tx in 0..tw {
            let sx = tx * w / tw;
            let c = src[sy * w + sx] as i32 - 16;
            let i = (sy / 2) * w + (sx & !1);
            let d = uv[i] as i32 - 128;
            let e = uv[i + 1] as i32 - 128;
            let o = (ty * tw + tx) * 4;
            out[o] = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            out[o + 1] = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            out[o + 2] = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            out[o + 3] = 255;
        }
    }
    (tw, th)
}

/// RGBA → NV12 (transparent pixels become black). Odd sizes lose their last row/column.
pub fn rgba_to_nv12(rgba: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let (ew, eh) = (w & !1, h & !1);
    let mut out = vec![0u8; ew * eh * 3 / 2];
    let px = |x: usize, y: usize| -> (i32, i32, i32) {
        let i = (y * w + x) * 4;
        let a = rgba[i + 3] as i32;
        ((rgba[i] as i32 * a) / 255, (rgba[i + 1] as i32 * a) / 255, (rgba[i + 2] as i32 * a) / 255)
    };
    for y in 0..eh {
        for x in 0..ew {
            let (r, g, b) = px(x, y);
            out[y * ew + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
        }
    }
    let uv = &mut out[ew * eh..];
    for y in 0..eh / 2 {
        for x in 0..ew / 2 {
            let (mut r, mut g, mut b) = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = px(x * 2 + dx, y * 2 + dy);
                r += p.0;
                g += p.1;
                b += p.2;
            }
            let (r, g, b) = (r / 4, g / 4, b / 4);
            uv[y * ew + x * 2] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            uv[y * ew + x * 2 + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
    (out, ew, eh)
}

/// The built-in cover: the Tether mark, dimmed, on black. "Camera's off, on purpose."
pub fn builtin_cover(w: usize, h: usize) -> Vec<u8> {
    let mut frame = vec![16u8; w * h * 3 / 2];
    frame[w * h..].fill(128);
    let luma = 72u8;
    let u = h as f64 * 0.4 / 512.0;
    let ox = w as f64 / 2.0 - 256.0 * u;
    let oy = h as f64 / 2.0 - 256.0 * u;
    let (cx, cy) = (ox + 256.0 * u, oy + 216.0 * u);
    let (outer, inner, dot, cap) = (130.0 * u, 94.0 * u, 34.0 * u, 18.0 * u);
    let (cable_y0, cable_y1) = (oy + 328.0 * u, oy + 430.0 * u);
    for y in 0..h {
        let py = y as f64 + 0.5;
        for x in 0..w {
            let px = x as f64 + 0.5;
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let cable = (px - cx).abs() <= cap && py >= cable_y0 && py <= cable_y1;
            let end = ((px - cx).powi(2) + (py - cable_y1).powi(2)).sqrt() <= cap;
            if (d <= outer && d >= inner) || d <= dot || cable || end {
                frame[y * w + x] = luma;
            }
        }
    }
    frame
}
