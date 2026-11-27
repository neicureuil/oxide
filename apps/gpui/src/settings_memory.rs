//! Where the app's memory goes, for Settings › Debug: the same Mach counters as the macOS app's `MemoryReport`.
use crate::oxide::tr;

/// What a part of the address space is used for, from the tag the system puts on each VM region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemoryKind {
    Heap,
    Graphics,
    Images,
    Media,
    WebKit,
    Stacks,
    Code,
    Other,
    /// GPU memory the app owns without mapping it (Metal textures): no region shows it.
    GpuTextures,
    /// The rest of the footprint no region shows (kernel-owned memory, page tables).
    Unmapped,
}

impl MemoryKind {
    pub fn label(self) -> String {
        tr(match self {
            Self::Heap => "ui.debug.kind_heap",
            Self::Graphics => "ui.debug.kind_graphics",
            Self::Images => "ui.debug.kind_images",
            Self::Media => "ui.debug.kind_media",
            Self::WebKit => "ui.debug.kind_webkit",
            Self::Stacks => "ui.debug.kind_stacks",
            Self::Code => "ui.debug.kind_code",
            Self::Other => "ui.debug.kind_other",
            Self::GpuTextures => {
                return crate::oxide::text("Mémoire GPU (textures)", "GPU memory (textures)")
                    .to_owned();
            }
            Self::Unmapped => {
                return crate::oxide::text(
                    "Mémoire non mappée (système)",
                    "Unmapped memory (system)",
                )
                .to_owned();
            }
        })
    }

    /// `VM_MEMORY_*` tags from <mach/vm_statistics.h>; untagged file mappings are code and data of libraries.
    pub fn of(tag: u32, file_backed: bool) -> Self {
        match tag {
            1..=9 | 11..=13 => Self::Heap,
            21 | 42 | 51 | 54..=58 | 66 | 68 | 88 | 100 => Self::Graphics,
            52 | 70 | 76 | 77 | 103 => Self::Images,
            90..=96 | 101 | 106 => Self::Media,
            63..=65 | 69 => Self::WebKit,
            30 | 31 => Self::Stacks,
            33 | 60 | 61 | 82 | 83 => Self::Code,
            0 if file_backed => Self::Code,
            _ => Self::Other,
        }
    }
}

/// Decoded pictures the window keeps.
#[derive(Clone, Copy, Debug, Default)]
pub struct ImageMemory {
    pub count: usize,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryReport {
    /// What Activity Monitor shows as Memory.
    pub footprint: u64,
    pub peak: u64,
    pub resident: u64,
    /// Dirty and compressed bytes per kind, largest first.
    pub kinds: Vec<(MemoryKind, u64)>,
    pub heap_in_use: u64,
    pub heap_allocated: u64,
    /// Heap pages written to (regions tagged malloc), used or freed but kept by malloc.
    pub heap_dirty: u64,
    /// Bytes the Rust allocator has handed out and not got back: the whole app's, core and window.
    pub rust_live: u64,
    pub avatars: ImageMemory,
    pub previews: ImageMemory,
    pub cache_preview_files: u64,
    pub cache_preview_bytes: u64,
    pub cache_photo_files: u64,
    pub cache_photo_bytes: u64,
}

#[cfg(target_os = "macos")]
pub use mach::{footprint, measure};
#[cfg(windows)]
pub use win::{footprint, measure};
#[cfg(not(any(target_os = "macos", windows)))]
pub fn footprint() -> u64 {
    0
}
#[cfg(not(any(target_os = "macos", windows)))]
pub fn measure() -> MemoryReport {
    let mut report = MemoryReport::default();
    fill_common(&mut report);
    report
}

/// What every platform reports alike: the Rust allocator and the core's picture cache.
fn fill_common(report: &mut MemoryReport) {
    report.rust_live = teams_core::rust_memory().live_bytes;
    let cache = teams_core::picture_cache();
    report.cache_preview_files = cache.preview_files;
    report.cache_preview_bytes = cache.preview_bytes;
    report.cache_photo_files = cache.photo_files;
    report.cache_photo_bytes = cache.photo_bytes;
}

/// Windows: the process's private bytes (Task Manager's "Memory"), its peak and its working set.
#[cfg(windows)]
mod win {
    use super::{MemoryReport, fill_common};
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCountersEx,
            cb: u32,
        ) -> i32;
    }

    fn counters() -> Option<ProcessMemoryCountersEx> {
        let mut counters = ProcessMemoryCountersEx {
            cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
            ..Default::default()
        };
        // SAFETY: the current process's pseudo handle, and a struct of the size given.
        let ok =
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        (ok != 0).then_some(counters)
    }

    pub fn footprint() -> u64 {
        counters().map_or(0, |c| c.private_usage as u64)
    }

    pub fn measure() -> MemoryReport {
        let mut report = MemoryReport::default();
        if let Some(c) = counters() {
            report.footprint = c.private_usage as u64;
            report.peak = c.peak_pagefile_usage as u64;
            report.resident = c.working_set_size as u64;
        }
        fill_common(&mut report);
        report
    }
}

