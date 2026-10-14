//! Firmware for the Waveshare ESP32-S3-ePaper-1.54G: a week of indoor temperature and humidity from its SHTC3 on the four-colour e-paper, on battery with no radio.
//!
//! Every wake runs from reset: hold the battery on, read the clock and the sensor, fold the reading into the current hour's average, redraw the panel once per hour (climate-render, the same code as the PC preview), then deep-sleep until the next 10-minute mark. The week of hourly averages lives in RTC slow memory, kept powered through deep sleep (a flat battery or the PWR button loses it). With a USB host attached the board waits awake instead of sleeping, so the serial port stays up for flashing and logs.
//!
//! The clock chip keeps UTC. If its time is invalid (first power-up, or its oscillator stopped), it is set from the build time.

#![no_std]
#![no_main]

mod epd;
mod sensors;

use climate_render::{H, HOURS, Reading, W, battery_permille, render};
use esp_backtrace as _;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::time::{Duration, Instant, Rate};
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();
include!(concat!(env!("OUT_DIR"), "/build_time.rs"));

/// Wake this often (seconds), on the clock's marks.
const SAMPLE_S: i64 = 600;
/// An hour with no reading.
const NONE: i16 = i16::MIN;
const MAGIC: u32 = 0x434C_4D31;

/// What deep sleep keeps: the week of hourly averages and the newest hour's running sums.
#[derive(Clone, Copy)]
#[repr(C)]
struct State {
    magic: u32,
    /// Running sums and count of this hour's readings.
    n: i32,
    sum_temp: i32,
    sum_humidity: i32,
    /// Unix hour (seconds / 3600) of the newest column, the last in the arrays.
    newest: i64,
    /// Hour last drawn on the panel.
    drawn: i64,
    temp: [i16; HOURS],
    humidity: [i16; HOURS],
}

// SAFETY: integers and integer arrays only, so every bit pattern is a valid State; MAGIC and the range checks catch garbage.
unsafe impl esp_hal::Persistable for State {}

#[esp_hal::ram(unstable(rtc_slow, persistent))]
static mut STATE: State = State { magic: 0, n: 0, sum_temp: 0, sum_humidity: 0, newest: 0, drawn: 0, temp: [0; HOURS], humidity: [0; HOURS] };

/// The panel's canvas, one ink code per pixel.
static mut CANVAS: [u8; W * H] = [0; W * H];

impl State {
    fn reset(&mut self, hour: i64) {
        *self = State { magic: MAGIC, n: 0, sum_temp: 0, sum_humidity: 0, newest: hour, drawn: i64::MIN, temp: [NONE; HOURS], humidity: [NONE; HOURS] };
    }

    /// Move the newest column on to `hour`, shifting the week left and leaving the skipped hours empty.
    fn advance(&mut self, hour: i64) {
        let k = (hour - self.newest).min(HOURS as i64) as usize;
        for a in [&mut self.temp, &mut self.humidity] {
            a.copy_within(k.., 0);
            a[HOURS - k..].fill(NONE);
        }
        (self.newest, self.n, self.sum_temp, self.sum_humidity) = (hour, 0, 0, 0);
    }

    /// Fold a reading into the newest hour's average (rounded).
    fn add(&mut self, r: Reading) {
        self.n += 1;
        self.sum_temp += r.temp as i32;
        self.sum_humidity += r.humidity as i32;
        let avg = |sum: i32| (sum + self.n / 2).div_euclid(self.n) as i16;
        self.temp[HOURS - 1] = avg(self.sum_temp);
        self.humidity[HOURS - 1] = avg(self.sum_humidity);
    }

    fn history(&self) -> [Option<Reading>; HOURS] {
        core::array::from_fn(|i| (self.temp[i] != NONE).then(|| Reading { temp: self.temp[i], humidity: self.humidity[i] }))
    }
}

/// Whether a USB host is talking to the board, watching for up to `window_ms`: its start-of-frame counter ticks every millisecond while one is. Right after a reset the host is still enumerating the board and sends no frames yet, so a fresh boot needs a window of seconds.
fn usb_host(delay: &Delay, window_ms: u32) -> bool {
    let frame = || esp_hal::peripherals::USB_DEVICE::regs().fram_num().read().sof_frame_index().bits();
    let before = frame();
    for _ in 0..window_ms {
        delay.delay_millis(1);
        if frame() != before {
            return true;
        }
    }
    false
}

