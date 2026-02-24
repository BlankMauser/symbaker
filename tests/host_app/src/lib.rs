#[no_mangle]
pub extern "C" fn host_calls_dep() -> i32 {
    dep_lib::dep_exported()
}