/// macOS: the same Mach counters as the macOS app's `MemoryReport`.
#[cfg(target_os = "macos")]
mod mach {
    use super::*;
    use std::ffi::c_void;

    type KernReturn = i32;
    const KERN_SUCCESS: KernReturn = 0;
    const TASK_VM_INFO: u32 = 22;
    /// `VM_REGION_SUBMAP_INFO_COUNT_64`: 76 bytes, packed on 4.
    const SUBMAP_INFO_COUNT: u32 = 19;

    #[repr(C)]
    #[derive(Default)]
    struct MallocStatistics {
        blocks_in_use: u32,
        size_in_use: usize,
        max_size_in_use: usize,
        size_allocated: usize,
    }

    unsafe extern "C" {
        static mach_task_self_: u32;
        static vm_kernel_page_size: usize;
        fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> KernReturn;
        fn mach_vm_region_recurse(
            task: u32,
            address: *mut u64,
            size: *mut u64,
            depth: *mut u32,
            info: *mut i32,
            count: *mut u32,
        ) -> KernReturn;
        fn malloc_zone_statistics(zone: *mut c_void, stats: *mut MallocStatistics);
    }

    fn u64_at(bytes: &[u8], offset: usize) -> u64 {
        bytes
            .get(offset..offset + 8)
            .map(|b| u64::from_ne_bytes(b.try_into().unwrap_or_default()))
            .unwrap_or(0)
    }
    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        bytes
            .get(offset..offset + 4)
            .map(|b| u32::from_ne_bytes(b.try_into().unwrap_or_default()))
            .unwrap_or(0)
    }

    /// What `task_vm_info` says beyond the regions: memory the process owns without mapping it.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Ledgers {
        /// GPU memory charged to the app (Metal textures and buffers), with its compressed part.
        pub graphics: u64,
        /// Same for media (video decoding, camera).
        pub media: u64,
        /// Same for the neural engine.
        pub neural: u64,
    }

    /// `task_vm_info` from the kernel, as bytes (offsets of the packed struct, revisions 1 to 3).
    fn task_vm_info() -> Option<Vec<u8>> {
        // Larger than any revision: the kernel fills what it knows and says how much.
        let mut buffer = [0i32; 128];
        let mut count = buffer.len() as u32;
        // SAFETY: the buffer holds `count` naturals, as `task_info` requires.
        let status = unsafe {
            task_info(
                mach_task_self_,
                TASK_VM_INFO,
                buffer.as_mut_ptr(),
                &mut count,
            )
        };
        if status != KERN_SUCCESS {
            return None;
        }
        Some(
            buffer[..count as usize]
                .iter()
                .flat_map(|n| n.to_ne_bytes())
                .collect(),
        )
    }

    /// (footprint, peak, resident).
    fn task_counters() -> Option<(u64, u64, u64)> {
        let bytes = task_vm_info()?;
        let resident = u64_at(&bytes, 16);
        let footprint = u64_at(&bytes, 144);
        let peak = u64_at(&bytes, 168);
        Some((footprint, peak, resident))
    }

    /// The per-tag ledgers (rev 3): footprint plus its compressed part, for graphics, media and neural memory.
    fn ledgers() -> Ledgers {
        let Some(bytes) = task_vm_info() else {
            return Ledgers::default();
        };
        // Missing on older kernels: `u64_at` reads 0 past what was filled.
        Ledgers {
            graphics: u64_at(&bytes, 272) + u64_at(&bytes, 280),
            media: u64_at(&bytes, 240) + u64_at(&bytes, 248),
            neural: u64_at(&bytes, 304) + u64_at(&bytes, 312),
        }
    }

    /// The footprint alone, cheap enough for the settings home.
    pub fn footprint() -> u64 {
        task_counters().map(|c| c.0).unwrap_or(0)
    }

    /// Reads the process counters and walks its regions; a few milliseconds, so off the main thread.
    pub fn measure() -> MemoryReport {
        let mut report = MemoryReport::default();
        if let Some((footprint, peak, resident)) = task_counters() {
            report.footprint = footprint;
            report.peak = peak;
            report.resident = resident;
        }
        let mut totals: Vec<(MemoryKind, u64)> = Vec::new();
        let mut gpu_mapped = 0u64;
        // SAFETY: a plain exported constant.
        let page = unsafe { vm_kernel_page_size } as u64;
        let (mut address, mut size, mut depth) = (0u64, 0u64, 0u32);
        loop {
            let mut info = [0i32; SUBMAP_INFO_COUNT as usize];
            let mut count = SUBMAP_INFO_COUNT;
            // SAFETY: every out pointer is valid; `info` holds `count` naturals.
            let status = unsafe {
                mach_vm_region_recurse(
                    mach_task_self_,
                    &mut address,
                    &mut size,
                    &mut depth,
                    info.as_mut_ptr(),
                    &mut count,
                )
            };
            if status != KERN_SUCCESS {
                break;
            }
            let bytes: Vec<u8> = info.iter().flat_map(|n| n.to_ne_bytes()).collect();
            if u32_at(&bytes, 48) != 0 {
                depth += 1;
                continue;
            }
            let user_tag = u32_at(&bytes, 20);
            let swapped = u32_at(&bytes, 32) as u64;
            let dirtied = u32_at(&bytes, 36) as u64;
            let external_pager = bytes[46] != 0;
            let used = (dirtied + swapped) * page;
            // `VM_MEMORY_IOACCELERATOR`: GPU memory mapped here, also in the graphics ledger.
            if user_tag == 88 {
                gpu_mapped += used;
            }
            if used > 0 {
                let kind = MemoryKind::of(user_tag, external_pager);
                match totals.iter_mut().find(|(k, _)| *k == kind) {
                    Some(entry) => entry.1 += used,
                    None => totals.push((kind, used)),
                }
            }
            address = address.saturating_add(size);
        }
        // What no region shows: the footprint also counts memory the app owns unmapped, mostly GPU textures,
        // which the graphics ledger counts along with the GPU memory mapped above.
        let regions: u64 = totals.iter().map(|k| k.1).sum();
        let unmapped = report.footprint.saturating_sub(regions);
        let gpu = ledgers().graphics.saturating_sub(gpu_mapped).min(unmapped);
        for (kind, bytes) in [
            (MemoryKind::GpuTextures, gpu),
            (MemoryKind::Unmapped, unmapped - gpu),
        ] {
            if bytes > 0 {
                totals.push((kind, bytes));
            }
        }
        report.heap_dirty = totals
            .iter()
            .find(|k| k.0 == MemoryKind::Heap)
            .map_or(0, |k| k.1);
        totals.sort_by(|a, b| b.1.cmp(&a.1));
        report.kinds = totals;
        fill_common(&mut report);
        let mut heap = MallocStatistics::default();
        // SAFETY: null means every zone; `heap` is a valid out struct.
        unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut heap) };
        report.heap_in_use = heap.size_in_use as u64;
        report.heap_allocated = heap.size_allocated as u64;
        report
    }
}

