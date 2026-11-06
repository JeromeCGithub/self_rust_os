//! Native C adapter allocations are private and independent of other allocators.
use alloc::alloc::{alloc, dealloc, Layout};
use core::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};
const HEADER: usize = 16;
static BLOCKS: AtomicUsize = AtomicUsize::new(0);
pub fn live_blocks() -> usize {
    BLOCKS.load(Ordering::Relaxed)
}
#[no_mangle]
extern "C" fn rjs_allocate(size: usize) -> *mut c_void {
    let Some(total) = size.checked_add(HEADER) else {
        return ptr::null_mut();
    };
    let Ok(layout) = Layout::from_size_align(total, 16) else {
        return ptr::null_mut();
    };
    // SAFETY: Layout validates size/alignment. A successful allocation is uniquely
    // owned; its 16-byte header and payload are disjoint and in bounds. Metadata
    // initializes before publication; atomic counts are diagnostic, not ownership.
    unsafe {
        let raw = alloc(layout);
        if raw.is_null() {
            return ptr::null_mut();
        }
        raw.cast::<usize>().write(size);
        BLOCKS.fetch_add(1, Ordering::Relaxed);
        raw.add(HEADER).cast()
    }
}
#[no_mangle]
unsafe extern "C" fn rjs_release(pointer: *mut c_void) {
    // SAFETY: The private C malloc ABI passes only a still-live payload returned
    // by rjs_allocate, once. Its header is aligned/initialized and remains owned;
    // reconstructing the exact original Layout preserves allocator ownership.
    unsafe {
        let raw = pointer.cast::<u8>().sub(HEADER);
        let size = raw.cast::<usize>().read();
        let layout = Layout::from_size_align(size + HEADER, 16).expect("owned C allocation layout");
        dealloc(raw, layout);
        BLOCKS.fetch_sub(1, Ordering::Relaxed);
    }
}
#[no_mangle]
unsafe extern "C" fn rjs_allocation_size(pointer: *const c_void) -> usize {
    // SAFETY: The C allocator calls this only for its live payload pointer. The
    // preceding aligned immutable header stays valid until the corresponding free.
    unsafe { pointer.cast::<u8>().sub(HEADER).cast::<usize>().read() }
}
#[no_mangle]
extern "C" fn rjs_acos(x: f64) -> f64 {
    libm::acos(x)
}
#[no_mangle]
extern "C" fn rjs_acosh(x: f64) -> f64 {
    libm::acosh(x)
}
#[no_mangle]
extern "C" fn rjs_asin(x: f64) -> f64 {
    libm::asin(x)
}
#[no_mangle]
extern "C" fn rjs_asinh(x: f64) -> f64 {
    libm::asinh(x)
}
#[no_mangle]
extern "C" fn rjs_atan(x: f64) -> f64 {
    libm::atan(x)
}
#[no_mangle]
extern "C" fn rjs_atanh(x: f64) -> f64 {
    libm::atanh(x)
}
#[no_mangle]
extern "C" fn rjs_cbrt(x: f64) -> f64 {
    libm::cbrt(x)
}
#[no_mangle]
extern "C" fn rjs_ceil(x: f64) -> f64 {
    libm::ceil(x)
}
#[no_mangle]
extern "C" fn rjs_cos(x: f64) -> f64 {
    libm::cos(x)
}
#[no_mangle]
extern "C" fn rjs_cosh(x: f64) -> f64 {
    libm::cosh(x)
}
#[no_mangle]
extern "C" fn rjs_exp(x: f64) -> f64 {
    libm::exp(x)
}
#[no_mangle]
extern "C" fn rjs_expm1(x: f64) -> f64 {
    libm::expm1(x)
}
#[no_mangle]
extern "C" fn rjs_fabs(x: f64) -> f64 {
    libm::fabs(x)
}
#[no_mangle]
extern "C" fn rjs_floor(x: f64) -> f64 {
    libm::floor(x)
}
#[no_mangle]
extern "C" fn rjs_log(x: f64) -> f64 {
    libm::log(x)
}
#[no_mangle]
extern "C" fn rjs_log10(x: f64) -> f64 {
    libm::log10(x)
}
#[no_mangle]
extern "C" fn rjs_log1p(x: f64) -> f64 {
    libm::log1p(x)
}
#[no_mangle]
extern "C" fn rjs_log2(x: f64) -> f64 {
    libm::log2(x)
}
#[no_mangle]
extern "C" fn rjs_round(x: f64) -> f64 {
    libm::round(x)
}
#[no_mangle]
extern "C" fn rjs_sin(x: f64) -> f64 {
    libm::sin(x)
}
#[no_mangle]
extern "C" fn rjs_sinh(x: f64) -> f64 {
    libm::sinh(x)
}
#[no_mangle]
extern "C" fn rjs_sqrt(x: f64) -> f64 {
    libm::sqrt(x)
}
#[no_mangle]
extern "C" fn rjs_tan(x: f64) -> f64 {
    libm::tan(x)
}
#[no_mangle]
extern "C" fn rjs_tanh(x: f64) -> f64 {
    libm::tanh(x)
}
#[no_mangle]
extern "C" fn rjs_trunc(x: f64) -> f64 {
    libm::trunc(x)
}
#[no_mangle]
extern "C" fn rjs_atan2(x: f64, y: f64) -> f64 {
    libm::atan2(x, y)
}
#[no_mangle]
extern "C" fn rjs_fmax(x: f64, y: f64) -> f64 {
    libm::fmax(x, y)
}
#[no_mangle]
extern "C" fn rjs_fmin(x: f64, y: f64) -> f64 {
    libm::fmin(x, y)
}
#[no_mangle]
extern "C" fn rjs_fmod(x: f64, y: f64) -> f64 {
    libm::fmod(x, y)
}
#[no_mangle]
extern "C" fn rjs_hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}
#[no_mangle]
extern "C" fn rjs_pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}
#[no_mangle]
extern "C" fn rjs_lrint(x: f64) -> i64 {
    let n = libm::rint(x);
    if !n.is_finite() || n >= 9223372036854775808.0 || n < -9223372036854775808.0 {
        i64::MIN
    } else {
        n as i64
    }
}
#[no_mangle]
unsafe extern "C" fn rjs_modf(x: f64, integer: *mut f64) -> f64 {
    let (fraction, whole) = libm::modf(x);
    // SAFETY: C modf's caller provides a live aligned exclusive output double;
    // QuickJS supplies stack storage and retains no Rust reference across the call.
    unsafe { integer.write(whole) };
    fraction
}
