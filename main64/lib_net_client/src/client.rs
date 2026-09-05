use lib_driver::drv;
use lib_driver::{SysError, UserDriverStatus};

/// Ring-3 handle to a running background NIC driver, resolved by name
/// (e.g. `"nic:rtl8139"`, `"nic:intel_nic"`) through the kernel
/// `DriverRegistry`.
pub struct NicClient {
    driver_id: u64,
}

impl NicClient {
    /// Resolves `name` via `DrvLookup`. Fails with
    /// `SysError::InvalidArgument` if no driver is currently registered
    /// under that name (e.g. the shell hasn't run `load <name>.drv` yet).
    pub fn open(name: &str) -> Result<Self, SysError> {
        let driver_id = drv::drv_lookup(name.as_bytes())?;
        Ok(Self { driver_id })
    }

    /// Sends a raw Ethernet frame to the driver.
    ///
    /// Fire-and-forget: per the `NetSend` role-based direction rule
    /// (`kernel/src/syscall/dispatch/driver.rs`), this always lands in the
    /// driver's TX ring (App → Driver), since the calling task is never the
    /// driver itself.
    pub fn send(&self, frame: &[u8]) -> Result<(), SysError> {
        drv::net_send(self.driver_id, frame)
    }

    /// Receives a frame from the driver's RX ring (Driver → App).
    ///
    /// `timeout_ms == 0` polls once and returns `SysError::Timeout`
    /// immediately if nothing is queued; a non-zero value blocks up to that
    /// many milliseconds. A frame larger than `buf.len()` is truncated.
    pub fn recv(&self, buf: &mut [u8], timeout_ms: u64) -> Result<usize, SysError> {
        drv::net_recv(self.driver_id, buf, timeout_ms)
    }

    /// Reads the driver's last-published status snapshot (MAC/IP/subnet/
    /// gateway/DNS, RX/TX counters, ARP table).
    pub fn query_status(&self) -> Result<UserDriverStatus, SysError> {
        drv::query_status(self.driver_id)
    }
}
