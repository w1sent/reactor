//! Gives the Windows executable its icon (Explorer, the taskbar and Alt-Tab read it from the
//! `.exe`'s resources). Other platforms take theirs elsewhere: `chrome::window_options` for
//! the window itself, `icon.icns` for a macOS bundle, the `.desktop` file for Linux launchers.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/icon.ico");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=could not embed the Windows icon: {error}");
        }
    }
}
