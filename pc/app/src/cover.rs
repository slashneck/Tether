//! The cover: what Tether Camera shows instead of you (hotkey, or when the connection drops). A still image, an
//! animated GIF, or the built-in card.

use std::path::Path;
use std::time::{Duration, Instant};

use image::AnimationDecoder;

use crate::frames;

/// Longest side GIF/image frames are kept at. Tether Camera scales them up to whatever the app asked for.
const MAX_SIDE: u32 = 960;
/// Memory cap for long GIFs (~300 frames at 480p).
const MAX_FRAMES: usize = 300;

pub struct Cover {
    frames: Vec<(Vec<u8>, Duration)>,
    pub width: u32,
    pub height: u32,
    total: Duration,
    started: Instant,
}

impl Cover {
    pub fn builtin() -> Cover {
        let (w, h) = (960, 540);
        Cover::from_frames(vec![(frames::builtin_cover(w, h), Duration::from_secs(1))], w as u32, h as u32)
    }

    fn from_frames(frames: Vec<(Vec<u8>, Duration)>, width: u32, height: u32) -> Cover {
        let total = frames.iter().map(|f| f.1).sum::<Duration>().max(Duration::from_millis(1));
        Cover { frames, width, height, total, started: Instant::now() }
    }

    /// Loads a GIF (animated) or PNG/JPEG/WebP (still). Errors are short, readable sentences.
    pub fn load(path: &Path) -> Result<Cover, String> {
        let is_gif = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
        let mut rgba_frames: Vec<(image::RgbaImage, Duration)> = Vec::new();
        if is_gif {
            let file = std::fs::File::open(path).map_err(|e| format!("Can't open the file ({e})"))?;
            let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file))
                .map_err(|e| format!("Not a readable GIF ({e})"))?;
            for frame in decoder.into_frames().take(MAX_FRAMES) {
                let frame = frame.map_err(|e| format!("Broken GIF frame ({e})"))?;
                let (num, den) = frame.delay().numer_denom_ms();
                let ms = if den == 0 { 100 } else { (num / den).max(20) };
                rgba_frames.push((frame.into_buffer(), Duration::from_millis(ms as u64)));
            }
        } else {
            let img = image::open(path).map_err(|e| format!("Not a readable image ({e})"))?;
            rgba_frames.push((img.to_rgba8(), Duration::from_secs(1)));
        }
        if rgba_frames.is_empty() {
            return Err("The file has no pictures in it".into());
        }
        let (w, h) = rgba_frames[0].0.dimensions();
        let scale = (MAX_SIDE as f64 / w.max(h) as f64).min(1.0);
        let (tw, th) = (((w as f64 * scale) as u32).max(2), ((h as f64 * scale) as u32).max(2));
        let mut out = Vec::with_capacity(rgba_frames.len());
        let mut size = (0, 0);
        for (img, delay) in rgba_frames {
            let img = if scale < 1.0 {
                image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle)
            } else {
                img
            };
            let (nv12, ew, eh) = frames::rgba_to_nv12(img.as_raw(), img.width() as usize, img.height() as usize);
            size = (ew as u32, eh as u32);
            out.push((nv12, delay));
        }
        Ok(Cover::from_frames(out, size.0, size.1))
    }

    /// The frame to show right now.
    pub fn current(&self) -> &[u8] {
        if self.frames.len() == 1 {
            return &self.frames[0].0;
        }
        let mut t = Duration::from_nanos((self.started.elapsed().as_nanos() % self.total.as_nanos()) as u64);
        for (frame, delay) in &self.frames {
            if t < *delay {
                return frame;
            }
            t -= *delay;
        }
        &self.frames[0].0
    }
}
