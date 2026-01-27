//! AT command protocol types and parsing for BG95 cellular modem.

use std::fmt;

// ============================================================================
// SUPPORTING TYPES
// ============================================================================

/// Network registration status (for CREG/CGREG).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegStatus {
    #[default]
    NotRegistered,       // 0 - Not registered, not searching
    RegisteredHome,      // 1 - Registered, home network
    Searching,           // 2 - Not registered, searching
    RegistrationDenied,  // 3 - Registration denied
    Unknown,             // 4 - Unknown
    RegisteredRoaming,   // 5 - Registered, roaming
}

impl RegStatus {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::NotRegistered,
            1 => Self::RegisteredHome,
            2 => Self::Searching,
            3 => Self::RegistrationDenied,
            5 => Self::RegisteredRoaming,
            _ => Self::Unknown,
        }
    }

    pub fn is_registered(&self) -> bool {
        matches!(self, Self::RegisteredHome | Self::RegisteredRoaming)
    }
}

/// SIM card status.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SimStatus {
    #[default]
    Unknown,
    Ready,
    SimPin,
    SimPuk,
    SimPin2,
    SimPuk2,
    PhNetPin,
    NotInserted,
    Other(String),
}

impl SimStatus {
    pub fn from_str(s: &str) -> Self {
        match s.trim() {
            "READY" => Self::Ready,
            "SIM PIN" => Self::SimPin,
            "SIM PUK" => Self::SimPuk,
            "SIM PIN2" => Self::SimPin2,
            "SIM PUK2" => Self::SimPuk2,
            "PH-NET PIN" => Self::PhNetPin,
            other => Self::Other(other.to_string()),
        }
    }
}

/// TCP/UDP service type for socket operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceType {
    TCP,
    UDP,
}

impl fmt::Display for ServiceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TCP => write!(f, "TCP"),
            Self::UDP => write!(f, "UDP"),
        }
    }
}

/// Power saving mode setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PSMMode {
    Disable,  // 0
    Enable,   // 1
    Reset,    // 2
}

/// Network mode (from QCSQ).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkMode {
    #[default]
    NoService,
    GSM,
    CatM1,  // eMTC
    NbIot,
}

impl NetworkMode {
    pub fn from_str(s: &str) -> Self {
        match s.trim().trim_matches('"') {
            "NOSERVICE" => Self::NoService,
            "GSM" => Self::GSM,
            "eMTC" | "CAT-M1" | "LTE" => Self::CatM1,
            "NBIoT" | "NB-IoT" => Self::NbIot,
            _ => Self::NoService,
        }
    }
}

// ============================================================================
// AT COMMANDS
// ============================================================================

/// AT commands that can be sent to the modem.
#[derive(Debug, Clone)]
pub enum ATCmd {
    /// Raw AT command (escape hatch). String should NOT include "AT" prefix or "\r\n".
    Raw(String),

    // === Signal / Network ===
    /// AT+CSQ - Signal quality (legacy)
    CSQ,
    /// AT+QCSQ - Extended signal quality (Quectel)
    QCSQ,
    /// AT+CREG? - Network registration status
    CREG,
    /// AT+CGREG? - GPRS registration status
    CGREG,
    /// AT+COPS? - Operator selection query
    COPS,
    /// AT+CPIN? - SIM PIN status
    CPIN,

    // === TCP/IP ===
    /// AT+QIOPEN - Open socket connection
    QIOpen {
        context_id: u8,
        connect_id: u8,
        service_type: ServiceType,
        host: String,
        port: u16,
    },
    /// AT+QISEND - Send data (data provided separately after ">" prompt)
    QISend {
        connect_id: u8,
        length: usize,
    },
    /// AT+QIRD - Read received data
    QIRd {
        connect_id: u8,
        max_len: u16,
    },
    /// AT+QICLOSE - Close socket
    QIClose {
        connect_id: u8,
    },

