//! Slice 2: panel transport.
//!
//! Trait shape 1:1 with `telenot-sim` (`read_chunk`/`write_frame`/`is_live`) so the
//! poll loop can share the same code as the host. Backends:
//!
//! - [`UartTransport`]: real panel over RS232 converter, **9600 8N1** (see
//!   `docs/HARDWARE.md`; current build: MOD-RS232 on UEXT → GPIO4/36, pins chosen
//!   in `main.rs`). UART1 — UART0 stays the console.
//! - [`TcpTransport`]: RS232-over-TCP (USR-TCP232 at the panel or `telenot-sim` as
//!   bench mock).
//! - [`SwitchableTransport`]: combines both and switches live (setup step "connection",
//!   without a reboot).

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::TickType;
use esp_idf_svc::hal::gpio::{AnyInputPin, AnyOutputPin};
use esp_idf_svc::hal::uart::config::{Config as UartConfig, DataBits, StopBits};
use esp_idf_svc::hal::uart::{UartDriver, UART1};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::sys::EspError;

/// Read timeout per poll round: short enough for responsive reaction, long enough that the
/// CPU does not spin hot. No byte → `Ok(Some(empty))` (the loop only ticks).
const READ_TIMEOUT_MS: u64 = 50;

/// Connection target from the setup step "connection" (persisted in NVS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelTarget {
    /// Internal RS232 (UART1 via the level shifter).
    Internal,
    /// RS232-over-TCP (USR-TCP232 or bench mock), `ip`/`port`.
    Tcp(String, u16),
}

pub trait PanelTransport {
    fn read_chunk(&mut self) -> io::Result<Option<Vec<u8>>>;
    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// Does the transport run indefinitely (TCP/UART) or does it end (replay)?
    /// (Currently unused — will matter for the replay transport.)
    #[allow(dead_code)]
    fn is_live(&self) -> bool;
    /// Switches the connection target (setup step "connection").
    /// `false` = backend cannot serve this target.
    fn set_target(&mut self, _target: &PanelTarget) -> bool {
        false
    }
    /// Changes the serial baud rate live (GMS plus: 115200; complex/GMS lite: 9600).
    /// `false` = backend has no own serial line (TCP: the USR-TCP232 sets its own baud).
    fn set_baud(&mut self, _baud: u32) -> bool {
        false
    }
}

fn esp_io(e: EspError) -> io::Error {
    io::Error::other(e.to_string())
}

/// RS232 to the Telenot panel via the level shifter on UART1. Pins are chosen in `main.rs`
/// (build variant, see `docs/HARDWARE.md`).
pub struct UartTransport<'d> {
    drv: UartDriver<'d>,
}

impl<'d> UartTransport<'d> {
    pub fn new(
        uart: UART1<'d>,
        tx: AnyOutputPin<'d>,
        rx: AnyInputPin<'d>,
        baud: u32,
    ) -> Result<Self, EspError> {
        let cfg = UartConfig::new()
            .baudrate(Hertz(baud))
            .data_bits(DataBits::DataBits8)
            .parity_none()
            .stop_bits(StopBits::STOP1);
        let drv = UartDriver::new(
            uart,
            tx,
            rx,
            Option::<AnyInputPin>::None,
            Option::<AnyOutputPin>::None,
            &cfg,
        )?;
        Ok(Self { drv })
    }
}

impl PanelTransport for UartTransport<'_> {
    fn read_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut buf = [0u8; 256];
        match self
            .drv
            .read(&mut buf, TickType::new_millis(READ_TIMEOUT_MS).ticks())
        {
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            // Timeout = no byte in this window → empty (loop only ticks), NOT an error.
            Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_TIMEOUT => Ok(Some(Vec::new())),
            Err(e) => Err(esp_io(e)),
        }
    }

    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut sent = 0;
        while sent < bytes.len() {
            sent += self.drv.write(&bytes[sent..]).map_err(esp_io)?;
        }
        Ok(())
    }

    fn is_live(&self) -> bool {
        true
    }

    fn set_baud(&mut self, baud: u32) -> bool {
        match self.drv.change_baudrate(Hertz(baud)) {
            Ok(_) => {
                log::info!("Panel-UART: Baudrate → {baud} (No-Reboot)");
                true
            }
            Err(e) => {
                log::warn!("Panel-UART: Baudrate {baud} fehlgeschlagen ({e})");
                false
            }
        }
    }
}

/// Bench transport: TCP client to the host mock (RS232-over-TCP), counterpart to `telenot-sim`.
pub struct TcpTransport {
    stream: TcpStream,
}

impl PanelTransport for TcpTransport {
    fn read_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut buf = [0u8; 256];
        match self.stream.read(&mut buf) {
            Ok(0) => Ok(None), // remote end closed
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(Some(Vec::new())),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(Some(Vec::new())),
            Err(e) => Err(e),
        }
    }

    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stream.write_all(bytes)
    }

    fn is_live(&self) -> bool {
        true
    }
}

/// TCP connect with a hard timeout (lwip default SYN retries would stall the loop for seconds).
fn tcp_connect_timeout(addr: &str, timeout: Duration) -> io::Result<TcpStream> {
    let sa = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::other(format!("Adresse nicht auflösbar: {addr}")))?;
    TcpStream::connect_timeout(&sa, timeout)
}

/// TCP transport that never blocks the poll loop while connecting: while the remote is
/// unavailable, `read_chunk` returns empty chunks (the loop ticks and drains intents)
/// and retries every 2 s. Replaces the blocking connect at boot.
pub struct ReconnectingTcp {
    addr: String,
    inner: Option<TcpTransport>,
    next_attempt: Instant,
}

