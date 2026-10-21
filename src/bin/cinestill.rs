//! cinestill IN OUT [--preset 800t|50d|portra400|…] [--halation 0.8] [--grain 0.35] ...
//! cinestill IN... --out-dir DIR [options]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cinestill::jpeg::{encode, Options, Pixels};
use cinestill::meta::read_jpeg;
use cinestill::{apply, Developer, MaskBuilder, Params};
use image::ImageDecoder;

const USAGE: &str = "\
usage: cinestill INPUT OUTPUT [options]
       cinestill INPUT... --out-dir DIR [options]

Gives photos a film look: red halation around lights, a tungsten cast,
faded blacks and grain, or another stock. Reads JPEG, PNG and WebP; writes
JPEG (keeping the original's camera details, not its location), PNG or
WebP. With --out-dir, each photo is saved there as NAME-STOCK.jpg.

options (defaults are the 800t stock):
  --preset NAME            800t, 50d, portra400, ektar100, superia400, trix400
  --halation N             glow strength, 0–2
  --spread N               glow radius, 0.25–3
  --grain N                grain amount, 0–1
  --grain-size N           grain size, 0.5–3
  --tungsten N             cool cast and teal shadows, 0–1
  --warmth N               cool to warm, -1–1
  --tint N                 green to magenta, -1–1
  --contrast N             0–1
  --fade N                 lifted blacks, 0–1
  --saturation N           0–2
  --mono N                 black and white, 0–1
  --vignette N             0–1
  --leak N                 light leak, 0–1
  --seed N                 grain pattern and leak placement
  --quality N              JPEG quality, 1–100 (default 92)
  --keep-location          keep GPS data from the original";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

struct Job {
    p: Params,
    stock: String,
    quality: u8,
    keep_location: bool,
}

fn run(args: Vec<String>) -> Result<(), String> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    let mut paths = Vec::new();
    let mut out_dir = None;
    let mut job = Job { p: Params::default(), stock: "800t".into(), quality: 92, keep_location: false };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if !arg.starts_with("--") {
            paths.push(arg);
            continue;
        }
        if arg == "--keep-location" {
            job.keep_location = true;
            continue;
        }
        let value = it.next().ok_or_else(|| format!("{arg} needs a value\n\n{USAGE}"))?;
        let num = || value.parse::<f32>().map_err(|_| format!("{arg}: '{value}' is not a number"));
        let p = &mut job.p;
        match arg.as_str() {
            "--preset" => {
                let name = value.to_ascii_lowercase().replace(['-', ' '], "");
                let (name, stock) = Params::STOCKS
                    .iter()
                    .find(|(n, _)| *n == name)
                    .ok_or_else(|| format!("unknown preset '{value}', use one of: {}", stock_names()))?;
                (job.stock, *p) = (name.to_string(), *stock);
            }
            "--halation" => p.halation = num()?,
            "--spread" => p.halation_spread = num()?,
            "--grain" => p.grain = num()?,
            "--grain-size" => p.grain_size = num()?,
            "--tungsten" => p.tungsten = num()?,
            "--warmth" => p.warmth = num()?,
            "--tint" => p.tint = num()?,
            "--contrast" => p.contrast = num()?,
            "--fade" => p.fade = num()?,
            "--saturation" => p.saturation = num()?,
            "--mono" => p.mono = num()?,
            "--vignette" => p.vignette = num()?,
            "--leak" => p.leak = num()?,
            "--seed" => p.seed = value.parse().map_err(|_| format!("--seed: '{value}' is not a whole number"))?,
            "--quality" => job.quality = value.parse().map_err(|_| format!("--quality: '{value}' is not 1–100"))?,
            "--out-dir" => out_dir = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {arg}\n\n{USAGE}")),
        }
    }

    match (out_dir, paths.as_slice()) {
        (None, [input, output]) => develop(input.as_ref(), output.as_ref(), &job),
        (Some(dir), inputs) if !inputs.is_empty() => {
            std::fs::create_dir_all(&dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
            let mut failed = 0;
            for input in inputs {
                let input = Path::new(input);
                let stem = input.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
                let output = dir.join(format!("{stem}-{}.jpg", job.stock));
                if let Err(e) = develop(input, &output, &job) {
                    eprintln!("{e}");
                    failed += 1;
                }
            }
            match failed {
                0 => Ok(()),
                n => Err(format!("{n} of {} photos failed", inputs.len())),
            }
        }
        _ => Err(USAGE.to_string()),
    }
}

fn stock_names() -> String {
    Params::STOCKS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
}

fn develop(input: &Path, output: &Path, job: &Job) -> Result<(), String> {
    let (inp, out) = (input.display(), output.display());
    let bytes = std::fs::read(input).map_err(|e| format!("can't read {inp}: {e}"))?;
    let read = || -> image::ImageResult<image::DynamicImage> {
        let mut decoder = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?.into_decoder()?;
        // Turned upright, as the saved EXIF will say.
        let orientation = decoder.orientation()?;
        let mut img = image::DynamicImage::from_decoder(decoder)?;
        img.apply_orientation(orientation);
        Ok(img)
    };
    let mut rgba = read().map_err(|e| format!("can't read {inp}: {e}"))?.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);

    let lower = output.to_string_lossy().to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        let mut b = MaskBuilder::new(w, h);
        b.add_rows(&rgba);
        let dev = Developer::new(&b.finish(), w, h, &job.p);
        let meta = read_jpeg(&bytes);
        let opts = Options { quality: job.quality, meta: &meta, keep_location: job.keep_location, display_p3: true, border: None };
        let jpeg = encode(Pixels { rgba: &rgba, width: w }, dev, w, h, &opts)
            .map_err(|e| format!("can't write {out}: {e}"))?;
        std::fs::write(output, jpeg).map_err(|e| format!("can't write {out}: {e}"))?;
    } else {
        apply(&mut rgba, w, h, &job.p);
        rgba.save(output).map_err(|e| format!("can't write {out}: {e}"))?;
    }
    eprintln!("{inp} → {out} ({w}×{h})");
    Ok(())
}
