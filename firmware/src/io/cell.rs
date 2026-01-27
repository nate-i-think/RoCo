//! Cellular modem (BG95) hub and status aggregation.
//!
//! Provides `ATHub` for async AT command execution and `CellStatus` for
//! aggregated modem state, following patterns from `ubx.rs` and `gps.rs`.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::MissedTickBehavior;
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use super::at::{
    ATCmd, ATResponse, NetworkMode, RegStatus, ResponseAccumulator, SimStatus, URC,
    try_parse_urc,
};

// ============================================================================
// ERROR TYPES
// ============================================================================

/// Errors that can occur during AT command execution.
#[derive(Debug)]
pub enum ATError {
    /// I/O error on serial port
    Io(std::io::Error),
    /// Command timed out waiting for response
    Timeout,
    /// ATHub has been closed/dropped
    HubClosed,
    /// Modem returned an error response (+CME ERROR or ERROR)
    ModemError { code: Option<u16>, message: Option<String> },
}

impl std::fmt::Display for ATError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::Timeout => write!(f, "command timed out"),
            Self::HubClosed => write!(f, "ATHub closed"),
            Self::ModemError { code, message } => {
                match (code, message) {
                    (Some(c), Some(m)) => write!(f, "modem error {}: {}", c, m),
                    (Some(c), None) => write!(f, "modem error {}", c),
                    (None, Some(m)) => write!(f, "modem error: {}", m),
                    (None, None) => write!(f, "modem error"),
                }
            }
        }
    }
}

impl std::error::Error for ATError {}

impl From<std::io::Error> for ATError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// ============================================================================
// ATHub
// ============================================================================

/// Central AT command hub for BG95 modem.
///
/// Simpler than `UBXHub` because:
/// - AT protocol is strictly sequential (one command at a time)
/// - Text-based, line-delimited (no binary framing)
/// - Single reader/writer task (not two)
///
/// # Example
/// ```ignore
/// let handles = init_cell_task().await?;
/// let (rssi, ber) = handles.hub.signal_quality().await?;
/// ```
#[derive(Clone)]
pub struct ATHub {
    inner: Arc<ATHubInner>,
}

struct ATHubInner {
    cmd_tx: mpsc::Sender<CmdRequest>,
    urc_tx: watch::Sender<Option<Arc<URC>>>,
}

struct CmdRequest {
    cmd: ATCmd,
    /// Binary data to send after ">" prompt (for QISEND)
    data: Option<Vec<u8>>,
    deadline: Duration,
    resp_tx: oneshot::Sender<Result<ATResponse, ATError>>,
}

impl ATHub {
    /// Spawn reader/writer task and return the hub handle.
    pub fn spawn(reader: ReadHalf<SerialStream>, writer: WriteHalf<SerialStream>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<CmdRequest>(16);
        let (urc_tx, _urc_rx) = watch::channel::<Option<Arc<URC>>>(None);

        let inner = Arc::new(ATHubInner {
            cmd_tx,
            urc_tx: urc_tx.clone(),
        });

        // Single task handles both reading and writing
        tokio::spawn(at_task(reader, writer, cmd_rx, urc_tx));

        Self { inner }
    }

    /// Execute an AT command and wait for response.
    ///
    /// Returns the parsed response or an error if the command fails or times out.
    pub async fn execute(&self, cmd: ATCmd, deadline: Duration) -> Result<ATResponse, ATError> {
        self.execute_with_data(cmd, None, deadline).await
    }

    /// Execute command with binary data payload (for QISEND).
    ///
    /// The data is sent after the modem responds with ">".
    pub async fn execute_with_data(
        &self,
        cmd: ATCmd,
        data: Option<Vec<u8>>,
        deadline: Duration,
    ) -> Result<ATResponse, ATError> {
        let (resp_tx, resp_rx) = oneshot::channel();

        let req = CmdRequest {
            cmd,
            data,
            deadline,
            resp_tx,
        };

        self.inner
            .cmd_tx
            .send(req)
            .await
            .map_err(|_| ATError::HubClosed)?;

        resp_rx.await.map_err(|_| ATError::HubClosed)?
    }

