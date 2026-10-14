//! The I2C parts: the SHTC3 temperature and humidity sensor (0x70) and the PCF85063 clock (0x51), which keeps UTC.

use climate_render::{Reading, civil_from_days, days_from_civil};
use esp_hal::Blocking;
use esp_hal::delay::Delay;
use esp_hal::i2c::master::I2c;

const SHTC3: u8 = 0x70;
const PCF85063: u8 = 0x51;

/// One SHTC3 measurement (temperature first, normal mode, no clock stretching), then back to sleep: hundredths of a °C and hundredths of a percent, if both words pass their CRC.
pub fn shtc3(i2c: &mut I2c<'_, Blocking>, delay: &Delay) -> Option<Reading> {
    i2c.write(SHTC3, &[0x35, 0x17]).ok()?; // wake
    delay.delay_micros(300);
    i2c.write(SHTC3, &[0x78, 0x66]).ok()?;
    delay.delay_millis(15);
    let mut b = [0u8; 6];
    let read = i2c.read(SHTC3, &mut b);
    let _ = i2c.write(SHTC3, &[0xB0, 0x98]); // sleep
    read.ok()?;
    if crc8(&b[0..2]) != b[2] || crc8(&b[3..5]) != b[5] {
        return None;
    }
    let (t, rh) = (u16::from_be_bytes([b[0], b[1]]) as i32, u16::from_be_bytes([b[3], b[4]]) as i32);
    // T = −45 + 175 · raw / 2^16 °C, RH = 100 · raw / 2^16 %, both in hundredths (the sensor's own resolution) and rounded.
    Some(Reading { temp: (((17500 * t + 32768) >> 16) - 4500) as i16, humidity: ((10000 * rh + 32768) >> 16) as i16 })
}

/// Sensirion's CRC-8: polynomial 0x31, initial 0xFF.
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0xFFu8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { crc << 1 ^ 0x31 } else { crc << 1 };
        }
    }
    crc
}

fn from_bcd(b: u8) -> i64 {
    (b >> 4) as i64 * 10 + (b & 15) as i64
}

fn to_bcd(v: i64) -> u8 {
    ((v / 10) << 4 | v % 10) as u8
}

/// The clock's time as unix seconds; None if it can't be read or its oscillator has stopped since it was last set (the time is then meaningless).
pub fn clock(i2c: &mut I2c<'_, Blocking>) -> Option<i64> {
    let mut b = [0u8; 7];
    i2c.write_read(PCF85063, &[0x04], &mut b).ok()?;
    if b[0] & 0x80 != 0 {
        return None;
    }
    let (s, mi, h) = (from_bcd(b[0] & 0x7F), from_bcd(b[1] & 0x7F), from_bcd(b[2] & 0x3F));
    let (d, mo, y) = (from_bcd(b[3] & 0x3F), from_bcd(b[5] & 0x1F), 2000 + from_bcd(b[6]));
    Some(days_from_civil(y, mo as u32, d as u32) * 86400 + h * 3600 + mi * 60 + s)
}

/// Set the clock to `unix` (UTC), which also clears its oscillator-stopped flag.
pub fn set_clock(i2c: &mut I2c<'_, Blocking>, unix: i64) -> bool {
    let (day, sod) = (unix.div_euclid(86400), unix.rem_euclid(86400));
    let (y, mo, d) = civil_from_days(day);
    let weekday = (day + 4).rem_euclid(7); // 0 = Sunday
    let regs = [0x04, to_bcd(sod % 60), to_bcd(sod / 60 % 60), to_bcd(sod / 3600), to_bcd(d as i64), weekday as u8, to_bcd(mo as i64), to_bcd(y - 2000)];
    i2c.write(PCF85063, &regs).is_ok()
}
