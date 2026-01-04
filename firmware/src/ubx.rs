use core::fmt;
use std::sync::mpsc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use std::io::{self};

use thiserror::Error;

const UBX_POSECEF: &[u8] = &[
    0x01, 0x01, // Class, ID
    0x00, 0x00, // Payload length, as a little endian u16. 
    // add checksum to end.
];
const SYNC_CHARS: (u8, u8) = (0xB5, 0x62);

/// A UBX data packet, which may or not be valid (Validity can
/// be determined with the `.validate()` function).
#[derive(Debug)]
pub struct UBX {
    pub sync_chars: (u8, u8),
    pub class: u8,
    pub id: u8,
    pub len: u16,
    pub payload: Vec<u8>,
    pub checksum: (u8, u8)
}

impl UBX {
    /// Produces a Fletcher-8 checksum from `bytes`.
    fn fletcher8(header: &[u8], payload: &[u8]) -> (u8, u8) {
        let mut ck_a: u8 = 0;
        let mut ck_b: u8 = 0;
    
        for &byte in header.iter().chain(payload.iter()) {
            ck_a = ck_a.wrapping_add(byte);
            ck_b = ck_b.wrapping_add(ck_a);
        }
    
        (ck_a, ck_b)
    }

    /// Creates a UBX packet with valid `sync_chars` and `checksum` given the 
    /// contents as input.
    /// - `payload` can be a variable number of bytes, corresponding to the
    /// `length` field; up to `u16::MAX`.
    pub fn builder(class: u8, id: u8, length: u16, payload: Vec<u8>) -> Self {
        let length_bytes = length.to_le_bytes();
        let checksum = Self::fletcher8(&[class, id, length_bytes[0], length_bytes[1]], &payload);
        Self {
            sync_chars: SYNC_CHARS,
            class: class,
            id: id,
            len: length,
            payload: payload,
            checksum: checksum,
        }
    }

    /// Returns the UBX packet as a complete Vec<u8> in the same order
    /// it was recieved or would be sent.
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8 + self.payload.len());
        bytes.push(self.sync_chars.0);
        bytes.push(self.sync_chars.1);
        bytes.push(self.class);
        bytes.push(self.id);
        bytes.extend_from_slice(&self.len.to_le_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes.push(self.checksum.0);
        bytes.push(self.checksum.1);
        bytes
    }

    /// Uses `UBX.bytes()` to create a String of the bytes in hex
    /// joined with one space (" ").
    pub fn bytes_string(&self) -> String {
        self.bytes().iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
    }

    /// Validates the UBX using the Fletcher-8 algorithm.
    pub fn is_valid(&self) -> bool {
        let mut ck_a: u8 = 0;
        let mut ck_b: u8 = 0;

        ck_a = ck_a.wrapping_add(self.class);
        ck_b = ck_b.wrapping_add(ck_a);
        
        ck_a = ck_a.wrapping_add(self.id);
        ck_b = ck_b.wrapping_add(ck_a);

        let len_le_bytes = self.len.to_le_bytes();
        ck_a = ck_a.wrapping_add(len_le_bytes[0]);
        ck_b = ck_b.wrapping_add(ck_a);
        ck_a = ck_a.wrapping_add(len_le_bytes[1]);
        ck_b = ck_b.wrapping_add(ck_a);

        for byte in &self.payload {
            ck_a = ck_a.wrapping_add(*byte);
            ck_b = ck_b.wrapping_add(ck_a);
        }

        (ck_a, ck_b) == self.checksum
    }
}

