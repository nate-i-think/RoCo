use std::sync::mpsc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader, ReadHalf};
use tokio_serial::SerialStream;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AT {
    Invalid {
        msg: Vec<u8>,
    },
    Empty,
    Ok,
    Error,
    Unknown {
        msg: String,
    }
}

pub async fn receive_at_data(
    reader: ReadHalf<SerialStream>,
    tx: mpsc::Sender<AT>,
) -> std::io::Result<()> {
    const BUFFER_SIZE: usize = 1024;
    let mut buf_reader = BufReader::with_capacity(4 * 1024, reader);
    let mut line_buf = Vec::new();

    loop {
        line_buf.clear();

        // Send messages split by "\r\n" or messages just a 
        // carriage return away
        let n = buf_reader.read_until(b'\r', &mut line_buf).await?;
        if n == 0 { /* EOF on the serial port. */ return Ok(()); }
        let new_line = buf_reader.read_u8().await?;
        if new_line == b'\n' {
            line_buf.push(b'\n');
        } else {
            // Check for a single-bit error:
            if (new_line ^ b'\n').count_ones() == 1 {
                println!("[receive_at_data]: saw a single bit error.");
                line_buf.push(b'\n');
                tx.send(parse_at_line(line_buf)).unwrap();
            } else {
                line_buf.push(new_line);
                tx.send(AT::Invalid { msg: line_buf }).unwrap();
            }

            line_buf = Vec::new();
            continue;
        }

        // `line_buf` is now something like "...\r\n", so we drop
        // the "\r\n" and send it off to be processed.
        if line_buf.last() == Some(&b'\n') {
            line_buf.pop();
        }
        if line_buf.last() == Some(&b'\r') {
            line_buf.pop();
        }
        if line_buf.len() == 0 {
            tx.send(AT::Empty).unwrap();
            continue;
        }
        tx.send(parse_at_line(line_buf)).unwrap();
        line_buf = Vec::new();
    }
}

fn parse_at_line(line: Vec<u8>) -> AT {
    let str = match std::str::from_utf8(&line) {
        Ok(s) => s,
        Err(_) => return AT::Invalid { msg: line },
    };

    match str {
        "OK" => return AT::Ok,
        "Error" => return AT::Error,
        _ => (),
    }

    AT::Unknown { msg: str.to_string() }
}