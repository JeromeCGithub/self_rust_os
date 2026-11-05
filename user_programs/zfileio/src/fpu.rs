// No Rust call sits between register setup and checking. XMM registers are
// caller-saved by the C ABI, so keep the syscall/yield loop wholly in assembly.
core::arch::global_asm!(
    r#"
.global cap_fp_probe
.type cap_fp_probe,@function
cap_fp_probe:
    sub rsp,16
    mov r8,rdi
    mov r9,rsi
    movq xmm0,rdi
    mov rax,rdi
    not rax
    movq xmm15,rax
    mov [rsp],rdi
    fld qword ptr [rsp]
    stmxcsr [rsp+8]
    mov r10,rdi
    and r10,3
    shl r10,13
    or r10,0x1f80
    mov [rsp+12],r10d
    ldmxcsr [rsp+12]
1:
    mov eax,102
    xor edi,edi
    xor esi,esi
    xor edx,edx
    int 0x80
    movq rax,xmm0
    cmp rax,r8
    jne 2f
    movq rax,xmm15
    mov rdx,r8
    not rdx
    cmp rax,rdx
    jne 2f
    fst qword ptr [rsp]
    cmp [rsp],r8
    jne 2f
    stmxcsr [rsp+12]
    cmp [rsp+12],r10d
    jne 2f
    dec r9
    jne 1b
    xor eax,eax
    jmp 3f
2:
    mov eax,1
3:
    fstp qword ptr [rsp]
    ldmxcsr [rsp+8]
    add rsp,16
    ret
.size cap_fp_probe,.-cap_fp_probe
"#
);
extern "C" {
    fn cap_fp_probe(value: u64, rounds: u64) -> u64;
}
pub fn run() {
    if userlib::args::arg(1) == Some("--fpu-child") {
        // SAFETY: Same bounded C-ABI probe, now in an independent address space.
        assert_eq!(unsafe { cap_fp_probe(0x4000000000000001, 2000) }, 0);
        userlib::sys::exit(0);
    }
    if userlib::args::arg(1) == Some("--invalid-simd") {
        // SAFETY: Deliberate user invalid opcode verifies process-only fault isolation.
        unsafe {
            core::arch::asm!("ud2", options(noreturn));
        }
    }
    let first = userlib::sys::spawn_with_args("/BIN/ZFILEIO.BIN", &["--fpu-child"]).unwrap();
    let second = userlib::sys::spawn_with_args("/BIN/ZFILEIO.BIN", &["--fpu-child"]).unwrap();
    let workers: [_; 8] = core::array::from_fn(|index| {
        userlib::thread::Builder::new()
            .spawn(move || {
                // SAFETY: Probe obeys the C ABI, accesses only its own bounded stack,
                // and intentionally uses x87/SSE2 state to verify the requested feature.
                unsafe { cap_fp_probe(0x3ff0000000000000 + index as u64, 1000) }
            })
            .unwrap()
    });
    for worker in workers {
        assert_eq!(
            worker.join().unwrap(),
            0,
            "floating-point/SIMD state leaked between threads"
        );
    }
    assert_eq!(userlib::sys::waitpid(first), Ok(0));
    assert_eq!(userlib::sys::waitpid(second), Ok(0));
    let invalid = userlib::sys::spawn_with_args("/BIN/ZFILEIO.BIN", &["--invalid-simd"]).unwrap();
    assert_eq!(userlib::sys::waitpid(invalid), Ok(6));
}
