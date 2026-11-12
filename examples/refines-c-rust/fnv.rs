#![no_std]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
#[no_mangle]
pub unsafe extern "C" fn fnv1a(p: *const u8, n: u32) -> u32 {
    let mut h: u32 = 2166136261;
    let mut i = 0u32;
    while i < n {
        h ^= *p.add(i as usize) as u32;
        h = h.wrapping_mul(16777619);
        i += 1;
    }
    h
}
