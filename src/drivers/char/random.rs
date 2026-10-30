//! RDRAND hardware entropy. No timestamps or protocol identifiers are RNG seeds.
/// Fill bytes using bounded retries for each hardware-sized word.
pub fn fill_with(out: &mut [u8], mut next: impl FnMut() -> Option<u64>) -> bool {
    for chunk in out.chunks_mut(8) {
        let mut value = None;
        for _ in 0..10 {
            if let Some(word) = next() {
                value = Some(word);
                break;
            }
        }
        let Some(value) = value else {
            return false;
        };
        chunk.copy_from_slice(&value.to_le_bytes()[..chunk.len()]);
    }
    true
}
#[cfg(target_os = "none")]
/// Obtain hardware entropy or return an explicit unavailable/failure error.
pub fn fill(out: &mut [u8]) -> Result<(), rustos_platform_abi::Error> {
    let rng = x86_64::instructions::random::RdRand::new()
        .ok_or(rustos_platform_abi::Error::Unavailable)?;
    if fill_with(out, || rng.get_u64()) {
        Ok(())
    } else {
        Err(rustos_platform_abi::Error::Failed)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn fills_complete_words_and_a_short_tail() {
        let mut out = [0; 11];
        assert!(fill_with(&mut out, || Some(0x0807060504030201)));
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3]);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn bounded_retry_and_empty_request() {
        let mut calls = 0;
        assert!(!fill_with(&mut [0; 8], || {
            calls += 1;
            None
        }));
        assert_eq!(calls, 10);
        assert!(fill_with(&mut [], || panic!(
            "empty fill must not touch hardware"
        )));
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn recovers_after_transient_hardware_failure() {
        let mut calls = 0;
        let mut out = [0; 8];
        assert!(fill_with(&mut out, || {
            calls += 1;
            if calls < 3 {
                None
            } else {
                Some(9)
            }
        }));
        assert_eq!(calls, 3);
        assert_eq!(out[0], 9);
    }
}