    // === Power Management ===
    /// AT+CPSMS - Power saving mode setting
    CPSMS {
        mode: PSMMode,
        tau: Option<String>,     // T3412 timer
        active: Option<String>,  // T3324 timer
    },
    /// AT+CFUN - Set UE functionality
    CFUN {
        mode: u8,  // 0=minimum, 1=full, 4=disable RF
    },

    // === Configuration ===
    /// ATE0 or ATE1 - Echo mode
    ATE(bool),
    /// AT+CMEE=<n> - Error message format (0=disable, 1=numeric, 2=verbose)
    CMEE(u8),
    /// AT+CREG=<n> - Enable/configure registration URC
    CREGSet(u8),
    /// AT+CGREG=<n> - Enable/configure GPRS registration URC
    CGREGSet(u8),
}

impl ATCmd {
    /// Serialize command to bytes ready for transmission.
    pub fn to_bytes(&self) -> Vec<u8> {
        let s = match self {
            Self::Raw(s) => format!("AT{}", s),
            Self::CSQ => "AT+CSQ".into(),
            Self::QCSQ => "AT+QCSQ".into(),
            Self::CREG => "AT+CREG?".into(),
            Self::CGREG => "AT+CGREG?".into(),
            Self::COPS => "AT+COPS?".into(),
            Self::CPIN => "AT+CPIN?".into(),

            Self::QIOpen { context_id, connect_id, service_type, host, port } => {
                format!(
                    "AT+QIOPEN={},{},\"{}\",\"{}\",{},0,1",
                    context_id, connect_id, service_type, host, port
                )
            }
            Self::QISend { connect_id, length } => {
                format!("AT+QISEND={},{}", connect_id, length)
            }
            Self::QIRd { connect_id, max_len } => {
                format!("AT+QIRD={},{}", connect_id, max_len)
            }
            Self::QIClose { connect_id } => {
                format!("AT+QICLOSE={}", connect_id)
            }

            Self::CPSMS { mode, tau, active } => {
                let m = match mode {
                    PSMMode::Disable => 0,
                    PSMMode::Enable => 1,
                    PSMMode::Reset => 2,
                };
                match (tau, active) {
                    (Some(t), Some(a)) => format!("AT+CPSMS={},,,\"{}\",\"{}\"", m, t, a),
                    _ => format!("AT+CPSMS={}", m),
                }
            }
            Self::CFUN { mode } => format!("AT+CFUN={}", mode),

            Self::ATE(on) => format!("ATE{}", if *on { 1 } else { 0 }),
            Self::CMEE(n) => format!("AT+CMEE={}", n),
            Self::CREGSet(n) => format!("AT+CREG={}", n),
            Self::CGREGSet(n) => format!("AT+CGREG={}", n),
        };
        format!("{}\r\n", s).into_bytes()
    }
}

// ============================================================================
// AT RESPONSES
// ============================================================================

/// Parsed AT response (solicited, from a command).
#[derive(Debug, Clone)]
pub enum ATResponse {
    /// Simple OK response
    Ok,
    /// ERROR or +CME ERROR: <code>
    Error { code: Option<u16>, message: Option<String> },

    // === Signal / Network ===
    /// +CSQ: <rssi>,<ber>
    CSQ { rssi: u8, ber: u8 },
    /// +QCSQ: "<mode>",<rssi>,<rsrp>,<sinr>,<rsrq>
    QCSQ {
        mode: NetworkMode,
        rssi: i16,
        rsrp: i16,
        sinr: i16,
        rsrq: i16,
    },
    /// +CREG: <n>,<stat>[,<lac>,<ci>[,<act>]]
    CREG {
        n: u8,
        status: RegStatus,
        lac: Option<u16>,
        ci: Option<u32>,
        act: Option<u8>,
    },
    /// +CGREG: <n>,<stat>[,<lac>,<ci>[,<act>]]
    CGREG {
        n: u8,
        status: RegStatus,
        lac: Option<u16>,
        ci: Option<u32>,
        act: Option<u8>,
    },
    /// +COPS: <mode>[,<format>,<oper>[,<act>]]
    COPS {
        mode: u8,
        format: Option<u8>,
        operator: Option<String>,
        act: Option<u8>,
    },
    /// +CPIN: <code>
    CPIN { status: SimStatus },

