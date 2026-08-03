#[verus_verify]
impl Demo {
    #[verus_spec(result => ensures result > 0)]
    fn checked() -> u8 {
        1
    }

    #[verifier::external_body]
    #[verifier::external_body]
    unsafe fn trusted() {}
}

#[verus_verify(external)]
fn excluded() {}

#[cfg(target_arch = "x86_64")]
#[verus_verify]
fn selected_for_x86() {
    atomic_with_ghost!(value => {
        assume(true);
        admit();
    });
}

#[cfg(not(target_arch = "x86_64"))]
fn inactive() {}
