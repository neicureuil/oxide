//! Gives freed heap memory back to the system. macOS malloc keeps pages it freed and returns them only
//! under memory pressure, so after a conversation switch or a burst of pictures the app's footprint
//! stayed tens of megabytes above what it uses (Settings › Debug: "in the heap", used / allocated).
use std::time::Duration;

/// How often the window checks while in front.
pub const EVERY: Duration = Duration::from_secs(30);

#[repr(C)]
#[derive(Default)]
struct MallocStatistics {
    blocks_in_use: u32,
    size_in_use: usize,
    max_size_in_use: usize,
    size_allocated: usize,
}

unsafe extern "C" {
    fn malloc_zone_statistics(zone: *mut std::ffi::c_void, stats: *mut MallocStatistics);
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
}

fn statistics() -> MallocStatistics {
    let mut stats = MallocStatistics::default();
    // SAFETY: null means every zone; `stats` is a valid out struct.
    unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut stats) };
    stats
}

/// Bytes malloc holds but the app does not use.
pub fn slack() -> usize {
    let stats = statistics();
    stats.size_allocated.saturating_sub(stats.size_in_use)
}

/// Returns every free page of every zone to the system; the bytes released.
pub fn relieve() -> usize {
    // SAFETY: null zone means all zones; a goal of 0 asks for as much as possible.
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_malloc_statistics_and_relieves() {
        // The struct matches malloc's: what is used is held, and something is in use.
        let stats = statistics();
        assert!(stats.size_in_use > 0);
        assert!(stats.size_allocated >= stats.size_in_use);
        let blocks: Vec<Vec<u8>> = (0..20_000).map(|_| vec![1u8; 1024]).collect();
        drop(blocks);
        // Bytes given back depend on the zones' state (other tests allocate meanwhile): it must just run.
        relieve();
        let _ = slack();
    }
}