    // === TCP/IP ===
    /// +QIOPEN: <connectID>,<err>
    QIOpen { connect_id: u8, err: u16 },
    /// SEND OK / SEND FAIL / +QISEND: <connectID>,<sent>,<acked>,<nacked>
    QISend { connect_id: u8, sent: u16, acked: u16, nacked: u16 },
    /// +QIRD: <read_len>\r\n<data>
    QIRd { connect_id: u8, data: Vec<u8> },

    // === Unknown ===
    /// Raw lines that couldn't be parsed into a specific type
    Unknown(Vec<String>),
}

// ============================================================================
// UNSOLICITED RESULT CODES (URCs)
// ============================================================================

/// URCs arrive asynchronously, not in response to commands.
#[derive(Debug, Clone)]
pub enum URC {
    // === Network ===
    /// +CREG: <stat>[,<lac>,<ci>[,<act>]]
    CREG {
        status: RegStatus,
        lac: Option<u16>,
        ci: Option<u32>,
        act: Option<u8>,
    },
    /// +CGREG: <stat>[,<lac>,<ci>[,<act>]]
    CGREG {
        status: RegStatus,
        lac: Option<u16>,
        ci: Option<u32>,
        act: Option<u8>,
    },

    // === TCP/IP ===
    /// +QIURC: "recv",<connectID>
    QIUrcRecv { connect_id: u8 },
    /// +QIURC: "closed",<connectID>
    QIUrcClosed { connect_id: u8 },
    /// +QIURC: "pdpdeact",<contextID>
    QIUrcPdpDeact { context_id: u8 },
    /// +QIOPEN: <connectID>,<err> (can arrive as URC in non-blocking mode)
    QIOpen { connect_id: u8, err: u16 },

    // === Power / System ===
    /// RDY - Module ready
    Ready,
    /// POWERED DOWN - Module powering off
    PoweredDown,
    /// +QUSIM: 1 - USIM detected
    QUsim,
    /// +QIND: SMS DONE
    SmsReady,

    // === Unknown ===
    Unknown(String),
}

// ============================================================================
// RESPONSE ACCUMULATOR
// ============================================================================

/// Collects multi-line responses until a terminal result code.
pub struct ResponseAccumulator {
    lines: Vec<String>,
    complete: bool,
}

impl ResponseAccumulator {
    pub fn new() -> Self {
        Self {
            lines: Vec::with_capacity(8),
            complete: false,
        }
    }

    /// Returns true if response collection is complete.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Feed a line (already trimmed of \r\n). Returns true if response is now complete.
    pub fn push(&mut self, line: &str) -> bool {
        // Skip empty lines
        if line.is_empty() {
            return false;
        }

        // Check for terminal responses
        if line == "OK"
            || line == "ERROR"
            || line == "SEND OK"
            || line == "SEND FAIL"
            || line.starts_with("+CME ERROR:")
            || line.starts_with("+CMS ERROR:")
        {
            self.lines.push(line.to_string());
            self.complete = true;
            return true;
        }

        self.lines.push(line.to_string());
        false
    }

    /// Consume and parse accumulated lines into ATResponse.
    pub fn finalize(self) -> ATResponse {
        parse_response_lines(&self.lines)
    }

