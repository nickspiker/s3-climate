//! The week of indoor climate on the 200×200 four-colour e-paper (black, white, yellow, red), integers only, for the firmware and the PC preview alike.
//!
//! Layout: a one-pixel battery bar along the top edge (yellow track, red fill); the current temperature (red) and humidity (yellow) large below it; below them the last 168 hours, one pixel column per hour with the newest at the right edge, as 2 px lines on black: temperature red, humidity yellow, white where they cross. Each auto-ranges to its own week of values, numbered in its line's colour beside the plot: temperature (°C) on the left, humidity (%) on the right. A white tick at each local midnight (US Pacific, daylight saving included) and the day's name under it.

#![no_std]

pub const W: usize = 200;
pub const H: usize = 200;
/// One pixel column per hour.
pub const HOURS: usize = 168;

/// The panel's four inks, as the canvas holds them.
pub const BLACK: u8 = 0;
pub const WHITE: u8 = 1;
pub const YELLOW: u8 = 2;
pub const RED: u8 = 3;

/// One hour's averages: temperature in tenths of a °C, relative humidity in tenths of a percent.
#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub temp: i16,
    pub humidity: i16,
}

pub struct Glyph {
    pub ch: char,
    pub w: u8,
    pub h: u8,
    /// Offset of the bitmap's top-left from the pen on the baseline.
    pub left: i8,
    pub top: i8,
    pub advance: u8,
    /// One bit per pixel, row by row, most significant bit first.
    pub bits: &'static [u8],
}

pub struct Font {
    pub ascent: i8,
    pub glyphs: &'static [Glyph],
}

include!(concat!(env!("OUT_DIR"), "/glyphs.rs"));

/// The plot: one column per hour from LEFT (the hour 167 hours before the newest) to RIGHT (just past the newest), with each axis's range running from BOTTOM up to BOTTOM − SPAN. The margins either side hold the axis numbers: temperature on the left, humidity on the right.
const LEFT: i32 = 16;
const RIGHT: i32 = LEFT + HOURS as i32;
const BOTTOM: i32 = 183;
const SPAN: i32 = 128;
/// Day ticks run from just below the plot to the bottom edge, dividing the day names.
const TICK_Y: i32 = BOTTOM + 3;
const DAY_BASELINE: i32 = 199;
const READOUT_BASELINE: i32 = 40;

/// One axis's range, in tenths, and the step between its numbers.
#[derive(Clone, Copy)]
struct Axis {
    lo: i32,
    hi: i32,
    step: i32,
}

impl Axis {
    /// Fit the week's values: the smallest step of 1, 2, 5, 10 or 20 units that labels the range (rounded out to whole steps) with at most five numbers, the range at least `min_span` (tenths) so a nearly flat week doesn't blow sensor noise up to full height. With no values, `fallback`.
    fn fit(values: impl Iterator<Item = i16> + Clone, fallback: (i32, i32), min_span: i32) -> Axis {
        let (lo, hi) = match (values.clone().min(), values.max()) {
            (Some(a), Some(b)) => (a as i32, b as i32),
            _ => fallback,
        };
        // Widen about the middle to the minimum span.
        let short = (min_span - (hi - lo)).max(0);
        let (lo, hi) = (lo - short / 2, hi + (short - short / 2));
        for step in [10, 20, 50, 100, 200] {
            let (a, b) = (lo.div_euclid(step) * step, -(-hi).div_euclid(step) * step);
            if (b - a) / step + 1 <= 5 {
                return Axis { lo: a, hi: b, step };
            }
        }
        Axis { lo: lo.div_euclid(500) * 500, hi: -(-hi).div_euclid(500) * 500, step: 500 }
    }

    fn y(&self, tenths: i32) -> i32 {
        (BOTTOM - ((tenths - self.lo) * SPAN + (self.hi - self.lo) / 2) / (self.hi - self.lo)).clamp(BOTTOM - SPAN, BOTTOM)
    }
}

