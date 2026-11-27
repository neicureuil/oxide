//! Bounded preview images. The renderer receives the downloaded file, not its network URL, and decodes it at the size given here.
use crate::app::*;
use crate::disk_cache;
use base64::Engine;
use teams_i18n::t;
/// Previews render in a 420×250 box; keep 2× for Retina and drop the rest.
const PREVIEW_W: u32 = 840;
const PREVIEW_H: u32 = 500;
pub const MAX_IMAGES: usize = 48;
/// Decoded pixels Swift may hold for the open conversation and earlier ones together.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// Of those, earlier conversations' previews: the disk cache brings back the others quickly.
pub const RETAINED_BYTES: usize = 16 * 1024 * 1024;

static IMAGE_BUDGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(MAX_IMAGE_BYTES);
static RETAINED_BUDGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(RETAINED_BYTES);

/// Decoded pixels the front end may hold: [`MAX_IMAGE_BYTES`] unless it set less.
pub fn max_image_bytes() -> usize {
    IMAGE_BUDGET.load(std::sync::atomic::Ordering::Relaxed)
}
/// Of those, earlier conversations' previews: [`RETAINED_BYTES`] unless the front end set less.
pub fn retained_bytes() -> usize {
    RETAINED_BUDGET.load(std::sync::atomic::Ordering::Relaxed)
}

