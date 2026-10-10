//! cinestill IN OUT [--preset 800t|50d] [--halation 0.8] [--grain 0.35] ...

use std::process::ExitCode;

use cinestill::{apply, Params};

const USAGE: &str = "\
usage: cinestill INPUT OUTPUT [options]

Gives a photo the CineStill look: red halation around lights, a tungsten
cast, faded blacks and film grain. Reads and writes JPEG, PNG and WebP.

options (defaults are the 800T preset):
  --preset 800t|50d        start from a preset
  --halation N             glow strength, 0–2
  --spread N               glow radius, 0.25–3
  --grain N                grain amount, 0–1
  --grain-size N           grain size, 0.5–3
  --tungsten N             cool cast and teal shadows, 0–1
  --contrast N             0–1
  --fade N                 lifted blacks, 0–1
  --saturation N           0–2
  --vignette N             0–1
  --seed N                 grain pattern
  --quality N              JPEG quality, 1–100 (default 92)";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    let mut paths = Vec::new();
    let mut p = Params::default();
    let mut quality = 92u8;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if !arg.starts_with("--") {
            paths.push(arg);
            continue;
        }
        let value = it.next().ok_or_else(|| format!("{arg} needs a value\n\n{USAGE}"))?;
        let num = || value.parse::<f32>().map_err(|_| format!("{arg}: '{value}' is not a number"));
        match arg.as_str() {
            "--preset" => {
                p = match value.to_ascii_lowercase().as_str() {
                    "800t" => Params::NIGHT_800T,
                    "50d" => Params::DAYLIGHT_50D,
                    _ => return Err(format!("unknown preset '{value}', use 800t or 50d")),
                }
            }
            "--halation" => p.halation = num()?,
            "--spread" => p.halation_spread = num()?,
            "--grain" => p.grain = num()?,
            "--grain-size" => p.grain_size = num()?,
            "--tungsten" => p.tungsten = num()?,
            "--contrast" => p.contrast = num()?,
            "--fade" => p.fade = num()?,
            "--saturation" => p.saturation = num()?,
            "--vignette" => p.vignette = num()?,
            "--seed" => p.seed = value.parse().map_err(|_| format!("--seed: '{value}' is not a whole number"))?,
            "--quality" => quality = value.parse().map_err(|_| format!("--quality: '{value}' is not 1–100"))?,
            _ => return Err(format!("unknown option {arg}\n\n{USAGE}")),
        }
    }
    let [input, output] = paths.as_slice() else {
        return Err(USAGE.to_string());
    };

    let img = image::open(input).map_err(|e| format!("can't read {input}: {e}"))?;
    let mut rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    apply(&mut rgba, w as usize, h as usize, &p);

    let lower = output.to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        let file = std::fs::File::create(output).map_err(|e| format!("can't write {output}: {e}"))?;
        let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), quality.clamp(1, 100));
        enc.encode_image(&rgb).map_err(|e| format!("can't write {output}: {e}"))?;
    } else {
        rgba.save(output).map_err(|e| format!("can't write {output}: {e}"))?;
    }
    eprintln!("{input} → {output} ({w}×{h})");
    Ok(())
}
