use std::{io::Write, sync::mpsc};
use std::time::Duration;

use display::Display;
use graphics::{shapes, BufferedRD};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use ubx::{ConfigItem, ConfigKey, UBXData, UBXPoll};
use std::io::{self};

mod ubx;
mod gps;
mod display;
mod graphics;
mod test;
mod utils;
mod fudge;
mod at;
mod cell;
use crate::at::AT;
use crate::gps::Distance;
use crate::ubx::UBX;

pub fn get_number_from_cmd() -> Option<i32> {
    use std::io::{self, Write, BufRead};

    io::stdout().flush().ok()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input).ok()?;
    input.trim().parse().ok()
}

pub fn print_title(title: &str) {
    const TOTAL_WIDTH: usize = 40;
    let prefix = "<== ";
    let suffix = "=>";

    let core_len = title.len();
    let fixed_len = prefix.len() + suffix.len() + core_len;
    let filler_len = TOTAL_WIDTH.saturating_sub(fixed_len);

    let filler = "=".repeat(filler_len);

    println!("\n{}{} {}{}", prefix, title, filler, suffix);
}

use tokio_gpiod::{Chip, Edge, Event, Input, Lines, Options, Output};
use tokio::time::{timeout};


async fn main_d() -> tokio::io::Result<()> {
    print_title("BG-95 TESTER 2");

    let port_name = "/dev/ttyAMA2";
    let baud_rate =  115_200; 

    let mut serial_port = tokio_serial::new(port_name, baud_rate)
        .data_bits(tokio_serial::DataBits::Eight)
        .flow_control(tokio_serial::FlowControl::None)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .timeout(Duration::from_secs(1))
        .open_native_async()
        .expect("The SerialStream should open, but failed:");
    let (mut serial_reader, mut serial_writer) = tokio::io::split(serial_port);
    println!("[main]: SerialStream port {} opened at {} baud.", port_name, baud_rate);


    let mut chip = Chip::new("gpiochip0").await.unwrap();
    println!("[main]: 24 HIGH");
    let options = Options::output([24])
        .values([true])
        .consumer("PWRKEY for BG95-M2");
    let pwr_key = chip.request_lines(options).await.unwrap();
    pwr_key.set_values([false]).await.unwrap();

    println!("skip init? 1 = true");
    if get_number_from_cmd().unwrap() != 1 {
        pwr_key.set_values([false]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(3000)).await;
        println!("[main]: 24 LOW");
        pwr_key.set_values([true]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(750)).await;
        println!("[main]: 24 HIGH");
        pwr_key.set_values([false]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5000)).await;
    }


    println!("[main]: Spawning `recieve_at_data()`");
    let (tx, mut rx) = mpsc::channel::<AT>();
    let _recieve_at_handle = tokio::spawn(async move {
        at::recieve_at_data(serial_reader, tx).await;
    });

    println!("\nCurrent packets:");
    loop {
        let resp = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        println!("{:?}", resp);
    }

    let _ = get_number_from_cmd();

    println!("\nTesting `ATI`");
    serial_writer.write_all(b"ATI\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    loop {
        let resp = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        println!("{:?}", resp);
    }

    let _ = get_number_from_cmd();

    println!("\nTesting `AT+CPIN?`");
    serial_writer.write_all(b"AT+CPIN?\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    loop {
        let resp = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        println!("{:?}", resp);
    }

    let _ = get_number_from_cmd();

    println!("\nTesting `AT+COPS?`");
    serial_writer.write_all(b"AT+COPS?\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    loop {
        let resp = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        println!("{:?}", resp);
    }

    let _ = get_number_from_cmd();

    println!("\nTesting `AT+QCSQ`");
    serial_writer.write_all(b"AT+QCSQ\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    loop {
        let resp = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        println!("{:?}", resp);
    }

    let _ = get_number_from_cmd();

    println!("\nTesting `AT+QNWINFO`");
    serial_writer.write_all(b"AT+QNWINFO\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    loop {
        let resp = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        println!("{:?}", resp);
    }

    let _ = get_number_from_cmd();

    println!("\nTesting `AT+CEREG?`");
    serial_writer.write_all(b"AT+CEREG?\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    loop {
        let resp = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        println!("{:?}", resp);
    }

    let _ = get_number_from_cmd();

    println!("\nTesting `AT+CFUN?`");
    serial_writer.write_all(b"AT+CFUN?\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    loop {
        let resp = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        println!("{:?}", resp);
    }

    let _ = get_number_from_cmd();


    println!("\nShutdown? 1 = yes");
    if get_number_from_cmd().unwrap() == 1 {
        println!("\nSending Shutdown...");
        pwr_key.set_values([false]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        println!("[main]: LOW");
        pwr_key.set_values([true]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        println!("[main]: HIGH");
        pwr_key.set_values([false]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
    
    println!("\nRemaining AT commands:");
    loop {
        let resp = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => break,
        };
        println!("{:?}", resp);
    }

    println!("\nDone!");

    Ok(())
}

async fn main7() -> tokio::io::Result<()> {
    print_title("BG-95 TESTER");

    let port_name = "/dev/ttyAMA2";
    let baud_rate =  115_200; 

    let mut serial_port = tokio_serial::new(port_name, baud_rate)
        .data_bits(tokio_serial::DataBits::Eight)
        .flow_control(tokio_serial::FlowControl::None)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .timeout(Duration::from_secs(1))
        .open_native_async()
        .expect("The SerialStream should open, but failed:");
    let (mut serial_reader, mut serial_writer) = tokio::io::split(serial_port);

    println!("[main]: SerialStream port {} opened at {} baud.", port_name, baud_rate);

    let mut chip = Chip::new("gpiochip0").await.unwrap();

    println!("HIGH");
    let options = Options::output([5])
        .values([true])
        .consumer("PWRKEY for BG95-M3");
    let pwr_key = chip.request_lines(options).await.unwrap();
    pwr_key.set_values([true]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(3000)).await;

    println!("LOW");
    pwr_key.set_values([false]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(750)).await;

    println!("HIGH");
    pwr_key.set_values([true]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10000)).await;

    // let options = Options::input([5])
    //     .consumer("PWRKEY for BG95-M3");
    // let pwr_key = chip.request_lines(options).await.unwrap();
    // println!("High-Z");
    // println!("Pull high");
    // drop(pwr_key);
    // let options = Options::output([5])
    //     .values([true])
    //     .consumer("PWRKEY for BG95-M3");
    // let pwr_key = chip.request_lines(options).await.unwrap();
    // // "Ensure that VBAT is stable before pulling down PWRKEY and keep the interval not less than 30 ms."
    // tokio::time::sleep(Duration::from_millis(35));
    // println!("Pull low");
    // pwr_key.set_values([false]).await.unwrap();
    // tokio::time::sleep(Duration::from_millis(750)).await;
    // drop(pwr_key);
    // let options = Options::input([5])
    //     .consumer("PWRKEY for BG95-M3");
    // let pwr_key = chip.request_lines(options).await.unwrap();
    // println!("High-Z");

    // tokio::time::sleep(Duration::from_millis(2_000)).await;

    println!("[main]: BG95-M3 PWRKEY init complete.");
    println!("[main]: Writing...");
    serial_writer.write_all(b"AT\r\n").await.unwrap();
    println!("[main]: Reading...");

    let mut resp = Vec::new();
    loop {
        // read exactly one byte with a timeout
        let mut b = [0u8];
        match timeout(Duration::from_secs(2), serial_reader.read_exact(&mut b)).await {
            Ok(Ok(_)) => {
                if let Ok(s) = String::from_utf8(vec![b[0]]) {
                    let str = s.replace("\n", "\\n ").replace("\r", "\\r");
                    print!("{}", str);
                } else {
                    print!(" b{:02X}", b[0]);
                }
                io::stdout().flush().ok().unwrap();
            }
            Ok(Err(e)) => {
                eprintln!("UART error: {}", e);
                break;
            }
            Err(_) => {
                eprintln!("Timed out waiting for response");
                break;
            }
        }
    }

    println!("<<  {}", String::from_utf8_lossy(&resp));
    println!("<<b {:?}", resp);

    Ok(())
}

async fn main3() -> tokio::io::Result<()> {
    print_title("SURVEY-IN TESTER");
    println!("\nStart? (1 == yes, 2 == skip setup): ");
    let num = get_number_from_cmd().unwrap();
    println!("cont = {:?}", !(num == 1 || num == 2));
    if !(num == 1 || num == 2) { return Ok(()); }

    let port_name = "/dev/serial0";
    let baud_rate =  38_400; 

    let mut serial_port = tokio_serial::new(port_name, baud_rate)
        .data_bits(tokio_serial::DataBits::Eight)
        .flow_control(tokio_serial::FlowControl::None)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .timeout(Duration::from_secs(1))
        .open_native_async()
        .expect("The SerialStream should open, but failed:");

    println!("[main]: SerialStream port {} opened at {} baud.", port_name, baud_rate);

    let (mut serial_reader, mut serial_writer) = tokio::io::split(serial_port);
    let (tx, mut rx) = mpsc::channel::<UBX>();
    let _recieve_ubx_handle = tokio::spawn(async move {
        ubx::receive_ubx_packets(&mut serial_reader, tx).await;
    });
    println!("[main]: Spawned [receive_ubx_packets].");
    
    if num != 2 {
        print!("\nSet survey in duration (seconds): ");
        let duration_seconds = get_number_from_cmd().unwrap();
    
        gps::begin_survey_in(&mut serial_writer, Duration::from_secs(duration_seconds as u64), Distance::from_cm(1.0)).await.unwrap();
    }

    while (match rx.try_recv() {
        Ok(ubx) => { println!("[main]: Recieved {}", ubx.bytes_string()); true  },
        Err(e) =>  { println!("[main]: UBX handle emptied.");      false }
    }) {}

    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        gps::fetch_survey_in_update(&mut rx, &mut serial_writer).await.unwrap();
    }

    Ok(())
}

