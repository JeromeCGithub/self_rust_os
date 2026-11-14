use rustos_process_abi::principal::{self as abi, Capabilities as Caps, Principal};
fn init() -> Principal {
    Principal {
        user: 0,
        session: 0,
        scope: abi::SYSTEM_SCOPE,
        flags: abi::INIT,
        capabilities: Caps::ALL,
    }
}
#[test]
fn snapshot_is_exact_canonical_and_cannot_mix_system_user_and_legacy_contexts() {
    let principal = Principal {
        user: 1,
        session: 7,
        scope: 9,
        flags: 0,
        capabilities: Caps::from_bits(Caps::FILE_READ.bits() | Caps::IPC.bits()).unwrap(),
    };
    let bytes = principal.encode().expect("native principal codec");
    assert_eq!(bytes.len(), 40);
    assert_eq!(&bytes[..4], &1u32.to_le_bytes());
    assert_eq!(&bytes[8..12], &1u32.to_le_bytes());
    assert_eq!(&bytes[12..16], &[0; 4]);
    assert_eq!(Principal::decode(&bytes), Ok(principal));
    for count in 0..40 {
        assert!(Principal::decode(&bytes[..count]).is_err());
    }
    let mut tail = bytes.to_vec();
    tail.push(0);
    assert!(Principal::decode(&tail).is_err());
    for offset in [0, 4, 12, 39] {
        let mut bad = bytes;
        bad[offset] = 0xff;
        assert!(Principal::decode(&bad).is_err());
    }
    for invalid in [
        Principal {
            scope: 0,
            ..principal
        },
        Principal {
            scope: 1,
            ..principal
        },
        Principal {
            session: 0,
            ..principal
        },
        Principal {
            flags: abi::INIT,
            ..principal
        },
        Principal {
            flags: abi::LEGACY,
            ..principal
        },
        Principal {
            user: u32::MAX,
            ..principal
        },
        Principal {
            session: u64::MAX,
            ..principal
        },
        Principal {
            scope: u64::MAX,
            ..principal
        },
        Principal {
            flags: abi::INIT | abi::LEGACY,
            ..init()
        },
    ] {
        assert!(invalid.encode().is_err(), "{invalid:?}");
    }
    assert_eq!(Principal::decode(&init().encode().unwrap()), Ok(init()));
}
#[test]
fn ordinary_spawn_preserves_identity_scope_and_drops_noninheritable_authority() {
    let parent = init();
    let child = parent.ordinary_child();
    assert_eq!((child.user, child.session, child.scope), (0, 0, 1));
    assert_eq!(child.flags, 0, "PID 1 authority is never inherited");
    assert!(!child.capabilities.contains(Caps::SYSTEM_WRITE));
    assert!(!child.capabilities.contains(Caps::SCOPE_CREATE));
    assert!(child.capabilities.contains(Caps::FILE_READ));
    assert_eq!(child.ordinary_child(), child);
    let user = Principal {
        user: 1,
        session: 7,
        scope: 9,
        flags: 0,
        capabilities: Caps::IPC,
    };
    assert_eq!(user.ordinary_child(), user);
    assert_eq!(
        Principal {
            flags: abi::LEGACY,
            ..parent
        }
        .ordinary_child()
        .flags,
        abi::LEGACY
    );
}
#[test]
fn explicit_attenuation_can_only_remove_known_bits_and_never_add_authority() {
    assert_eq!(Caps::ALL.attenuate(Caps::IPC), Ok(Caps::IPC));
    assert_eq!(Caps::IPC.attenuate(Caps::NONE), Ok(Caps::NONE));
    assert!(Caps::IPC.attenuate(Caps::SPAWN).is_err());
    for bits in [512, 1 << 63, u64::MAX] {
        assert!(Caps::from_bits(bits).is_err());
    }
}
