#![cfg_attr(debug_assertions, allow(unused_variables, dead_code))]

mod ubx;
mod gps;
mod display;
mod graphics;
mod utils;
mod at;
mod cell;

pub fn get_number() -> Option<i32> {
    use std::io::{self, Write, BufRead};

    io::stdout().flush().ok()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input).ok()?;
    input.trim().parse().ok()
}

#[tokio::main]
async fn main() -> tokio::io::Result<()> {
    println!("Program ran successfully!");
    Ok(())
}
