//! Profile photos: asked for once per id and name, decoded off the main thread at the core's size, dropped when the core evicts them.
use gpui::RenderImage;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use teams_core::ImageData;

#[derive(Default)]
pub struct Avatars {
    pub images: HashMap<String, Arc<RenderImage>>,
    /// `id|name` pairs already asked of the core.
    pub requested: HashSet<String>,
    pub decoding: HashSet<String>,
    /// Ids the core still holds: anything else is dropped.
    pub kept: HashSet<String>,
}

impl Avatars {
    /// Returns the ids to take from the core and decode, and the pictures dropped (to free on the GPU).
    pub fn keep(&mut self, ids: Vec<String>) -> (Vec<String>, Vec<Arc<RenderImage>>) {
        if ids.is_empty() {
            self.requested.clear();
        }
        self.kept = ids.into_iter().collect();
        let evicted: HashSet<String> = self
            .images
            .keys()
            .filter(|id| !self.kept.contains(*id))
            .cloned()
            .collect();
        let dropped: Vec<Arc<RenderImage>> = evicted.iter().filter_map(|id| self.images.remove(id)).collect();
        if !evicted.is_empty() {
            self.requested
                .retain(|key| !evicted.contains(key.split('|').next().unwrap_or_default()));
        }
        let wanted = self
            .kept
            .iter()
            .filter(|id| !self.images.contains_key(*id) && self.decoding.insert((*id).clone()))
            .cloned()
            .collect();
        (wanted, dropped)
    }
}

/// Decodes the downloaded file at the core's size, into the BGRA frame GPUI uploads.
pub fn decode(data: ImageData) -> Option<Arc<RenderImage>> {
    let picture = image::load_from_memory(&data.data).ok()?;
    let mut pixels = picture
        .resize_to_fill(
            data.width.max(1),
            data.height.max(1),
            image::imageops::FilterType::Triangle,
        )
        .into_rgba8();
    for px in pixels.pixels_mut() {
        px.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}