    /// Borrow the accumulated lines.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

impl Default for ResponseAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// PARSING FUNCTIONS
// ============================================================================

/// Try parsing a line as a URC. Returns None if it's not a recognized URC.
pub fn try_parse_urc(line: &str) -> Option<URC> {
    let line = line.trim();

    // System URCs
    if line == "RDY" {
        return Some(URC::Ready);
    }
    if line == "POWERED DOWN" {
        return Some(URC::PoweredDown);
    }
    if line == "+QUSIM: 1" {
        return Some(URC::QUsim);
    }
    if line == "+QIND: SMS DONE" {
        return Some(URC::SmsReady);
    }

    // +CREG: <stat>[,<lac>,<ci>[,<act>]] (URC form - no <n> prefix)
    if let Some(rest) = line.strip_prefix("+CREG: ") {
        return parse_reg_urc(rest).map(|(status, lac, ci, act)| {
            URC::CREG { status, lac, ci, act }
        });
    }

    // +CGREG: <stat>[,<lac>,<ci>[,<act>]]
    if let Some(rest) = line.strip_prefix("+CGREG: ") {
        return parse_reg_urc(rest).map(|(status, lac, ci, act)| {
            URC::CGREG { status, lac, ci, act }
        });
    }

    // +QIURC: "event",<param>
    if let Some(rest) = line.strip_prefix("+QIURC: ") {
        return parse_qiurc(rest);
    }

    // +QIOPEN: <connectID>,<err> (can be URC in non-blocking mode)
    if let Some(rest) = line.strip_prefix("+QIOPEN: ") {
        return parse_qiopen(rest).map(|(connect_id, err)| {
            URC::QIOpen { connect_id, err }
        });
    }

    None
}

/// Parse registration URC: <stat>[,<lac>,<ci>[,<act>]]
fn parse_reg_urc(s: &str) -> Option<(RegStatus, Option<u16>, Option<u32>, Option<u8>)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.is_empty() {
        return None;
    }

    let status = RegStatus::from_u8(parts[0].trim().parse().ok()?);

    let lac = parts.get(1)
        .and_then(|s| parse_hex_or_dec(s.trim().trim_matches('"')));
    let ci = parts.get(2)
        .and_then(|s| parse_hex_or_dec_u32(s.trim().trim_matches('"')));
    let act = parts.get(3)
        .and_then(|s| s.trim().parse().ok());

    Some((status, lac, ci, act))
}

/// Parse +QIURC: "event",<param>
fn parse_qiurc(s: &str) -> Option<URC> {
    // Format: "recv",<connectID> or "closed",<connectID> or "pdpdeact",<contextID>
    let parts: Vec<&str> = s.splitn(2, ',').collect();
    if parts.is_empty() {
        return None;
    }

    let event = parts[0].trim().trim_matches('"');
    let param = parts.get(1).map(|s| s.trim());

    match event {
        "recv" => {
            let connect_id = param?.parse().ok()?;
            Some(URC::QIUrcRecv { connect_id })
        }
        "closed" => {
            let connect_id = param?.parse().ok()?;
            Some(URC::QIUrcClosed { connect_id })
        }
        "pdpdeact" => {
            let context_id = param?.parse().ok()?;
            Some(URC::QIUrcPdpDeact { context_id })
        }
        _ => Some(URC::Unknown(format!("+QIURC: {}", s))),
    }
}

/// Parse +QIOPEN: <connectID>,<err>
fn parse_qiopen(s: &str) -> Option<(u8, u16)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() < 2 {
        return None;
    }
    let connect_id = parts[0].trim().parse().ok()?;
    let err = parts[1].trim().parse().ok()?;
    Some((connect_id, err))
}

