//! The `drivers` app's `load <name.drv>` command (Phase 2 Step 7 of
//! `docs/nic_driver_design.md` §4.2-4.3): spawns a NIC driver as a
//! background process, without blocking on `process::wait`.
//!
//! Moved here from the shell (`user_programs/shell/src/load_driver.rs`,
//! issue #105) — `load` used to work only because it ran inside the
//! shell's own already-privileged process; `spawn_driver` below requires
//! `Capabilities::SPAWN_DRIVER`, delegated to `DRIVERS.BIN` by the shell at
//! `Exec` time (issue #107).
//!
//! Whether `file` is even worth attempting is answered by
//! `lib_driver::spawn::probe_driver`, a read-only kernel query over the
//! same binary-name-to-PCI-ID table `SpawnDriver` itself derives grants
//! from (`kernel/src/drivers/driver_db.rs`). An earlier version of this
//! file kept its own, hand-duplicated copy of that table (plus its own PCI
//! enumeration loop) purely to print a friendly error before spawning
//! anything — two independent sources of truth for the same mapping that
//! had to be kept in sync by hand. `probe_driver` asks the kernel directly
//! instead.

#[cfg(not(test))]
use lib_kaos::println;

/// Spawns `file` as a background driver process (no `process::wait` --
/// the driver runs independently and the REPL prompt returns immediately).
///
/// Resource grants (MMIO regions) are **not** computed here and `None` is
/// passed to `spawn_driver`: the kernel always derives the authoritative
/// grant itself from its own PCI enumeration
/// (`kernel::drivers::driver_db::derive_grants`, called from
/// `syscall_spawn_driver_impl`) precisely so an unprivileged caller can
/// never hand itself an arbitrary physical-memory grant by lying about it.
/// `None` means "accept the kernel-derived grant unconditionally", which is
/// exactly what every existing driver-spawn path in this codebase already
/// relies on.
#[cfg(not(test))]
pub fn load_driver(file: &str) {
    // Step 1: ask the kernel whether this is even worth attempting, and
    // print a clear, distinct error for each failure mode without spawning
    // anything.
    match lib_driver::spawn::probe_driver(file) {
        Err(_) => {
            println!("[drivers] Unknown driver '{}'.", file);
            return;
        }
        Ok(false) => {
            println!(
                "[drivers] Error: no matching PCI device found for '{}'.",
                file
            );
            return;
        }
        Ok(true) => {}
    }

    // Step 2: grants are derived kernel-side (see doc comment above); pass
    // None to accept them unconditionally.
    let caps = 1; // MMIO (1)

    // Step 3: spawn in the background -- no process::wait() call. `file` is
    // passed through as-is: both `driver_db::lookup_driver` and the FAT32
    // VFS lookup `SpawnDriver` triggers are already case-insensitive, so no
    // client-side canonicalization is needed.
    match lib_driver::spawn::spawn_driver(file, caps, None) {
        Ok(tid) => {
            println!("[drivers] Driver '{}' started as TID {}", file, tid);
        }
        Err(err) => {
            println!("[drivers] Failed to load '{}': {:?}", file, err);
        }
    }
}
