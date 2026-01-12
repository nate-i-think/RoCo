pub fn get_number_cmdline() -> Option<i32> {
    use std::io::{self, Write, BufRead};

    io::stdout().flush().ok()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input).ok()?;
    input.trim().parse().ok()
}
