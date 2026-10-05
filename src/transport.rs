//! Getting FGL bytes from here to a Boca printer.
//!
//! Four routes, covering the two connections these printers ship with:
//!
//! | | |
//! |---|---|
//! | [`Kind::Ethernet`] | Raw TCP to port 9100. Every Boca ethernet and WiFi interface listens there; nothing is wrapped around the FGL. |
//! | [`Kind::Device`] | Write straight at a device node. This is the USB path when the printer's USB mode is serial (`<usbs>`), which appears as `/dev/cu.usbmodem*` on macOS and `/dev/ttyACM*` on Linux. It is also the Linux USB-printer path, `/dev/usb/lp0`. |
//! | [`Kind::Spooler`] | Hand the job to the system print queue as raw bytes. This is the USB path when the printer is left in its default printer mode (`<usbp>`) and installed as a CUPS queue. |
//! | [`Kind::File`] | Write a `.fgl` file. Test without burning stock. |
//!
//! [`send`] blocks, so the app runs it on a background thread.

use std::io::Write as _;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::fgl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ethernet,
    Device,
    Spooler,
    File,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Ethernet, Kind::Device, Kind::Spooler, Kind::File];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ethernet => "Ethernet",
            Kind::Device => "USB device",
            Kind::Spooler => "Print queue",
            Kind::File => "Save .fgl",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Kind::Ethernet => "Raw TCP to port 9100.",
            Kind::Device => {
                "USB in serial mode. /dev/cu.usbmodem* on macOS, /dev/ttyACM0 or /dev/usb/lp0 on Linux."
            }
            Kind::Spooler => "USB in printer mode. Pipes through lp -o raw to a CUPS queue.",
            Kind::File => "Write the job to disk without printing.",
        }
    }

    /// lp(1) is a CUPS thing. Windows has no equivalent to shell out to
    /// portably, so that platform gets the other three.
    pub fn supported_here(self) -> bool {
        match self {
            Kind::Spooler => !cfg!(target_os = "windows"),
            _ => true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Target {
    pub kind: Kind,
    /// Hostname or IP for [`Kind::Ethernet`].
    pub host: String,
    pub port: u16,
    /// Device node for [`Kind::Device`].
    pub device: String,
    /// Queue name for [`Kind::Spooler`].
    pub queue: String,
    /// Output path for [`Kind::File`].
    pub path: String,
    /// Seconds to wait on a TCP connect before giving up.
    pub connect_timeout_s: u8,
}

impl Default for Target {
    fn default() -> Self {
        Self {
            kind: Kind::File,
            host: "192.168.1.100".into(),
            port: fgl::RAW_TCP_PORT,
            device: default_device().into(),
            queue: "Boca".into(),
            path: "ticket.fgl".into(),
            connect_timeout_s: 5,
        }
    }
}

fn default_device() -> &'static str {
    if cfg!(target_os = "macos") {
        "/dev/cu.usbmodem1101"
    } else if cfg!(target_os = "windows") {
        r"\\.\COM3"
    } else {
        "/dev/usb/lp0"
    }
}

impl Target {
    /// Where a job would go, for the print button.
    pub fn describe(&self) -> String {
        match self.kind {
            Kind::Ethernet => format!("{}:{}", self.host, self.port),
            Kind::Device => self.device.clone(),
            Kind::Spooler => self.queue.clone(),
            Kind::File => self.path.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Outcome {
    pub ok: bool,
    pub message: String,
}

fn ok(message: String) -> Outcome {
    Outcome {
        ok: true,
        message,
    }
}

fn fail(message: String) -> Outcome {
    Outcome {
        ok: false,
        message,
    }
}

/// Push `payload` at `target`.
///
/// Never returns an error: every failure becomes a message the UI can show,
/// because "printer is unplugged" is a normal Tuesday and not something to
/// unwind over.
pub fn send(target: &Target, payload: &[u8]) -> Outcome {
    match target.kind {
        Kind::File => send_file(target, payload),
        Kind::Device => send_device(target, payload),
        Kind::Ethernet => send_ethernet(target, payload),
        Kind::Spooler => send_spooler(target, payload),
    }
}

fn send_file(target: &Target, payload: &[u8]) -> Outcome {
    if target.path.is_empty() {
        return fail("no output path".into());
    }
    match std::fs::write(&target.path, payload) {
        Ok(()) => ok(format!("Wrote {} bytes to {}", payload.len(), target.path)),
        Err(e) => fail(format!("Could not write {}: {e}", target.path)),
    }
}

fn send_device(target: &Target, payload: &[u8]) -> Outcome {
    if target.device.is_empty() {
        return fail("no device path".into());
    }
    // Opened write-only rather than created: this is a device node that already
    // exists, and truncating it would be wrong.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(false)
        .open(&target.device);

    let mut file = match file {
        Ok(f) => f,
        Err(e) => return fail(format!("Could not open {}: {e}", target.device)),
    };
    if let Err(e) = file.write_all(payload) {
        return fail(format!("Write to {} failed: {e}", target.device));
    }
    if let Err(e) = file.flush() {
        return fail(format!("Flush to {} failed: {e}", target.device));
    }
    ok(format!("Sent {} bytes to {}", payload.len(), target.device))
}

fn send_ethernet(target: &Target, payload: &[u8]) -> Outcome {
    if target.host.is_empty() {
        return fail("no printer address".into());
    }

    let addrs = match (target.host.as_str(), target.port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(e) => return fail(format!("Cannot resolve {}: {e}", target.host)),
    };
    let Some(addr) = addrs.first() else {
        return fail(format!("{} resolved to nothing", target.host));
    };

    let timeout = Duration::from_secs(target.connect_timeout_s.max(1) as u64);
    let mut stream = match TcpStream::connect_timeout(addr, timeout) {
        Ok(s) => s,
        Err(e) => {
            return fail(format!(
                "Cannot reach {}:{}: {e}",
                target.host, target.port
            ));
        }
    };
    let _ = stream.set_write_timeout(Some(timeout));

    if let Err(e) = stream.write_all(payload) {
        return fail(format!("Send to {} failed: {e}", target.host));
    }
    if let Err(e) = stream.flush() {
        return fail(format!("Send to {} failed: {e}", target.host));
    }
    ok(format!(
        "Sent {} bytes to {}:{}",
        payload.len(),
        target.host,
        target.port
    ))
}

fn send_spooler(target: &Target, payload: &[u8]) -> Outcome {
    if !Kind::Spooler.supported_here() {
        return fail("Print queues need lp(1); use a device path on this platform".into());
    }
    if target.queue.is_empty() {
        return fail("no queue name".into());
    }

    // -o raw stops CUPS filtering the stream; FGL must arrive byte for byte.
    let child = std::process::Command::new("lp")
        .args(["-d", &target.queue, "-o", "raw"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => return fail(format!("Could not run lp: {e}")),
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(payload) {
            let _ = child.wait();
            return fail(format!("lp rejected the job: {e}"));
        }
        // Dropping stdin closes the pipe, which lp needs before it will finish.
    }

    match child.wait() {
        Ok(status) if status.success() => ok(format!(
            "Queued {} bytes on {}",
            payload.len(),
            target.queue
        )),
        Ok(status) => fail(format!(
            "lp exited {} — is \"{}\" a real queue?",
            status.code().unwrap_or(-1),
            target.queue
        )),
        Err(e) => fail(format!("lp did not finish: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let t = Target::default();
        assert_eq!(t.host, "192.168.1.100");
        assert_eq!(t.port, 9100);
        assert_eq!(t.path, "ticket.fgl");
    }

    #[test]
    fn describe_reports_the_destination_for_each_kind() {
        let mut t = Target {
            kind: Kind::Ethernet,
            ..Default::default()
        };
        assert_eq!(t.describe(), "192.168.1.100:9100");

        t.kind = Kind::File;
        assert_eq!(t.describe(), "ticket.fgl");

        t.kind = Kind::Spooler;
        assert_eq!(t.describe(), "Boca");
    }

    #[test]
    fn file_transport_writes_the_payload_where_it_was_told_to() {
        let dir = std::env::temp_dir().join(format!("ticketsmith-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("job.fgl");

        let target = Target {
            kind: Kind::File,
            path: path.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let payload = b"<NR><F3><RC10,10>HELLO<p>";
        let outcome = send(&target, payload);
        assert!(outcome.ok, "{}", outcome.message);

        assert_eq!(std::fs::read(&path).unwrap(), payload);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_output_path_fails_without_panicking() {
        let target = Target {
            kind: Kind::File,
            path: "/definitely/not/a/directory/job.fgl".into(),
            ..Default::default()
        };
        let outcome = send(&target, b"x");
        assert!(!outcome.ok);
        assert!(!outcome.message.is_empty());
    }

    #[test]
    fn a_missing_device_fails_without_panicking() {
        let target = Target {
            kind: Kind::Device,
            device: "/dev/definitely-not-a-printer".into(),
            ..Default::default()
        };
        let outcome = send(&target, b"x");
        assert!(!outcome.ok);
    }

    #[test]
    fn an_unroutable_address_reports_instead_of_hanging() {
        // 203.0.113.0/24 is TEST-NET-3, reserved for documentation and
        // guaranteed not to be routed anywhere.
        let target = Target {
            kind: Kind::Ethernet,
            host: "203.0.113.1".into(),
            port: 9100,
            connect_timeout_s: 1,
            ..Default::default()
        };
        let started = std::time::Instant::now();
        let outcome = send(&target, b"x");
        assert!(!outcome.ok);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "connect timeout was not honoured"
        );
    }

    #[test]
    fn spooler_is_unavailable_on_windows_only() {
        assert_eq!(Kind::Spooler.supported_here(), !cfg!(target_os = "windows"));
        assert!(Kind::Ethernet.supported_here());
    }
}