#[esp_hal::main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz));
    let delay = Delay::new();
    // Not a wake from deep sleep: just flashed, reset or powered on. Then a USB host may be attached; give its serial monitor a moment to start listening before anything prints.
    let fresh = esp_hal::system::wakeup_cause().is_empty();
    if fresh && usb_host(&delay, 3000) {
        delay.delay_millis(1500);
    }
    println!("boot: reset {:?}, fresh {fresh}", esp_hal::system::reset_reason());
    // Keep the battery switched on: the PWR button starts the board, and BAT_Control holds it on from then, through deep sleep as well. Released and re-held around setting it, so it never drops.
    let mut battery = Output::new(p.GPIO17, Level::High, OutputConfig::default());
    battery.set_pad_hold(false);
    battery.set_pad_hold(true);
    // The green LED (to 3V3, on when its pin is low) stays off, and the panel's power switch (on when low) stays off until a redraw.
    let mut led = Output::new(p.GPIO3, Level::High, OutputConfig::default());
    led.set_pad_hold(false);
    led.set_pad_hold(true);
    let mut panel_power = Output::new(p.GPIO6, Level::High, OutputConfig::default());
    panel_power.set_pad_hold(false);
    // The I2C bus is shared with the audio codecs (and the panel connector's touch lines): unpowered, they clamp it low through their input protection, so both rails go on while the sensors are read, as Waveshare's examples do.
    let mut audio_power = Output::new(p.GPIO42, Level::Low, OutputConfig::default());
    panel_power.set_low();
    delay.delay_millis(10);

    // The battery, through its ÷2 divider on GPIO4 (always connected): averaged over 16 reads, at rest, before the rails and the panel draw current.
    let battery_mv = {
        let mut config = AdcConfig::new();
        let mut pin = config.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO4, Attenuation::_11dB);
        let mut adc = Adc::new(p.ADC1, config);
        let sum: u32 = (0..16).map(|_| adc.read_blocking(&mut pin) as u32).sum();
        (sum / 16 * 2) as u16
    };
    println!("battery: {battery_mv} mV, {} permille", battery_permille(battery_mv));

    let mut low_power = LowPower::new(p.LPWR);
    let mut i2c = I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(100))).expect("I2C").with_sda(p.GPIO47).with_scl(p.GPIO48);
    let now = match sensors::clock(&mut i2c) {
        Some(t) => t,
        None => {
            println!("clock: invalid, setting it to the build time");
            let set = sensors::set_clock(&mut i2c, BUILD_UNIX_SECS);
            println!("clock: set {set}");
            BUILD_UNIX_SECS
        }
    };
    let reading = sensors::shtc3(&mut i2c, &delay);
    audio_power.set_high();
    println!("{now}: {:?}", reading);

    // SAFETY: single-threaded, and nothing else touches the state.
    let state = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
    let hour = now.div_euclid(3600);
    if state.magic != MAGIC || hour < state.newest || state.n < 0 {
        state.reset(hour);
    }
    // Just flashed, reset or powered on: show the panel straight away rather than waiting for the next hour.
    if fresh {
        state.drawn = i64::MIN;
    }
    if hour > state.newest {
        state.advance(hour);
    }
    if let Some(r) = reading {
        state.add(r);
    }

    println!("state: newest {} drawn {} n {}", state.newest, state.drawn, state.n);
    if state.drawn != hour {
        let mut panel = epd::Epd {
            sck: Output::new(p.GPIO12, Level::Low, OutputConfig::default()),
            mosi: Output::new(p.GPIO13, Level::Low, OutputConfig::default()),
            cs: Output::new(p.GPIO11, Level::High, OutputConfig::default()),
            dc: Output::new(p.GPIO10, Level::High, OutputConfig::default()),
            rst: Output::new(p.GPIO9, Level::High, OutputConfig::default()),
            busy: Input::new(p.GPIO8, InputConfig::default()),
            delay,
        };
        // SAFETY: as for STATE.
        let canvas = unsafe { &mut *core::ptr::addr_of_mut!(CANVAS) };
        render(canvas, &state.history(), reading, Some(battery_permille(battery_mv)), now);
        // The refresh runs on its own for ~20 s: on battery, light-sleep through it in quarter seconds instead of polling awake, with the panel's power and control lines held so they can't drift. With a USB host attached, poll awake instead: the USB serial port doesn't survive light sleep, and power doesn't matter there. Capped at a minute.
        let nap = !usb_host(&delay, 20);
        let mut naps = 0;
        let shown = panel.init() && {
            panel.start(canvas);
            panel.hold(true);
            panel_power.set_pad_hold(true);
            while panel.busy() && naps < 240 {
                if nap {
                    low_power.set_wakeup_deadline(Instant::now() + Duration::from_millis(250));
                    low_power.sleep_light(RtcSleepConfig::default());
                } else {
                    delay.delay_millis(250);
                }
                naps += 1;
            }
            panel.hold(false);
            panel_power.set_pad_hold(false);
            !panel.busy()
        };
        panel.sleep();
        println!("panel: {} after {naps} quarter seconds ({})", if shown { "drawn" } else { "timed out" }, if nap { "light sleep" } else { "awake" });
        // Only a finished refresh counts; otherwise the next wake tries again.
        if shown {
            state.drawn = hour;
        }
        panel.release();
    }
    panel_power.set_high();
    panel_power.set_pad_hold(true);

    // Until a couple of seconds past the next 10-minute mark on the clock.
    let wait = (SAMPLE_S - now.rem_euclid(SAMPLE_S) + 2) as u64;
    if usb_host(&delay, if fresh { 3000 } else { 20 }) {
        println!("USB host: waiting {wait} s awake");
        delay.delay_millis(wait as u32 * 1000);
        esp_hal::system::software_reset();
    }
    low_power.set_wakeup_deadline(Instant::now() + Duration::from_secs(wait));
    // The default deep sleep powers RTC slow memory down too; keep it, it holds the week.
    let mut config = RtcSleepConfig::deep();
    config.set_rtc_slowmem_pd_en(false);
    low_power.sleep_deep(config);
}
