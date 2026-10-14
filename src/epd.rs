//! The 200×200 four-colour e-paper: a JD79667-family controller (as on the tide clock's 2.9" panel), with the init sequence from Waveshare's 1.54G example. Unlike the tide clock's, this controller ignores commands sent as one burst under a single chip select at 10 MHz: the bus is bit-banged at about 2 MHz with chip select dropped around every byte, as Waveshare's driver does (a whole frame still takes only ~40 ms). Two bits per pixel, first pixel in the top bits: 00 black, 01 white, 10 yellow, 11 red, the canvas's own ink codes.

use climate_render::{H, W};
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, Output};

pub struct Epd<'d> {
    /// The bus, bit-banged: clock and data out (mode 0, most significant bit first).
    pub sck: Output<'d>,
    pub mosi: Output<'d>,
    pub cs: Output<'d>,
    pub dc: Output<'d>,
    pub rst: Output<'d>,
    /// Low while the controller works.
    pub busy: Input<'d>,
    pub delay: Delay,
}

/// A full refresh takes about 20 s; give up well past that rather than drain the battery waiting.
const BUSY_LIMIT_MS: u32 = 60_000;

impl Epd<'_> {
    /// Clock bytes out: data set while the clock is low, latched by the controller on the rising edge, about 2 MHz. Chip select drops around every byte, as Waveshare's driver does.
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.cs.set_low();
            for bit in (0..8).rev() {
                if b >> bit & 1 != 0 {
                    self.mosi.set_high();
                } else {
                    self.mosi.set_low();
                }
                self.delay.delay_nanos(200);
                self.sck.set_high();
                self.delay.delay_nanos(200);
                self.sck.set_low();
            }
            self.cs.set_high();
        }
    }

    fn command(&mut self, c: u8, data: &[u8]) {
        self.dc.set_low();
        self.cs.set_low();
        self.write(&[c]);
        self.cs.set_high();
        if !data.is_empty() {
            self.dc.set_high();
            self.cs.set_low();
            self.write(data);
            self.cs.set_high();
        }
    }

    /// Wait for the controller to finish; false if it never did. It takes a moment to pull busy low after a command, so polling straight away would see it idle before it has started (and cut its power mid-refresh); Waveshare waits 100 ms first too.
    fn wait(&mut self) -> bool {
        self.delay.delay_millis(100);
        for _ in 0..BUSY_LIMIT_MS / 10 {
            if self.busy.is_high() {
                return true;
            }
            self.delay.delay_millis(10);
        }
        false
    }

    /// Reset the controller and power its drivers up.
    pub fn init(&mut self) -> bool {
        // Waveshare's reset timing.
        self.rst.set_high();
        self.delay.delay_millis(20);
        self.rst.set_low();
        self.delay.delay_millis(5);
        self.rst.set_high();
        self.delay.delay_millis(20);
        self.command(0x4D, &[0x78]);
        self.command(0x00, &[0x0F, 0x29]); // panel setting
        self.command(0x06, &[0x0D, 0x12, 0x30, 0x20, 0x19, 0x2A, 0x22]); // booster soft start
        self.command(0x50, &[0x37]); // border and data interval
        self.command(0x61, &[(W >> 8) as u8, W as u8, (H >> 8) as u8, H as u8]); // resolution
        self.command(0xE9, &[0x01]);
        self.command(0x30, &[0x08]); // frame rate
        self.command(0x04, &[]); // power on
        self.wait()
    }

    /// Send the canvas (one ink code per pixel) and start the refresh, which runs about 20 s on its own with busy low.
    pub fn start(&mut self, px: &[u8; W * H]) {
        self.command(0x10, &[]);
        self.dc.set_high();
        self.cs.set_low();
        let mut row = [0u8; W / 4];
        for y in 0..H {
            for (i, byte) in row.iter_mut().enumerate() {
                let p = &px[y * W + 4 * i..][..4];
                *byte = p[0] << 6 | p[1] << 4 | p[2] << 2 | p[3];
            }
            self.write(&row);
        }
        self.cs.set_high();
        self.command(0x12, &[0x00]); // refresh
        // Busy drops within a millisecond of the command.
        self.delay.delay_millis(5);
    }

    /// Whether the controller is still working.
    pub fn busy(&self) -> bool {
        self.busy.is_low()
    }

    /// Hold (or release) the lines that must not move while the chip sleeps through a refresh: a dropped reset would abort it.
    pub fn hold(&mut self, on: bool) {
        for line in [&mut self.rst, &mut self.cs, &mut self.dc] {
            line.set_pad_hold(on);
        }
    }

    /// Drive the control lines low, before the panel's power is cut: held high, they would feed the unpowered controller through its input protection.
    pub fn release(mut self) {
        self.cs.set_low();
        self.dc.set_low();
        self.rst.set_low();
    }

    /// Power the drivers down (the image stays without power). No deep sleep (0x07): the rail is cut straight after anyway, and Waveshare's own firmware never sends it.
    pub fn sleep(&mut self) {
        self.command(0x02, &[0x00]);
        self.wait();
    }
}