/// Draw the week ending in the hour holding `now` (unix seconds) into `px` (one ink per pixel, row by row). `history[i]` is the hour starting at (now / 3600 − 167 + i) · 3600, so the newest is last; None where there's no reading. `current` is shown large at the top. `battery`: charge in permille, for the bar along the top edge.
pub fn render(px: &mut [u8; W * H], history: &[Option<Reading>; HOURS], current: Option<Reading>, battery: Option<u16>, now: i64) {
    px.fill(BLACK);
    // The battery: the top row, red from the left as far as the charge, yellow beyond.
    if let Some(permille) = battery {
        let fill = (permille.min(1000) as usize * W + 500) / 1000;
        for (x, p) in px[..W].iter_mut().enumerate() {
            *p = if x < fill { RED } else { YELLOW };
        }
    }
    let week = history.iter().flatten().chain(current.iter());
    let axes = [Axis::fit(week.clone().map(|r| r.temp), (180, 260), 20), Axis::fit(week.map(|r| r.humidity), (300, 600), 40)];
    let value = |pick: usize, r: &Reading| if pick == 0 { r.temp } else { r.humidity } as i32;
    // Humidity first, then temperature: where either lands on the other, white.
    for (pick, ink) in [(1, YELLOW), (0, RED)] {
        for i in 1..HOURS {
            if let (Some(a), Some(b)) = (history[i - 1], history[i]) {
                let axis = &axes[pick];
                line(px, (LEFT + i as i32 - 1, axis.y(value(pick, &a))), (LEFT + i as i32, axis.y(value(pick, &b))), ink);
            }
        }
    }

    // Each axis's numbers in its line's colour, centred on their heights: temperature right-aligned against the plot's left edge, humidity left-aligned against its right edge.
    let cap = AXIS.glyphs[0].h as i32;
    for (pick, ink) in [(0, RED), (1, YELLOW)] {
        let axis = &axes[pick];
        let mut v = axis.lo;
        while v <= axis.hi {
            let mut buf = [0u8; 8];
            let s = number(&mut buf, v / 10, "");
            let (x, align) = if pick == 0 { (LEFT - 1, Align::Right) } else { (RIGHT + 1, Align::Left) };
            text(px, &AXIS, s, x, axis.y(v) + cap / 2, align, ink);
            v += axis.step;
        }
    }

    // Local midnights in the window: a tick at each, and the day's name centred under its span where it fits.
    let first_hour = now.div_euclid(3600) - (HOURS as i64 - 1);
    let x_of = |t: i64| LEFT as i64 + (t.div_euclid(3600) - first_hour);
    let mut day = local_day(first_hour * 3600);
    loop {
        let (start, end) = (midnight(day), midnight(day + 1));
        let (x0, x1) = (x_of(start), x_of(end));
        if x0 >= RIGHT as i64 {
            break;
        }
        if x0 >= LEFT as i64 {
            for y in TICK_Y..H as i32 {
                px[y as usize * W + x0 as usize] = WHITE;
            }
        }
        let (a, b) = (x0.max(LEFT as i64), x1.min(RIGHT as i64));
        if b - a >= 18 {
            let name = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"][(day + 4).rem_euclid(7) as usize];
            text(px, &SMALL, name, ((a + b) / 2) as i32, DAY_BASELINE, Align::Centre, WHITE);
        }
        day += 1;
    }

    // Now, large: temperature on the left, humidity on the right, rounded to whole units.
    if let Some(r) = current {
        let mut buf = [0u8; 8];
        let t = number(&mut buf, (r.temp as i32 + 5).div_euclid(10), "°");
        text(px, &BIG, t, W as i32 / 4 + 4, READOUT_BASELINE, Align::Centre, RED);
        let h = number(&mut buf, (r.humidity as i32 + 5).div_euclid(10), "%");
        text(px, &BIG, h, 3 * W as i32 / 4, READOUT_BASELINE, Align::Centre, YELLOW);
    }
}

