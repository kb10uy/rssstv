//! Embeds the Windows resources the application loads at runtime.
//!
//! `embed-resource` is a host-gated build dependency, so `cfg(target_os)` here
//! reads the same platform Cargo used to select it. Compiling for a non-Windows
//! target from a Windows host is handled by `embed-resource` itself, which
//! reports `NotWindows` and links nothing.

fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=assets/rssstv.rc");
        println!("cargo:rerun-if-changed=assets/icon.ico");

        // The resource is linked into every artifact rather than the binary
        // alone, so the tests that read the icon back can find it.
        embed_resource::compile_for_everything("assets/rssstv.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("could not compile the application resources");
    }
}
