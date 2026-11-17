//! "Look": zoom/pan, how the picture fills the camera frame (fit/fill/stretch), and filters. All on NV12.
//! The fast path (no zoom, same size, no filter) copies nothing extra.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Fit {
    /// Whole picture, black bars if the shape differs.
    Fit,
    /// Fills the frame, cutting off the edges that don't fit.
    Fill,
    /// Fills the frame by squashing/stretching.
    Stretch,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    None,
    Mono,
    Noir,
    Warm,
    Cool,
    Vivid,
    Fade,
}

impl Filter {
    pub const ALL: [Filter; 7] = [Filter::None, Filter::Mono, Filter::Noir, Filter::Warm, Filter::Cool, Filter::Vivid, Filter::Fade];

    pub fn label(self) -> &'static str {
        match self {
            Filter::None => "None",
            Filter::Mono => "B&W",
            Filter::Noir => "Noir",
            Filter::Warm => "Warm",
            Filter::Cool => "Cool",
            Filter::Vivid => "Vivid",
            Filter::Fade => "Fade",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Look {
    /// 1.0 = no zoom, up to 4.0.
    pub zoom: f32,
    /// Where the zoomed view sits, -1..1 (0 = centre).
    pub pan_x: f32,
    pub pan_y: f32,
    pub fit: Fit,
    pub filter: Filter,
    /// -100..100
    pub brightness: i32,
    /// -100..100
    pub contrast: i32,
    /// -100..100
    pub saturation: i32,
}

impl Default for Look {
    fn default() -> Self {
        Look { zoom: 1.0, pan_x: 0.0, pan_y: 0.0, fit: Fit::Fill, filter: Filter::None, brightness: 0, contrast: 0, saturation: 0 }
    }
}

impl Look {
    fn has_color_change(&self) -> bool {
        self.filter != Filter::None || self.brightness != 0 || self.contrast != 0 || self.saturation != 0
    }
}

/// Applies the look, producing an `out_w` x `out_h` frame. Returns false when `src` can be used unchanged.
pub fn process(src: &[u8], sw: usize, sh: usize, out_w: usize, out_h: usize, look: &Look, out: &mut Vec<u8>) -> bool {
    let zoomed = look.zoom > 1.001;
    let same_size = sw == out_w && sh == out_h;
    if !zoomed && same_size && !look.has_color_change() {
        return false;
    }
    let (out_w, out_h) = (out_w & !1, out_h & !1);
    out.resize(out_w * out_h * 3 / 2, 0);

    if !zoomed && same_size {
        let n = out.len();
        out.copy_from_slice(&src[..n]);
    } else {
        // 1. The part of the source we look at (zoom + pan).
        let z = look.zoom.clamp(1.0, 4.0) as f64;
        let (mut cw, mut ch) = (sw as f64 / z, sh as f64 / z);
        let mut cx = (sw as f64 - cw) / 2.0 * (1.0 + look.pan_x.clamp(-1.0, 1.0) as f64);
        let mut cy = (sh as f64 - ch) / 2.0 * (1.0 + look.pan_y.clamp(-1.0, 1.0) as f64);
        // 2. How it goes into the output frame.
        let mut dst = (0usize, 0usize, out_w, out_h);
        let src_aspect = cw / ch;
        let out_aspect = out_w as f64 / out_h as f64;
        match look.fit {
            Fit::Stretch => {}
            Fit::Fill => {
                if src_aspect > out_aspect {
                    let nw = ch * out_aspect;
                    cx += (cw - nw) / 2.0;
                    cw = nw;
                } else {
                    let nh = cw / out_aspect;
                    cy += (ch - nh) / 2.0;
                    ch = nh;
                }
            }
            Fit::Fit => {
                if src_aspect > out_aspect {
                    let h = ((out_w as f64 / src_aspect) as usize).min(out_h) & !1;
                    dst = (0, ((out_h - h) / 2) & !1, out_w, h);
                } else {
                    let w = ((out_h as f64 * src_aspect) as usize).min(out_w) & !1;
                    dst = (((out_w - w) / 2) & !1, 0, w, out_h);
                }
            }
        }
        if dst != (0, 0, out_w, out_h) {
            out[..out_w * out_h].fill(16);
            out[out_w * out_h..].fill(128);
        }
        resample(src, sw, sh, (cx, cy, cw, ch), out, out_w, out_h, dst);
    }
    color(out, out_w, out_h, look);
    true
}

/// Bilinear luma, nearest chroma: maps `crop` of the source onto `dst` of the output.
#[allow(clippy::too_many_arguments)]
fn resample(
    src: &[u8],
    sw: usize,
    sh: usize,
    crop: (f64, f64, f64, f64),
    out: &mut [u8],
    ow: usize,
    oh: usize,
    dst: (usize, usize, usize, usize),
) {
    let (cx, cy, cw, ch) = crop;
    let (dx, dy, dw, dh) = dst;
    if dw == 0 || dh == 0 {
        return;
    }
    let sx_step = cw / dw as f64;
    let sy_step = ch / dh as f64;
    // Fixed point 16.16 source x for every output column.
    let xs: Vec<(usize, u32)> = (0..dw)
        .map(|x| {
            let fx = (cx + (x as f64 + 0.5) * sx_step - 0.5).clamp(0.0, (sw - 2) as f64);
            (fx as usize, ((fx.fract()) * 256.0) as u32)
        })
        .collect();
    for y in 0..dh {
        let fy = (cy + (y as f64 + 0.5) * sy_step - 0.5).clamp(0.0, (sh - 2) as f64);
        let (y0, wy) = (fy as usize, (fy.fract() * 256.0) as u32);
        let r0 = &src[y0 * sw..y0 * sw + sw];
        let r1 = &src[(y0 + 1) * sw..(y0 + 1) * sw + sw];
        let row = &mut out[(dy + y) * ow + dx..(dy + y) * ow + dx + dw];
        for (px, &(x0, wx)) in row.iter_mut().zip(&xs) {
            let a = r0[x0] as u32 * (256 - wx) + r0[x0 + 1] as u32 * wx;
            let b = r1[x0] as u32 * (256 - wx) + r1[x0 + 1] as u32 * wx;
            *px = ((a * (256 - wy) + b * wy) >> 16) as u8;
        }
    }
    let (suv, ouv) = (&src[sw * sh..], &mut out[ow * oh..]);
    for y in 0..dh / 2 {
        let sy = ((cy + (y as f64 * 2.0 + 1.0) * sy_step) as usize / 2).min(sh / 2 - 1);
        for x in 0..dw / 2 {
            let sx = ((cx + (x as f64 * 2.0 + 1.0) * sx_step) as usize / 2).min(sw / 2 - 1);
            let s = sy * sw + sx * 2;
            let d = (dy / 2 + y) * ow + dx + x * 2;
            ouv[d] = suv[s];
            ouv[d + 1] = suv[s + 1];
        }
    }
}

/// Brightness/contrast via a luma table, saturation/tint on chroma.
fn color(frame: &mut [u8], w: usize, h: usize, look: &Look) {
    if !look.has_color_change() {
        return;
    }
    let (mut bright, mut contrast, mut sat) = (look.brightness as f32, look.contrast as f32, look.saturation as f32);
    let (mut tint_u, mut tint_v) = (0f32, 0f32);
    match look.filter {
        Filter::None => {}
        Filter::Mono => sat = -100.0,
        Filter::Noir => {
            sat = -100.0;
            contrast += 45.0;
            bright -= 8.0;
        }
        Filter::Warm => {
            tint_u = -10.0;
            tint_v = 12.0;
            sat += 10.0;
        }
        Filter::Cool => {
            tint_u = 12.0;
            tint_v = -8.0;
        }
        Filter::Vivid => {
            sat += 45.0;
            contrast += 15.0;
        }
        Filter::Fade => {
            sat -= 35.0;
            contrast -= 30.0;
            bright += 8.0;
        }
    }
    let c = 1.0 + contrast.clamp(-100.0, 100.0) / 100.0;
    let b = bright.clamp(-100.0, 100.0) * 1.2;
    let lut: Vec<u8> = (0..256)
        .map(|y| (((y as f32 - 126.0) * c + 126.0 + b).clamp(16.0, 235.0)) as u8)
        .collect();
    for px in &mut frame[..w * h] {
        *px = lut[*px as usize];
    }
    let s = (1.0 + sat.clamp(-100.0, 100.0) / 100.0).max(0.0);
    if s != 1.0 || tint_u != 0.0 || tint_v != 0.0 {
        let uv = &mut frame[w * h..w * h * 3 / 2];
        for pair in uv.chunks_exact_mut(2) {
            pair[0] = ((pair[0] as f32 - 128.0) * s + 128.0 + tint_u).clamp(16.0, 240.0) as u8;
            pair[1] = ((pair[1] as f32 - 128.0) * s + 128.0 + tint_v).clamp(16.0, 240.0) as u8;
        }
    }
}