/// A 2 px line from `a` to `b`, stamping a 2×2 brush along it (clipped to the plot); landing on the other line's ink makes white.
fn line(px: &mut [u8; W * H], (mut x, mut y): (i32, i32), (x1, y1): (i32, i32), ink: u8) {
    let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
    let (sx, sy) = (if x < x1 { 1 } else { -1 }, if y < y1 { 1 } else { -1 });
    let mut err = dx + dy;
    loop {
        for (ox, oy) in [(0, 0), (-1, 0), (0, 1), (-1, 1)] {
            let (px_x, px_y) = (x + ox, y + oy);
            if (LEFT..RIGHT).contains(&px_x) && (0..H as i32).contains(&px_y) {
                let p = &mut px[px_y as usize * W + px_x as usize];
                *p = if *p == BLACK || *p == ink { ink } else { WHITE };
            }
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

enum Align {
    Left,
    Right,
    Centre,
}

/// `s` in `font` with its baseline at `baseline`, starting at `x` (Left), ending there (Right) or centred on it.
fn text(px: &mut [u8; W * H], font: &Font, s: &str, x: i32, baseline: i32, align: Align, ink: u8) {
    let glyph = |ch| font.glyphs.iter().find(|g| g.ch == ch);
    let width: i32 = s.chars().filter_map(glyph).map(|g| g.advance as i32).sum();
    let mut pen = match align {
        Align::Left => x,
        Align::Right => x - width,
        Align::Centre => x - width / 2,
    };
    for g in s.chars().filter_map(glyph) {
        for gy in 0..g.h as i32 {
            for gx in 0..g.w as i32 {
                let bit = gy * g.w as i32 + gx;
                if g.bits[(bit / 8) as usize] & (0x80 >> (bit % 8)) != 0 {
                    let (cx, cy) = (pen + g.left as i32 + gx, baseline + g.top as i32 + gy);
                    if (0..W as i32).contains(&cx) && (0..H as i32).contains(&cy) {
                        px[cy as usize * W + cx as usize] = ink;
                    }
                }
            }
        }
        pen += g.advance as i32;
    }
}

/// `n` in decimal followed by `unit`, written into `buf`.
fn number<'a>(buf: &'a mut [u8; 8], n: i32, unit: &str) -> &'a str {
    let mut digits = [0u8; 6];
    let (mut m, mut k) = (n.unsigned_abs(), 0);
    loop {
        digits[k] = b'0' + (m % 10) as u8;
        k += 1;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    let mut len = 0;
    if n < 0 {
        buf[0] = b'-';
        len = 1;
    }
    for d in digits[..k].iter().rev() {
        buf[len] = *d;
        len += 1;
    }
    buf[len..len + unit.len()].copy_from_slice(unit.as_bytes());
    len += unit.len();
    core::str::from_utf8(&buf[..len]).unwrap()
}

/// The local (US Pacific) day number holding `unix`: days since 1970-01-01.
fn local_day(unix: i64) -> i64 {
    (unix + tz_offset_secs(unix)).div_euclid(86400)
}

/// Unix seconds of local midnight starting local day `day`. Daylight saving changes at 02:00, so midnight keeps the evening before's offset; 07:00 UTC is still before either change.
fn midnight(day: i64) -> i64 {
    day * 86400 - tz_offset_secs(day * 86400 + 7 * 3600)
}

// US Pacific local time, DST-aware, from the tide clock firmware (tideglyph-fw): integer calendar maths (Howard Hinnant), day number ↔ civil date, then the US DST window.
/// Civil date (year, month, day) of day number `z0` (days since 1970-01-01).
pub fn civil_from_days(z0: i64) -> (i64, u32, u32) {
    let z = z0 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Day number (days since 1970-01-01) of a civil date.
pub fn days_from_civil(y0: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y0 - 1 } else { y0 };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + (d as i64 - 1);
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Unix seconds of `hour_utc` on the `n`th Sunday of `month` in `year`.
fn nth_sunday_unix(year: i64, month: u32, n: i64, hour_utc: i64) -> i64 {
    let first = days_from_civil(year, month, 1);
    let wd = (first + 4).rem_euclid(7); // 0 = Sunday (1970-01-01 was a Thursday)
    let first_sunday_dom = 1 + ((7 - wd) % 7);
    days_from_civil(year, month, (first_sunday_dom + 7 * (n - 1)) as u32) * 86400 + hour_utc * 3600
}

/// Pacific offset at `unix`: PDT (−7 h) inside the US DST window (2nd Sunday of March 02:00 → 1st Sunday of November 02:00 local), else PST (−8 h).
pub fn tz_offset_secs(unix: i64) -> i64 {
    let (year, _, _) = civil_from_days(unix.div_euclid(86400));
    let dst_start = nth_sunday_unix(year, 3, 2, 10); // 02:00 PST = 10:00 UTC
    let dst_end = nth_sunday_unix(year, 11, 1, 9); // 02:00 PDT = 09:00 UTC
    if unix >= dst_start && unix < dst_end { -7 * 3600 } else { -8 * 3600 }
}

/// A 1S LiPo's charge (permille) from its resting voltage (mV), on the standard discharge curve (the tide clock's): empty at 3.27 V and below, full at 4.20 V and above. Sample after minutes idle, so the cell is near open-circuit.
pub fn battery_permille(mv: u16) -> u16 {
    // (mV, permille), descending.
    const CURVE: [(u16, u16); 21] = [
        (4200, 1000), (4150, 950), (4110, 900), (4080, 850), (4020, 800),
        (3980, 750), (3950, 700), (3910, 650), (3870, 600), (3850, 550),
        (3840, 500), (3820, 450), (3800, 400), (3790, 350), (3770, 300),
        (3750, 250), (3730, 200), (3710, 150), (3690, 100), (3610, 50),
        (3270, 0),
    ];
    if mv >= CURVE[0].0 {
        return 1000;
    }
    let i = CURVE.iter().position(|&(v, _)| mv >= v).unwrap_or(CURVE.len());
    if i == CURVE.len() {
        return 0;
    }
    let ((hv, hp), (lv, lp)) = (CURVE[i - 1], CURVE[i]);
    lp + ((hp - lp) as u32 * (mv - lv) as u32 / (hv - lv) as u32) as u16
}