#[tokio::main]
async fn main() -> tokio::io::Result<()> {
    println!("1: display::main() -> display testing\n3 -> fudge\nany_key: main::main().");

    match get_number_from_cmd() {
        Some(1) => { 
            println!("1 to exit:");
            let mut display = Display::new().unwrap();
            display.clear();
            display.update();
            let mut i = 0;
            let mut input = String::new();
            loop {
                use std::io::{self, Write, BufRead};
                std::io::stdin().lock().read_line(&mut input);
                shapes::fill(&graphics::Color::from_rgb888(i, 0, 0), &mut display);
                display.update();
                i += 10;
            }
    
            return Ok(());
        },
        Some(2) => {
            println!("running test...");
            crate::test::main();
            return Ok(());
        }
        Some(3) => {
            println!("fudging...");
            let mut display = Display::new().unwrap();
            display.clear();
            display.update();
    
            let start = std::time::Instant::now();
            loop {
                fudge::blit_image_from_file(&mut display, "fudge1.png").unwrap();
                fudge::blit_image_from_file(&mut display, "fudge2.png").unwrap();
    
                if start.elapsed() >= Duration::from_secs(30) {
                    break;
                }
            }
    
            loop {
                fudge::blit_image_from_file(&mut display, "fudge3.png").unwrap();
                fudge::blit_image_from_file(&mut display, "fudge4.png").unwrap();
            }
    
            println!("fudged.");
        }
        _ => (),
    }

    // Configure the serial port
    let port_name = "/dev/serial0"; // Replace with the correct serial port
    let baud_rate =  38_400; // Standard for ZED-F9P

    let mut serial_port = tokio_serial::new(port_name, baud_rate)
        .data_bits(tokio_serial::DataBits::Eight)
        .flow_control(tokio_serial::FlowControl::None)
        .parity(tokio_serial::Parity::None)
        .stop_bits(tokio_serial::StopBits::One)
        .timeout(Duration::from_secs(1))
        .open_native_async()
        .expect("The SerialStream should open, but failed:");

    println!("SerialStream port {} opened at {} baud.", port_name, baud_rate);

    let (mut serial_reader, mut serial_writer) = tokio::io::split(serial_port);

    println!("Spawning [receive_ubx_packets].");

    let (tx, mut rx) = mpsc::channel::<UBX>();
    let _recieve_ubx_handle = tokio::spawn(async move {
        ubx::receive_ubx_packets(&mut serial_reader, tx).await;
    });

    println!("Hey! :)\n\nWhat do you want to do?\n1: Poll NAVPVT and Await Result\n2: Poll CFG-VALGET: 0x30210001\n");
    match get_number_from_cmd() {
        Some(1) => {
            let ubx_frame = ubx::UBXPoll::NAVPVT.build_ubx().bytes();
            println!("sending ubx_frame: {:02X?}", ubx_frame);
            serial_writer.write_all(&ubx_frame).await;
            println!("Done writing.");
        
            let timeout = rx.recv_timeout(Duration::from_secs(5));
        
            match timeout {
                Ok(ubx_msg) => {
                    println!("");
                    println!("[rx] received a UBX message:");
                    println!("{}", ubx_msg.bytes_string());
                    println!("Validation: {:?}", ubx_msg.is_valid());
                    let data = ubx::UBXData::process_raw_ubx(&ubx_msg).unwrap();
                    match data {
                        ubx::UBXData::NAVPVT { latitude, longitude, height, fix_type, .. } => {
                            println!("Lat: {latitude}, Lon: {longitude}, Height: {height}, fix_type: {fix_type:?}");
                        }
                        _ => println!("Did not parse data!")
                    }
                },
                Err(e) => {
                    panic!("Did not receieve any UBX message within the timeout of 5 seconds: \"{e}\".");
                }
            }
        },
        Some(2) => {
            let ubx_poll = ubx::UBXPoll::CFGVALGET { 
                layer: ubx::CfgLayer::RAM,
                positon: 0,
                cfg_keys: vec![
                    ConfigKey::TmodePosType,
                ]
            };
            let ubx_frame = ubx_poll.build_ubx().bytes();
            println!("sending ubx_frame: {:02X?}", ubx_frame);
            serial_writer.write_all(&ubx_frame).await;
            println!("Done writing.");

            let timeout = rx.recv_timeout(Duration::from_secs(5));

            match timeout {
                Ok(ubx_msg) => {
                    println!("");
                    println!("[rx] received a UBX message:");
                    println!("{}", ubx_msg.bytes_string());
                    println!("Validation: {:?}", ubx_msg.is_valid());
                    let data = ubx::UBXData::process_raw_ubx(&ubx_msg).unwrap();

                    match data {
                        UBXData::CFGVALGET { data, .. } => {
                            println!("Parsed sucessfully into incoming CFGVALGET!");
                            println!("key: {:02X?}", data[0].config_key);
                            match data.get(0).unwrap().config_val {
                                ubx::ConfigValue::E1(v) => println!("value: {}", v),
                                _ => println!("invalid format here"),
                            }
                        }
                        _ => println!("Did not parse data properly!"),
                    };
                },
                Err(e) => {
                    panic!("Did not receieve any UBX message within the timeout of 5 seconds: \"{e}\".");
                }
            }

            println!("Want to change cfg to 1? 0 -> no 1 -> yes: ");
            match get_number_from_cmd() {
                Some(1) => {
                    println!("\n\nTrying to change cfg to `1`");

                    let ubx_poll = ubx::UBXPoll::CFGVALSET { 
                        ram: true, 
                        bbr: false, 
                        flash: false, 
                        cfg_data: vec![ConfigItem {
                            config_key: ConfigKey::TmodePosType,
                            config_val: ubx::ConfigValue::E1(1),
                        }]
                    };
                    let ubx_frame = ubx_poll.build_ubx().bytes();
                    println!("sending ubx_frame: {:02X?}", ubx_frame);
                    serial_writer.write_all(&ubx_frame).await;
                    println!("Done writing.");
        
                    // would need to handle a possible UBX-ACK-NAK traditionally
                },
                Some(_) | None => {},
            }
        },
        _ => { println!("Invalid Response. :(")}
    }

    println!("\nClosing...\n");

    Ok(())
}

// struct SurveyInConfig {
//     ram: bool,
//     bbr: bool,
//     flash: bool,
//     min_duration: Duration,
//     min_accuracy_mm: u32,
// }

// async fn enable_gps_survey_in(serial_writer: &mut WriteHalf<SerialStream>, reciever: mpsc::Receiver<UBX>) {
//     use ubx;

//     const CFG_TMODEMODE_SURVEY_IN_ENABLED: u8 = 1;
//     let msg_enable_survey_in = UBXPoll::CFGVALSET { 
//         ram: true,
//         bbr: false,
//         flash: false, 
//         cfg_data: vec![ConfigItem {
//             config_key: ConfigKey::TmodeMode,
//             config_val: E1(CFG_TMODEMODE_SURVEY_IN_ENABLED)
//         }]
//     };
// }