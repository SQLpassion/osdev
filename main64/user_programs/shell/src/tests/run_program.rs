//! Host unit tests for `requested_capabilities_for`, the shell's policy
//! decision of which capabilities to delegate to a child it execs (issue
//! #107). Never touches a syscall — the underlying delegation *mechanism*
//! is already covered kernel-side by `resolve_delegated_capabilities`'s own
//! tests; this only proves the shell asks for the right thing for the
//! right binary.

use super::requested_capabilities_for;

const SPAWN_DRIVER: u64 = lib_kaos::process::capabilities::SPAWN_DRIVER;
const UNLOAD_DRIVER: u64 = lib_kaos::process::capabilities::UNLOAD_DRIVER;

#[test]
fn test_drivers_bin_gets_exactly_the_driver_management_capabilities() {
    assert_eq!(
        requested_capabilities_for("DRIVERS.BIN"),
        SPAWN_DRIVER | UNLOAD_DRIVER
    );
}

#[test]
fn test_drivers_bin_match_is_case_insensitive() {
    // Filesystem lookups elsewhere in this shell (e.g. the `.bin`/`.BIN`
    // direct-execution shortcut) are already case-insensitive; this must be
    // too, or a user typing "drivers.bin" would silently get an
    // unprivileged, non-functional copy of the app.
    assert_eq!(
        requested_capabilities_for("drivers.bin"),
        SPAWN_DRIVER | UNLOAD_DRIVER
    );
    assert_eq!(
        requested_capabilities_for("Drivers.Bin"),
        SPAWN_DRIVER | UNLOAD_DRIVER
    );
}

#[test]
fn test_every_other_binary_gets_zero_capabilities() {
    // Regression guard: no other program — including ones that sound
    // related, or that share a prefix — must ever be granted these
    // capabilities. Delegation is a narrow, exact-name allowlist, not a
    // pattern match.
    for name in [
        "TUI.BIN",
        "KBASIC.BIN",
        "SHELL.BIN",
        "HELLO.BIN",
        "DRIVERS2.BIN",
        "NOT_DRIVERS.BIN",
        "DRIVER.BIN",
        "",
    ] {
        assert_eq!(
            requested_capabilities_for(name),
            0,
            "'{}' must not be granted any delegated capabilities",
            name
        );
    }
}
