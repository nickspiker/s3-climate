//! Bake the glyphs as const tables, one bit per pixel: Oxanium rasterised at each size (on where the outline covers at least half the pixel), and the axis digits hand-drawn 6×12 (assets/font/axis, white is ink) in the style of the tide clock's 12 px digits, narrow enough that two fit beside the plot. Float is fine here: this runs on the PC at build time.

use std::fmt::Write as _;
use std::path::PathBuf;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};

const TTF: &str = "../assets/font/Oxanium-Regular+glyphs.ttf";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed={TTF}");
    let ttf = std::fs::read(TTF).expect("read font");
    let font = FontRef::try_from_slice(&ttf).expect("parse font");
    let mut src = String::new();
    // Readouts: the current temperature and humidity.
    emit(&mut src, &font, "BIG", 44.0, "0123456789-°%");
    pixel_font(&mut src, "AXIS", "axis", "0123456789");
    // Labels: day names, and the dozenal digits Zil..Stelor (U+0010..U+001B).
    let small: String = "SMTWFuoehra".chars().chain((0x10..0x1C).filter_map(char::from_u32)).collect();
    emit(&mut src, &font, "SMALL", 13.0, &small);
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("glyphs.rs");
    std::fs::write(out, src).expect("write glyphs.rs");
}

/// `pub static <name>: Font` with `chars` at `em` pixels to the em (as CSS means font size)
fn emit(src: &mut String, font: &FontRef, name: &str, em: f32, chars: &str) {
    let scale = PxScale::from(em * font.height_unscaled() / font.units_per_em().unwrap());
    let sf = font.as_scaled(scale);
    let _ = writeln!(src, "pub static {name}: Font = Font {{ ascent: {}, glyphs: &[", sf.ascent().round() as i32);
    for ch in chars.chars() {
        let id = font.glyph_id(ch);
        let advance = sf.h_advance(id).round() as i32;
        let (mut w, mut h, mut left, mut top, mut bits) = (0, 0, 0, 0, Vec::new());
        if let Some(outline) = font.outline_glyph(id.with_scale_and_position(scale, point(0.0, 0.0))) {
            let b = outline.px_bounds();
            (w, h, left, top) = (b.width() as usize, b.height() as usize, b.min.x as i32, b.min.y as i32);
            let mut on = vec![false; w * h];
            outline.draw(|x, y, c| on[y as usize * w + x as usize] = c >= 0.5);
            bits = pack(&on);
        }
        let _ = writeln!(src, "    Glyph {{ ch: {:?}, w: {w}, h: {h}, left: {left}, top: {top}, advance: {advance}, bits: &{bits:?} }},", ch);
    }
    src.push_str("] };\n");
}

/// Pack on/off pixels eight to a byte, most significant bit first.
fn pack(on: &[bool]) -> Vec<u8> {
    on.chunks(8).map(|byte| byte.iter().enumerate().fold(0u8, |a, (i, &v)| a | (v as u8) << (7 - i))).collect()
}


/// `pub static <name>: Font` from hand-drawn 1-bit PNGs in assets/font/<dir>, one per character named after it: each stands on the baseline, 1 px from the next.
fn pixel_font(src: &mut String, name: &str, dir: &str, chars: &str) {
    let _ = writeln!(src, "pub static {name}: Font = Font {{ ascent: 0, glyphs: &[");
    for ch in chars.chars() {
        let path = format!("../assets/font/{dir}/{ch}.png");
        println!("cargo::rerun-if-changed={path}");
        let mut dec = png::Decoder::new(std::fs::File::open(&path).expect("open glyph PNG"));
        dec.set_transformations(png::Transformations::EXPAND);
        let mut reader = dec.read_info().expect("glyph PNG header");
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("glyph PNG");
        let (w, h) = (info.width as usize, info.height as usize);
        let bpp = (info.line_size / w).max(1);
        let on: Vec<bool> = (0..w * h).map(|i| buf[(i / w) * info.line_size + (i % w) * bpp] > 128).collect();
        let _ = writeln!(src, "    Glyph {{ ch: {ch:?}, w: {w}, h: {h}, left: 0, top: -{h}, advance: {}, bits: &{:?} }},", w + 1, pack(&on));
    }
    src.push_str("] };\n");
}
