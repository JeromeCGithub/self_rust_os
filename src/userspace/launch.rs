//! Copy complete untrusted argv/envp records before loading or scheduling.
use crate::scheduler::{grow_resource, reserve_resource, ResourceBuffer, ResourceLease};
use alloc::{string::String, vec::Vec};
use rustos_scope_core::Usage;
const ERROR: u64 = u64::MAX - 1;
struct StringsSnapshot {
    values: Vec<String>,
    _lease: ResourceLease,
}
impl core::ops::Deref for StringsSnapshot {
    type Target = [String];
    fn deref(&self) -> &[String] {
        &self.values
    }
}
fn capacity_extra<T>(lease: &ResourceLease, capacity: usize, expected: usize) -> Result<(), ()> {
    let extra = capacity
        .checked_sub(expected)
        .and_then(|n| n.checked_mul(core::mem::size_of::<T>()))
        .ok_or(())?;
    if extra != 0 {
        grow_resource(
            lease,
            Usage {
                kernel_bytes: extra as u64,
                ..Usage::ZERO
            },
        )
        .map_err(|_| ())?;
    }
    Ok(())
}
fn word(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("fixed wire word"))
}
fn strings(pointer: u64, count: u64, budget: &mut usize) -> Result<StringsSnapshot, ()> {
    if count > rustos_process_abi::MAX_VECTOR as u64 {
        return Err(());
    }
    let count = count as usize;
    let mut records = ResourceBuffer::zeroed(count * 16).map_err(|_| ())?;
    if !super::usercopy::from_user(pointer, &mut records) {
        return Err(());
    }
    let mut payload = 0usize;
    for record in records.chunks_exact(16) {
        let length = word(&record[8..]);
        if length >= rustos_process_abi::MAX_STARTUP_BYTES as u64 {
            return Err(());
        }
        *budget = budget
            .checked_add(length as usize + 1)
            .filter(|&n| n <= rustos_process_abi::MAX_STARTUP_BYTES)
            .ok_or(())?;
        payload = payload.checked_add(length as usize).ok_or(())?;
    }
    let expected = count
        .checked_mul(core::mem::size_of::<String>())
        .and_then(|n| n.checked_add(payload))
        .ok_or(())?;
    // The Vec and every contained String drop before the owning charge. A wire
    // snapshot is separately owned, so no mutable user records survive copying.
    let lease = reserve_resource(Usage {
        kernel_bytes: expected as u64,
        ..Usage::ZERO
    })
    .map_err(|_| ())?;
    let mut values = Vec::new();
    values.try_reserve_exact(count).map_err(|_| ())?;
    capacity_extra::<String>(&lease, values.capacity(), count)?;
    for record in records.chunks_exact(16) {
        let pointer = word(&record[..8]);
        let length = word(&record[8..]) as usize;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(|_| ())?;
        capacity_extra::<u8>(&lease, bytes.capacity(), length)?;
        bytes.resize(length, 0);
        if !super::usercopy::from_user(pointer, &mut bytes) || bytes.contains(&0) {
            return Err(());
        }
        values.push(String::from_utf8(bytes).map_err(|_| ())?);
    }
    Ok(StringsSnapshot {
        values,
        _lease: lease,
    })
}
pub(crate) fn spawn(path: u64, request: u64) -> u64 {
    let mut bytes = [0; 48];
    if !super::usercopy::from_user(request, &mut bytes) {
        return ERROR;
    }
    let fields: [u64; 6] = core::array::from_fn(|index| word(&bytes[index * 8..index * 8 + 8]));
    if fields[0] != 1 || fields[1] > 1 || (fields[1] == 1 && !crate::scheduler::can_connect_stdio())
    {
        return ERROR;
    }
    let request = rustos_ipc_abi::Spawn {
        argv: fields[2],
        argc: fields[3],
        envp: fields[4],
        envc: fields[5],
        bindings: 0,
        binding_count: 0,
    };
    with_launch_snapshot(request, |argv, envp, _| {
        let Some(path) = crate::fs::handles::path(path, crate::fs::handles::PathKind::Existing)
        else {
            return ERROR;
        };
        super::syscall::spawn_at(&path, fields[1] == 1, argv, envp)
    })
}

