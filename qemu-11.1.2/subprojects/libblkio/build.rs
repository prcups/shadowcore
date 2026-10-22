// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=src/error-msg.c");
    cc::Build::new()
        .flag("-std=c11")
        .opt_level(2)
        .file("src/error-msg.c")
        .compile("error-msg");

    println!(
        "cargo:rustc-cdylib-link-arg=-Wl,-soname=libblkio.so.{}",
        env::var("CARGO_PKG_VERSION_MAJOR").unwrap()
    );
}
