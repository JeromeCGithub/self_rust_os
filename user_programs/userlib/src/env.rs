//! Process-local environment with snapshots for inherited spawning.
use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use core::sync::atomic::{AtomicPtr, Ordering};
static STARTUP: AtomicPtr<*const u8> = AtomicPtr::new(core::ptr::null_mut());
static VARIABLES: spin::Mutex<Option<BTreeMap<String, String>>> = spin::Mutex::new(None);
/// Install immutable kernel-owned private startup strings before application code.
/// # Safety
/// Pointer names an aligned null-terminated table of at most MAX_VECTOR valid
/// immutable UTF-8 C strings in process-owned stack memory for process lifetime.
pub(crate) unsafe fn initialize(pointer: *const *const u8) {
    STARTUP.store(pointer as *mut *const u8, Ordering::Release);
}
fn initialized(map: &mut Option<BTreeMap<String, String>>) -> &mut BTreeMap<String, String> {
    if map.is_none() {
        let mut variables = BTreeMap::new();
        let pointer = STARTUP.load(Ordering::Acquire);
        if !pointer.is_null() {
            for index in 0..rustos_process_abi::MAX_VECTOR {
                // SAFETY: Startup initialization precedes every reader. This
                // bounded table and its strings remain private and immutable;
                // no safe SDK operation unmaps or writes the initial stack.
                let string = unsafe {
                    let data = pointer.add(index).read();
                    if data.is_null() {
                        break;
                    }
                    let mut length = 0;
                    while length < rustos_process_abi::MAX_STARTUP_BYTES
                        && data.add(length).read() != 0
                    {
                        length += 1;
                    }
                    core::str::from_utf8(core::slice::from_raw_parts(data, length))
                        .expect("validated environment UTF-8")
                };
                if let Some((key, value)) = string.split_once('=') {
                    variables.insert(key.to_string(), value.to_string());
                }
            }
        }
        *map = Some(variables);
    }
    map.as_mut().expect("environment initialized")
}
/// Read a process-local variable as an owned value, safe across concurrent edits.
pub fn var(key: &str) -> Option<String> {
    initialized(&mut VARIABLES.lock()).get(key).cloned()
}
/// Set a variable for this process and future inherited spawns.
pub fn set_var(key: &str, value: &str) -> Result<(), ()> {
    if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
        return Err(());
    }
    initialized(&mut VARIABLES.lock()).insert(key.to_string(), value.to_string());
    Ok(())
}
/// Remove a variable without changing another process's environment.
pub fn remove_var(key: &str) {
    initialized(&mut VARIABLES.lock()).remove(key);
}
/// Snapshot all variables without retaining a lock across spawning or callbacks.
pub fn vars() -> Vec<(String, String)> {
    initialized(&mut VARIABLES.lock())
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
/// Encode an owned environment snapshot for the launch boundary.
pub(crate) fn snapshot() -> Vec<String> {
    vars()
        .into_iter()
        .map(|(key, value)| alloc::format!("{}={}", key, value))
        .collect()
}
