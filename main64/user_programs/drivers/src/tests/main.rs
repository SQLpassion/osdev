//! Host unit tests for the `drivers` REPL's pure command-parsing logic
//! (`parse_command`). Never touches a syscall, mirroring this project's
//! convention (see `shell::load_driver`'s `resolve_driver_filename` tests)
//! of keeping pure, I/O-free decision logic separately testable from the
//! syscall-touching code around it.

use super::{parse_command, Command};

#[test]
fn test_parse_command_empty_input_is_empty() {
    assert_eq!(parse_command(""), Command::Empty);
}

#[test]
fn test_parse_command_whitespace_only_input_is_empty() {
    assert_eq!(parse_command("   "), Command::Empty);
    assert_eq!(parse_command("\t  \t"), Command::Empty);
}

#[test]
fn test_parse_command_help_with_no_arguments() {
    assert_eq!(parse_command("help"), Command::Help);
}

#[test]
fn test_parse_command_help_with_extra_arguments_is_still_help() {
    // Extra words after a recognized command name are ignored, not an
    // error — matches the shell's own `help` (and every other zero-arg
    // shell command), which never rejects trailing garbage.
    assert_eq!(parse_command("help me please"), Command::Help);
}

#[test]
fn test_parse_command_list_with_no_arguments() {
    assert_eq!(parse_command("list"), Command::List);
}

#[test]
fn test_parse_command_list_with_extra_arguments_is_still_list() {
    assert_eq!(parse_command("list drivers now"), Command::List);
}

#[test]
fn test_parse_command_unknown_command() {
    assert_eq!(parse_command("frobnicate"), Command::Unknown("frobnicate"));
}

#[test]
fn test_parse_command_is_case_sensitive() {
    // Dispatch is case-sensitive, matching the shell's own `execute_command`
    // (which never normalizes case for its command word either).
    assert_eq!(parse_command("HELP"), Command::Unknown("HELP"));
    assert_eq!(parse_command("List"), Command::Unknown("List"));
}

#[test]
fn test_parse_command_load_and_unload_are_unknown_before_their_own_phases() {
    // `load`/`unload` are implemented in later phases (issues #105/#106);
    // until then they must fall through to `Unknown`, not panic or be
    // silently accepted.
    assert_eq!(parse_command("load rtl8139.drv"), Command::Unknown("load"));
    assert_eq!(
        parse_command("unload nic:rtl8139"),
        Command::Unknown("unload")
    );
}

#[test]
fn test_parse_command_ignores_leading_and_trailing_whitespace() {
    assert_eq!(parse_command("  help  "), Command::Help);
    assert_eq!(parse_command("  list  "), Command::List);
}
