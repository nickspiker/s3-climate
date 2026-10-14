//! Render the climate panel on the PC through the firmware's own code, from a week of mock readings (a daily swing and a slow drift, nothing measured), and write it at 3× to /tmp/s3-climate-preview/panel.png in approximate ink colours.

use std::time::{SystemTime, UNIX_EPOCH};

use climate_render::{BLACK, H, HOURS, RED, Reading, W, WHITE, YELLOW, render};

const OUT: &str = "/tmp/s3-climate-preview/panel.png";

fn main() {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let first_hour = now / 3600 - (HOURS as i64 - 1);
    let mut history = [None; HOURS];
    for (i, slot) in history.iter_mut().enumerate() {
        let local_h = ((first_hour + i as i64) * 3600 + climate_render::tz_offset_secs(now)) as f64 / 3600.0;
        let day = (std::f64::consts::TAU * (local_h - 15.0) / 24.0).cos();
        let drift = (i as f64 / 40.0).sin();
        let wobble = ((i * 7919) % 13) as f64 / 13.0 - 0.5;
        // A gap of a few hours, as when the battery ran flat.
        if (60..64).contains(&i) {
            continue;
        }
        *slot = Some(Reading { temp: (2120.0 + 140.0 * day + 80.0 * drift + 20.0 * wobble) as i16, humidity: (4700.0 - 600.0 * day - 400.0 * drift + 80.0 * wobble) as i16 });
    }
    let mut px = Box::new([0u8; W * H]);
    render(&mut px, &history, history[HOURS - 1], Some(620), now);

    const Z: usize = 3;
    let ink = |v: u8| match v {
        BLACK => [24, 24, 26],
        WHITE => [228, 226, 218],
        YELLOW => [232, 192, 0],
        RED => [182, 28, 32],
        _ => [255, 0, 255],
    };
    let mut rgb = vec![0u8; W * Z * H * Z * 3];
    for y in 0..H * Z {
        for x in 0..W * Z {
            rgb[(y * W * Z + x) * 3..][..3].copy_from_slice(&ink(px[(y / Z) * W + x / Z]));
        }
    }
    std::fs::create_dir_all("/tmp/s3-climate-preview").unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(OUT).unwrap()), (W * Z) as u32, (H * Z) as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
    println!("{OUT}");
}
