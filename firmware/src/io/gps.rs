use std::{time::Duration};
use std::sync::Arc;
use std::ops::{Add, Sub, Mul, Div};
use tokio::io::{Error, ErrorKind, AsyncWriteExt, WriteHalf};
use chrono::Local;
use tokio::sync::watch;
use tokio::time::{self, MissedTickBehavior};
use tokio_serial::{SerialPort, SerialPortBuilderExt, SerialStream};

use super::ubx::{UBXData, UBXHub, UBXPoll};
use super::ubx::{self, cfg::{ConfigItem, ConfigKey, ConfigValue}};

// pub async fn begin_survey_in(
//     serial_writer: &mut WriteHalf<SerialStream>, 
//     min_duration: Duration, 
//     accuracy_threshold: Distance, 
// ) -> tokio::io::Result<()> {
//     let survey_in_frame = ubx::UBXPoll::CFGVALSET { 
//         ram: true, 
//         bbr: false, 
//         flash: false, 
//         cfg_data: vec![
//             ConfigItem { 
//                 config_key: ConfigKey::TmodeMode,
//                 config_val: ConfigValue::E1(0x01), // SURVEY_IN
//             },
//             ConfigItem {
//                 config_key: ConfigKey::TmodeSurveyInMinDuration,
//                 config_val: ConfigValue::U4(min_duration.as_secs().clamp(0, u32::MAX.into()) as u32),
//             },
//             ConfigItem {
//                 config_key: ConfigKey::TmodeSurveyInAccLimit,
//                 config_val: ConfigValue::U4(accuracy_threshold.to_ubx_0_1mm()),
//             }
//         ],
//     }.build_ubx().bytes();

//     serial_writer.write_all(&survey_in_frame).await?;

//     println!("[begin_survey_in] Started Survey In, which will complete at [{}]", 
//         (Local::now() + 
//             chrono::Duration::from_std(min_duration)
//                 .map_err(|e| Error::new(ErrorKind::InvalidInput, e))?
//         )
//         .format("%Y-%m-%d %H:%M:%S")
//     );

//     Ok(())
// }

// pub async fn fetch_survey_in_update(
//     ubx_rx: &mut std::sync::mpsc::Receiver<UBX>, 
//     serial_writer: &mut WriteHalf<SerialStream>
// ) -> tokio::io::Result<()> {
//     let nav_svin_frame = ubx::UBXPoll::NAVSVIN.build_ubx().bytes();
//     serial_writer.write_all(&nav_svin_frame).await?;
//     let timeout = ubx_rx.recv_timeout(Duration::from_secs(1));
//     match timeout {
//         Ok(ubx_msg) => {
//             if !ubx_msg.is_valid() { panic!("[fetch_survey_in_update]: Recieved an invalid UBX message!") }
//             let data = ubx::UBXData::process_raw_ubx(&ubx_msg).unwrap();
//             match data {
//                 ubx::UBXData::NAVSVIN { itow_timestamp, duration, mean_x, mean_y, mean_z, mean_x_hp, mean_y_hp, mean_z_hp, mean_acc, observations_used, valid, active } => {
//                     println!("\n[fetch_survey_in_update] Survey in update recieved:");
//                     println!("    ITOW:             {}", itow_timestamp);
//                     println!("    observations:     {}", observations_used);
//                     println!("    valid:            {}", valid);
//                     println!("    active:           {}", active);
//                     println!("    meanX_km:         {}", (((mean_x as f64) + (mean_x_hp as f64 * 0.01)) / 100_000.0));
//                     println!("    meanY_km:         {}", (((mean_y as f64) + (mean_y_hp as f64 * 0.01)) / 100_000.0));
//                     println!("    meanZ_km:         {}", (((mean_z as f64) + (mean_z_hp as f64 * 0.01)) / 100_000.0));
//                     println!("    duration (s):     {}", duration);
//                     println!("    mean_acc (mm):    {}", (mean_acc as f64 / 10.0));
//                     Ok(())
//                 },
//                 _ => panic!("[fetch_survey_in_update]: Did not recieve proper packet type: {:?}", data),
//             }
//         },
//         Err(e) => {
//             panic!("Did not receieve any UBX message within the timeout of 1 second: \"{e}\".");
//         }
//     }
// }

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
// gps.rs


