// The icon of the program, which Explorer shows and a link takes from its target. The file is the
// compiled resource the icon generator writes; the linker of MSVC takes it as it is.

use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
        let res = manifest
            .join("..")
            .join("..")
            .join("assets")
            .join("pult.res");
        println!("cargo:rerun-if-changed={}", res.display());
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
    println!("cargo:rerun-if-changed=build.rs");
}