/// An async producer of UBX packets. This will output packets indefinitely
/// on the `tx` line until it is hung up. Packets may be invalid; check 
/// validity with `packet.is_valid()`.
/// 
/// This function may get snagged for an indefinite period of time if an 
/// invalid packet is sent with an unreasonably large `length` field, i.e.,
/// 0xFF 0xFF, which is of course 65535 or u16::MAX. It would not stop
/// interpreting incoming data as payload data until it had recieved `length`
/// bytes as input. A watchdog is thus recommended for unreliable connections.
pub async fn receive_ubx_packets(
    reader: &mut ReadHalf<SerialStream>,
    tx: mpsc::Sender<UBX>,
) -> std::io::Result<()> {
    const BUFFER_SIZE: usize = 1024;
    let mut buffer = vec![0u8; BUFFER_SIZE];

    #[derive(PartialEq)]
    enum ParseState {
        AwaitingSync,
        AwaitingClass,
        AwaitingID,
        AwaitingLengthA,
        AwaitingLengthB,
        AwaitingPayload,
        AwaitingChecksumA,
        AwaitingChecksumB,
        Finished,
    }

    let mut state: ParseState = ParseState::AwaitingSync;
    let mut sync_chars: (u8, u8) = (0u8, 0u8);
    let mut class: u8 = 0u8;
    let mut id: u8 = 0u8;
    let mut length_a: u8 = 0u8; // The first byte of the length u16
    let mut length: u16 = 0u16;
    let mut payload: Vec<u8> = Vec::with_capacity(0);
    let mut checksum: (u8, u8) = (0u8, 0u8);

    loop {
        //! TODO: USE HARDWARE(/OS?) BUFFER TO DECREASE RETURNS TO THIS HANDLE.
        //! Hardware buffer: 64 bits.
        //tokio::time::sleep(time::Duration::from_millis(10));
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            println!("[recieve_ubx_packets]: No more serial data to parse.");
            return Ok(())
        }

        let mut buffer_slice = &buffer[..n];

        loop {
            if state == ParseState::AwaitingSync {
                const SYNC_CHAR_1: u8 = SYNC_CHARS.0;
                const SYNC_CHAR_2: u8 = SYNC_CHARS.1;
                let mut i = 0;
                let mut flag = true;
                while i < buffer_slice.len() && flag {
                    match buffer_slice[i] {
                        SYNC_CHAR_1 => {
                            sync_chars.0 = SYNC_CHAR_1;
                        },
                        SYNC_CHAR_2 => {
                            if sync_chars.0 == SYNC_CHAR_1 {
                                sync_chars.1 = SYNC_CHAR_2;
                                buffer_slice = &buffer_slice[(i + 1)..];
                                state = ParseState::AwaitingClass;
                                flag = false;
                            }
                        },
                        _ => {
                            sync_chars.0 = 0x00;
                            sync_chars.1 = 0x00;
                        },
                    }
                    i += 1;
                }
                if flag { break; }
            }

            if state == ParseState::AwaitingClass {
                if buffer_slice.len() == 0 { break; }
                class = buffer_slice[0];
                buffer_slice = &buffer_slice[1..];
                state = ParseState::AwaitingID
            }

            if state == ParseState::AwaitingID {
                if buffer_slice.len() == 0 { break; }
                id = buffer_slice[0];
                buffer_slice = &buffer_slice[1..];
                state = ParseState::AwaitingLengthA;
            }

            if state == ParseState::AwaitingLengthA {
                if buffer_slice.len() == 0 { break; }
                length_a = buffer_slice[0];
                buffer_slice = &buffer_slice[1..];
                state = ParseState::AwaitingLengthB;
            }

            if state == ParseState::AwaitingLengthB {
                if buffer_slice.len() == 0 { break; }
                length = u16::from_le_bytes([length_a, buffer_slice[0]]);
                buffer_slice = &buffer_slice[1..];
                state = ParseState::AwaitingPayload;

                payload = Vec::with_capacity(length as usize);
            }

            if state == ParseState::AwaitingPayload {
                if buffer_slice.len() == 0 { break; }
                if payload.len() + buffer_slice.len()  < length.into() {
                    payload.extend_from_slice(buffer_slice);
                    break;
                } else {
                    let cutoff = length as usize - payload.len();
                    payload.extend_from_slice(&buffer_slice[..cutoff]);
                    buffer_slice = &buffer_slice[cutoff..];
                    state = ParseState::AwaitingChecksumA;
                }
            }

            if state == ParseState::AwaitingChecksumA {
                if buffer_slice.len() == 0 { break; }
                checksum.0 = buffer_slice[0];
                buffer_slice = &buffer_slice[1..];
                state = ParseState::AwaitingChecksumB;
            }

            if state == ParseState::AwaitingChecksumB {
                if buffer_slice.len() == 0 { break; }
                checksum.1 = buffer_slice[0];
                buffer_slice = &buffer_slice[1..];
                state = ParseState::Finished;
            }

            let tmp_payload = payload;
            let ubx = UBX {
                sync_chars: sync_chars.clone(),
                class: class,
                id: id,
                len: length,
                payload: tmp_payload,
                checksum: checksum.clone(),
            };

            if tx.send(ubx).is_err() {
                println!("[recieve_ubx_packets]: Consumer dropped, stopping producer.");
                return Ok(())
            }

            state = ParseState::AwaitingSync;
            sync_chars = (0u8, 0u8);
            payload = Vec::with_capacity(0);            

            if buffer_slice.len() == 0 { break; }
        }
    }
}

#[derive(Error, Debug)]
pub enum UBXParseErr {
    #[error("The checksum {checksum:?} was invalid and could not be parsed.")]
    InvalidChecksum { checksum: (u8, u8), },

    #[error("The UBX packet {0:?} had a payload of size zero and could not be parsed; did you parse a poll request?")]
    ZeroPayload(UBX),

    #[error("The payload was of length {payload_len:?} and should have been of length {correct_len:?} and thus could not be parsed.")]
    InvalidPayloadLength { payload_len: usize, correct_len: usize, },

    #[error("The provided payload was invalid: {payload_as_string}")]
    InvalidPayload { payload_as_string: String, },

    #[error("UBX messages do not support this class (it is invalid): {0:X}")]
    InvalidClass(u8),

    #[error("This class of UBX message has not been implemented yet: {0:X}.")]
    UnimplementedClass(u8),

    #[error("This UBX message id is not supported by this parser: {0:X}.")] 
    UnsupportedID(u8),

    #[error("Invalid data: {reasoning:?}")]
    InvalidData { reasoning: String, },

    #[error("The configuration data was of invalid length: {payload_as_string}")]
    InvalidCfgData { payload_as_string: String, },

    #[error("Could not parse UBX CFG key {0:x}. It is either invalid or (...more likely) not described in this implementation.")]
    UnknownCfgKey(u32),

    #[error("UBX CFG key did have its specified value when parsing.")]
    NoCfgValue,
    
    // #[error("I/O error")]
    // IoError(#[from] std::io::Error), // Automatically convert std::io::Error
}

/// Represents the types of UBX messages that can be polled.
#[derive(Debug)]
pub enum UBXPoll {
    NAVPOSECEF,
    NAVPOSLLH,
    NAVPVT,
    NAVSVIN,