/// A snapshot of the latest GNSS messages.
///
/// Note: fields are `Arc<T>` so cloning a GNSSFix is cheap (just Arc clones),
/// and call sites remain ergonomic (`fix.nav_pvt.ground_speed`, etc.).
#[derive(Clone, Debug)]
pub struct GNSSFix {
    pub nav_pvt:       Arc<ubx::nav::NAVPVT>,
    pub nav_relposned: Arc<ubx::nav::NAVRELPOSNED>,
    pub nav_sig:       Arc<ubx::nav::NAVSIG>,
    pub nav_dop:       Arc<ubx::nav::NAVDOP>,
    pub nav_hpposllh:  Arc<ubx::nav::NAVHPPOSLLH>,
    pub mon_rf:        Arc<ubx::mon::MONRF>,
    pub sec_sig:       Arc<ubx::sec::SECSIG>,
}

impl Default for GNSSFix {
    fn default() -> Self {
        Self {
            nav_pvt:       Arc::new(ubx::nav::NAVPVT::default()),
            nav_relposned: Arc::new(ubx::nav::NAVRELPOSNED::default()),
            nav_sig:       Arc::new(ubx::nav::NAVSIG::default()),
            nav_dop:       Arc::new(ubx::nav::NAVDOP::default()),
            nav_hpposllh:  Arc::new(ubx::nav::NAVHPPOSLLH::default()),
            mon_rf:        Arc::new(ubx::mon::MONRF::default()),
            sec_sig:       Arc::new(ubx::sec::SECSIG::default()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GNSSFixHandle {
    rx: watch::Receiver<GNSSFix>,
}

impl GNSSFixHandle {
    /// Returns a new Receiver subscribed to the GNSSFix stream.
    pub fn subscribe(&self) -> watch::Receiver<GNSSFix> {
        self.rx.clone()
    }

    /// Clones the latest GNSSFix (cheap: only Arc clones).
    pub fn latest(&self) -> GNSSFix {
        self.rx.borrow().clone()
    }

    /// Borrow the latest fix without cloning; do not hold across `.await`.
    pub fn borrow(&self) -> watch::Ref<'_, GNSSFix> {
        self.rx.borrow()
    }

    /// Helper to safely read values with a closure (borrow-first).
    pub fn with_latest<R>(&self, f: impl FnOnce(&GNSSFix) -> R) -> R {
        let fix = self.rx.borrow();
        f(&*fix)
    }
}

/// Returned handles so callers can both read fixes and issue UBX requests.
#[derive(Clone)]
pub struct GNSSHandles {
    pub fix: GNSSFixHandle,
    pub ubx: UBXHub,
}

pub async fn init_gnss_task() -> GNSSHandles {
    const PORT_NAME: &str = "/dev/serial0";
    // ZED-F9P default after power cycle (RAM config only).
    const START_BAUD_RATE: u32 = 38_400;
    const RUNNING_BAUD_RATE: u32 = 230_400;

    let (fix_tx, fix_rx) = watch::channel(GNSSFix::default());

    let mut serial_port = tokio_serial::new(PORT_NAME, START_BAUD_RATE)
        .data_bits(tokio_serial::DataBits::Eight)
        .flow_control(tokio_serial::FlowControl::None)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .timeout(Duration::from_secs(1))
        .open_native_async()
        .expect("SerialStream open failed");

    tracing::info!(
        target: "InitGNSSTask",
        "SerialStream opened at {} baud on {}.",
        START_BAUD_RATE,
        PORT_NAME
    );

    // Configure UART1 baud via CFG-VALSET (RAM only), then switch host baud.
    let cfg_uart1_baud = UBXPoll::CFGVALSET {
        ram: true,
        bbr: false,
        flash: false,
        cfg_data: vec![ubx::cfg::ConfigItem {
            config_key: ubx::cfg::ConfigKey::UART1BaudRate,
            config_val: ubx::cfg::ConfigValue::U4(RUNNING_BAUD_RATE),
        }],
    }
    .build_ubx()
    .bytes();

    serial_port
        .write_all(&cfg_uart1_baud)
        .await
        .expect("Failed to write CFG-VALSET for baud");

    // Per manual: let baud change settle.
    tokio::time::sleep(Duration::from_millis(100)).await;

    serial_port
        .set_baud_rate(RUNNING_BAUD_RATE)
        .expect("Failed to set host baud rate");

    tracing::info!(
        target: "InitGNSSTask",
        "SerialStream configured to {} baud.",
        RUNNING_BAUD_RATE
    );

    let (serial_reader, serial_writer) = tokio::io::split(serial_port);

    // Spawn UBXHub (dedicated writer + reader/parser/router).
    let hub = UBXHub::spawn(serial_reader, serial_writer);

    // Spawn periodic poll task.
    tokio::spawn(gnss_poll_task(hub.clone()));

    // Spawn fix aggregation task.
    tokio::spawn(gnss_fix_task(fix_tx, hub.clone()));

    GNSSHandles {
        fix: GNSSFixHandle { rx: fix_rx },
        ubx: hub,
    }
}

async fn gnss_poll_task(hub: UBXHub) {
    // Prebuild frames once (avoid repeated Vec allocations).
    let nav_pvt_frame      = Arc::<[u8]>::from(UBXPoll::NAVPVT.build_ubx().bytes());
    let nav_relpos_frame   = Arc::<[u8]>::from(UBXPoll::NAVRELPOSNED.build_ubx().bytes());
    let nav_dop_frame      = Arc::<[u8]>::from(UBXPoll::NAVDOP.build_ubx().bytes());
    let nav_sig_frame      = Arc::<[u8]>::from(UBXPoll::NAVSIG.build_ubx().bytes());
    let nav_hppos_frame    = Arc::<[u8]>::from(UBXPoll::NAVHPPOSLLH.build_ubx().bytes());
    let mon_rf_frame       = Arc::<[u8]>::from(UBXPoll::MONRF.build_ubx().bytes());
    let sec_sig_frame      = Arc::<[u8]>::from(UBXPoll::SECSIG.build_ubx().bytes());

    // Keep your cadence; tune later.
    const SLOW_DOWN: u64 = 2;

    let mut nav_pvt_interval     = time::interval(Duration::from_millis(1000 * SLOW_DOWN));
    let mut nav_relpos_interval  = time::interval(Duration::from_millis(2000 * SLOW_DOWN));
    let mut nav_dop_interval     = time::interval(Duration::from_millis(10_000 * SLOW_DOWN));
    let mut nav_sig_interval     = time::interval(Duration::from_millis(5_000 * SLOW_DOWN));
    let mut nav_hppos_interval   = time::interval(Duration::from_millis(1000 * SLOW_DOWN));
    let mut mon_rf_interval      = time::interval(Duration::from_millis(15_000 * SLOW_DOWN));
    let mut sec_sig_interval     = time::interval(Duration::from_millis(30_000 * SLOW_DOWN));

    nav_pvt_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    nav_relpos_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    nav_dop_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    nav_sig_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    nav_hppos_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    mon_rf_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    sec_sig_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;

            _ = nav_pvt_interval.tick() => {
                if let Err(e) = hub.send_frame_arc(nav_pvt_frame.clone()).await {
                    tracing::warn!(target: "GNSSPoll", "NAVPVT poll send failed: {:?}", e);
                }
            }

            _ = nav_relpos_interval.tick() => {
                if let Err(e) = hub.send_frame_arc(nav_relpos_frame.clone()).await {
                    tracing::warn!(target: "GNSSPoll", "NAVRELPOSNED poll send failed: {:?}", e);
                }
            }

            _ = nav_dop_interval.tick() => {
                if let Err(e) = hub.send_frame_arc(nav_dop_frame.clone()).await {
                    tracing::warn!(target: "GNSSPoll", "NAVDOP poll send failed: {:?}", e);
                }
            }

            _ = nav_sig_interval.tick() => {
                if let Err(e) = hub.send_frame_arc(nav_sig_frame.clone()).await {
                    tracing::warn!(target: "GNSSPoll", "NAVSIG poll send failed: {:?}", e);
                }
            }

            _ = nav_hppos_interval.tick() => {
                if let Err(e) = hub.send_frame_arc(nav_hppos_frame.clone()).await {
                    tracing::warn!(target: "GNSSPoll", "NAVHPPOSLLH poll send failed: {:?}", e);
                }
            }

            _ = mon_rf_interval.tick() => {
                if let Err(e) = hub.send_frame_arc(mon_rf_frame.clone()).await {
                    tracing::warn!(target: "GNSSPoll", "MONRF poll send failed: {:?}", e);
                }
            }

            _ = sec_sig_interval.tick() => {
                if let Err(e) = hub.send_frame_arc(sec_sig_frame.clone()).await {
                    tracing::warn!(target: "GNSSPoll", "SECSIG poll send failed: {:?}", e);
                }
            }
        }
    }
}

async fn gnss_fix_task(tx: watch::Sender<GNSSFix>, hub: UBXHub) {
    // Subscribe to UBXHub topics once.
    let mut nav_pvt_rx     = hub.topic(UBXPoll::NAVPVT).subscribe();
    let mut nav_relpos_rx  = hub.topic(UBXPoll::NAVRELPOSNED).subscribe();
    let mut nav_dop_rx     = hub.topic(UBXPoll::NAVDOP).subscribe();
    let mut nav_sig_rx     = hub.topic(UBXPoll::NAVSIG).subscribe();
    let mut nav_hppos_rx   = hub.topic(UBXPoll::NAVHPPOSLLH).subscribe();
    let mut mon_rf_rx      = hub.topic(UBXPoll::MONRF).subscribe();
    let mut sec_sig_rx     = hub.topic(UBXPoll::SECSIG).subscribe();

    // Cached latest typed values (Arcs), defaulted for ergonomic access.
    let mut nav_pvt:      Arc<ubx::nav::NAVPVT>        = Arc::new(ubx::nav::NAVPVT::default());
    let mut nav_relpos:   Arc<ubx::nav::NAVRELPOSNED>  = Arc::new(ubx::nav::NAVRELPOSNED::default());
    let mut nav_dop:      Arc<ubx::nav::NAVDOP>        = Arc::new(ubx::nav::NAVDOP::default());
    let mut nav_sig:      Arc<ubx::nav::NAVSIG>        = Arc::new(ubx::nav::NAVSIG::default());
    let mut nav_hppos:    Arc<ubx::nav::NAVHPPOSLLH>   = Arc::new(ubx::nav::NAVHPPOSLLH::default());
    let mut mon_rf:       Arc<ubx::mon::MONRF>         = Arc::new(ubx::mon::MONRF::default());
    let mut sec_sig:      Arc<ubx::sec::SECSIG>        = Arc::new(ubx::sec::SECSIG::default());

    // Publish cadence for GNSSFix snapshots.
    let mut send_interval = time::interval(Duration::from_millis(1000));
    send_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;

            _ = nav_pvt_rx.changed() => {
                if let Some(d) = nav_pvt_rx.borrow().as_ref() {
                    match &**d {
                        UBXData::NAVPVT(x) => nav_pvt = x.clone(),
                        _ => {}
                    }
                }
            }

            _ = nav_relpos_rx.changed() => {
                if let Some(d) = nav_relpos_rx.borrow().as_ref() {
                    match &**d {
                        UBXData::NAVRELPOSNED(x) => nav_relpos = x.clone(),
                        _ => {}
                    }
                }
            }

            _ = nav_dop_rx.changed() => {
                if let Some(d) = nav_dop_rx.borrow().as_ref() {
                    match &**d {
                        UBXData::NAVDOP(x) => nav_dop = x.clone(),
                        _ => {}
                    }
                }
            }

            _ = nav_sig_rx.changed() => {
                if let Some(d) = nav_sig_rx.borrow().as_ref() {
                    match &**d {
                        UBXData::NAVSIG(x) => nav_sig = x.clone(),
                        _ => {}
                    }
                }
            }

            _ = nav_hppos_rx.changed() => {
                if let Some(d) = nav_hppos_rx.borrow().as_ref() {
                    match &**d {
                        UBXData::NAVHPPOSLLH(x) => nav_hppos = x.clone(),
                        _ => {}
                    }
                }
            }

            _ = mon_rf_rx.changed() => {
                if let Some(d) = mon_rf_rx.borrow().as_ref() {
                    match &**d {
                        UBXData::MONRF(x) => mon_rf = x.clone(),
                        _ => {}
                    }
                }
            }

            _ = sec_sig_rx.changed() => {
                if let Some(d) = sec_sig_rx.borrow().as_ref() {
                    match &**d {
                        UBXData::SECSIG(x) => sec_sig = x.clone(),
                        _ => {}
                    }
                }
            }

            _ = send_interval.tick() => {
                let fix = GNSSFix {
                    nav_pvt:       nav_pvt.clone(),
                    nav_relposned: nav_relpos.clone(),
                    nav_sig:       nav_sig.clone(),
                    nav_dop:       nav_dop.clone(),
                    nav_hpposllh:  nav_hppos.clone(),
                    mon_rf:        mon_rf.clone(),
                    sec_sig:       sec_sig.clone(),
                };

                let _ = tx.send(fix);
            }
        }
    }
}
