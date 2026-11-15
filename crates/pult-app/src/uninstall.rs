//! `pult-app --uninstall`: the program removes the install it is part of, in its own process.
//!
//! This is what the entry in Settings > Apps runs. The work is `appkit-uninstall`'s (the kit's
//! crate that pult, streamtap and the other apps share): it takes away what `scripts/install.ps1`
//! made, with the checks of `scripts/uninstall.ps1` (which stays, for the terminal): the start
//! with Windows, the Start menu entry and the desktop shortcut when they lead into this install,
//! the files of a release, and last the entry in Settings > Apps. It asks first, in a window; no
//! shell and no other process is started. The program cannot delete its own file and moves it to
//! the folder for temporary files instead.
//!
//! Here is only what is pult's own: [`PRODUCT`].

pub use appkit_uninstall::{Product, run};

/// pult: the window, the command line, and the start with Windows.
pub const PRODUCT: Product = Product {
    name: "pult",
    program: "pult-app.exe",
    program_dir: "",
    link: "pult.lnk",
    programs: &["pult.exe", "pult-app.exe"],
    files: &[
        "pult.exe",
        "pult-app.exe",
        "uninstall.ps1",
        "example-profile.toml",
        "LICENSE",
        "README.md",
        "VERSION",
    ],
    folders: &[],
    run_value: Some("pult"),
    env: "PULT",
    close: "Quit in the tray menu",
    kept: r"your profiles and %APPDATA%\pult (settings, logs)",
    first: None,
};

#[cfg(test)]
mod tests {
    use super::PRODUCT;

    #[test]
    fn the_product_fits_together() {
        assert_eq!(PRODUCT.check(), Ok(()));
    }
}
