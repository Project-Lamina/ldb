//! The one platform ldb debugs, and what it says on every other one.
//!
//! Three things in ldb are platform-bound and none of them has a second
//! implementation: [`crate::inferior`] drives the child with ptrace,
//! [`crate::object::elf`] is the only object reader, and [`crate::arch`]
//! describes x86_64 alone. A build for anything else compiles without those
//! modules and refuses at startup, because a clear refusal is worth more than
//! a wall of compiler errors from `cargo install`.

/// The target ldb debugs, spelled the way the refusal and the README spell it.
pub const SUPPORTED: &str = "Linux x86_64 ELF";

/// The cfg condition that gates every platform-bound module in the crate.
/// It is repeated as an attribute rather than hidden behind a build script so
/// that reading any one of those modules shows what it depends on.
pub const fn is_supported() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64"))
}

/// Why this build can debug nothing, or `None` on a build that can.
pub fn unsupported_reason() -> Option<String> {
    if is_supported() {
        return None;
    }
    Some(refusal_for(std::env::consts::OS, std::env::consts::ARCH))
}

/// Print the refusal and exit. Called from `main` on a build where
/// [`is_supported`] is false, before anything tries to open a file.
pub fn refuse() -> ! {
    let reason = unsupported_reason().unwrap_or_else(|| format!("ldb debugs {}.", SUPPORTED));
    eprintln!("ldb: {}", reason);
    std::process::exit(2)
}

/// The refusal text for an operating system and architecture, naming the parts
/// of ldb that platform would need. Split out from [`unsupported_reason`] so it
/// can be tested from the build that is supported.
pub fn refusal_for(os: &str, arch: &str) -> String {
    let missing = match (os, arch) {
        ("macos" | "ios", _) => {
            "ldb has no Mach-O reader, and it drives the inferior with ptrace rather than \
             mach exception ports"
        }
        ("windows", _) => {
            "ldb has no PE reader, and it drives the inferior with ptrace rather than \
             WaitForDebugEvent"
        }
        ("linux" | "android", _) => {
            "ldb's breakpoint is the x86_64 int3 and the registers it reads are x86_64"
        }
        (_, "x86_64") => "ldb drives the inferior with Linux ptrace",
        _ => "ldb drives the inferior with Linux ptrace, reads ELF, and knows x86_64 only",
    };
    format!(
        "ldb debugs {}. {} on {} is not supported yet: {}.",
        SUPPORTED,
        os_name(os),
        arch,
        missing
    )
}

fn os_name(os: &str) -> &str {
    match os {
        "macos" => "macOS",
        "ios" => "iOS",
        "windows" => "Windows",
        "linux" => "Linux",
        "android" => "Android",
        "freebsd" => "FreeBSD",
        "netbsd" => "NetBSD",
        "openbsd" => "OpenBSD",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_the_platform_and_what_is_missing() {
        let mac = refusal_for("macos", "x86_64");
        assert!(mac.contains("ldb debugs Linux x86_64 ELF"));
        assert!(mac.contains("macOS on x86_64 is not supported yet"));
        assert!(mac.contains("Mach-O"));
        assert!(mac.contains("mach exception ports"));

        let win = refusal_for("windows", "x86_64");
        assert!(win.contains("Windows on x86_64 is not supported yet"));
        assert!(win.contains("PE reader"));
        assert!(win.contains("WaitForDebugEvent"));
    }

    #[test]
    fn a_supported_os_on_another_architecture_refuses_on_the_architecture() {
        let text = refusal_for("linux", "aarch64");
        assert!(text.contains("Linux on aarch64 is not supported yet"));
        assert!(text.contains("int3"));
        assert!(!text.contains("ptrace"));
    }

    #[test]
    fn a_reason_is_given_exactly_when_the_build_is_unsupported() {
        assert_eq!(is_supported(), unsupported_reason().is_none());
    }
}
