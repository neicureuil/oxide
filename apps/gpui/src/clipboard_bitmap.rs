//! Windows: a clipboard picture offered only as a bitmap (`CF_DIB`: Print Screen, Paint, many apps). GPUI reads
//! pictures from the "PNG", GIF and JPEG formats only, so the composer would see nothing to paste.
use std::ffi::c_void;

#[link(name = "user32")]
unsafe extern "system" {
    fn OpenClipboard(owner: *mut c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut c_void;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalLock(memory: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(memory: *mut c_void) -> i32;
    fn GlobalSize(memory: *mut c_void) -> usize;
}

const CF_DIB: u32 = 8;

/// The clipboard's bitmap as PNG, if there is one.
pub fn read_png() -> Option<gpui::Image> {
    let dib = read_dib()?;
    let image =
        image::load_from_memory_with_format(&bmp_file(&dib)?, image::ImageFormat::Bmp).ok()?;
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(gpui::Image::from_bytes(
        gpui::ImageFormat::Png,
        png.into_inner(),
    ))
}

fn read_dib() -> Option<Vec<u8>> {
    // SAFETY: plain Win32 calls; the data is copied out while the clipboard is open and the block locked.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_DIB);
        let data = if handle.is_null() {
            None
        } else {
            let size = GlobalSize(handle);
            let bytes = GlobalLock(handle) as *const u8;
            let copy = (!bytes.is_null() && size > 0)
                .then(|| std::slice::from_raw_parts(bytes, size).to_vec());
            GlobalUnlock(handle);
            copy
        };
        CloseClipboard();
        data
    }
}

/// A device-independent bitmap with the 14-byte file header a `.bmp` starts with.
fn bmp_file(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |at: usize| Some(u32::from_le_bytes(dib.get(at..at + 4)?.try_into().ok()?));
    let header = u32_at(0)? as usize;
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32_at(16)?;
    let used = u32_at(32)? as usize;
    let palette = if bits <= 8 {
        (if used == 0 { 1 << bits } else { used }) * 4
    } else {
        used * 4
    };
    // BI_BITFIELDS with the old 40-byte header: the three masks follow it.
    let masks = if header == 40 && compression == 3 {
        12
    } else {
        0
    };
    let offset = 14 + header + masks + palette;
    let size = 14 + dib.len();
    let mut file = Vec::with_capacity(size);
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&u32::try_from(size).ok()?.to_le_bytes());
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&u32::try_from(offset).ok()?.to_le_bytes());
    file.extend_from_slice(dib);
    Some(file)
}