    /// Subscribe to URCs (Unsolicited Result Codes).
    ///
    /// URCs are broadcast to all subscribers when they arrive.
    pub fn subscribe_urc(&self) -> watch::Receiver<Option<Arc<URC>>> {
        self.inner.urc_tx.subscribe()
    }

    // ========================================================================
    // Convenience methods
    // ========================================================================

    /// Query signal quality (AT+CSQ).
    ///
    /// Returns (rssi, ber). RSSI 99 = unknown. Convert to dBm: -113 + (rssi * 2).
    pub async fn signal_quality(&self) -> Result<(u8, u8), ATError> {
        match self.execute(ATCmd::CSQ, Duration::from_secs(5)).await? {
            ATResponse::CSQ { rssi, ber } => Ok((rssi, ber)),
            ATResponse::Error { code, message } => Err(ATError::ModemError { code, message }),
            _ => Err(ATError::ModemError { code: None, message: None }),
        }
    }

    /// Query extended signal quality (AT+QCSQ).
    pub async fn signal_quality_extended(&self) -> Result<(NetworkMode, i16, i16, i16, i16), ATError> {
        match self.execute(ATCmd::QCSQ, Duration::from_secs(5)).await? {
            ATResponse::QCSQ { mode, rssi, rsrp, sinr, rsrq } => {
                Ok((mode, rssi, rsrp, sinr, rsrq))
            }
            ATResponse::Error { code, message } => Err(ATError::ModemError { code, message }),
            _ => Err(ATError::ModemError { code: None, message: None }),
        }
    }

    /// Query SIM status (AT+CPIN?).
    pub async fn sim_status(&self) -> Result<SimStatus, ATError> {
        match self.execute(ATCmd::CPIN, Duration::from_secs(5)).await? {
            ATResponse::CPIN { status } => Ok(status),
            ATResponse::Error { code, message } => Err(ATError::ModemError { code, message }),
            _ => Err(ATError::ModemError { code: None, message: None }),
        }
    }

    /// Query network registration status (AT+CREG?).
    pub async fn registration_status(&self) -> Result<RegStatus, ATError> {
        match self.execute(ATCmd::CREG, Duration::from_secs(5)).await? {
            ATResponse::CREG { status, .. } => Ok(status),
            ATResponse::Error { code, message } => Err(ATError::ModemError { code, message }),
            _ => Err(ATError::ModemError { code: None, message: None }),
        }
    }

    /// Query operator info (AT+COPS?).
    pub async fn operator(&self) -> Result<Option<String>, ATError> {
        match self.execute(ATCmd::COPS, Duration::from_secs(10)).await? {
            ATResponse::COPS { operator, .. } => Ok(operator),
            ATResponse::Error { code, message } => Err(ATError::ModemError { code, message }),
            _ => Err(ATError::ModemError { code: None, message: None }),
        }
    }

    /// Set modem functionality (AT+CFUN).
    ///
    /// - 0: Minimum functionality (airplane mode)
    /// - 1: Full functionality
    /// - 4: Disable RF (but keep SIM)
    pub async fn set_functionality(&self, mode: u8) -> Result<(), ATError> {
        match self.execute(ATCmd::CFUN { mode }, Duration::from_secs(15)).await? {
            ATResponse::Ok => Ok(()),
            ATResponse::Error { code, message } => Err(ATError::ModemError { code, message }),
            _ => Ok(()),
        }
    }
}

// ============================================================================
// AT TASK
// ============================================================================