/// "12.3 MB", as `ByteCountFormatter`'s memory style (binary units, decimal names).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!(
            "{bytes} {}",
            if crate::oxide::english() {
                "bytes"
            } else {
                "octets"
            }
        );
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    let text = if unit <= 1 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    };
    let text = if crate::oxide::english() {
        text
    } else {
        text.replace('.', ",")
    };
    let name = if crate::oxide::english() {
        UNITS[unit].to_owned()
    } else {
        UNITS[unit].replace('B', "o")
    };
    format!("{text} {name}")
}

/// "Freed but kept by malloc", for the heap card and the report.
pub fn freed_kept_label() -> String {
    crate::oxide::text("Libéré mais gardé par malloc", "Freed but kept by malloc").to_owned()
}
pub fn rust_label() -> String {
    crate::oxide::text(
        "Allocations Rust (toute l'app)",
        "Rust allocations (whole app)",
    )
    .to_owned()
}

impl MemoryReport {
    /// Heap pages written to but not in use: malloc keeps them until it gives them back.
    pub fn freed_kept(&self) -> u64 {
        self.heap_dirty.saturating_sub(self.heap_in_use)
    }

    /// Plain text to paste in a bug report.
    pub fn text(&self) -> String {
        let mut lines = vec![format!(
            "{}: {} ({} {}, {} {})",
            tr("ui.debug.footprint"),
            size(self.footprint),
            tr("ui.debug.peak"),
            size(self.peak),
            tr("ui.debug.resident"),
            size(self.resident)
        )];
        lines.extend(
            self.kinds
                .iter()
                .map(|(k, b)| format!("{}: {}", k.label(), size(*b))),
        );
        lines.push(format!(
            "{}: {} / {}",
            tr("ui.debug.heap"),
            size(self.heap_in_use),
            size(self.heap_allocated)
        ));
        lines.push(format!(
            "{}: {}",
            freed_kept_label(),
            size(self.freed_kept())
        ));
        lines.push(format!("{}: {}", rust_label(), size(self.rust_live)));
        lines.push(format!(
            "{}: {}, {}",
            tr("ui.debug.avatars"),
            self.avatars.count,
            size(self.avatars.bytes)
        ));
        lines.push(format!(
            "{}: {}, {}",
            tr("ui.debug.previews"),
            self.previews.count,
            size(self.previews.bytes)
        ));
        lines.push(format!(
            "{}: {} ({} {}, {} {})",
            tr("ui.debug.disk_cache"),
            size(self.cache_preview_bytes + self.cache_photo_bytes),
            tr("ui.debug.cache_previews"),
            self.cache_preview_files,
            tr("ui.debug.cache_photos"),
            self.cache_photo_files
        ));
        lines.join("\n")
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn measures_this_process() {
        let report = measure();
        assert!(report.footprint > 0);
        assert!(report.resident > 0);
        assert!(report.peak >= report.footprint / 2);
        assert!(report.heap_in_use > 0);
        assert!(!report.kinds.is_empty());
        // Every byte of the footprint is in a kind, the unmapped ones included.
        let total: u64 = report.kinds.iter().map(|k| k.1).sum();
        assert!(
            total >= report.footprint * 95 / 100,
            "{total} of {}",
            report.footprint
        );
        assert!(report.rust_live > 0);
    }

    #[test]
    fn sizes() {
        assert!(size(512).starts_with("512"));
        assert!(size(3 * 1024 * 1024).starts_with('3'));
    }
}
