use symbaker::symbaker;

#[symbaker]
pub extern "C" fn auto_named() -> i32 {
    1
}

#[symbaker(prefix = "custom")]
pub extern "C" fn attr_named() -> i32 {
    2
}