/// The core AT reader/writer task.
///
/// Handles serial I/O, command-response correlation, and URC dispatch.
async fn at_task(
    reader: ReadHalf<SerialStream>,
    mut writer: WriteHalf<SerialStream>,
    mut cmd_rx: mpsc::Receiver<CmdRequest>,
    urc_tx: watch::Sender<Option<Arc<URC>>>,
) {
    let mut buf_reader = BufReader::with_capacity(4096, reader);
    let mut line_buf = String::with_capacity(256);

    // Pending command state
    let mut pending: Option<(ResponseAccumulator, oneshot::Sender<Result<ATResponse, ATError>>)> =
        None;
    let mut pending_deadline: Option<tokio::time::Instant> = None;
    let mut pending_data: Option<Vec<u8>> = None;

    loop {
        tokio::select! {
            biased;

            // Handle incoming command requests (only if no command pending)
            Some(req) = cmd_rx.recv(), if pending.is_none() => {
                // Send the command
                let bytes = req.cmd.to_bytes();
                tracing::debug!(target: "ATHub", "TX: {}", String::from_utf8_lossy(&bytes).trim());

                if let Err(e) = writer.write_all(&bytes).await {
                    tracing::error!(target: "ATHub", "Write error: {}", e);
                    let _ = req.resp_tx.send(Err(ATError::Io(e)));
                    continue;
                }

                // Set up response accumulator
                pending = Some((ResponseAccumulator::new(), req.resp_tx));
                pending_deadline = Some(tokio::time::Instant::now() + req.deadline);
                pending_data = req.data;
            }

            // Read lines from modem
            result = buf_reader.read_line(&mut line_buf) => {
                match result {
                    Ok(0) => {
                        tracing::warn!(target: "ATHub", "Serial EOF");
                        break;
                    }
                    Ok(_) => {
                        let line = line_buf.trim_end_matches(&['\r', '\n'][..]);

                        // Skip empty lines
                        if line.is_empty() {
                            line_buf.clear();
                            continue;
                        }

                        tracing::debug!(target: "ATHub", "RX: {}", line);

                        // Check for ">" prompt (QISEND ready for data)
                        if line == ">" || line.ends_with('>') {
                            if let Some(data) = pending_data.take() {
                                tracing::debug!(target: "ATHub", "TX data: {} bytes", data.len());
                                if let Err(e) = writer.write_all(&data).await {
                                    tracing::error!(target: "ATHub", "Data write error: {}", e);
                                    if let Some((_, resp_tx)) = pending.take() {
                                        let _ = resp_tx.send(Err(ATError::Io(e)));
                                    }
                                    pending_deadline = None;
                                }
                            }
                            line_buf.clear();
                            continue;
                        }

                        // Try as URC first
                        if let Some(urc) = try_parse_urc(line) {
                            tracing::debug!(target: "ATHub", "URC: {:?}", urc);
                            let _ = urc_tx.send(Some(Arc::new(urc)));
                            line_buf.clear();
                            continue;
                        }

                        // Feed to response accumulator if command pending
                        if let Some((ref mut acc, _)) = pending {
                            if acc.push(line) {
                                // Response complete
                                let (acc, resp_tx) = pending.take().unwrap();
                                let response = acc.finalize();
                                tracing::debug!(target: "ATHub", "Response: {:?}", response);
                                let _ = resp_tx.send(Ok(response));
                                pending_deadline = None;
                            }
                        }

                        line_buf.clear();
                    }
                    Err(e) => {
                        tracing::error!(target: "ATHub", "Read error: {}", e);
                        if let Some((_, resp_tx)) = pending.take() {
                            let _ = resp_tx.send(Err(ATError::Io(e)));
                        }
                        break;
                    }
                }
            }

            // Timeout check
            _ = async {
                if let Some(deadline) = pending_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                if let Some((_, resp_tx)) = pending.take() {
                    tracing::warn!(target: "ATHub", "Command timeout");
                    let _ = resp_tx.send(Err(ATError::Timeout));
                }
                pending_deadline = None;
                pending_data = None;
            }
        }
    }

    tracing::warn!(target: "ATHub", "AT task exiting");
}

// ============================================================================
// CELL STATUS
// ============================================================================

/// Aggregated cellular modem status snapshot.
///
/// Similar to `GNSSFix` - provides a consistent view of modem state.
#[derive(Clone, Debug, Default)]
pub struct CellStatus {
    /// RSSI in dBm (99 → -128 = unknown)
    pub rssi_dbm: i16,
    /// RSRP in dBm (LTE reference signal power)
    pub rsrp: i16,
    /// SINR in dB (signal-to-interference-plus-noise ratio)
    pub sinr: i16,
    /// RSRQ in dB (reference signal received quality)
    pub rsrq: i16,
    /// Network mode (GSM, CatM1, NB-IoT, etc.)
    pub network_mode: NetworkMode,
    /// Network registration status
    pub reg_status: RegStatus,
    /// Operator name (if known)
    pub operator: Option<String>,
    /// SIM status
    pub sim_status: SimStatus,
}