/// Copy the complete v2 request, vectors and endpoint records before filesystem
/// loading can release entry. Rights/owner/quota checks repeat under the scheduler
/// lock immediately before child resource construction and publication.
pub(crate) fn spawn_bound(path: u64, request: u64) -> u64 {
    use rustos_ipc_abi as abi;
    let mut bytes = [0; abi::SPAWN_SIZE];
    if !super::usercopy::from_user(request, &mut bytes) {
        return ERROR;
    }
    let Ok(request) = abi::Spawn::decode(&bytes) else {
        return ERROR;
    };
    let Some(path) = crate::fs::handles::path(path, crate::fs::handles::PathKind::Existing) else {
        return ERROR;
    };
    with_launch_snapshot(request, |argv, envp, bindings| {
        super::syscall::spawn_at_bound(&path, argv, envp, bindings)
    })
}
/// Snapshot an entire mapped user image, then let the existing native loader
/// validate it and atomically publish its private frames/explicit endpoint moves.
pub fn spawn_image(image: u64, packet: u64) -> u64 {
    use rustos_ipc_abi as abi;
    let mut bytes = [0; abi::IMAGE_SPAWN_SIZE];
    if !super::usercopy::from_user(packet, &mut bytes) {
        return ERROR;
    }
    let Ok(request) = abi::ImageSpawn::decode(&bytes) else {
        return ERROR;
    };
    let maximum = rustos_thread_abi::IMAGE_END - super::USER_CODE_START;
    if request.image_length > maximum
        || !super::usercopy::validate(image, request.image_length, false)
    {
        return ERROR;
    }
    let Ok(length) = usize::try_from(request.image_length) else {
        return ERROR;
    };
    let Ok(mut binary) = crate::scheduler::ResourceBuffer::zeroed(length) else {
        return ERROR;
    };
    if !super::usercopy::from_user(image, &mut binary) || !crate::scheduler::current_is_live() {
        return ERROR;
    }
    with_launch_snapshot(request.launch, |argv, envp, bindings| {
        crate::scheduler::spawn_process_with_bindings(&binary, 0, argv, envp, bindings)
            .map(u64::from)
            .unwrap_or(ERROR)
    })
}
/// Snapshot a decoded native launch request in the current process context.
/// Kernel callbacks receive only owned validated vectors/binding metadata; the
/// constructor must still authenticate endpoint ownership before publication.
pub fn with_launch_snapshot(
    request: rustos_ipc_abi::Spawn,
    finish: impl FnOnce(&[&str], &[&str], &[rustos_ipc_abi::Binding]) -> u64,
) -> u64 {
    use rustos_ipc_abi as abi;
    if request.encode().is_err() {
        return ERROR;
    }
    let mut records = [0; abi::MAX_BINDINGS * abi::BINDING_SIZE];
    let length = request.binding_count as usize * abi::BINDING_SIZE;
    if !super::usercopy::from_user(request.bindings, &mut records[..length]) {
        return ERROR;
    }
    let Ok(bindings_lease) = reserve_resource(Usage {
        kernel_bytes: request.binding_count * core::mem::size_of::<abi::Binding>() as u64,
        ..Usage::ZERO
    }) else {
        return ERROR;
    };
    let mut bindings = Vec::new();
    if bindings
        .try_reserve_exact(request.binding_count as usize)
        .is_err()
    {
        return ERROR;
    }
    if capacity_extra::<abi::Binding>(
        &bindings_lease,
        bindings.capacity(),
        request.binding_count as usize,
    )
    .is_err()
    {
        return ERROR;
    }
    for record in records[..length].chunks_exact(abi::BINDING_SIZE) {
        let Ok(binding) = abi::Binding::decode(record) else {
            return ERROR;
        };
        bindings.push(binding);
    }
    let mut budget = 0;
    let Ok(arguments) = strings(request.argv, request.argc, &mut budget) else {
        return ERROR;
    };
    let Ok(environment) = strings(request.envp, request.envc, &mut budget) else {
        return ERROR;
    };
    let Ok(references_lease) = reserve_resource(Usage {
        kernel_bytes: ((arguments.len() + environment.len()) * core::mem::size_of::<&str>()) as u64,
        ..Usage::ZERO
    }) else {
        return ERROR;
    };
    let mut argv = Vec::new();
    let mut envp = Vec::new();
    if argv.try_reserve_exact(arguments.len()).is_err()
        || envp.try_reserve_exact(environment.len()).is_err()
    {
        return ERROR;
    }
    if capacity_extra::<&str>(&references_lease, argv.capacity(), arguments.len()).is_err()
        || capacity_extra::<&str>(&references_lease, envp.capacity(), environment.len()).is_err()
    {
        return ERROR;
    }
    argv.extend(arguments.iter().map(String::as_str));
    envp.extend(environment.iter().map(String::as_str));
    finish(&argv, &envp, &bindings)
}