/// Bounds the previews' decoded pixels: a front end keeping each picture twice (in memory and on the
/// GPU, as GPUI does) halves the defaults to stay within the same total. Set before the core boots.
#[uniffi::export]
pub fn set_image_budget(total_bytes: u64, retained_bytes: u64) {
    use std::sync::atomic::Ordering::Relaxed;
    IMAGE_BUDGET.store(total_bytes as usize, Relaxed);
    RETAINED_BUDGET.store((retained_bytes as usize).min(total_bytes as usize), Relaxed);
}
#[derive(Debug, Clone)]
pub enum ImageState {
    Loading,
    Ready(LoadedImage),
    Failed(String),
}
#[derive(Debug, Clone)]
pub struct LoadedImage {
    /// Size to decode at: the file's, fitted in the display box.
    pub width: u32,
    pub height: u32,
    /// The file as downloaded; empty once handed to [`Handoff`]: the state only keeps the size.
    pub data: Arc<Vec<u8>>,
    /// Pixels once decoded, counted against [`MAX_IMAGE_BYTES`].
    pub bytes: usize,
}
/// Ready preview files and voicemail audio awaiting Swift, behind their own lock so taking them never waits on an update.
#[derive(Debug, Default)]
pub struct Handoff {
    images: HashMap<String, (u32, u32, Arc<Vec<u8>>)>,
    /// Id, content type and bytes.
    voicemail: Option<(String, String, Vec<u8>)>,
}
impl Handoff {
    /// Moves the file of a ready preview here; the state keeps its size for the budget.
    pub fn offer(&mut self, src: String, image: &mut LoadedImage) {
        let data = std::mem::take(&mut image.data);
        self.images.insert(src, (image.width, image.height, data));
    }
    /// Swift has not taken this preview yet.
    pub fn waiting(&self, src: &str) -> bool {
        self.images.contains_key(src)
    }
    pub fn withdraw(&mut self, src: &str) {
        self.images.remove(src);
    }
    pub fn take_image(&mut self, src: &str) -> Option<(u32, u32, Vec<u8>)> {
        let (width, height, data) = self.images.remove(src)?;
        let data = Arc::try_unwrap(data).unwrap_or_else(|shared| shared.as_ref().clone());
        Some((width, height, data))
    }
    pub fn offer_voicemail(&mut self, id: String, kind: String, data: Vec<u8>) {
        self.voicemail = Some((id, kind, data));
    }
    pub fn withdraw_voicemail(&mut self) {
        self.voicemail = None;
    }
    pub fn take_voicemail(&mut self, id: &str) -> Option<(String, Vec<u8>)> {
        if self.voicemail.as_ref().is_none_or(|(a, _, _)| a != id) {
            return None;
        }
        self.voicemail.take().map(|(_, kind, data)| (kind, data))
    }
}
/// Previews of closed conversations (LRU first) whose pixels Swift keeps; they share `MAX_IMAGE_BYTES` and are evicted first.
#[derive(Debug, Default)]
pub struct Retained {
    entries: Vec<(String, LoadedImage)>,
    bytes: usize,
}
impl Retained {
    pub fn keep(&mut self, src: String, image: LoadedImage) {
        self.take(&src);
        self.bytes += image.bytes;
        self.entries.push((src, image));
    }
    pub fn take(&mut self, src: &str) -> Option<LoadedImage> {
        let index = self.entries.iter().position(|(s, _)| s == src)?;
        let (_, image) = self.entries.remove(index);
        self.bytes -= image.bytes;
        Some(image)
    }
    /// Drops the oldest previews until at most `budget` bytes remain.
    pub fn evict_to(&mut self, budget: usize) {
        let mut evicted = 0;
        while self.bytes > budget && evicted < self.entries.len() {
            self.bytes -= self.entries[evicted].1.bytes;
            evicted += 1;
        }
        self.entries.drain(..evicted);
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn srcs(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(src, _)| src)
    }
}
pub async fn load(client: Client, src: String) -> Result<LoadedImage, String> {
    let bytes = if src.starts_with("data:image/") {
        let (meta, data) = src
            .split_once(',')
            .ok_or(t("core.media.invalid_image_data"))?;
        if !meta.ends_with(";base64") || data.len() > 12 * 1024 * 1024 {
            return Err(t("core.media.unsupported_embedded_image_format").into());
        }
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| t("core.media.invalid_image_data"))?
    } else if let Some(bytes) = disk_cache::preview(&src).await {
        bytes
    } else {
        let bytes = client
            .download_image(&src)
            .await
            .map_err(|e| e.to_string())?;
        disk_cache::store(disk_cache::Kind::Preview, &src, &bytes).await;
        bytes
    };
    tokio::task::spawn_blocking(move || inspect(bytes, PREVIEW_W, PREVIEW_H))
        .await
        .map_err(|_| t("core.media.image_decoding_interrupted").to_owned())?
}
pub(crate) fn inspect_avatar(bytes: Vec<u8>) -> Result<LoadedImage, String> {
    inspect(bytes, 128, 128)
}
/// Reads only the header: Swift decodes the file straight at the fitted size, so no full-size pixels exist here.
fn inspect(bytes: Vec<u8>, max_width: u32, max_height: u32) -> Result<LoadedImage, String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| t("core.media.unknown_image_format"))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| t("core.media.invalid_image_or_unsupported_format"))?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 8_000_000 {
        return Err(t("core.media.image_too_large_to_preview").into());
    }
    let (width, height) = fit(width, height, max_width, max_height);
    Ok(LoadedImage {
        width,
        height,
        data: Arc::new(bytes),
        bytes: width as usize * height as usize * 4,
    })
}
/// Scales down, never up, keeping the aspect ratio.
fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width <= max_width && height <= max_height {
        return (width, height);
    }
    let ratio = f64::min(
        f64::from(max_width) / f64::from(width),
        f64::from(max_height) / f64::from(height),
    );
    let scaled = |side: u32| ((f64::from(side) * ratio).round() as u32).max(1);
    (scaled(width), scaled(height))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn image(bytes: usize) -> LoadedImage {
        LoadedImage {
            width: 1,
            height: 1,
            data: Arc::default(),
            bytes,
        }
    }
    #[test]
    fn retained_previews_evict_the_least_recently_shown_first() {
        let mut retained = Retained::default();
        retained.keep("a".into(), image(10));
        retained.keep("b".into(), image(10));
        retained.keep("c".into(), image(10));
        // Shown again: "a" becomes the most recent.
        let a = retained.take("a").unwrap();
        retained.keep("a".into(), a);
        retained.evict_to(20);
        assert_eq!(retained.srcs().collect::<Vec<_>>(), ["c", "a"]);
        assert_eq!(retained.bytes(), 20);
        retained.evict_to(0);
        assert_eq!(retained.srcs().count(), 0);
        assert_eq!(retained.bytes(), 0);
    }
    #[test]
    fn large_images_are_downscaled_to_preview_size() {
        let mut png = Vec::new();
        image::RgbImage::new(3000, 2000)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let loaded = inspect(png, PREVIEW_W, PREVIEW_H).unwrap();
        assert_eq!((loaded.width, loaded.height), (750, 500));
        assert_eq!(loaded.bytes, 750 * 500 * 4);
        assert_eq!(
            fit(100, 50, PREVIEW_W, PREVIEW_H),
            (100, 50),
            "never scaled up"
        );
    }
}
