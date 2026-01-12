use std::sync::mpsc;

use tokio::io::{AsyncReadExt, ReadHalf};
use tokio_serial::SerialStream;

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
/// 0xFFFF, which is of course 65535 or u16::MAX. It would not stop
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
        // TODO: USE HARDWARE(/OS?) BUFFER TO DECREASE RETURNS TO THIS HANDLE.
        // Hardware buffer: 64 bits.
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
    MONRF,

    NAVSTATUS,
    NAVDOP,
    NAVSIG,
    NAVSAT,
    NAVPOSECEF,
    NAVPOSLLH,
    NAVPVT,
    NAVSVIN,
    NAVRELPOSNED,

    RXMRTCM,

    SECSIG,

    CFGVALGET {
        layer: cfg::CfgLayer,
        /// You probably want this field just to be `0`.
        positon: u16,
        cfg_keys: Vec<cfg::ConfigKey>
    },
    CFGVALSET {
        ram: bool,
        bbr: bool,
        flash: bool,
        cfg_data: Vec<cfg::ConfigItem>
    }
}

impl UBXPoll {
    /// Match returns the associated `id` and `class` header bytes for the UBX
    /// packet that this `UBXPoll` describes.
    /// - Returns: `(u8, u8)` which represents `(class, id)`
    pub const fn get_id_and_class(&self) -> (u8, u8) {
        match self {
            Self::MONRF =>            (0x0A, 0x38),

            Self::NAVSTATUS =>        (0x01, 0x03),
            Self::NAVDOP =>           (0x01, 0x04),
            Self::NAVSIG =>           (0x01, 0x43),
            Self::NAVSAT =>           (0x01, 0x35),
            Self::NAVPOSECEF =>       (0x01, 0x01),
            Self::NAVPOSLLH =>        (0x01, 0x02),
            Self::NAVPVT =>           (0x01, 0x07),
            Self::NAVSVIN =>          (0x01, 0x3B),
            Self::NAVRELPOSNED =>     (0x01, 0x3C),

            Self::RXMRTCM =>          (0x02, 0x32),

            Self::SECSIG =>           (0x27, 0x09),


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
    fn build_cfgvalget(&self, layer: cfg::CfgLayer, position: u16, cfg_keys: &Vec<cfg::ConfigKey>) -> UBX {
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

    fn build_cfgvalset(&self, ram: bool, bbr: bool, flash: bool, cfg_data: &Vec<cfg::ConfigItem>) -> UBX {
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
    /// Message acknoweldged (Output).
    ACKACK {
        /// Class ID of the Acknowledged Message.
        class_id: u8,
        /// Message ID of the Acknowledged Message.
        message_id: u8,
    },

    ACKNAK {
        /// Class ID of the Not-Acknowledged Message.
        class_id: u8,
        /// Message ID of the Not-Acknowledged Message.
        message_id: u8,
    },

    /// A UBX-CFG-VALGET message, specifically the second variant which is the
    /// incoming one with key-value pairs for the retrieved data.
    /// 
    /// You will have to process the key-value pairs yourself and make meaning of them.
    /// 
    /// Usually you recieve them one at a time unless you are sending commands that need to all be applied at once.
    CFGVALGET {
        origin_layer: cfg::CfgLayer,
        position: u16,
        data: Vec<cfg::ConfigItem>,
    },

    /// Information for each RF block. There are as many RF 
    /// blocks reported as bands supported by this receiver.
    MONRF {
        version: u8,
        n_blocks: u8,
        rf_blocks: Vec<mon::RFBlock>,
    },

    /// Receiver navigation status.
    NAVSTATUS {
        itow_timestamp: u32,

        fix_type: nav::GNSSFixType,

        gps_fix_ok: bool,
        diff_soln: bool,
        week_number_is_valid: bool,
        time_of_week_is_valid: bool,

        diff_corrections_available: bool,
        carrier_solution_is_valid: bool,
        map_matching: nav::MapMatchingStatus,

        psm_state: nav::PowerSaveModeState,
        spoofing_detection_state: nav::SpoofingDetectionState,
        carrier_solution: nav::CarrierPhaseRangeSolutionStatus,

        /// Time to first fix (millisecond time tag)
        time_to_first_fix: u32,
        /// Milliseconds since startup / reset
        millis_since_startup: u32,
    },


    /// Dilution of precision.
    ///
    /// DOP values are dimensionless. All DOP values are scaled by
    /// a factor of 100 (e.g. 156 => 1.56).
    ///
    /// ### Field units:
    /// - `itow_timestamp`: ms
    /// - `g_dop`: 0.01 (dimensionless)
    /// - `p_dop`: 0.01 (dimensionless)
    /// - `t_dop`: 0.01 (dimensionless)
    /// - `v_dop`: 0.01 (dimensionless)
    /// - `h_dop`: 0.01 (dimensionless)
    /// - `n_dop`: 0.01 (dimensionless)
    /// - `e_dop`: 0.01 (dimensionless)
    NAVDOP {
        itow_timestamp: u32,
        g_dop: u16,
        p_dop: u16,
        t_dop: u16,
        v_dop: u16,
        h_dop: u16,
        n_dop: u16,
        e_dop: u16,
    },

    /// Signal information.
    ///
    /// This message displays information about signals currently
    /// tracked or searched by the receiver.
    ///
    /// ### Field units:
    /// - `itow_timestamp`: ms
    /// - `pr_residual`: 0.1 m
    /// - `cno`: dBHz
    NAVSIG {
        itow_timestamp: u32,
        version: u8,
        num_sigs: u8,
        signals: Vec<nav::SignalInfo>,
    },

    /// Satellite information.
    ///
    /// ### Field units:
    /// - `itow_timestamp`: ms
    /// - `svs[*].cno`: dBHz
    /// - `svs[*].elevation`: deg
    /// - `svs[*].azimuth`: deg
    /// - `svs[*].pseudorange_residual`: 0.1 m
    NAVSAT {
        itow_timestamp: u32,
        version: u8,
        num_svs: u8,
        svs: Vec<nav::SVInfo>,
    },

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

    /// High precision geodetic position solution.
    ///
    /// ### Field units:
    /// - `itow_timestamp`: ms
    /// - `longitude`: deg * 1e-7
    /// - `latitude`: deg * 1e-7
    /// - `height`: mm
    /// - `height_msl`: mm
    /// - `lon_hp`: deg * 1e-9
    /// - `lat_hp`: deg * 1e-9
    /// - `height_hp`: 0.1 mm
    /// - `height_msl_hp`: 0.1 mm
    /// - `horiz_acc`: 0.1 mm
    /// - `verti_acc`: 0.1 mm
    NAVHPPOSLLH {
        version: u8,
        invalid_llh: bool,

        itow_timestamp: u32,

        longitude: i32,
        latitude: i32,
        height: i32,
        height_msl: i32,

        lon_hp: i8,
        lat_hp: i8,
        height_hp: i8,
        height_msl_hp: i8,

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

        fix_type: nav::GNSSFixType,

        gnss_fix_ok: bool,
        differential_solution: bool,
        psm_state: nav::PowerSaveModeState,
        heading_is_valid: bool,
        carrier_solution: nav::CarrierPhaseRangeSolutionStatus,
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

    /// Relative positioning information in NED frame.
    ///
    /// This message contains the relative position vector from the
    /// reference station to the rover, including accuracy figures,
    /// in the local topological system defined at the reference
    /// station.
    ///
    /// ### Field units:
    /// - `itow_timestamp`: ms
    /// - `rel_pos_n`: cm
    /// - `rel_pos_e`: cm
    /// - `rel_pos_d`: cm
    /// - `rel_pos_length`: cm
    /// - `rel_pos_heading`: 1e-5 deg
    /// - `rel_pos_hp_n`: 0.1 mm
    /// - `rel_pos_hp_e`: 0.1 mm
    /// - `rel_pos_hp_d`: 0.1 mm
    /// - `rel_pos_hp_length`: 0.1 mm
    /// - `acc_n`: 0.1 mm
    /// - `acc_e`: 0.1 mm
    /// - `acc_d`: 0.1 mm
    /// - `acc_length`: 0.1 mm
    /// - `acc_heading`: 1e-5 deg
    NAVRELPOSNED {
        version: u8,
        ref_station_id: u16,
        itow_timestamp: u32,

        rel_pos_n: i32,
        rel_pos_e: i32,
        rel_pos_d: i32,
        rel_pos_length: i32,
        rel_pos_heading: i32,

        rel_pos_hp_n: i8,
        rel_pos_hp_e: i8,
        rel_pos_hp_d: i8,
        rel_pos_hp_length: i8,

        acc_n: u32,
        acc_e: u32,
        acc_d: u32,
        acc_length: u32,
        acc_heading: u32,

        gnss_fix_ok: bool,
        diff_soln: bool,
        rel_pos_valid: bool,
        carrier_solution: nav::CarrierPhaseRangeSolutionStatus,
        is_moving: bool,
        ref_pos_miss: bool,
        ref_obs_miss: bool,
        rel_pos_heading_valid: bool,
        rel_pos_normalized: bool,

        flags_raw: u32,
    },

    /// RTCM input status.
    ///
    /// This message shows info on a received RTCM input message.
    /// It is output upon successful parsing of an RTCM input
    /// message, irrespective of whether the RTCM message is
    /// supported or not by the receiver.
    RXMRTCM {
        /// Message version (0x02 for this version)
        version: u8,

        /// 0 when RTCM message received and passed CRC check, 1 when failed.
        /// When CRC failed, `ref_station` and `msg_type` might be corrupted.
        crc_failed: bool,
        /// RTCM message used successfully by the receiver.
        msg_used: rxm::MsgUsed,

        /// Message subtype, only applicable to u-blox proprietary RTCM
        /// message 4072 (not available on all products)
        sub_type: u16,
        /// Reference station ID
        ref_station: u16,
        /// Message type
        msg_type: u16,
    },

    /// Signal security information.
    ///
    /// Information related to the security, i.e. availability and
    /// integrity, of the signals.
    ///
    /// ### Field units:
    /// - `cent_freq_khz`: kHz (floored to nearest kHz multiple)
    SECSIG {
        version: u8,

        jam_det_enabled: bool,
        jam_state: sec::JammingState,

        spoof_det_enabled: bool,
        spoof_state: sec::SpoofingState,

        jam_num_cent_freqs: u8,
        jam_states: Vec<sec::JamStateCentFreq>,
    },
}

/// Parses an array slice of little endian bytes into a number using that type's
/// built in `from_le_bytes()`.
macro_rules! parse_to {
    ($slice:expr, $ty:ty) => {{
        const SIZE: usize = core::mem::size_of::<$ty>();
        debug_assert!(
            $slice.len() == SIZE,
            "Slice length must match target type size"
        );
        let mut bytes = [0u8; SIZE];
        bytes.copy_from_slice($slice);
        <$ty>::from_le_bytes(bytes)
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
            0x01 => Self::nav_packet(ubx),                           // NAV
            0x02 => Self::rxm_packet(ubx),                           // RXM
            0x04 => Err(UBXParseErr::UnimplementedClass(ubx.class)), // INF
            0x05 => Self::ack_packet(ubx),                           // ACK
            0x06 => Self::cfg_packet(ubx),                           // CFG
            0x09 => Err(UBXParseErr::UnimplementedClass(ubx.class)), // UPD
            0x0A => Self::mon_packet(ubx),                           // MON
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
            0x01 => nav::parse_posecef(ubx),
            0x02 => nav::parse_posllh(ubx),
            0x03 => nav::parse_status(ubx),
            0x04 => nav::parse_dop(ubx),
            0x07 => nav::parse_pvt(ubx),
            0x14 => nav::parse_hpposllh(ubx),
            0x35 => nav::parse_sat(ubx),
            0x3B => nav::parse_svin(ubx),
            0x3C => nav::parse_relposned(ubx),
            0x43 => nav::parse_sig(ubx),
            _ => Err(UBXParseErr::UnsupportedID(ubx.id)),
        }
    }

    fn ack_packet(ubx: &UBX) -> ParseResult {
        match ubx.id {
            0x00 => ack::parse_nak(ubx),
            0x01 => ack::parse_ack(ubx),
            _ => Err(UBXParseErr::UnsupportedID(ubx.id))
        }
    }

    fn cfg_packet(ubx: &UBX) -> ParseResult {
        match ubx.id {
            0x8B => cfg::cfg_valget_incoming(ubx),
            _ => Err(UBXParseErr::UnsupportedID(ubx.id)),
        }   
    }

    fn mon_packet(ubx: &UBX) -> ParseResult {
        match ubx.id {
            0x38 => mon::parse_rf(ubx),
            _ => Err(UBXParseErr::UnsupportedID(ubx.id)),
        }   
    }

    fn rxm_packet(ubx: &UBX) -> ParseResult {
        match ubx.id {
            0x32 => rxm::parse_rtcm(ubx),
            _ => Err(UBXParseErr::UnsupportedID(ubx.id)),
        }   
    }

    fn sec_packet(ubx: &UBX) -> ParseResult {
        match ubx.id {
            0x09 => sec::parse_sig(ubx),
            _ => Err(UBXParseErr::UnsupportedID(ubx.id)),
        }   
    }
}

/// A simple module which handles and processes UBX-ACK-XXXX messages,
/// the messages in the UBX-ACK class are used to indicate
/// acknowledgement or rejection of input messages (e.g. UBX-CFG).
pub mod ack {
    use super::{ParseResult, UBXData, UBX};

    pub fn parse_ack(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 2;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;

        let p = &ubx.payload;

        Ok(UBXData::ACKACK {
            class_id: p[0],
            message_id: p[1],
        })
    }

    pub fn parse_nak(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 2;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;

        let p = &ubx.payload;

        Ok(UBXData::ACKNAK {
            class_id: p[0],
            message_id: p[1],
        })
    }
}

/// A module which handles and processes UBX-CFG-XXXX messages.
/// Seperated for organization purposes because this codebase
/// is enormous.
pub mod cfg {
    use super::{UBXParseErr, ParseResult, UBXData, UBX};

    #[repr(u8)]
    #[derive(Debug, Clone, Copy)]
    pub enum CfgLayer {
        RAM = 0x00,
        BBR = 0x01,
        Flash = 0x02,
        Default = 0x07,
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

            let mut config_items: Vec<ConfigItem> = Vec::new();
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

    pub fn cfg_valget_incoming(ubx: &UBX) -> ParseResult {
        let valid_size = ubx.payload.len() >= 9;
        const VALGET_INCOMING_VERSION: u8 = 0x01;
        if ubx.payload[0] != VALGET_INCOMING_VERSION { 
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

/// A module which handles and processes UBX-MON-XXXX messages,
/// the messages in the UBX-MON class are used to report the 
/// receiver status, such as hardware status or I/O subsystem 
/// statistics.
pub mod mon {
    use num_enum::TryFromPrimitive;

    use super::{UBX, ParseResult, UBXData};

    pub fn parse_rf(ubx: &UBX) -> ParseResult {
        let p: &Vec<u8> = &ubx.payload;

        let version = p[0];
        let n_blocks = p[1] as usize;

        let expected_payload_size = 4 + n_blocks * 24;
        UBXData::verify_payload_size(ubx, expected_payload_size)?;

        let mut rf_blocks: Vec<RFBlock> = vec![];

        for k in 0..n_blocks {
            let i = 4 + k * 24;
            rf_blocks.push(RFBlock::parse(&p[i..(i+24)]));
        }

        Ok(UBXData::MONRF { version, n_blocks: p[1], rf_blocks })
    }

    #[derive(Debug)]
    pub struct RFBlock {
        /// RF block ID (0 = L1 band, 1 = L2 or L5 band depending 
        /// on product configuration)
        block_id: BlockIDBand,
        /// Depricated in version 0x02; use UBX-SEC-SIG instead.
        /// Will always report zero in 0x02.
        jamming_state: JammingState,
        /// Status of the antenna supervisor state machine.
        antenna_status: AntennaStatus,
        /// POST status word.
        post_status: u32,
        /// Noise level as measured by the GPS core.
        noise_per_ms: u16,
        /// AGC Monitor, as percentage of maximum gain, range 
        /// 0 to 8191 (100%).
        agc_count: u16,
        /// CW interference suppression level, scaled (0 = no CW
        /// jamming, 255 = strong CW jamming).
        cw_suppression: u8,
        /// Imbalance of I-part of complex signal, scaled (-128 = 
        /// max. negative imbalance, 127 = max. positive imbalance).
        offset_i: i8,
        /// Magnitude of I-part of complex signal, scaled (0 = no 
        /// signal, 255 = max.magnitude).
        magnitude_i: u8,
        /// Imbalance of Q-part of complex signal, scaled (-128 =
        /// max. negative imbalance, 127 = max. positive imbalance).
        offset_q: i8,
        /// Magnitude of Q-part of complex signal, scaled (0 = no 
        /// signal, 255 = max.magnitude).
        magnitude_q: u8,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum BlockIDBand {
        L1 = 0x00,
        L2orL5 = 0x01,
        Invalid = 0xFF,
    }

    #[derive(Debug)]
    pub enum JammingState {
        Unknown,
        NoSignficantJamming,
        WarningSomeJamming,
        CriticalInterferenceVisible,
        Depricated,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum AntennaStatus {
        Init = 0x00,
        DontKnow = 0x01,
        Okay = 0x02,
        Short = 0x03,
        Open = 0x04,
        Invalid = 0xFF,
    }

    impl RFBlock {
        fn parse(b: &[u8]) -> Self {
            Self {
                block_id: {
                    BlockIDBand::try_from_primitive(b[0]).unwrap_or(BlockIDBand::Invalid)
                },
                jamming_state: JammingState::Depricated,
                antenna_status: { 
                    AntennaStatus::try_from_primitive(b[2]).unwrap_or(AntennaStatus::Invalid)
                },
                post_status: parse_to!(&b[4..8], u32),
                // 9–12 reserved
                noise_per_ms: parse_to!(&b[12..14], u16),
                agc_count: parse_to!(&b[14..16], u16),
                cw_suppression: b[16],
                offset_i: b[17] as i8,
                magnitude_i: b[18],
                offset_q: b[19] as i8,
                magnitude_q: b[20],
            }
        }
    }
}

/// A module which handles and processes UBX-NAV-XXXX messages,
/// the messages in the UBX-NAV class are used to report the
/// navigation solution and related information.
pub mod nav {
    use num_enum::TryFromPrimitive;

    use super::{ParseResult, UBXData, UBXParseErr, UBX};

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive, Clone)]
    pub enum MapMatchingStatus {
        None = 0x00,
        ValidButNotUsed = 0x01,
        ValidAndUsed = 0x02,
        ValidAndUsedDeadReckoning = 0x03,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive, Clone)]
    pub enum SpoofingDetectionState {
        UnknownOrDeactivated = 0x00,
        NoSpoofingIndicated = 0x01,
        SpoofingIndicated = 0x02,
        MultipleSpoofingIndications = 0x03,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive, Clone)]
    pub enum GNSSFixType {
        NoFix = 0x00,
        DeadReckoningOnly = 0x01,
        Fix2D = 0x02,
        Fix3D = 0x03,
        GNSSAndDeadReckoning = 0x04,
        OnlyTimeFix = 0x05,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, Clone, TryFromPrimitive)]
    pub enum CarrierPhaseRangeSolutionStatus {
        NoSolution = 0x00,
        SolutionWithFloatingAmbiguities = 0x01,
        SolutionWithFixedAmbiguities = 0x02,
        Invalid = 0xFF,
    }

    #[derive(Debug, Clone)]
    pub enum PowerSaveModeState {
        NotActive,
        Enabled,
        Acquisition,
        Tracking,
        PowerOptimizedTracking,
        Inactive,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum GnssId {
        GPS = 0x00,
        SBAS = 0x01,
        Galileo = 0x02,
        BeiDou = 0x03,
        IMES = 0x04,
        QZSS = 0x05,
        GLONASS = 0x06,
        NavIC = 0x07,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum SignalQualityIndicator {
        NoSignal = 0x00,
        SearchingSignal = 0x01,
        SignalAcquired = 0x02,
        SignalDetectedButUnusable = 0x03,
        CodeLockedAndTimeSynchronized = 0x04,
        CodeAndCarrierLockedAndTimeSynchronized5 = 0x05,
        CodeAndCarrierLockedAndTimeSynchronized6 = 0x06,
        CodeAndCarrierLockedAndTimeSynchronized7 = 0x07,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum CorrectionSource {
        NoCorrections = 0x00,
        SBASCorrections = 0x01,
        BeiDouCorrections = 0x02,
        RTCM2Corrections = 0x03,
        RTCM3OSRCorrections = 0x04,
        RTCM3SSRCorrections = 0x05,
        QZSSSLASCorrections = 0x06,
        SPARTNCorrections = 0x07,
        CLASCorrections = 0x08,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum IonosphericModel {
        NoModel = 0x00,
        KlobucharModelTransmittedByGPS = 0x01,
        SBASModel = 0x02,
        KlobucharModelTransmittedByBeiDou = 0x03,
        IonoDelayDerivedFromDualFrequencyObservations = 0x08,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum SignalHealth {
        Unknown = 0x00,
        Healthy = 0x01,
        Unhealthy = 0x02,
        Invalid = 0xFF,
    }

        #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum GNSSIdentifier {
        GPS = 0x00,
        SBAS = 0x01,
        Galileo = 0x02,
        BeiDou = 0x03,
        IMES = 0x04,
        QZSS = 0x05,
        GLONASS = 0x06,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum QualityIndicator {
        NoSignal = 0x00,
        SearchingSignal = 0x01,
        SignalAcquired = 0x02,
        SignalDetectedButUnusable = 0x03,
        CodeLockedTimeSynchronized = 0x04,
        CodeAndCarrierLockedTimeSynchronized5 = 0x05,
        CodeAndCarrierLockedTimeSynchronized6 = 0x06,
        CodeAndCarrierLockedTimeSynchronized7 = 0x07,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum Health {
        Unknown = 0x00,
        Healthy = 0x01,
        Unhealthy = 0x02,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum OrbitSource {
        NoOrbitInfo = 0x00,
        Ephemeris = 0x01,
        Almanac = 0x02,
        AssistNowOffline = 0x03,
        AssistNowAutonomous = 0x04,
        OtherOrbitInfo5 = 0x05,
        OtherOrbitInfo6 = 0x06,
        OtherOrbitInfo7 = 0x07,
        Invalid = 0xFF,
    }

    pub fn parse_posecef(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 20;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;
        
        Ok(UBXData::NAVPOSECEF { 
            itow_timestamp: parse_to!(&ubx.payload[0..4], u32), 
            ecef_x: parse_to!(&ubx.payload[4..8], i32), 
            ecef_y: parse_to!(&ubx.payload[8..12], i32), 
            ecef_z: parse_to!(&ubx.payload[12..16], i32), 
            pos_accuracy: parse_to!(&ubx.payload[16..20], u32),
        })
    }

    pub fn parse_posllh(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 28;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;
        
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

    pub fn parse_pvt(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 92;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;
        
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
                p[21] & 0b0010 == 0b0010
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

    pub fn parse_svin(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 40;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;
        
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

    pub fn parse_status(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 16;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;

        let p = &ubx.payload;

        let flags = p[5];
        let fix_stat = p[6];
        let flags2 = p[7];

        Ok(UBXData::NAVSTATUS {
            itow_timestamp: parse_to!(&p[0..4], u32),

            fix_type: GNSSFixType::try_from_primitive(p[4]).unwrap_or(GNSSFixType::Invalid),

            gps_fix_ok: flags & 0b0000_0001 == 0b0000_0001,
            diff_soln: flags & 0b0000_0010 == 0b0000_0010,
            week_number_is_valid: flags & 0b0000_0100 == 0b0000_0100,
            time_of_week_is_valid: flags & 0b0000_1000 == 0b0000_1000,

            diff_corrections_available: fix_stat & 0b0000_0001 == 0b0000_0001,
            carrier_solution_is_valid: fix_stat & 0b0000_0010 == 0b0000_0010,
            map_matching: {
                let mm = (fix_stat & 0b1100_0000) >> 6;
                MapMatchingStatus::try_from_primitive(mm).unwrap_or(MapMatchingStatus::Invalid)
            },

            // NOTE: NAV-STATUS psmState meanings differ from NAV-PVT.
            psm_state: {
                let psm_state = flags2 & 0b0000_0011;
                match psm_state {
                    0 => PowerSaveModeState::Acquisition,
                    1 => PowerSaveModeState::Tracking,
                    2 => PowerSaveModeState::PowerOptimizedTracking,
                    3 => PowerSaveModeState::Inactive,
                    _ => {
                        return Err(UBXParseErr::InvalidData {
                            reasoning: format!(
                                "NAV-STATUS psm_state invalid: extracted: {}, full byte: {:02X}",
                                psm_state, flags2
                            )
                            .to_string(),
                        })
                    }
                }
            },

            spoofing_detection_state: {
                let s = (flags2 & 0b0001_1000) >> 3;
                SpoofingDetectionState::try_from_primitive(s)
                    .unwrap_or(SpoofingDetectionState::Invalid)
            },

            carrier_solution: {
                let carr_soln = (flags2 & 0b1100_0000) >> 6;
                CarrierPhaseRangeSolutionStatus::try_from_primitive(carr_soln)
                    .unwrap_or(CarrierPhaseRangeSolutionStatus::Invalid)
            },

            time_to_first_fix: parse_to!(&p[8..12], u32),
            millis_since_startup: parse_to!(&p[12..16], u32),
        })
    }

    pub fn parse_dop(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 18;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;

        let p = &ubx.payload;

        Ok(UBXData::NAVDOP {
            itow_timestamp: parse_to!(&p[0..4], u32),
            g_dop: parse_to!(&p[4..6], u16),
            p_dop: parse_to!(&p[6..8], u16),
            t_dop: parse_to!(&p[8..10], u16),
            v_dop: parse_to!(&p[10..12], u16),
            h_dop: parse_to!(&p[12..14], u16),
            n_dop: parse_to!(&p[14..16], u16),
            e_dop: parse_to!(&p[16..18], u16),
        })
    }

    pub fn parse_sig(ubx: &UBX) -> ParseResult {
        let p = &ubx.payload;

        let version = p[4];
        let num_sigs = p[5] as usize;

        let expected_payload_size = 8 + num_sigs * 16;
        UBXData::verify_payload_size(ubx, expected_payload_size)?;

        let mut signals: Vec<SignalInfo> = vec![];

        for k in 0..num_sigs {
            let i = 8 + k * 16;
            signals.push(SignalInfo::parse(&p[i..(i + 16)]));
        }

        Ok(UBXData::NAVSIG {
            itow_timestamp: parse_to!(&p[0..4], u32),
            version,
            num_sigs: p[5],
            signals,
        })
    }

    #[derive(Debug)]
    pub struct SignalInfo {
        /// GNSS identifier (see Satellite Numbering)
        pub gnss_id: GnssId,
        /// Satellite identifier (see Satellite Numbering)
        pub sv_id: u8,
        /// New style signal identifier (see Signal Identifiers)
        pub sig_id: u8,
        /// Only used for GLONASS: frequency slot + 7 (range 0..13)
        pub freq_id: u8,
        /// Pseudorange residual (0.1 m)
        pub pr_residual: i16,
        /// Carrier-to-noise density ratio (signal strength) (dBHz)
        pub cno: u8,
        /// Signal quality indicator
        pub quality_ind: SignalQualityIndicator,
        /// Correction source
        pub corr_source: CorrectionSource,
        /// Ionospheric model used
        pub iono_model: IonosphericModel,
        /// Signal health flag (from `sig_flags`)
        pub health: SignalHealth,
        /// Signal related flags (raw)
        pub sig_flags: u16,
    }

    impl SignalInfo {
        fn parse(b: &[u8]) -> Self {
            let sig_flags = parse_to!(&b[10..12], u16);
            let health_bits = (sig_flags & 0b0000_0000_0000_0011) as u8;

            Self {
                gnss_id: GnssId::try_from_primitive(b[0]).unwrap_or(GnssId::Invalid),
                sv_id: b[1],
                sig_id: b[2],
                freq_id: b[3],
                pr_residual: parse_to!(&b[4..6], i16),
                cno: b[6],
                quality_ind: SignalQualityIndicator::try_from_primitive(b[7])
                    .unwrap_or(SignalQualityIndicator::Invalid),
                corr_source: CorrectionSource::try_from_primitive(b[8])
                    .unwrap_or(CorrectionSource::Invalid),
                iono_model: IonosphericModel::try_from_primitive(b[9])
                    .unwrap_or(IonosphericModel::Invalid),
                health: SignalHealth::try_from_primitive(health_bits)
                    .unwrap_or(SignalHealth::Invalid),
                sig_flags,
            }
        }
    }

    pub fn parse_sat(ubx: &UBX) -> ParseResult {
        let p = &ubx.payload;

        let num_svs = p[5] as usize;
        let expected_payload_size = 8 + num_svs * 12;
        UBXData::verify_payload_size(ubx, expected_payload_size)?;

        let mut svs: Vec<SVInfo> = vec![];
        for k in 0..num_svs {
            let i = 8 + k * 12;
            svs.push(SVInfo::parse(&p[i..(i + 12)]));
        }

        Ok(UBXData::NAVSAT {
            itow_timestamp: parse_to!(&p[0..4], u32),
            version: p[4],
            num_svs: p[5],
            svs,
        })
    }

    #[derive(Debug)]
    pub struct SVInfo {
        /// GNSS identifier (see Satellite Numbering for assignment).
        pub gnss_id: GNSSIdentifier,
        /// Satellite identifier (see Satellite Numbering for assignment).
        pub sv_id: u8,
        /// Carrier to noise ratio (signal strength).
        pub cno: u8,
        /// Elevation (range: +/-90), unknown if out of range.
        pub elevation: i8,
        /// Azimuth (range 0-360), unknown if elevation is out of range.
        pub azimuth: i16,
        /// Pseudorange residual (0.1 m).
        pub pseudorange_residual: i16,

        /// Signal quality indicator.
        pub quality_indicator: QualityIndicator,
        /// Signal in the subset specified in Signal Identifiers is currently
        /// being used for navigation.
        pub sv_used: bool,
        /// Signal health flag.
        pub health: Health,
        /// Differential correction data is available for this SV.
        pub diff_corr: bool,
        /// Carrier smoothed pseudorange used.
        pub smoothed: bool,

        /// Orbit source.
        pub orbit_source: OrbitSource,
        /// Ephemeris is available for this SV.
        pub eph_available: bool,
        /// Almanac is available for this SV.
        pub alm_available: bool,
        /// AssistNow Offline data is available for this SV.
        pub ano_available: bool,
        /// AssistNow Autonomous data is available for this SV.
        pub aop_available: bool,

        /// SBAS corrections have been used for a signal in the subset
        /// specified in Signal Identifiers.
        pub sbas_corr_used: bool,
        /// RTCM corrections have been used for a signal in the subset
        /// specified in Signal Identifiers.
        pub rtcm_corr_used: bool,
        /// QZSS SLAS corrections have been used for a signal in the subset
        /// specified in Signal Identifiers.
        pub slas_corr_used: bool,
        /// SPARTN corrections have been used for a signal in the subset
        /// specified in Signal Identifiers.
        pub spartn_corr_used: bool,
        /// Pseudorange corrections have been used for a signal in the subset
        /// specified in Signal Identifiers.
        pub pr_corr_used: bool,
        /// Carrier range corrections have been used for a signal in the subset
        /// specified in Signal Identifiers.
        pub cr_corr_used: bool,
        /// Range rate (Doppler) corrections have been used for a signal in the subset
        /// specified in Signal Identifiers.
        pub do_corr_used: bool,
        /// CLAS corrections have been used for a signal in the subset
        /// specified in Signal Identifiers.
        pub clas_corr_used: bool,
    }

    impl SVInfo {
        fn parse(b: &[u8]) -> Self {
            let flags = parse_to!(&b[8..12], u32);

            Self {
                gnss_id: GNSSIdentifier::try_from_primitive(b[0]).unwrap_or(GNSSIdentifier::Invalid),
                sv_id: b[1],
                cno: b[2],
                elevation: b[3] as i8,
                azimuth: parse_to!(&b[4..6], i16),
                pseudorange_residual: parse_to!(&b[6..8], i16),

                quality_indicator: {
                    let q = (flags & 0x0000_0007) as u8;
                    QualityIndicator::try_from_primitive(q).unwrap_or(QualityIndicator::Invalid)
                },
                sv_used: (flags & 0x0000_0008) == 0x0000_0008,
                health: {
                    let h = ((flags & 0x0000_0030) >> 4) as u8;
                    Health::try_from_primitive(h).unwrap_or(Health::Invalid)
                },
                diff_corr: (flags & 0x0000_0040) == 0x0000_0040,
                smoothed: (flags & 0x0000_0080) == 0x0000_0080,

                orbit_source: {
                    let o = ((flags & 0x0000_0700) >> 8) as u8;
                    OrbitSource::try_from_primitive(o).unwrap_or(OrbitSource::Invalid)
                },
                eph_available: (flags & 0x0000_0800) == 0x0000_0800,
                alm_available: (flags & 0x0000_1000) == 0x0000_1000,
                ano_available: (flags & 0x0000_2000) == 0x0000_2000,
                aop_available: (flags & 0x0000_4000) == 0x0000_4000,

                sbas_corr_used: (flags & 0x0001_0000) == 0x0001_0000,
                rtcm_corr_used: (flags & 0x0002_0000) == 0x0002_0000,
                slas_corr_used: (flags & 0x0004_0000) == 0x0004_0000,
                spartn_corr_used: (flags & 0x0008_0000) == 0x0008_0000,
                pr_corr_used: (flags & 0x0010_0000) == 0x0010_0000,
                cr_corr_used: (flags & 0x0020_0000) == 0x0020_0000,
                do_corr_used: (flags & 0x0040_0000) == 0x0040_0000,
                clas_corr_used: (flags & 0x0080_0000) == 0x0080_0000,
            }
        }
    }

    pub fn parse_hpposllh(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 36;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;

        let p = &ubx.payload;

        Ok(UBXData::NAVHPPOSLLH {
            version: p[0],

            invalid_llh: {
                p[3] & 0b0000_0001 == 0b0000_0001
            },

            itow_timestamp: parse_to!(&p[4..8], u32),

            longitude: parse_to!(&p[8..12], i32),
            latitude: parse_to!(&p[12..16], i32),
            height: parse_to!(&p[16..20], i32),
            height_msl: parse_to!(&p[20..24], i32),

            lon_hp: p[24] as i8,
            lat_hp: p[25] as i8,
            height_hp: p[26] as i8,
            height_msl_hp: p[27] as i8,

            horiz_acc: parse_to!(&p[28..32], u32),
            verti_acc: parse_to!(&p[32..36], u32),
        })
    }

    pub fn parse_relposned(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 64;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;

        let p = &ubx.payload;

        let flags_raw = parse_to!(&p[60..64], u32);

        Ok(UBXData::NAVRELPOSNED {
            version: p[0],
            ref_station_id: parse_to!(&p[2..4], u16),
            itow_timestamp: parse_to!(&p[4..8], u32),

            rel_pos_n: parse_to!(&p[8..12], i32),
            rel_pos_e: parse_to!(&p[12..16], i32),
            rel_pos_d: parse_to!(&p[16..20], i32),
            rel_pos_length: parse_to!(&p[20..24], i32),
            rel_pos_heading: parse_to!(&p[24..28], i32),

            // 28..32 reserved.

            rel_pos_hp_n: p[32] as i8,
            rel_pos_hp_e: p[33] as i8,
            rel_pos_hp_d: p[34] as i8,
            rel_pos_hp_length: p[35] as i8,

            acc_n: parse_to!(&p[36..40], u32),
            acc_e: parse_to!(&p[40..44], u32),
            acc_d: parse_to!(&p[44..48], u32),
            acc_length: parse_to!(&p[48..52], u32),
            acc_heading: parse_to!(&p[52..56], u32),

            // 56..60 reserved.

            gnss_fix_ok: flags_raw & 0b0000_0000_0000_0000_0000_0000_0000_0001
                == 0b0000_0000_0000_0000_0000_0000_0000_0001,
            diff_soln: flags_raw & 0b0000_0000_0000_0000_0000_0000_0000_0010
                == 0b0000_0000_0000_0000_0000_0000_0000_0010,
            rel_pos_valid: flags_raw & 0b0000_0000_0000_0000_0000_0000_0000_0100
                == 0b0000_0000_0000_0000_0000_0000_0000_0100,

            carrier_solution: {
                let carr_soln = ((flags_raw >> 3) & 0b11) as u8;
                CarrierPhaseRangeSolutionStatus::try_from_primitive(carr_soln)
                    .unwrap_or(CarrierPhaseRangeSolutionStatus::Invalid)
            },

            is_moving: flags_raw & 0b0000_0000_0000_0000_0000_0000_0010_0000
                == 0b0000_0000_0000_0000_0000_0000_0010_0000,
            ref_pos_miss: flags_raw & 0b0000_0000_0000_0000_0000_0000_0100_0000
                == 0b0000_0000_0000_0000_0000_0000_0100_0000,
            ref_obs_miss: flags_raw & 0b0000_0000_0000_0000_0000_0000_1000_0000
                == 0b0000_0000_0000_0000_0000_0000_1000_0000,
            rel_pos_heading_valid: flags_raw & 0b0000_0000_0000_0000_0000_0001_0000_0000
                == 0b0000_0000_0000_0000_0000_0001_0000_0000,
            rel_pos_normalized: flags_raw & 0b0000_0000_0000_0000_0000_0010_0000_0000
                == 0b0000_0000_0000_0000_0000_0010_0000_0000,

            flags_raw,
        })
    }
}

/// A module which handles and processes UBX-RXM-XXXX messages,
/// the messages in the UBX-RXM class are used to report raw
/// receiver measurement and input status information.
pub mod rxm {
    use num_enum::TryFromPrimitive;

    use super::{ParseResult, UBXData, UBX};

    pub fn parse_rtcm(ubx: &UBX) -> ParseResult {
        const EXPECTED_PAYLOAD_SIZE: usize = 8;
        UBXData::verify_payload_size(ubx, EXPECTED_PAYLOAD_SIZE)?;

        let p = &ubx.payload;

        let flags = p[1];
        let msg_used = (flags & 0b0000_0110) >> 1;

        Ok(UBXData::RXMRTCM {
            version: p[0],

            crc_failed: flags & 0b0000_0001 == 0b0000_0001,
            msg_used: MsgUsed::try_from_primitive(msg_used).unwrap_or(MsgUsed::Invalid),

            sub_type: parse_to!(&p[2..4], u16),
            ref_station: parse_to!(&p[4..6], u16),
            msg_type: parse_to!(&p[6..8], u16),
        })
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum MsgUsed {
        DoNotKnow = 0x00,
        NotUsed = 0x01,
        UsedSuccessfully = 0x02,
        Invalid = 0xFF,
    }
}

/// A module which handles and processes UBX-SEC-XXXX messages,
/// the messages in the UBX-SEC class are used to report security
/// related information.
pub mod sec {
    use num_enum::TryFromPrimitive;

    use super::{ParseResult, UBXData, UBX};

    pub fn parse_sig(ubx: &UBX) -> ParseResult {
        let p = &ubx.payload;

        // Need at least the fixed header to read jam_num_cent_freqs.
        UBXData::verify_payload_size(ubx, 4)?;

        let version = p[0];
        let sig_sec_flags = p[1];
        let jam_num_cent_freqs = p[3] as usize;

        let expected_payload_size = 4 + jam_num_cent_freqs * 4;
        UBXData::verify_payload_size(ubx, expected_payload_size)?;

        let jam_det_enabled = sig_sec_flags & 0b0000_0001 == 0b0000_0001;
        let jam_state = {
            let jam_state = (sig_sec_flags & 0b0000_0110) >> 1;
            JammingState::try_from_primitive(jam_state).unwrap_or(JammingState::Invalid)
        };

        let spoof_det_enabled = sig_sec_flags & 0b0000_1000 == 0b0000_1000;
        let spoof_state = {
            let spoof_state = (sig_sec_flags & 0b0111_0000) >> 4;
            SpoofingState::try_from_primitive(spoof_state).unwrap_or(SpoofingState::Invalid)
        };

        let mut jam_states: Vec<JamStateCentFreq> = vec![];
        for k in 0..jam_num_cent_freqs {
            let i = 4 + k * 4;
            jam_states.push(JamStateCentFreq::parse(&p[i..(i + 4)]));
        }

        Ok(UBXData::SECSIG {
            version,

            jam_det_enabled,
            jam_state,

            spoof_det_enabled,
            spoof_state,

            jam_num_cent_freqs: p[3],
            jam_states,
        })
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum JammingState {
        Unknown = 0x00,
        NoJammingIndicated = 0x01,
        WarningJammingIndicated = 0x02,
        Invalid = 0xFF,
    }

    #[repr(u8)]
    #[derive(Debug, TryFromPrimitive)]
    pub enum SpoofingState {
        Unknown = 0x00,
        NoSpoofingIndicated = 0x01,
        SpoofingIndicated = 0x02,
        SpoofingAffirmed = 0x03,
        Invalid = 0xFF,
    }

    #[derive(Debug)]
    pub struct JamStateCentFreq {
        /// Center frequency (kHz), floored to the nearest kHz multiple.
        pub cent_freq_khz: u32,
        /// Flag indicates whether signals on the given center frequency
        /// are considered jammed.
        pub jammed: bool,
        /// Raw `jamStateCent` word.
        pub raw: u32,
    }

    impl JamStateCentFreq {
        fn parse(b: &[u8]) -> Self {
            let raw = parse_to!(&b[0..4], u32);

            Self {
                cent_freq_khz: raw & 0x00FF_FFFF,
                jammed: raw & 0x0100_0000 == 0x0100_0000,
                raw,
            }
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
    
        let p = (x * x + y * y).sqrt();
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