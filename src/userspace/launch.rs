//! Copy complete untrusted argv/envp records before loading or scheduling.
use alloc::{string::String, vec::Vec};
const ERROR: u64 = u64::MAX - 1;
fn word(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("fixed wire word"))
}
fn strings(pointer: u64, count: u64, budget: &mut usize) -> Result<Vec<String>, ()> {
    if count > rustos_process_abi::MAX_VECTOR as u64 {
        return Err(());
    }
    let mut records = Vec::new();
    records
        .try_reserve_exact(count as usize * 16)
        .map_err(|_| ())?;
    records.resize(count as usize * 16, 0);
    if !super::usercopy::from_user(pointer, &mut records) {
        return Err(());
    }
    let mut strings = Vec::new();
    strings.try_reserve_exact(count as usize).map_err(|_| ())?;
    for record in records.chunks_exact(16) {
        let pointer = word(&record[..8]);
        let length = word(&record[8..]);
        if length >= rustos_process_abi::MAX_STARTUP_BYTES as u64 {
            return Err(());
        }
        *budget = budget
            .checked_add(length as usize + 1)
            .filter(|&n| n <= rustos_process_abi::MAX_STARTUP_BYTES)
            .ok_or(())?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length as usize).map_err(|_| ())?;
        bytes.resize(length as usize, 0);
        if !super::usercopy::from_user(pointer, &mut bytes) || bytes.contains(&0) {
            return Err(());
        }
        strings.push(String::from_utf8(bytes).map_err(|_| ())?);
    }
    Ok(strings)
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
    let mut budget = 0;
    let Ok(arguments) = strings(fields[2], fields[3], &mut budget) else {
        return ERROR;
    };
    let Ok(environment) = strings(fields[4], fields[5], &mut budget) else {
        return ERROR;
    };
    let Some(path) = crate::fs::handles::path(path, crate::fs::handles::PathKind::Existing) else {
        return ERROR;
    };
    let mut argv = Vec::new();
    if argv.try_reserve_exact(arguments.len()).is_err() {
        return ERROR;
    }
    argv.extend(arguments.iter().map(String::as_str));
    let mut envp = Vec::new();
    if envp.try_reserve_exact(environment.len()).is_err() {
        return ERROR;
    }
    envp.extend(environment.iter().map(String::as_str));
    super::syscall::spawn_at(&path, fields[1] == 1, &argv, &envp)
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
    bound(request, |argv, envp, bindings| {
        super::syscall::spawn_at_bound(&path, argv, envp, bindings)
    })
}
/// Snapshot an entire mapped user image, then let the existing native loader
/// validate it and atomically publish its private frames/explicit endpoint moves.
pub(crate) fn spawn_image(image: u64, packet: u64) -> u64 {
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
    let mut binary = Vec::new();
    if binary
        .try_reserve_exact(request.image_length as usize)
        .is_err()
    {
        return ERROR;
    }
    binary.resize(request.image_length as usize, 0);
    if !super::usercopy::from_user(image, &mut binary) || !crate::scheduler::current_is_live() {
        return ERROR;
    }
    bound(request.launch, |argv, envp, bindings| {
        crate::scheduler::spawn_process_with_bindings(&binary, 0, argv, envp, bindings)
            .map(u64::from)
            .unwrap_or(ERROR)
    })
}
fn bound(
    request: rustos_ipc_abi::Spawn,
    finish: impl FnOnce(&[&str], &[&str], &[rustos_ipc_abi::Binding]) -> u64,
) -> u64 {
    use rustos_ipc_abi as abi;
    let mut records = [0; abi::MAX_BINDINGS * abi::BINDING_SIZE];
    let length = request.binding_count as usize * abi::BINDING_SIZE;
    if !super::usercopy::from_user(request.bindings, &mut records[..length]) {
        return ERROR;
    }
    let mut bindings = Vec::new();
    if bindings
        .try_reserve_exact(request.binding_count as usize)
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
    let mut argv = Vec::new();
    let mut envp = Vec::new();
    if argv.try_reserve_exact(arguments.len()).is_err()
        || envp.try_reserve_exact(environment.len()).is_err()
    {
        return ERROR;
    }
    argv.extend(arguments.iter().map(String::as_str));
    envp.extend(environment.iter().map(String::as_str));
    finish(&argv, &envp, &bindings)
}
