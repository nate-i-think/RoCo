use std::sync::mpsc::Receiver;
use std::{time::Duration};
use std::ops::{Add, Sub, Mul, Div};
use tokio::io::{Error, ErrorKind, AsyncWriteExt, WriteHalf};
use chrono::Local;
use tokio::sync::watch;
use tokio::time::sleep;
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use crate::io::ubx::{self, cfg::{ConfigItem, ConfigKey, ConfigValue}, UBX};

pub async fn begin_survey_in(
    serial_writer: &mut WriteHalf<SerialStream>, 
    min_duration: Duration, 
    accuracy_threshold: Distance, 
) -> tokio::io::Result<()> {
    let survey_in_frame = ubx::UBXPoll::CFGVALSET { 
        ram: true, 
        bbr: false, 
        flash: false, 
        cfg_data: vec![
            ConfigItem { 
                config_key: ConfigKey::TmodeMode,
                config_val: ConfigValue::E1(0x01), // SURVEY_IN
            },
            ConfigItem {
                config_key: ConfigKey::TmodeSurveyInMinDuration,
                config_val: ConfigValue::U4(min_duration.as_secs().clamp(0, u32::MAX.into()) as u32),
            },
            ConfigItem {
                config_key: ConfigKey::TmodeSurveyInAccLimit,
                config_val: ConfigValue::U4(accuracy_threshold.to_ubx_0_1mm()),
            }
        ],
    }.build_ubx().bytes();

    serial_writer.write_all(&survey_in_frame).await?;

    println!("[begin_survey_in] Started Survey In, which will complete at [{}]", 
        (Local::now() + 
            chrono::Duration::from_std(min_duration)
                .map_err(|e| Error::new(ErrorKind::InvalidInput, e))?
        )
        .format("%Y-%m-%d %H:%M:%S")
    );

    Ok(())
}

pub async fn fetch_survey_in_update(
    ubx_rx: &mut Receiver<UBX>, 
    serial_writer: &mut WriteHalf<SerialStream>
) -> tokio::io::Result<()> {
    let nav_svin_frame = ubx::UBXPoll::NAVSVIN.build_ubx().bytes();

    serial_writer.write_all(&nav_svin_frame).await?;

    let timeout = ubx_rx.recv_timeout(Duration::from_secs(1));

    match timeout {
        Ok(ubx_msg) => {
            if !ubx_msg.is_valid() { panic!("[fetch_survey_in_update]: Recieved an invalid UBX message!") }
            let data = ubx::UBXData::process_raw_ubx(&ubx_msg).unwrap();
            match data {
                ubx::UBXData::NAVSVIN { itow_timestamp, duration, mean_x, mean_y, mean_z, mean_x_hp, mean_y_hp, mean_z_hp, mean_acc, observations_used, valid, active } => {
                    println!("\n[fetch_survey_in_update] Survey in update recieved:");
                    println!("    ITOW:             {}", itow_timestamp);
                    println!("    observations:     {}", observations_used);
                    println!("    valid:            {}", valid);
                    println!("    active:           {}", active);
                    println!("    meanX_km:         {}", (((mean_x as f64) + (mean_x_hp as f64 * 0.01)) / 100_000.0));
                    println!("    meanY_km:         {}", (((mean_y as f64) + (mean_y_hp as f64 * 0.01)) / 100_000.0));
                    println!("    meanZ_km:         {}", (((mean_z as f64) + (mean_z_hp as f64 * 0.01)) / 100_000.0));
                    println!("    duration (s):     {}", duration);
                    println!("    mean_acc (mm):    {}", (mean_acc as f64 / 10.0));
                    Ok(())
                },
                _ => panic!("[fetch_survey_in_update]: Did not recieve proper packet type: {:?}", data),
            }
        },
        Err(e) => {
            panic!("Did not receieve any UBX message within the timeout of 1 second: \"{e}\".");
        }
    }
}

/// A length in meters (f64 precision), with convenient unit conversions.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Distance(f64);

impl Distance {
    /// Zero distance
    pub const fn zero() -> Self {
        Distance(0.0)
    }

    /// Create from meters
    pub fn from_meters(m: f64) -> Self {
        Distance(m)
    }

    /// Get in meters
    pub fn to_meters(self) -> f64 {
        self.0
    }

    /// Create from millimeters
    pub fn from_mm(mm: f64) -> Self {
        Distance(mm / 1_000.0)
    }

    /// Get in millimeters
    pub fn to_mm(self) -> f64 {
        self.0 * 1_000.0
    }

    /// Create from centimeters
    pub fn from_cm(cm: f64) -> Self {
        Distance(cm / 100.0)
    }

    /// Get in centimeters
    pub fn to_cm(self) -> f64 {
        self.0 * 100.0
    }

    /// Create from kilometers
    pub fn from_km(km: f64) -> Self {
        Distance(km * 1_000.0)
    }

    /// Get in kilometers
    pub fn to_km(self) -> f64 {
        self.0 / 1_000.0
    }

    /// Convert to the UBX “0.1 mm units” (i.e. mm × 10, rounded)
    pub fn to_ubx_0_1mm(self) -> u32 {
        (self.to_mm() * 10.0).round() as u32
    }
}

impl Add for Distance {
    type Output = Self;
    fn add(self, o: Self) -> Self { Distance(self.0 + o.0) }
}
impl Sub for Distance {
    type Output = Self;
    fn sub(self, o: Self) -> Self { Distance(self.0 - o.0) }
}
impl Mul<f64> for Distance {
    type Output = Self;
    fn mul(self, s: f64) -> Self { Distance(self.0 * s) }
}
impl Div<f64> for Distance {
    type Output = Self;
    fn div(self, s: f64) -> Self { Distance(self.0 / s) }
}

#[derive(Clone, Debug)]
pub struct GNSSFix {
    pub rtk_enabled: bool,
    pub fix_status: ubx::nav::GNSSFixType,
    pub unix_ms: u64,
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub speed_mps: f32,
    pub heading_deg: f32,
    pub horizontal_acc_mps: f32,
    pub vertical_acc_mps: f32,
}

impl Default for GNSSFix {
    fn default() -> Self {
        Self {
            rtk_enabled: false,
            fix_status: ubx::nav::GNSSFixType::NoFix,
            unix_ms: 0,
            lat_deg: 0.0,
            lon_deg: 0.0,
            speed_mps: 0.0,
            heading_deg: 0.0,
            horizontal_acc_mps: 0.0,
            vertical_acc_mps: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GNSSFixHandle {
    rx: watch::Receiver<GNSSFix>,
}

impl GNSSFixHandle {
    pub fn subscribe(&self) -> watch::Receiver<GNSSFix> {
        self.rx.clone()
    }

    pub fn latest(&self) -> GNSSFix {
        self.rx.borrow().clone()
    }
}

pub fn init_gnss_task() -> GNSSFixHandle {
    let (tx, rx) = watch::channel(GNSSFix::default());

    let port_name = "/dev/serial0"; // <--- CORRECT THIS 
    let baud_rate =  38_400; 

    let mut serial_port = tokio_serial::new(port_name, baud_rate)
        .data_bits(tokio_serial::DataBits::Eight)
        .flow_control(tokio_serial::FlowControl::None)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .timeout(Duration::from_secs(1))
        .open_native_async()
        .expect("The SerialStream should open, but failed:");

    tokio::spawn(gnss_task(tx, serial_port));

    GNSSFixHandle { 
        rx
    }
}

async fn gnss_task(tx: watch::Sender<GNSSFix>, mut serial_port: SerialStream) {
    loop {
        sleep(Duration::from_millis(1000)).await;
    }
}