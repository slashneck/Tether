//! The test card: the Tether mark on black, with a bar sweeping along the bottom so you can see it's live.
//! Rendered once per frame size, then only the bar is redrawn.

use std::time::Instant;

const BLACK: u8 = 16;
const WHITE: u8 = 235;
const GREY: u8 = 48;
const CHROMA_NEUTRAL: u8 = 128;

pub struct Pattern {
    base: Vec<u8>,
    size: (usize, usize),
    started: Instant,
}

impl Pattern {
    pub fn new() -> Self {
        Pattern { base: Vec::new(), size: (0, 0), started: Instant::now() }
    }

    pub fn render(&mut self, out: &mut [u8], w: usize, h: usize) {
        if self.size != (w, h) {
            self.base = base_frame(w, h);
            self.size = (w, h);
        }
        out.copy_from_slice(&self.base);

        // Sweeping bar along a thin grey track near the bottom.
        let t = self.started.elapsed().as_secs_f64();
        let track_y = h * 88 / 100;
        let track_x0 = w / 4;
        let track_w = w / 2;
        let bar_w = track_w / 5;
        let phase = (t / 1.6).fract();
        let ping = if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 };
        let eased = ping * ping * (3.0 - 2.0 * ping);
        let x0 = track_x0 + ((track_w - bar_w) as f64 * eased) as usize;
        let thickness = (h / 270).max(2);
        for y in track_y..track_y + thickness {
            out[y * w + x0..y * w + x0 + bar_w].fill(WHITE);
        }
    }
}

fn base_frame(w: usize, h: usize) -> Vec<u8> {
    let mut frame = vec![BLACK; w * h * 3 / 2];
    frame[w * h..].fill(CHROMA_NEUTRAL);

    // The mark, scaled from the 512-unit app icon so it fills ~55% of the frame height.
    let u = h as f64 * 0.55 / 512.0;
    let ox = w as f64 / 2.0 - 256.0 * u;
    let oy = h as f64 * 0.44 - 256.0 * u;
    let (cx, cy) = (ox + 256.0 * u, oy + 216.0 * u);
    let (outer, inner, dot) = (130.0 * u, 94.0 * u, 34.0 * u);
    let (cable_x0, cable_x1) = (ox + 238.0 * u, ox + 274.0 * u);
    let (cable_y0, cable_y1) = (oy + 328.0 * u, oy + 430.0 * u);
    let cap = 18.0 * u;

    let y_min = (cy - outer).max(0.0) as usize;
    let y_max = ((cable_y1 + cap).ceil() as usize).min(h);
    for y in y_min..y_max {
        let py = y as f64 + 0.5;
        for x in 0..w {
            let px = x as f64 + 0.5;
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let in_ring = d <= outer && d >= inner;
            let in_dot = d <= dot;
            let in_cable = px >= cable_x0 && px <= cable_x1 && py >= cable_y0 && py <= cable_y1;
            let in_cap = ((px - cx).powi(2) + (py - cable_y1).powi(2)).sqrt() <= cap;
            if in_ring || in_dot || in_cable || in_cap {
                frame[y * w + x] = WHITE;
            }
        }
    }

    // The empty track the bar runs on.
    let track_y = h * 88 / 100;
    let thickness = (h / 270).max(2);
    for y in track_y..track_y + thickness {
        frame[y * w + w / 4..y * w + w / 4 + w / 2].fill(GREY);
    }
    frame
}
