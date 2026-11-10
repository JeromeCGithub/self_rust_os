use rustos_process_abi::principal::Capabilities as Caps;

#[test]
fn ambient_domains_are_denied_before_pointer_or_object_processing() {
    for (right, calls) in [
        (Caps::FILE_READ, &[5, 6, 27, 33][..]),
        (Caps::FILE_WRITE, &[7, 12][..]),
        (Caps::SPAWN, &[8, 29, 30, 126, 127][..]),
        (Caps::THREADS, &[98][..]),
        (Caps::IPC, &[120][..]),
        (Caps::NETWORK, &[64, 65, 66, 67, 69, 70][..]),
        (Caps::AUDIO, &[82, 83][..]),
    ] {
        for &number in calls {
            assert!(
                !Caps::NONE.permits(number, 0, 0),
                "ambient syscall {number}"
            );
            assert!(right.permits(number, 0, 0), "domain syscall {number}");
            let without = Caps::from_bits(Caps::ALL.bits() & !right.bits()).unwrap();
            assert!(!without.permits(number, 0, 0), "missing domain {number}");
        }
    }
    for number in [110, 119, 131, 512, u64::MAX] {
        assert!(!Caps::ALL.permits(number, 0, 0), "unknown syscall {number}");
    }
}

#[test]
fn descriptor_modes_include_creation_and_truncation_side_effects() {
    let rw = Caps::from_bits(Caps::FILE_READ.bits() | Caps::FILE_WRITE.bits()).unwrap();
    for (flags, read, write) in [
        (0, true, false),
        (1, false, true),
        (2, true, true),
        (0o100, true, true),
        (0o101, false, true),
        (0o102, true, true),
        (0o1001, false, true),
        (0o1002, true, true),
    ] {
        assert!(!Caps::NONE.permits(23, 0, flags));
        assert_eq!(Caps::FILE_READ.permits(23, 0, flags), read && !write);
        assert_eq!(Caps::FILE_WRITE.permits(23, 0, flags), write && !read);
        assert!(rw.permits(23, 0, flags));
    }
    for flags in [3, 0o1000, 0o1100, 0o2000, u64::MAX] {
        assert!(
            !Caps::ALL.permits(23, 0, flags),
            "invalid open flags {flags:o}"
        );
    }
    for number in [2, 4] {
        assert!(Caps::NONE.permits(number, 0, 0));
        assert!(!Caps::NONE.permits(number, 3, 0));
        assert!(Caps::FILE_READ.permits(number, 3, 0));
    }
    for fd in [1, 2] {
        assert!(Caps::NONE.permits(25, fd, 0));
    }
    assert!(!Caps::NONE.permits(25, 3, 0));
    assert!(Caps::FILE_WRITE.permits(25, 3, 0));
}

#[test]
fn owned_transport_cleanup_and_progress_do_not_require_factory_rights() {
    for number in [
        0, 1, 3, 9, 10, 17, 22, 24, 26, 28, 31, 32, 34, 35, 36, 37, 68, 80, 81, 85, 96, 97, 99,
        100, 101, 102, 103, 104, 105, 106, 107, 109, 121, 122, 123, 124, 125, 128, 129, 130,
    ] {
        assert!(
            Caps::NONE.permits(number, 0, 0),
            "owned/progress syscall {number}"
        );
    }
    assert!(Caps::NONE.permits(84, 17, 0));
    assert!(!Caps::NONE.permits(84, 17, 1));
    assert!(!Caps::NONE.permits(84, 17, 2));
    assert!(Caps::AUDIO.permits(84, 17, 2));
    for number in 0..=130 {
        for first in [0, 1, 6, u64::MAX] {
            if rustos_process_abi::isolation::permits(number, first) {
                assert!(Caps::NONE.permits(number, first, 0));
            }
        }
    }
}
