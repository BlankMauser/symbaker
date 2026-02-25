use symbaker::symbaker;

#[symbaker]
pub extern "C" fn dep_exported() -> i32 {
    7
}
