use std::time::Duration;

use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use tokio_gpiod::{Chip, Edge, Event, Input, Lines, Options, Output};

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

pub fn print_bytes(arr: &[u8]) {
    print!("[ ");
    for b in arr {
        print!("{:08b}/{:02X}, ", *b, *b);
    }
    println!("]");
}

pub async fn test() {
    print_title("Opening GPIO pins");
    let mut chip = Chip::new("gpiochip0").await.unwrap();
    // Aquire BUSY, RESET, and DIO1.
    //
    // 22: NSS/CS (out)
    // 23: DOI_1  (in)
    // 24: RESET  (out)
    // 25: BUSY   (in)
    let options = Options::output([22])
        .values([true])
        .consumer("LoRa Chip DIO1 line.");
    let nss = chip.request_lines(options).await.unwrap();        
    let options = Options::input([23])
        .edge(tokio_gpiod::EdgeDetect::Rising)
        .consumer("LoRa Chip DIO1 line.");
    let dio1 = chip.request_lines(options).await.unwrap();
    let options = Options::output([24])
        .values([true])
        .consumer("LoRa Chip RESET line.");
    let reset = chip.request_lines(options).await.unwrap();
    let options = Options::input([25])
        .edge(tokio_gpiod::EdgeDetect::Both)
        .consumer("LoRa Chip BUSY line.");
    let busy = chip.request_lines(options).await.unwrap();
    drop(chip);
    println!("Done.");

    let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, 100_000, Mode::Mode0).unwrap();

    print_title("Hardware Reset");
    reset.set_values([false]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    reset.set_values([true]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // SetDIO3asTCXOCtrl SPI Transaction
    let cmnd = [0x97, 0x00, 0x00, 0x00];
    let mut rx = [0u8; 4];
    spi.transfer(&mut rx, &cmnd).unwrap();

    let cmnd = [0x1D, 0x07, 0x40];
    let mut rx = [0u8; 3];
    spi.transfer(&mut rx, &cmnd).unwrap();

    print!("RX: ");
    print_bytes(&rx);

    println!("Done.\n");
}

pub async fn test2() {
    tokio::time::sleep(Duration::from_millis(2000)).await;

    print_title("Opening GPIO pins");
    let mut chip = Chip::new("gpiochip0").await.unwrap();
    // Aquire BUSY, RESET, and DIO1.
    //
    // 22: NSS/CS (out)
    // 23: DOI_1  (in)
    // 24: RESET  (out)
    // 25: BUSY   (in)
    let options = Options::output([27])
        .values([true])
        .consumer("LoRa Chip DIO1 line.");
    let test = chip.request_lines(options).await.unwrap();       
    let options = Options::output([22])
        .values([true])
        .consumer("LoRa Chip DIO1 line.");
    let nss = chip.request_lines(options).await.unwrap();        
    let options = Options::input([23])
        .edge(tokio_gpiod::EdgeDetect::Rising)
        .consumer("LoRa Chip DIO1 line.");
    let dio1 = chip.request_lines(options).await.unwrap();
    let options = Options::output([24])
        .values([true])
        .consumer("LoRa Chip RESET line.");
    let reset = chip.request_lines(options).await.unwrap();
    let options = Options::input([25])
        .edge(tokio_gpiod::EdgeDetect::Both)
        .consumer("LoRa Chip BUSY line.");
    let busy = chip.request_lines(options).await.unwrap();
    drop(chip);
    println!("Done.");

    test.set_values([false]).await.unwrap();

    print_title("Hardware Reset");
    reset.set_values([false]).await.unwrap();
    tokio::time::sleep(Duration::from_micros(100)).await;
    reset.set_values([true]).await.unwrap();
    let mut n = 0;
    while !*busy.get_values([false; 1]).await.unwrap().get(0).unwrap() {
        n += 1;
        println!("  [RESET] BUSY LOW {n}");
    }
    let mut n = 0;
    while *busy.get_values([false; 1]).await.unwrap().get(0).unwrap() {
        n += 1;
        println!("  [RESET] BUSY HIGH {n}");
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    println!("Reset complete.");
    println!("Reset complete.\n");

    let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, 100_000, Mode::Mode0).unwrap();
    println!("BUSY: {:?}", *busy.get_values([false]).await.unwrap().get(0).unwrap());
    tokio::time::sleep(Duration::from_millis(10));
    println!("BUSY: {:?}", *busy.get_values([false]).await.unwrap().get(0).unwrap());

    println!("WriteRegister()");
    //            CMD   ADR1  ADR2 
    let tx_buf =     [0x0D, 0x00, 0x00, 0x03, 0x01, 0x04, 0x01, 0x05, 0x09, 0x02, 0x06, 0x05];
    let mut rx_buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    spi.transfer(&mut rx_buf, &tx_buf).unwrap();
    print!("TX: "); print_bytes(&tx_buf);
    print!("RX: "); print_bytes(&rx_buf);
    println!("BUSY: {:?}", *busy.get_values([false]).await.unwrap().get(0).unwrap());
    tokio::time::sleep(Duration::from_millis(10));
    println!("BUSY: {:?}", *busy.get_values([false]).await.unwrap().get(0).unwrap());

    println!("ReadRegister()");
    let tx_buf =     [0x1D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    let mut rx_buf = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    spi.transfer(&mut rx_buf, &tx_buf).unwrap();
    print!("TX: "); print_bytes(&tx_buf);
    print!("RX: "); print_bytes(&rx_buf);
    println!("BUSY: {:?}", *busy.get_values([false]).await.unwrap().get(0).unwrap());
    tokio::time::sleep(Duration::from_millis(10));
    println!("BUSY: {:?}", *busy.get_values([false]).await.unwrap().get(0).unwrap());


    test.set_values([true]).await.unwrap();
    println!("\nDone.")
}

pub async fn test1() {
    print_title("Opening GPIO pins");
    let mut chip = Chip::new("gpiochip0").await.unwrap();
    // Aquire BUSY, RESET, and DIO1.
    //
    // 22: NSS/CS (out)
    // 23: DOI_1  (in)
    // 24: RESET  (out)
    // 25: BUSY   (in)
    let options = Options::output([22])
        .values([true])
        .consumer("LoRa Chip DIO1 line.");
    let nss = chip.request_lines(options).await.unwrap();        
    let options = Options::input([23])
        .edge(tokio_gpiod::EdgeDetect::Rising)
        .consumer("LoRa Chip DIO1 line.");
    let dio1 = chip.request_lines(options).await.unwrap();
    let options = Options::output([24])
        .values([true])
        .consumer("LoRa Chip RESET line.");
    let reset = chip.request_lines(options).await.unwrap();
    let options = Options::input([25])
        .edge(tokio_gpiod::EdgeDetect::Both)
        .consumer("LoRa Chip BUSY line.");
    let busy = chip.request_lines(options).await.unwrap();
    drop(chip);
    println!("Done.");


    print_title("Hardware Reset");
    reset.set_values([false]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1)).await;
    reset.set_values([true]).await.unwrap();
    let mut n = 0;
    while !*busy.get_values([false; 1]).await.unwrap().get(0).unwrap() {
        n += 1;
        println!("  [RESET] BUSY LOW {n}");
    }
    let mut n = 0;
    while *busy.get_values([false; 1]).await.unwrap().get(0).unwrap() {
        n += 1;
        println!("  [RESET] BUSY HIGH {n}");
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    println!("Reset complete.");


    print_title("Configuring SPI");
    let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, 7_812_500, Mode::Mode0).unwrap();
    println!("Done.");


    print_title("SetStandby()");
    loop {
        if !*busy.get_values([false;1]).await.unwrap().get(0).unwrap() {
            println!("  [waiting] No longer busy.");
            tokio::time::sleep(Duration::from_micros(10)).await;
            break;
        }
        println!("  [waiting] Busy.");
    }
    let tx_buffer = [0x80, 0x00];
    let mut rx_buffer = [0u8; 2];
    println!("tx:"); print_bytes(&tx_buffer);
    let transfer_bytes = spi.transfer(&mut rx_buffer, &tx_buffer).unwrap();
    println!("{transfer_bytes} bytes transferred sucessfully!");
    println!("rx: "); print_bytes(&rx_buffer);
    let mut n = 0;
    while *busy.get_values([false; 1]).await.unwrap().get(0).unwrap() {
        n += 1;
        println!("  [waiting] BUSY HIGH {n}");
    }

    print_title("SetPacketType()");
    loop {
        if !*busy.get_values([false;1]).await.unwrap().get(0).unwrap() {
            println!("  [waiting] No longer busy.");
            tokio::time::sleep(Duration::from_micros(10)).await;
            break;
        }
        println!("  [waiting] Busy.");
    }
    let tx_buffer = [0x8A, 0x01];
    let mut rx_buffer = [0u8; 2];
    println!("tx:"); print_bytes(&tx_buffer);
    let transfer_bytes = spi.transfer(&mut rx_buffer, &tx_buffer).unwrap();
    println!("{transfer_bytes} bytes transferred sucessfully!");
    println!("rx: "); print_bytes(&rx_buffer);
    let mut n = 0;
    while *busy.get_values([false; 1]).await.unwrap().get(0).unwrap() {
        n += 1;
        println!("  [waiting] BUSY HIGH {n}");
    }

    print_title("GetStatus()");
    loop {
        if !*busy.get_values([false;1]).await.unwrap().get(0).unwrap() {
            println!("  [waiting] No longer busy.");
            tokio::time::sleep(Duration::from_micros(10)).await;
            break;
        }
        println!("  [waiting] Busy.");
    }
    let tx_buffer = [0xC0, 0x00];
    let mut rx_buffer = [0u8; 2];
    println!("tx:"); print_bytes(&tx_buffer);
    let transfer_bytes = spi.transfer(&mut rx_buffer, &tx_buffer).unwrap();
    println!("{transfer_bytes} bytes transferred sucessfully!");
    println!("rx: "); print_bytes(&rx_buffer);
    let mut n = 0;
    while *busy.get_values([false; 1]).await.unwrap().get(0).unwrap() {
        n += 1;
        println!("  [waiting] BUSY HIGH {n}");
    }

    println!("\nDone.");
}