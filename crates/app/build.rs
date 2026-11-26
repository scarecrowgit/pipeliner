//! Bakes the runtime library directories from `shell.nix` into the binary's RUNPATH.
//! GPUI dlopen()s Wayland, Vulkan and X11 libraries at runtime; on NixOS they are only
//! found through LD_LIBRARY_PATH inside nix-shell, so without this the release binary
//! panics with `NoWaylandLib` when launched from a normal terminal or desktop entry.

fn main() {
    println!("cargo:rerun-if-env-changed=PIPELINER_RUNTIME_LIBS");
    if let Ok(dirs) = std::env::var("PIPELINER_RUNTIME_LIBS") {
        let dirs: Vec<&str> = dirs.split(':').filter(|d| !d.is_empty()).collect();
        if !dirs.is_empty() {
            println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{}", dirs.join(":"));
        }
    }
}
