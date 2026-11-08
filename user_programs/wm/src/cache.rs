//! Window-owned, fallibly allocated caches; no display-sized per-slot BSS arrays.
extern crate alloc;
use alloc::vec::Vec;
pub struct Cache {
    pub width: usize,
    pub height: usize,
    pub content: Vec<u32>,
    pub backdrop: Vec<u32>,
}
impl Cache {
    pub fn empty() -> Self {
        Self {
            width: 0,
            height: 0,
            content: Vec::new(),
            backdrop: Vec::new(),
        }
    }
    pub fn new(width: usize, height: usize) -> Option<Self> {
        if width == 0 || height == 0 || width > 1920 || height > 1080 {
            return None;
        }
        fn buffer(len: usize) -> Option<Vec<u32>> {
            let mut v = Vec::new();
            v.try_reserve_exact(len).ok()?;
            v.resize(len, 0);
            Some(v)
        }
        Some(Self {
            width,
            height,
            content: buffer(width.checked_mul(height)?)?,
            backdrop: buffer(width.checked_mul(height.checked_add(28)?)?)?,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_dimensions_bound_both_owned_buffers_and_initialize_every_pixel() {
        let cache = Cache::new(1920, 992).expect("full workspace cache");
        assert_eq!((cache.width, cache.height), (1920, 992));
        assert_eq!(cache.content.len(), 1920 * 992);
        assert_eq!(cache.backdrop.len(), 1920 * 1020);
        assert!(cache.content.iter().all(|&x| x == 0));
        assert!(cache.backdrop.iter().all(|&x| x == 0));
        for (w, h) in [
            (0, 1),
            (1, 0),
            (1921, 100),
            (100, 1081),
            (usize::MAX, usize::MAX),
        ] {
            assert!(Cache::new(w, h).is_none());
        }
    }
}
