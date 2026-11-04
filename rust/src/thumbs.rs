//! Image loader that decodes off the UI thread and scales images down to the size they are drawn at.
//! A 600x900 cover shown as a 164 px tile becomes a 256 px texture; nothing stays in RAM once the
//! texture is uploaded (if egui asks again, it is simply decoded again).

use egui::load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};
use egui::{ColorImage, Context};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

enum Entry {
    Pending,
    Ready(Arc<ColorImage>),
    /// handed to egui already; the texture lives on the GPU now
    Delivered,
    Failed(String),
}

#[derive(Default)]
pub struct Thumbs {
    cache: Arc<Mutex<HashMap<(String, u32), Entry>>>,
}

/// Size buckets, so a tile that animates between sizes keeps one texture.
pub fn bucket(px: f32) -> u32 {
    [256, 512, 1024, 2048, 4096].into_iter().find(|b| *b as f32 >= px).unwrap_or(4096)
}

fn max_side(hint: SizeHint) -> u32 {
    match hint {
        SizeHint::Width(w) => w,
        SizeHint::Height(h) => h,
        SizeHint::Size(w, h) => w.max(h),
        SizeHint::Scale(_) => 4096,
    }
}

impl ImageLoader for Thumbs {
    fn id(&self) -> &str {
        concat!(module_path!(), "::Thumbs")
    }

    fn load(&self, ctx: &Context, uri: &str, hint: SizeHint) -> ImageLoadResult {
        if !(uri.starts_with("file://") || uri.starts_with("http://") || uri.starts_with("https://")) {
            return Err(LoadError::NotSupported);
        }
        let lower = uri.to_ascii_lowercase();
        if lower.ends_with(".svg") || lower.ends_with(".gif") {
            return Err(LoadError::NotSupported); // animated / vector: leave to egui_extras
        }
        let max = max_side(hint);
        let key = (uri.to_string(), max);
        {
            let mut cache = self.cache.lock().unwrap();
            match cache.get(&key) {
                Some(Entry::Pending) => return Ok(ImagePoll::Pending { size: None }),
                Some(Entry::Failed(e)) => return Err(LoadError::Loading(e.clone())),
                Some(Entry::Ready(img)) => {
                    let img = img.clone();
                    cache.insert(key, Entry::Delivered);
                    return Ok(ImagePoll::Ready { image: img });
                }
                Some(Entry::Delivered) | None => {}
            }
        }
        let bytes = match ctx.try_load_bytes(uri)? {
            BytesPoll::Pending { size } => return Ok(ImagePoll::Pending { size }),
            BytesPoll::Ready { bytes, .. } => bytes,
        };
        self.cache.lock().unwrap().insert(key.clone(), Entry::Pending);
        let (cache, ctx) = (self.cache.clone(), ctx.clone());
        std::thread::spawn(move || {
            let entry = match decode(&bytes, max).and_then(|img| if is_steam_placeholder(&key.0, &bytes) { Err("Steam placeholder".into()) } else { Ok(img) }) {
                Ok(img) => Entry::Ready(Arc::new(img)),
                Err(e) => Entry::Failed(e),
            };
            // the compressed bytes are not needed any more (textures of other sizes stay)
            for loader in ctx.loaders().bytes.lock().iter() {
                loader.forget(&key.0);
            }
            cache.lock().unwrap().insert(key, entry);
            ctx.request_repaint();
        });
        Ok(ImagePoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        self.cache.lock().unwrap().retain(|(u, _), _| u != uri);
    }

    fn forget_all(&self) {
        self.cache.lock().unwrap().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .unwrap()
            .values()
            .map(|e| match e {
                Entry::Ready(i) => i.pixels.len() * 4,
                _ => 0,
            })
            .sum()
    }
}

/// The Steam CDN answers for a game without portrait art with a plain grey 300×450 picture, not
/// a 404: treat it as missing so the next candidate (the wide header) is used.
fn is_steam_placeholder(uri: &str, bytes: &[u8]) -> bool {
    if !uri.contains("steamstatic.com") || !uri.contains("library_600x900") || bytes.len() > 8 * 1024 {
        return false;
    }
    image::load_from_memory(bytes).is_ok_and(|img| img.color() == image::ColorType::L8 && img.width() == 300 && img.height() == 450)
}

fn decode(bytes: &[u8], max: u32) -> Result<ColorImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let img = if img.width().max(img.height()) > max {
        img.resize(max, max, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets() {
        assert_eq!(bucket(164.0), 256);
        assert_eq!(bucket(300.0), 512);
        assert_eq!(bucket(1920.0), 2048);
        assert_eq!(bucket(9000.0), 4096);
    }

    #[test]
    fn downscales() {
        let mut png = Vec::new();
        image::DynamicImage::new_rgba8(600, 900)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let img = decode(&png, 256).unwrap();
        assert_eq!(img.size[1], 256);
        assert!(img.size[0] <= 171 && img.size[0] >= 170);
    }
}