    CFGVALGET {
        layer: CfgLayer,
        /// You probably want this field just to be `0`.
        positon: u16,
        cfg_keys: Vec<ConfigKey>
    },
    CFGVALSET {
        ram: bool,
        bbr: bool,
        flash: bool,
        cfg_data: Vec<ConfigItem>
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum CfgLayer {
    RAM = 0,
    BBR = 1,
    Flash = 2,
    Default = 7,
}

impl TryFrom<u8> for CfgLayer {
    type Error = String;
    
    fn try_from(value: u8) -> Result<Self, String> {
        Ok(match value {
            0 => Self::RAM,
            1 => Self::BBR,
            2 => Self::Flash,
            7 => Self::Default,
            _ => return Err(format!("Invalid value produced when attempting to parse {} into CfgLayer.", value)),
        })
    }
}

impl UBXPoll {
    /// Match returns the associated `id` and `class` header bytes for the UBX
    /// packet that this `UBXPoll` describes.
    /// - Returns: `(u8, u8)` which represents `(class, id)`
    pub const fn get_id_and_class(&self) -> (u8, u8) {
        match self {
            Self::NAVPOSECEF =>       (0x01, 0x01),
            Self::NAVPOSLLH =>        (0x01, 0x02),
            Self::NAVPVT =>           (0x01, 0x07),
            Self::NAVSVIN =>          (0x01, 0x3B),

            Self::CFGVALGET { .. } => (0x06, 0x8B),
            Self::CFGVALSET { .. } => (0x06, 0x8A),
        }
    }

    /// Builds a poll UBX packet (no payload) from its specifed variant.
    /// 
    /// Avoid sending more than 64 `ConfigItem`s within `cfg_data` if using
    /// a `CFGVALGET`/`CFGVALSET`/`CFGVALDEL` message as this may cause issues
    /// within the reciever.
    /// ### Panics
    /// Will panic if the variant is of a CFG type with a `Vec<u8>` payload
    /// and `payload.len() > u16::MAX`
    pub fn build_ubx(&self) -> UBX {
        // Special pattern to catch CFG messages because they require more
        // internal structure than just an empty payload.
        match self {
            Self::CFGVALGET { layer, positon, cfg_keys } => return self.build_cfgvalget(*layer, *positon, cfg_keys),
            Self::CFGVALSET { ram, bbr, flash, cfg_data } => return self.build_cfgvalset(*ram, *bbr, *flash, cfg_data),
            _ => (),
        };
        
        let (class, id) = self.get_id_and_class();
        let checksum = UBX::fletcher8(&[class, id, 0x00, 0x00], &[]);
        UBX {
            sync_chars: (SYNC_CHARS.0, SYNC_CHARS.1),
            class: class,
            id: id,
            len: 0x0000,
            payload: Vec::with_capacity(0),
            checksum: checksum,
        }
    }

    /// ### Panics
    /// Will panic if the bitwise representation of cfg_key is greater than `u16::MAX - 3`. 
    /// 
    /// Recomended to avoid sending `cfg_key.len() > 64` as this may cause issues for the receiver.
    fn build_cfgvalget(&self, layer: CfgLayer, position: u16, cfg_keys: &Vec<ConfigKey>) -> UBX {
        // 8 -> expected bytes for an outgoing CFG_GET packet with one data block.
        let mut payload: Vec<u8> = Vec::with_capacity(8); 
        /// For incoming messages, this value is 0x01, but since this is outgoing, it is 0x00.
        const VERSION: u8 = 0x00; 
        payload.push(VERSION);
        payload.push(layer as u8);
        let pos_bytes = position.to_le_bytes();
        payload.push(pos_bytes[0]);
        payload.push(pos_bytes[1]);
        for key in cfg_keys {
            let bytes = (*key as u32).to_le_bytes();
            payload.extend(bytes);
        }

        let (class, id) = self.get_id_and_class();
        let length = payload.len();
        let length_bytes = length.to_le_bytes();
        let checksum = UBX::fletcher8(&[class, id, length_bytes[0], length_bytes[1]], &payload);

        UBX {
            sync_chars: (SYNC_CHARS.0, SYNC_CHARS.1),
            class: class,
            id: id,
            len: length.try_into().unwrap(),
            payload: payload,
            checksum: checksum,
        }
    }

    fn build_cfgvalset(&self, ram: bool, bbr: bool, flash: bool, cfg_data: &Vec<ConfigItem>) -> UBX {
        // 12 -> expected bytes from a CFG packet with one data block.
        let mut payload: Vec<u8> = Vec::with_capacity(12); 
        const VERSION: u8 = 0x00;
        payload.push(VERSION);

        println!("ram: {ram}, bbr: {bbr}, flash: {flash}");

        let layers = (ram as u8) | ((bbr as u8) << 1) | ((flash as u8) << 2);
        println!("layers: {:08b}", layers);
        
        payload.push(layers);
        const RESERVED: u8 = 0x00;
        payload.push(RESERVED);
        payload.push(RESERVED);

        for data in cfg_data {
            let bytes = data.as_bytes();
            payload.extend(bytes);
        }

        let (class, id) = self.get_id_and_class();
        let length = payload.len();
        let length_bytes = length.to_le_bytes();
        let checksum = UBX::fletcher8(&[class, id, length_bytes[0], length_bytes[1]], &payload);

        UBX {
            sync_chars: (SYNC_CHARS.0, SYNC_CHARS.1),
            class: class,
            id: id,
            len: length.try_into().unwrap(),
            payload: payload,
            checksum: checksum,
        }
    }
}

/// Represents data extracted from specific types of recieved UBX messages.
/// Uses a `UBX` struct as the source of data when building.
#[derive(Debug)]
pub enum UBXData {
    /// ### Field units:
    /// - `itow_timestamp`: ms
    /// - `ecef_x`: cm
    /// - `ecef_y`: cm
    /// - `ecef_z`: cm
    /// - `position_accuracy_estimate`: cm
    NAVPOSECEF {
        itow_timestamp: u32,
        ecef_x: i32,
        ecef_y: i32,
        ecef_z: i32,
        pos_accuracy: u32,
    },
    /// ### Field units:
    /// - `itow_timestamp`: ms
    /// - `longitude`: deg
    /// - `latitude`: deg
    /// - `height`: mm
    /// - `height_msl`: mm
    /// - `horiz_acc`: mm
    /// - `verti_acc`: mm
    NAVPOSLLH {
        itow_timestamp: u32,
        longitude: i32,
        latitude: i32,
        height: i32,
        height_msl: i32,
        horiz_acc: u32,
        verti_acc: u32,
    },
    /// ### Field units:
    /// Look it up yourself: https://cdn.sparkfun.com/assets/f/7/4/3/5/PM-15136.pdf.
    /// 
    /// Holy fuck.
    NAVPVT {
        itow_timestamp: u32,
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        min: u8,
        sec: u8,

        valid_date: bool,
        valid_time: bool,
        fully_resolved: bool,
        valid_mag: bool,

        time_acc: u32,
        nano: i32,

        fix_type: GNSSFixType,

        gnss_fix_ok: bool,
        differential_solution: bool,
        psm_state: PowerSaveModeState,
        heading_is_valid: bool,
        carrier_solution: CarrierPhaseRangeSolutionStatus,
        confirmed_date: bool,
        confirmed_time: bool,

        num_satellites_used: u8,
        latitude: i32,
        longitude: i32,
        height: i32,
        height_msl: i32,
        horiz_acc: u32,
        verti_acc: u32,
        velocity_north: i32,
        velocity_east: i32,
        velocity_down: i32,
        ground_speed: i32,
        heading_of_motion: i32,
        speed_acc: u32,
        heading_acc: u32,
        position_dop: u16,

        invalid_lon_lat_height_and_hmsl: bool,

        reserved_bits_79_5: [u8; 5],

        heading_of_vehicle: i32,
        magnetic_declination: i16,
        magnetic_declination_acc: u16, 
    },
    /// ### Survey-in Data
    /// This message contains information about survey-in parameters.
    /// 
    /// ### Field units:
    /// 
    /// Look it up yourself: https://cdn.sparkfun.com/assets/f/7/4/3/5/PM-15136.pdf.
    NAVSVIN {
        itow_timestamp: u32,
        duration: u32,
        mean_x: i32,
        mean_y: i32,
        mean_z: i32,
        mean_x_hp: i8,
        mean_y_hp: i8,
        mean_z_hp: i8,
        mean_acc: u32,
        observations_used: u32,
        valid: bool,
        active: bool,
    },
    /// A UBX-CFG-VALGET message, specifically the second variant which is the
    /// incoming one with key-value pairs for the retrieved data.
    /// 
    /// You will have to process the key-value pairs yourself and make meaning of them.
    /// 
    /// Usually you recieve them one at a time unless you are sending commands that need to all be applied at once.
    CFGVALGET {
        origin_layer: CfgLayer,
        position: u16,
        data: Vec<ConfigItem>,
    }
}

#[derive(Debug)]
pub enum GNSSFixType {
    NoFix,
    DeadReckoningOnly,
    Fix2D,
    Fix3D,
    GNSSAndDeadReckoning,
    OnlyTimeFix,
}

#[derive(Debug)]
pub enum PowerSaveModeState {
    NotActive,
    Enabled,
    Acquisition,
    Tracking,
    PowerOptimizedTracking,
    Inactive,
}

#[derive(Debug)]
pub enum CarrierPhaseRangeSolutionStatus {
    NoSolution,
    SolutionWithFloatingAmbiguities,
    SolutionWithFixedAmbiguities,
}

/// Parses an array slice of little endian bytes into a number using that type's
/// built in `from_le_bytes()`. Contains a compile-time check of the array slice's 
/// size instead of a runtime check, using an `unsafe` block.
macro_rules! parse_to {
    ($slice:expr, $ty:ty) => {{
        const SIZE: usize = std::mem::size_of::<$ty>();
        debug_assert!($slice.len() == SIZE, "Slice length must match target type size");
        // SAFETY: The caller guarantees that the slice has the correct size.
        unsafe { <$ty>::from_le_bytes(*($slice.as_ptr() as *const [u8; SIZE])) }
    }};
}

type ParseResult = Result<UBXData, UBXParseErr>;
// An `impl` block that pipes `ubx` structs into usable data.
impl UBXData {
    /// Takes a `UBX` packet and extracts information from it 
    pub fn process_raw_ubx(ubx: &UBX) -> ParseResult {
        if !ubx.is_valid() { return Err(UBXParseErr::InvalidChecksum { checksum: ubx.checksum, }); }
        if ubx.len as usize != ubx.payload.len() { 
            return Err(UBXParseErr::InvalidPayload { payload_as_string: format!("{:?}", ubx.payload) }); 
        }

        match ubx.class {
            0x01 => Self::nav_packet(ubx),
            0x02 => Err(UBXParseErr::UnimplementedClass(ubx.class)), // RXM
            0x04 => Err(UBXParseErr::UnimplementedClass(ubx.class)), // INF
            0x05 => Err(UBXParseErr::UnimplementedClass(ubx.class)), // ACK
            0x06 => Self::cfg_packet(ubx),
            0x09 => Err(UBXParseErr::UnimplementedClass(ubx.class)), // UPD
            0x0A => Err(UBXParseErr::UnimplementedClass(ubx.class)), // MON
            0x0D => Err(UBXParseErr::UnimplementedClass(ubx.class)), // TIM
            0x13 => Err(UBXParseErr::UnimplementedClass(ubx.class)), // MGA
            0x21 => Err(UBXParseErr::UnimplementedClass(ubx.class)), // LOG
            0x27 => Err(UBXParseErr::UnimplementedClass(ubx.class)), // SEC
            _ => Err(UBXParseErr::InvalidClass(ubx.class)),
        }
    }

    fn verify_payload_size(ubx: &UBX, expected_payload_size: usize) -> Result<(), UBXParseErr> {
        if ubx.payload.len() != expected_payload_size { 
            Err(UBXParseErr::InvalidPayloadLength {
                payload_len: ubx.payload.len(), correct_len: expected_payload_size 
            })
        } else {
            Ok(())
        }
    }

    fn nav_packet(ubx: &UBX) -> ParseResult {
        match ubx.id {
            0x01 => Self::nav_posecef(ubx),
            0x02 => Self::nav_posllh(ubx),
            0x07 => Self::nav_pvt(ubx),
            0x3B => Self::nav_svin(ubx),
            _ => Err(UBXParseErr::UnsupportedID(ubx.id)),
        }
    }

    fn nav_posecef(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 20;
        Self::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;
        
        Ok(UBXData::NAVPOSECEF { 
            itow_timestamp: parse_to!(&ubx.payload[0..4], u32), 
            ecef_x: parse_to!(&ubx.payload[4..8], i32), 
            ecef_y: parse_to!(&ubx.payload[8..12], i32), 
            ecef_z: parse_to!(&ubx.payload[12..16], i32), 
            pos_accuracy: parse_to!(&ubx.payload[16..20], u32),
        })
    }

    fn nav_posllh(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 28;
        Self::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;
        
        Ok(UBXData::NAVPOSLLH { 
            itow_timestamp: parse_to!(&ubx.payload[0..4], u32), 
            longitude: parse_to!(&ubx.payload[4..8], i32), 
            latitude: parse_to!(&ubx.payload[8..12], i32), 
            height: parse_to!(&ubx.payload[12..16], i32), 
            height_msl: parse_to!(&ubx.payload[16..20], i32), 
            horiz_acc:parse_to!(&ubx.payload[20..24], u32), 
            verti_acc: parse_to!(&ubx.payload[24..28], u32),
        })
    }

    fn nav_pvt(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 92;
        Self::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;
        
        let p = &ubx.payload;

        Ok(UBXData::NAVPVT { 
            itow_timestamp: parse_to!(&p[0..4], u32),
            year: parse_to!(&p[4..6], u16),
            month: p[6],
            day: p[7],
            hour: p[8],
            min: p[9],
            sec: p[10],

            valid_date: {
                p[11] & 0b0001 == 0b0001
            },
            valid_time: {
                p[11] & 0b0010 == 0b0010
            },
            fully_resolved: {
                p[11] & 0b0100 == 0b0100
            },
            valid_mag: {
                p[11] & 0b1000 == 0b1000
            },

            time_acc: parse_to!(&p[12..16], u32),
            nano: parse_to!(&p[16..20], i32),

            fix_type: {
                match p[20] {
                    0 => GNSSFixType::NoFix,
                    1 => GNSSFixType::DeadReckoningOnly,
                    2 => GNSSFixType::Fix2D,
                    3 => GNSSFixType::Fix3D,
                    4 => GNSSFixType::GNSSAndDeadReckoning,
                    5 => GNSSFixType::OnlyTimeFix,
                    _ => return Err(UBXParseErr::InvalidData { 
                        reasoning: format!("NAV-PVT fix_type invalid: {}", p[20]).to_string(),
                    }),
                }
            },

            gnss_fix_ok: {
                p[21] & 0b0001 == 0b0001
            },
            differential_solution: {
                p[21] & 0b0010 == 0b0001
            },
            psm_state: {
                let psm_state = (p[21] & 0b00011100) >> 2;
                match psm_state {
                    0 => PowerSaveModeState::NotActive,
                    1 => PowerSaveModeState::Enabled,
                    2 => PowerSaveModeState::Acquisition,
                    3 => PowerSaveModeState::Tracking,
                    4 => PowerSaveModeState::PowerOptimizedTracking,
                    5 => PowerSaveModeState::Inactive,
                    _ => return Err(UBXParseErr::InvalidData { 
                        reasoning: format!("NAV-PVT psm_state invalid; extracted psm_state: {:X}, full_byte: {:X}", psm_state, p[20]).to_string(),
                    }),
                }
            },
            heading_is_valid: {
                p[21] & 0b00100000 == 0b00100000
            },
            carrier_solution: {
                let carrier_solution = (p[21] & 0b11000000) >> 6;
                match carrier_solution {
                    0 => CarrierPhaseRangeSolutionStatus::NoSolution,
                    1 => CarrierPhaseRangeSolutionStatus::SolutionWithFixedAmbiguities,
                    2 => CarrierPhaseRangeSolutionStatus::SolutionWithFloatingAmbiguities,
                    _ => return Err(UBXParseErr::InvalidData { 
                        reasoning: format!("NAV-PVT carrier_solution invalid: extracted carrier_solution: {}, full byte: {}", carrier_solution, p[21]).to_string(),
                    }),                }
            },
            confirmed_date: {
                p[22] & 0b01000000 == 0b01000000
            },
            confirmed_time: {
                p[22] & 0b10000000 == 0b10000000
            },

            num_satellites_used: p[23],
            longitude: parse_to!(&p[24..28], i32),
            latitude: parse_to!(&p[28..32], i32),
            height: parse_to!(&p[32..36], i32),
            height_msl: parse_to!(&p[36..40], i32),
            horiz_acc: parse_to!(&p[40..44], u32),
            verti_acc: parse_to!(&p[44..48], u32),
            velocity_north: parse_to!(&p[48..52], i32),
            velocity_east: parse_to!(&p[52..56], i32),
            velocity_down: parse_to!(&p[56..60], i32),
            ground_speed: parse_to!(&p[60..64], i32),
            heading_of_motion: parse_to!(&p[64..68], i32),
            speed_acc: parse_to!(&p[68..72], u32),
            heading_acc: parse_to!(&p[72..76], u32),
            position_dop: parse_to!(&p[76..78], u16),

            invalid_lon_lat_height_and_hmsl: {
                p[78] == 0b0000_0001
            },

            reserved_bits_79_5: p[79..84].to_owned().try_into().unwrap(),

            heading_of_vehicle: parse_to!(&p[84..88], i32),
            magnetic_declination: parse_to!(&p[88..90], i16),
            magnetic_declination_acc: parse_to!(&p[90..92], u16),
        })
    }

    fn nav_svin(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 40;
        Self::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;
        
        let p = &ubx.payload;

        Ok(UBXData::NAVSVIN { 
            itow_timestamp: parse_to!(&p[4..8], u32), 
            duration: parse_to!(&p[8..12], u32), 
            mean_x: parse_to!(&p[12..16], i32), 
            mean_y: parse_to!(&p[16..20], i32), 
            mean_z: parse_to!(&p[20..24], i32), 
            mean_x_hp: p[24] as i8, 
            mean_y_hp: p[25] as i8, 
            mean_z_hp: p[26] as i8, 
            mean_acc: parse_to!(&p[28..32], u32), 
            observations_used: parse_to!(&p[32..36], u32), 
            valid: p[36] == 0x01, 
            active: p[37] == 0x01, 
        })
    }

    fn cfg_packet(ubx: &UBX) -> ParseResult {
        match ubx.id {
            0x8B => Self::cfg_valget_incoming(ubx),
            _ => Err(UBXParseErr::UnsupportedID(ubx.id)),
        }   
    }

    fn cfg_valget_incoming(ubx: &UBX) -> ParseResult {
        let valid_size = ubx.payload.len() >= 9;
        const VALGET_INCOMING_VERSION: u8 = 0x01;
        if (ubx.payload[0] != VALGET_INCOMING_VERSION) { 
            return Err(UBXParseErr::InvalidPayload { 
                payload_as_string: format!("Incoming version was invalid; version: {:02X?}, payload: {:?}", ubx.payload[0], ubx.payload)
            });
        }
        Ok(UBXData::CFGVALGET { 
            origin_layer: match CfgLayer::try_from(ubx.payload[1]) {
                Ok(layer) => layer,
                Err(e) => return Err(UBXParseErr::InvalidPayload { payload_as_string: 
                    format!("Incoming layer was invalid; layer: {:02X?}, payload: {:?}", ubx.payload[1], ubx.payload)
                }),
            },
            position: u16::from_be_bytes([ubx.payload[3], ubx.payload[4]]),
            data: ConfigItem::parse_config_data(&ubx.payload)? 
        })
    }
}

/// Represents a configuration data key-value pair, as can be sent with the 
/// `UBX-CFG-VALSET` and `UBX-CFG-VALGET` messages.
#[derive(Debug)]
pub struct ConfigItem {
    pub config_key: ConfigKey,
    pub config_val: ConfigValue,
}

impl ConfigItem {
    /// Returns the bitwise representation of this `ConfigItem` such that it 
    /// can be appended to a UBX `UBX-CFG-VALSET` message.
    pub fn as_bytes(&self) -> Vec<u8> {
        let mut bytes = (self.config_key as u32)
                        .to_le_bytes()
                        .to_vec();
        bytes.extend(self.config_val.get_bytes());
        bytes
    }

    /// Parses the payload of a UBX `UBX-CFG-VALGET` message, which is a 
    /// series of bitwise `ConfigItem`s with no padding.
    /// 
    /// Automatically starts on the fourth byte so you need not create another `vec`.
    pub fn parse_config_data(data: &Vec<u8>) -> Result<Vec<Self>, UBXParseErr> {
        macro_rules! checked_get_and_increment {
            ($vec:expr, $index:expr) => {{
                let value = match $vec.get($index) {
                    Some(value) => value,
                    None => return Err(UBXParseErr::NoCfgValue),
                };
                $index += 1;
                value
            }};
        }

        macro_rules! extract_cast_increment {
            ($vec:expr, $index:expr, $type:ty) => {{
                const LEN: usize = std::mem::size_of::<$type>();
                if $index + LEN > $vec.len() {
                    return Err(UBXParseErr::NoCfgValue);
                }
                let slice = &$vec[$index..($index + LEN)];
                $index += LEN;
                <$type>::from_le_bytes(unsafe { slice.try_into().unwrap_unchecked() })
            }};
        }

        let mut config_items: Vec<ConfigItem>  = Vec::new();
        let data_len = data.len();
        let mut i: usize = 4;
        loop {
            if i == data_len { break; }
            if (i + 4) >= data_len { return Err(UBXParseErr::InvalidCfgData { payload_as_string: format!("{:?}", data) })}
            
            let key: u32 = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            println!("parsing: {} {} {} {} into a key...", data[i], data[i + 1], data[i + 2], data[i + 3]);
            println!("parsed key: {}, a.k.a., {:02X?}", key, key);
            i += 4;
            let key_classification = ConfigKey::from_u32(key).ok_or(UBXParseErr::UnknownCfgKey(key))?;
            use ConfigKey::*;
            use ConfigValue::*;
            let value: ConfigValue = match key_classification {
                TmodeMode => E1(*checked_get_and_increment!(data, i)),
                TmodePosType => E1(*checked_get_and_increment!(data, i)),

                TmodeLat => I4(extract_cast_increment!(data, i, i32)),
                TmodeLon => I4(extract_cast_increment!(data, i, i32)),
                TmodeHeight => I4(extract_cast_increment!(data, i, i32)),

                TmodeLatHP => I1(extract_cast_increment!(data, i, i8)),
                TmodeLonHP => I1(extract_cast_increment!(data, i, i8)),
                TmodeHeightHP => I1(extract_cast_increment!(data, i, i8)),

                TmodeFixedPosAcc => U4(extract_cast_increment!(data, i, u32)),
                TmodeSurveyInMinDuration => U4(extract_cast_increment!(data, i, u32)),
                TmodeSurveyInAccLimit => U4(extract_cast_increment!(data, i, u32)),
            };

            config_items.push(ConfigItem { config_key: key_classification, config_val: value });
        }

        Ok(config_items)
    }
}

/// Generates the `ConfigKey` enum from a singular source of truth. Duplicate values
/// must be avoided or they may cause undefined behavior.
macro_rules! config_keys {
    (
        $(
            $(#[doc = $doc:expr])*
            $name:ident = $value:expr
        ),* $(,)?
    ) => {
        #[repr(u32)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ConfigKey {
            $(
                $(#[doc = $doc])*
                $name = $value,
            )*
        }

        impl ConfigKey {
            /// Converts a `u32` into the corresponding `ConfigKey`.
            pub fn from_u32(value: u32) -> Option<Self> {
                match value {
                    $(
                        $value => Some(Self::$name),
                    )*
                    _ => None,
                }
            }
        }
    };
}

config_keys!(
    /// ### CFG-TMODE-MODE (`E1`)
    /// Receiver mode
    /// - `0`: Disabled
    /// - `1`: Survey In
    /// - `2`: Fixed Mode (true ARP position information required)
    TmodeMode = 0x20030001,

    /// ### CFG-TMODE-POS_TYPE (`E1`)
    /// Determines whether the ARP position is given in ECEF or LAT/LON/HEIGHT.
    /// - `0`: Position is in ECEF
    /// - `1`: Position is Lat/Lon/Height
    TmodePosType = 0x20030002,

    /// ### CFG-TMODE-LAT (`I4`)
    /// Latitude of the ARP position.
    TmodeLat = 0x40030009,

    /// ### CFG-TMODE-LON (`I4`)
    /// Longitude of the ARP position.
    TmodeLon = 0x4003000a,

    /// ### CFG-TMODE-HEIGHT (`I4`)
    /// Height of the ARP position.
    TmodeHeight = 0x4003000b,

    /// ### CFG-TMODE-LAT_HP (`I1`)
    /// High-precision latitude of the ARP position.
    TmodeLatHP = 0x2003000c,

    /// ### CFG-TMODE-LON_HP (`I1`)
    /// High-precision longitude of the ARP position.
    TmodeLonHP = 0x2003000d,

    /// ### CFG-TMODE-HEIGHT_HP (`I1`)
    /// High-precision height of the ARP position.
    TmodeHeightHP = 0x2003000e,

    /// ### CFG-TMODE-FIXED_POS_ACC (`U4`)
    /// Fixed position 3D accuracy.
    TmodeFixedPosAcc = 0x4003000f,

    /// ### CFG-TMODE-SVIN_MIN_DUR (`U4`)
    /// Survey-in minimum duration. This will only be used if `CFG-TMODE-MODE=SURVEY_IN`.
    TmodeSurveyInMinDuration = 0x40030010,

    /// ### CFG-TMODE-SVIN_ACC_LIMIT (`U4`)
    /// Survey-in position accuracy limit. This will only be used if `CFG-TMODE-MODE=SURVEY_IN`.
    TmodeSurveyInAccLimit = 0x40030011,
);

/// Represents a configuration value, which depending on the associated configuration
/// key, can be either a single bit (which is sent as full byte with only the least 
/// signficant bit used), a byte, two bytes, four bytes, or eight bytes.
#[derive(Debug)]
pub enum ConfigValue {
    // unsigned little-endian integers:
    U1(u8),
    U2(u16),
    U4(u32),
    U8(u64),

    // signed little-endian, two's complement integers:
    I1(i8),
    I2(i16),
    I4(i32),
    I8(i64),

    // IEEE754 single (32-bit) and double (64-bit) precision floats:
    R4(f32),
    R8(f64),

    // unsigned little-endian enumerations:
    E1(u8),
    E2(u16),
    E4(u32),

    // unsigned little-endian integers for bitfields and other binary data, such as strings:
    X1(u8),
    X2(u16),
    X4(u32),
    X8(u64),

    // single-bit boolean (true = 1, false = 0), recieved/sent as a u1 but stored as a bool
    L(bool),
}

impl ConfigValue {
    /// Returns the value inside the enum as a `vec` little endian bytes.
    /// In accordance with the implimentation specifications, this `vec` will
    /// range between one and eight bytes.
    pub fn get_bytes(&self) -> Vec<u8> {
        // Yes, I could do a `macro_rules!` with a `$variant` match
        // or something but that's not absolutely runtime-safe.
        match &self {
            Self::I1(v) => v.to_le_bytes().to_vec(),
            Self::U1(v) => v.to_le_bytes().to_vec(),
            Self::I4(v) => v.to_le_bytes().to_vec(),
            Self::U2(v) => v.to_le_bytes().to_vec(),
            Self::R4(v) => v.to_le_bytes().to_vec(),
            Self::U4(v) => v.to_le_bytes().to_vec(),
            Self::E1(v) => v.to_le_bytes().to_vec(),
            Self::U8(v) => v.to_le_bytes().to_vec(),
            Self::E4(v) => v.to_le_bytes().to_vec(),
            Self::I2(v) => v.to_le_bytes().to_vec(),
            Self::X2(v) => v.to_le_bytes().to_vec(),
            Self::I8(v) => v.to_le_bytes().to_vec(),
            Self::X8(v) => v.to_le_bytes().to_vec(),
            Self::R8(v) => v.to_le_bytes().to_vec(),
            Self::E2(v) => v.to_le_bytes().to_vec(),
            Self::X1(v) => v.to_le_bytes().to_vec(),
            Self::X4(v) => v.to_le_bytes().to_vec(),
            Self::L(v) => vec![*v as u8],
        }
    }
}

/// A module for collecting various, typically constant-time geodetic functions.
mod geodesy {
    /// Converts Earth-Centered, Earth-Fixed (ECEF) coordinates to Latitude, Longitude, and Altitude (LLA).
    ///
    /// ### Arguments
    ///
    /// * `x` - ECEF X coordinate in meters.
    /// * `y` - ECEF Y coordinate in meters.
    /// * `z` - ECEF Z coordinate in meters.
    ///
    /// ### Returns
    ///
    /// A tuple `(latitude, longitude, altitude)`:
    /// - `latitude` (f64): Latitude in degrees.
    /// - `longitude` (f64): Longitude in degrees.
    /// - `altitude` (f64): Altitude above the WGS84 ellipsoid in meters.
    ///
    /// ### Example
    ///
    /// ```
    /// let (lat, lon, alt) = ecef_to_lla(1_000_000.0, 5_000_000.0, 3_000_000.0);
    /// println!("Latitude: {}, Longitude: {}, Altitude: {}", lat, lon, alt);
    /// ```
    ///
    /// ### Notes
    ///
    /// - The function assumes the WGS84 reference ellipsoid.
    /// - Altitude is relative to the ellipsoid, not mean sea level (MSL).
    /// - Latitude is computed iteratively for accuracy, with a convergence threshold of `1e-12`,
    /// or, in the highly unlikely case of not converging in `<~7` iterations, a maximum of `15`.
    fn ecef_to_lla(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        const A: f64 = 6378137.0; // WGS84 semi-major axis (meters)
        const F: f64 = 1.0 / 298.257223563; // WGS84 flattening
        const E2: f64 = 2.0 * F - F * F; // Square of eccentricity
    
        let lambda = y.atan2(x); // Longitude
    
        let mut p = (x * x + y * y).sqrt();
        let mut phi = (z / p).atan(); // Initial guess for latitude on sphere
        let mut phi_prev;
    
        const MAX_ITER: usize = 15; // Highly unlikely but a sanity check.
        let mut i = 0;
        loop {
            i += 1;
            phi_prev = phi;
            let sin_phi = phi.sin();
            let n = A / (1.0 - E2 * sin_phi * sin_phi).sqrt();
            phi = (z + E2 * n * sin_phi).atan2(p);
    
            if ((phi - phi_prev).abs() < 1e-12) || (i > MAX_ITER) {
                break;
            }
        }
    
        let sin_phi = phi.sin();
        let n = A / (1.0 - E2 * sin_phi * sin_phi).sqrt();
        let h = p / phi.cos() - n; // Altitude
    
        (phi.to_degrees(), lambda.to_degrees(), h)
    }
    
}