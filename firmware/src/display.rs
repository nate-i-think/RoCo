use std::fs::{OpenOptions, File};
use std::io::{self, BufRead, Write, Seek, SeekFrom};
use crate::graphics::{self, BufferedRD, Color, BufErr};

pub struct Display {
    buf: Vec<u8>,
    fb_file: File,
}

impl Display {
    pub fn new() -> Result<Self, io::Error> {
        const BYTES_PER_PIXEL: usize = 2;
        let mut buf: Vec<u8> = vec![0xFF; Self::WIDTH * Self::HEIGHT * BYTES_PER_PIXEL];
        const FB_PATH: &str = "/dev/fb0";
        let mut fb = OpenOptions::new().write(true).open(FB_PATH)?;

        Ok(Self {
            buf: buf,
            fb_file: fb,
        })
    }

    /// Consumes the buffer and closes its file reference.
    pub fn drop(buf: Self) {
        drop(buf)
    }
}

use libc;
const FBIO_WAITFORVSYNC: libc::c_ulong = 0x40044620;

/// Waits for the vertical sync signal on the given file descriptor.
fn wait_for_vsync(fd: i32) -> io::Result<()> {
    let mut dummy: u32 = 0;
    let ret = unsafe { libc::ioctl(fd, FBIO_WAITFORVSYNC, &mut dummy) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl BufferedRD for Display {
    const WIDTH: usize = 800;
    const HEIGHT: usize = 480;

    #[inline]
    fn draw(&mut self, x: usize, y: usize, color: &Color) {
        let index = (y * Self::WIDTH + x) * 2;
        self.buf[index] = color.bitwise_color.0;
        self.buf[index + 1] = color.bitwise_color.1;
    }

    #[inline]
    fn get(&self, x: usize, y: usize) -> Result<Color, BufErr> {
        let index = (y * Self::WIDTH + x) * 2;
        match (self.buf.get(index), self.buf.get(index + 1)) {
            (Some(a), Some(b)) => Ok(Color { bitwise_color: (*a, *b) }),
            _ => Err(BufErr::OutOfBounds { 
                px: x, py: y, bx: Self::WIDTH, by: Self::HEIGHT 
            }),
        }
    }

    fn update(&mut self) {
        // Wait for vertical sync.
        use std::os::unix::io::AsRawFd;
        if let Err(e) = wait_for_vsync(self.fb_file.as_raw_fd()) {
            eprintln!("Failed to wait for vsync: {}", e);
        }

        self.fb_file.seek(SeekFrom::Start(0));
        self.fb_file.write_all(&self.buf);
    }

    fn clear(&mut self) {
        const WHITE: u8 = 0xFF;
        self.buf.fill(WHITE);
    }
}







pub fn main() -> io::Result<()> {
    // Open the framebuffer device for writing.
    let fb_path = "/dev/fb0";
    let mut fb = OpenOptions::new().write(true).open(fb_path)?;
    
    // For this example, we use a fixed block of data.
    // Here we create a block with 1024 bytes with a simple repeating pattern.
    let block: Vec<u8> = generate_gradient(800, 480);

    println!("Enter a number (n): I will write n bytes from the block to fb0.");
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    loop {
        // Prompt the user.
        print!("Enter n (or 'q' to quit): ");
        io::stdout().flush()?;  // Ensure the prompt is printed immediately.
        
        let mut input = String::new();
        reader.read_line(&mut input)?;
        let trimmed = input.trim();
        
        // Allow quitting.
        if trimmed.eq_ignore_ascii_case("q") {
            break;
        }
        
        // Parse the input as a number.
        let mut n: usize = match trimmed.parse() {
            Ok(num) => num,
            Err(_) => {
                println!("Invalid number. Please try again.");
                continue;
            }
        };
        
        // Limit n to the size of our block.
        if n > block.len() {
            println!("Requested {} bytes, but block size is {}. Using block size.", n, block.len());
            n = block.len();
        }
        
        // Optional: Seek to the beginning of fb0.
        fb.seek(SeekFrom::Start(0))?;
        
        // Write the first n bytes from the block to the framebuffer.
        fb.write_all(&block[..n])?;
        println!("Wrote {} bytes to fb0.", n);
    }
    
    Ok(())
}

// remember to check the format of fb0 using the `fbset` command in terminal. this is rgb565

fn generate_gradient(width: usize, height: usize) -> Vec<u8> {
    // 16 bits per pixel means 2 bytes per pixel.
    let mut buffer = Vec::with_capacity(width * height * 2);
    
    for y in 0..height {
        for x in 0..width {
            // Scale red to 5 bits (0-31)
            let r = (x * 31 / (width - 1)) as u16;
            // Scale green to 6 bits (0-63)
            let g = (y * 63 / (height - 1)) as u16;
            // Scale blue to 5 bits (0-31), using a diagonal gradient (x+y)
            let b = ((x + y) * 31 / (width + height - 2)) as u16;
            
            // Pack the pixel according to the mode's rgba specification: 5/11,6/5,5/0
            // red occupies bits 11-15, green bits 5-10, blue bits 0-4.
            let pixel: u16 = (r << 11) | (g << 5) | b;
            
            // Write the 16-bit pixel to the buffer in little-endian order.
            buffer.push((pixel & 0xFF) as u8);  // lower 8 bits
            buffer.push((pixel >> 8) as u8);      // upper 8 bits
        }
    }
    
    buffer
}