impl CellStatus {
    /// Returns true if registered to a network (home or roaming).
    pub fn is_registered(&self) -> bool {
        self.reg_status.is_registered()
    }

    /// Returns true if SIM is ready.
    pub fn sim_ready(&self) -> bool {
        matches!(self.sim_status, SimStatus::Ready)
    }
}

/// Handle for subscribing to `CellStatus` updates.
#[derive(Clone)]
pub struct CellStatusHandle {
    rx: watch::Receiver<CellStatus>,
}

impl CellStatusHandle {
    /// Get a new receiver subscribed to status updates.
    pub fn subscribe(&self) -> watch::Receiver<CellStatus> {
        self.rx.clone()
    }

    /// Clone the latest status.
    pub fn latest(&self) -> CellStatus {
        self.rx.borrow().clone()
    }

    /// Borrow the latest status (do not hold across `.await`).
    pub fn borrow(&self) -> watch::Ref<'_, CellStatus> {
        self.rx.borrow()
    }

    /// Read status via closure without cloning.
    pub fn with_latest<R>(&self, f: impl FnOnce(&CellStatus) -> R) -> R {
        let status = self.rx.borrow();
        f(&*status)
    }
}

// ============================================================================
// CELL HANDLES
// ============================================================================

/// Returned handles for interacting with the cellular modem.
#[derive(Clone)]
pub struct CellHandles {
    /// Aggregated status (signal, registration, etc.)
    pub status: CellStatusHandle,
    /// Direct hub access for sending commands
    pub hub: ATHub,
}

// ============================================================================
// INITIALIZATION
// ============================================================================