/// Parse accumulated response lines into ATResponse.
fn parse_response_lines(lines: &[String]) -> ATResponse {
    if lines.is_empty() {
        return ATResponse::Unknown(vec![]);
    }

    // Last line is the result code
    let result = lines.last().map(|s| s.as_str()).unwrap_or("");

    // Check for error
    if result == "ERROR" || result == "SEND FAIL" {
        return ATResponse::Error { code: None, message: None };
    }
    if let Some(rest) = result.strip_prefix("+CME ERROR: ") {
        let rest = rest.trim();
        // Could be numeric or text
        if let Ok(code) = rest.parse::<u16>() {
            return ATResponse::Error { code: Some(code), message: None };
        } else {
            return ATResponse::Error { code: None, message: Some(rest.to_string()) };
        }
    }
    if let Some(rest) = result.strip_prefix("+CMS ERROR: ") {
        let rest = rest.trim();
        if let Ok(code) = rest.parse::<u16>() {
            return ATResponse::Error { code: Some(code), message: None };
        } else {
            return ATResponse::Error { code: None, message: Some(rest.to_string()) };
        }
    }

    // Parse data lines (all but last)
    let data_lines = if lines.len() > 1 { &lines[..lines.len() - 1] } else { &[] };

    for line in data_lines {
        if let Some(resp) = parse_data_line(line) {
            return resp;
        }
    }

    // Check for SEND OK (QISEND success)
    if result == "SEND OK" {
        return ATResponse::Ok;
    }

    if result == "OK" && data_lines.is_empty() {
        return ATResponse::Ok;
    }

    ATResponse::Unknown(lines.to_vec())
}

/// Try to parse a single data line into a typed response.
fn parse_data_line(line: &str) -> Option<ATResponse> {
    let line = line.trim();

    // +CSQ: <rssi>,<ber>
    if let Some(rest) = line.strip_prefix("+CSQ: ") {
        let parts: Vec<&str> = rest.split(',').collect();
        if parts.len() >= 2 {
            let rssi = parts[0].trim().parse().ok()?;
            let ber = parts[1].trim().parse().ok()?;
            return Some(ATResponse::CSQ { rssi, ber });
        }
    }

    // +QCSQ: "<mode>",<rssi>,<rsrp>,<sinr>,<rsrq>
    if let Some(rest) = line.strip_prefix("+QCSQ: ") {
        let parts: Vec<&str> = rest.split(',').collect();
        if !parts.is_empty() {
            let mode = NetworkMode::from_str(parts[0]);
            let rssi = parts.get(1).and_then(|s| s.trim().parse().ok()).unwrap_or(-128);
            let rsrp = parts.get(2).and_then(|s| s.trim().parse().ok()).unwrap_or(-140);
            let sinr = parts.get(3).and_then(|s| s.trim().parse().ok()).unwrap_or(-200);
            let rsrq = parts.get(4).and_then(|s| s.trim().parse().ok()).unwrap_or(-200);
            return Some(ATResponse::QCSQ { mode, rssi, rsrp, sinr, rsrq });
        }
    }

    // +CREG: <n>,<stat>[,<lac>,<ci>[,<act>]]
    if let Some(rest) = line.strip_prefix("+CREG: ") {
        if let Some((n, status, lac, ci, act)) = parse_reg_response(rest) {
            return Some(ATResponse::CREG { n, status, lac, ci, act });
        }
    }

    // +CGREG: <n>,<stat>[,<lac>,<ci>[,<act>]]
    if let Some(rest) = line.strip_prefix("+CGREG: ") {
        if let Some((n, status, lac, ci, act)) = parse_reg_response(rest) {
            return Some(ATResponse::CGREG { n, status, lac, ci, act });
        }
    }

    // +COPS: <mode>[,<format>,<oper>[,<act>]]
    if let Some(rest) = line.strip_prefix("+COPS: ") {
        if let Some(resp) = parse_cops_response(rest) {
            return Some(resp);
        }
    }

    // +CPIN: <status>
    if let Some(rest) = line.strip_prefix("+CPIN: ") {
        let status = SimStatus::from_str(rest);
        return Some(ATResponse::CPIN { status });
    }

    // +QIOPEN: <connectID>,<err>
    if let Some(rest) = line.strip_prefix("+QIOPEN: ") {
        if let Some((connect_id, err)) = parse_qiopen(rest) {
            return Some(ATResponse::QIOpen { connect_id, err });
        }
    }

    // +QISEND: <connectID>,<sent>,<acked>,<nacked>
    if let Some(rest) = line.strip_prefix("+QISEND: ") {
        let parts: Vec<&str> = rest.split(',').collect();
        if parts.len() >= 4 {
            let connect_id = parts[0].trim().parse().ok()?;
            let sent = parts[1].trim().parse().ok()?;
            let acked = parts[2].trim().parse().ok()?;
            let nacked = parts[3].trim().parse().ok()?;
            return Some(ATResponse::QISend { connect_id, sent, acked, nacked });
        }
    }

    None
}

