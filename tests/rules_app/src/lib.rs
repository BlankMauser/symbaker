use symbaker::symbaker_module;

#[symbaker_module(
    include_regex = "^keep_,special$",
    exclude_glob = "*skip*",
    template = "{prefix}{sep}{module}_{name}{suffix}",
    suffix = "_x"
)]
mod exports {
    pub extern "C" fn keep_one() -> i32 {
        1
    }

    pub extern "C" fn keep_skip() -> i32 {
        2
    }

    pub extern "C" fn special() -> i32 {
        3
    }

    pub extern "C" fn other() -> i32 {
        4
    }
}
