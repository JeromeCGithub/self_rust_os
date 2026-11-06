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