const RECONNECT_EVERY: Duration = Duration::from_secs(2);

impl ReconnectingTcp {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            inner: None,
            next_attempt: Instant::now(),
        }
    }

    fn drop_link(&mut self, why: &str) {
        log::warn!(
            "Panel-TCP {} getrennt ({why}) — Reconnect in 2 s",
            self.addr
        );
        self.inner = None;
        self.next_attempt = Instant::now() + RECONNECT_EVERY;
    }
}

impl PanelTransport for ReconnectingTcp {
    fn read_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        let Some(inner) = self.inner.as_mut() else {
            if Instant::now() >= self.next_attempt {
                self.next_attempt = Instant::now() + RECONNECT_EVERY;
                match tcp_connect_timeout(&self.addr, Duration::from_secs(1)) {
                    Ok(stream) => {
                        stream
                            .set_read_timeout(Some(Duration::from_millis(200)))
                            .ok();
                        stream.set_nodelay(true).ok();
                        log::info!("Panel-Transport (TCP) verbunden: {}", self.addr);
                        self.inner = Some(TcpTransport { stream });
                    }
                    Err(e) => log::warn!("Panel-TCP {} nicht erreichbar ({e})", self.addr),
                }
            } else {
                // Don't spin hot but remain responsive for intents.
                esp_idf_svc::hal::delay::FreeRtos::delay_ms(50);
            }
            return Ok(Some(Vec::new()));
        };
        match inner.read_chunk() {
            Ok(None) => {
                self.drop_link("EOF");
                Ok(Some(Vec::new()))
            }
            Err(e) => {
                self.drop_link(&e.to_string());
                Ok(Some(Vec::new()))
            }
            ok => ok,
        }
    }

    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self.inner.as_mut() {
            // No link: discard frames (the runtime polls cyclically anyway).
            None => Ok(()),
            Some(inner) => match inner.write_frame(bytes) {
                Err(e) => {
                    self.drop_link(&e.to_string());
                    Ok(())
                }
                ok => ok,
            },
        }
    }

    fn is_live(&self) -> bool {
        true
    }

    fn set_target(&mut self, target: &PanelTarget) -> bool {
        let PanelTarget::Tcp(ip, port) = target else {
            return false; // Internal can only be served by SwitchableTransport.
        };
        let addr = format!("{ip}:{port}");
        if addr != self.addr {
            log::info!("Panel-Transport-Ziel: {} → {addr}", self.addr);
            self.addr = addr;
            self.inner = None;
            self.next_attempt = Instant::now();
        }
        true
    }
}

/// Production transport: internal RS232 (UART) and TCP (USR-TCP232/bench mock) in one backend,
/// live-switchable via the setup step "connection" — no reboot needed.
/// The TCP branch is created lazily on the first TCP target (no address exists before that).
pub struct SwitchableTransport<'d> {
    uart: UartTransport<'d>,
    tcp: Option<ReconnectingTcp>,
    internal: bool,
}

impl<'d> SwitchableTransport<'d> {
    pub fn new(uart: UartTransport<'d>, initial: PanelTarget) -> Self {
        let mut s = Self {
            uart,
            tcp: None,
            internal: true,
        };
        s.set_target(&initial);
        s
    }
}

impl PanelTransport for SwitchableTransport<'_> {
    fn read_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        if self.internal {
            self.uart.read_chunk()
        } else {
            match self.tcp.as_mut() {
                Some(tcp) => tcp.read_chunk(),
                // TCP selected but no target yet: just tick, don't busy-spin.
                None => {
                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(50);
                    Ok(Some(Vec::new()))
                }
            }
        }
    }

    fn write_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.internal {
            self.uart.write_frame(bytes)
        } else {
            match self.tcp.as_mut() {
                Some(tcp) => tcp.write_frame(bytes),
                None => Ok(()), // wie ReconnectingTcp ohne Link: verwerfen, Loop pollt neu
            }
        }
    }

    fn is_live(&self) -> bool {
        true
    }

    fn set_baud(&mut self, baud: u32) -> bool {
        // Always applied to the UART: switching back from TCP to internal must come up
        // with the configured baud. TCP itself is baud-agnostic (USR-TCP232 side).
        self.uart.set_baud(baud)
    }

    fn set_target(&mut self, target: &PanelTarget) -> bool {
        match target {
            PanelTarget::Internal => {
                if !self.internal {
                    log::info!("Panel-Transport: TCP → interne RS232 (UART)");
                }
                self.internal = true;
                self.tcp = None; // close any open TCP connection
            }
            PanelTarget::Tcp(ip, port) => {
                if self.internal {
                    log::info!("Panel-Transport: interne RS232 → TCP {ip}:{port}");
                }
                self.internal = false;
                match self.tcp.as_mut() {
                    Some(tcp) => {
                        tcp.set_target(target);
                    }
                    None => self.tcp = Some(ReconnectingTcp::new(format!("{ip}:{port}"))),
                }
            }
        }
        true
    }
}

/// Active connection probe for setup ("check connection"): TCP-Connect (2 s timeout),
/// then listen for up to 4 s to confirm the GMS actually sends bytes. Returns
/// `(connected, data_seen)` for [`telenot_app::app::ConnCheckView::from_probe`].
/// Blocking — call only from a worker thread, never from the poll loop.
pub fn probe_tcp(ip: &str, port: u16) -> (bool, bool) {
    let Ok(stream) = tcp_connect_timeout(&format!("{ip}:{port}"), Duration::from_secs(2)) else {
        return (false, false);
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok();
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut buf = [0u8; 64];
    let mut stream = stream;
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(n) if n > 0 => return (true, true),
            Ok(_) => break, // EOF
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    (true, false)
}