/// Parse registration response: <n>,<stat>[,<lac>,<ci>[,<act>]]
fn parse_reg_response(s: &str) -> Option<(u8, RegStatus, Option<u16>, Option<u32>, Option<u8>)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() < 2 {
        return None;
    }

    let n = parts[0].trim().parse().ok()?;
    let status = RegStatus::from_u8(parts[1].trim().parse().ok()?);

    let lac = parts.get(2)
        .and_then(|s| parse_hex_or_dec(s.trim().trim_matches('"')));
    let ci = parts.get(3)
        .and_then(|s| parse_hex_or_dec_u32(s.trim().trim_matches('"')));
    let act = parts.get(4)
        .and_then(|s| s.trim().parse().ok());

    Some((n, status, lac, ci, act))
}

/// Parse COPS response: <mode>[,<format>,<oper>[,<act>]]
fn parse_cops_response(s: &str) -> Option<ATResponse> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.is_empty() {
        return None;
    }

    let mode = parts[0].trim().parse().ok()?;
    let format = parts.get(1).and_then(|s| s.trim().parse().ok());
    let operator = parts.get(2).map(|s| s.trim().trim_matches('"').to_string());
    let act = parts.get(3).and_then(|s| s.trim().parse().ok());

    Some(ATResponse::COPS { mode, format, operator, act })
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Parse hex (with or without 0x prefix) or decimal string to u16.
fn parse_hex_or_dec(s: &str) -> Option<u16> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()
    } else if s.chars().all(|c| c.is_ascii_hexdigit()) && s.len() == 4 {
        // Looks like hex without prefix (common in AT responses)
        u16::from_str_radix(s, 16).ok()
    } else {
        s.parse().ok()
    }
}

/// Parse hex (with or without 0x prefix) or decimal string to u32.
fn parse_hex_or_dec_u32(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else if s.chars().all(|c| c.is_ascii_hexdigit()) && s.len() >= 4 {
        // Looks like hex without prefix
        u32::from_str_radix(s, 16).ok()
    } else {
        s.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_csq() {
        let mut acc = ResponseAccumulator::new();
        acc.push("+CSQ: 18,99");
        acc.push("OK");

        match acc.finalize() {
            ATResponse::CSQ { rssi: 18, ber: 99 } => {}
            other => panic!("Expected CSQ, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_creg() {
        let mut acc = ResponseAccumulator::new();
        acc.push("+CREG: 2,1,\"00C3\",\"0001A2B3\",8");
        acc.push("OK");

        match acc.finalize() {
            ATResponse::CREG { n: 2, status: RegStatus::RegisteredHome, lac: Some(0x00C3), ci: Some(0x0001A2B3), act: Some(8) } => {}
            other => panic!("Expected CREG, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_cme_error() {
        let mut acc = ResponseAccumulator::new();
        acc.push("+CME ERROR: 10");

        match acc.finalize() {
            ATResponse::Error { code: Some(10), message: None } => {}
            other => panic!("Expected CME ERROR 10, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_urc_creg() {
        let urc = try_parse_urc("+CREG: 1").unwrap();
        match urc {
            URC::CREG { status: RegStatus::RegisteredHome, lac: None, ci: None, act: None } => {}
            other => panic!("Expected CREG URC, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_urc_qiurc_recv() {
        let urc = try_parse_urc("+QIURC: \"recv\",0").unwrap();
        match urc {
            URC::QIUrcRecv { connect_id: 0 } => {}
            other => panic!("Expected QIURC recv, got {:?}", other),
        }
    }
}