/// Initialize the cellular modem task.
///
/// Opens the serial port, spawns the ATHub, configures the modem,
/// and starts the status polling task.
///
/// # Returns
/// `CellHandles` with status subscription and hub access.
pub async fn init_cell_task() -> Result<CellHandles, ATError> {
    const PORT_NAME: &str = "/dev/ttyAMA2";
    const BAUD_RATE: u32 = 115_200;

    tracing::info!(target: "InitCellTask", "Opening {} at {} baud", PORT_NAME, BAUD_RATE);

    let serial_port = tokio_serial::new(PORT_NAME, BAUD_RATE)
        .data_bits(tokio_serial::DataBits::Eight)
        .flow_control(tokio_serial::FlowControl::None)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .timeout(Duration::from_secs(1))
        .open_native_async()
        .map_err(|e| ATError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

    tracing::info!(target: "InitCellTask", "Serial port opened");

    let (reader, writer) = tokio::io::split(serial_port);
    let hub = ATHub::spawn(reader, writer);

    // Initial configuration
    configure_modem(&hub).await?;

    // Status aggregation
    let (status_tx, status_rx) = watch::channel(CellStatus::default());
    tokio::spawn(cell_poll_task(hub.clone(), status_tx));

    Ok(CellHandles {
        status: CellStatusHandle { rx: status_rx },
        hub,
    })
}

/// Configure modem with initial settings.
async fn configure_modem(hub: &ATHub) -> Result<(), ATError> {
    // Small delay to let modem settle after port open
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Disable echo
    tracing::debug!(target: "InitCellTask", "Disabling echo (ATE0)");
    match hub.execute(ATCmd::ATE(false), Duration::from_secs(2)).await {
        Ok(_) => {}
        Err(ATError::Timeout) => {
            // Echo might already be off, or modem busy. Try once more.
            tracing::warn!(target: "InitCellTask", "ATE0 timed out, retrying");
            hub.execute(ATCmd::ATE(false), Duration::from_secs(2)).await?;
        }
        Err(e) => return Err(e),
    }

    // Enable numeric error codes
    tracing::debug!(target: "InitCellTask", "Enabling numeric errors (CMEE=1)");
    hub.execute(ATCmd::CMEE(1), Duration::from_secs(2)).await?;

    // Enable network registration URCs with location info
    tracing::debug!(target: "InitCellTask", "Enabling registration URCs (CREG=2, CGREG=2)");
    hub.execute(ATCmd::CREGSet(2), Duration::from_secs(2)).await?;
    hub.execute(ATCmd::CGREGSet(2), Duration::from_secs(2)).await?;

    tracing::info!(target: "InitCellTask", "Modem configured");
    Ok(())
}

/// Periodic polling task for cell status.
async fn cell_poll_task(hub: ATHub, tx: watch::Sender<CellStatus>) {
    let mut urc_rx = hub.subscribe_urc();

    // Poll intervals (cellular state changes slowly)
    let mut signal_interval = tokio::time::interval(Duration::from_secs(30));
    let mut reg_interval = tokio::time::interval(Duration::from_secs(60));
    let mut sim_interval = tokio::time::interval(Duration::from_secs(120));

    signal_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    reg_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    sim_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut status = CellStatus::default();

    // Initial poll
    if let Ok((rssi, _)) = hub.signal_quality().await {
        status.rssi_dbm = rssi_to_dbm(rssi);
    }
    if let Ok(sim) = hub.sim_status().await {
        status.sim_status = sim;
    }
    if let Ok(reg) = hub.registration_status().await {
        status.reg_status = reg;
    }
    let _ = tx.send(status.clone());

    loop {
        tokio::select! {
            biased;

            // Handle URCs
            _ = urc_rx.changed() => {
                if let Some(urc) = urc_rx.borrow().as_ref() {
                    match &**urc {
                        URC::CREG { status: reg, .. } |
                        URC::CGREG { status: reg, .. } => {
                            if status.reg_status != *reg {
                                tracing::info!(target: "CellPoll", "Registration changed: {:?}", reg);
                                status.reg_status = *reg;
                                let _ = tx.send(status.clone());
                            }
                        }
                        URC::Ready => {
                            tracing::info!(target: "CellPoll", "Modem ready");
                        }
                        URC::PoweredDown => {
                            tracing::warn!(target: "CellPoll", "Modem powered down");
                        }
                        _ => {}
                    }
                }
            }

            // Poll signal quality (every 30s)
            _ = signal_interval.tick() => {
                // Try extended signal first
                match hub.signal_quality_extended().await {
                    Ok((mode, rssi, rsrp, sinr, rsrq)) => {
                        status.network_mode = mode;
                        status.rssi_dbm = rssi;
                        status.rsrp = rsrp;
                        status.sinr = sinr;
                        status.rsrq = rsrq;
                        let _ = tx.send(status.clone());
                    }
                    Err(_) => {
                        // Fallback to basic CSQ
                        if let Ok((rssi, _)) = hub.signal_quality().await {
                            status.rssi_dbm = rssi_to_dbm(rssi);
                            let _ = tx.send(status.clone());
                        }
                    }
                }
            }

            // Poll registration (every 60s)
            _ = reg_interval.tick() => {
                if let Ok(reg) = hub.registration_status().await {
                    if status.reg_status != reg {
                        tracing::info!(target: "CellPoll", "Registration: {:?}", reg);
                        status.reg_status = reg;
                    }
                }
                if let Ok(op) = hub.operator().await {
                    if status.operator != op {
                        tracing::info!(target: "CellPoll", "Operator: {:?}", op);
                        status.operator = op;
                    }
                }
                let _ = tx.send(status.clone());
            }

            // Poll SIM status (every 120s)
            _ = sim_interval.tick() => {
                if let Ok(sim) = hub.sim_status().await {
                    if status.sim_status != sim {
                        tracing::info!(target: "CellPoll", "SIM status: {:?}", sim);
                        status.sim_status = sim;
                        let _ = tx.send(status.clone());
                    }
                }
            }
        }
    }
}

/// Convert GSM RSSI (0-31, 99) to dBm.
fn rssi_to_dbm(rssi: u8) -> i16 {
    if rssi == 99 {
        -128 // Unknown
    } else {
        -113 + (rssi as i16 * 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rssi_to_dbm() {
        assert_eq!(rssi_to_dbm(0), -113);
        assert_eq!(rssi_to_dbm(15), -83);
        assert_eq!(rssi_to_dbm(31), -51);
        assert_eq!(rssi_to_dbm(99), -128);
    }
}
