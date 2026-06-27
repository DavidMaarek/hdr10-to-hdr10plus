//! End-to-end orchestration: decode + measure -> JSON -> extract -> inject ->
//! mux -> (optional) verify. See spec §1 and §7.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use colored::Colorize;
use ffmpeg_next::{codec, format, frame, media, software, util::color};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::cli::Cli;
use crate::external;
use crate::json;
use crate::measure::{measure_frame, FrameMeasurement};
use crate::scene::{cut_allowed, histogram_difference, label_frames};

/// Run the full conversion for a single input file.
pub fn run(cli: &Cli) -> Result<()> {
    let input = Path::new(&cli.input);
    if !input.exists() {
        bail!("input file not found: {}", cli.input);
    }

    let (measurements, cuts, fps) = analyze(cli).context("frame analysis failed")?;
    if measurements.is_empty() {
        bail!("no frames were decoded from the input");
    }
    let total_frames = measurements.len() as u32;
    println!(
        "Analyzed {} frames; detected {} scene(s).",
        total_frames,
        cuts.len() + 1
    );

    let labels = label_frames(total_frames, cuts);
    let doc = json::build(&measurements, &labels, cli.target_nits);
    let json_text =
        serde_json::to_string_pretty(&doc).context("failed to serialize HDR10+ JSON")?;

    // Decide where the metadata JSON lives and whether we keep it.
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .context("input has no valid file stem")?
        .to_string();
    let parent = input.parent().unwrap_or_else(|| Path::new("."));

    if cli.json_only {
        let json_path = cli.json_out.clone().unwrap_or_else(|| {
            parent
                .join(format!("{stem}.hdr10plus.json"))
                .to_string_lossy()
                .into_owned()
        });
        fs::write(&json_path, &json_text)
            .with_context(|| format!("failed to write {json_path}"))?;
        println!("{} {}", "Wrote metadata JSON:".green(), json_path);
        return Ok(());
    }

    // End-to-end path: work inside a temp dir next to the output.
    let output_path = cli.output.clone().unwrap_or_else(|| {
        parent
            .join(format!("{stem}.HDR10plus.mkv"))
            .to_string_lossy()
            .into_owned()
    });

    if Path::new(&output_path).exists() {
        bail!("output already exists: {output_path}");
    }

    let temp_dir = parent.join(format!("mkvhdr10plus_temp_{stem}"));
    fs::create_dir_all(&temp_dir)
        .with_context(|| format!("failed to create temp dir {}", temp_dir.display()))?;

    let result = inject_and_mux(cli, input, &temp_dir, &output_path, &json_text, fps);

    // The user can request the JSON be preserved regardless of cleanup.
    if let Some(json_out) = &cli.json_out {
        let _ = fs::write(json_out, &json_text);
    }

    if cli.keep_temp {
        println!("Intermediate files kept in {}", temp_dir.display());
    } else {
        let _ = fs::remove_dir_all(&temp_dir);
    }

    result?;
    println!("{} {}", "Done:".green().bold(), output_path);
    Ok(())
}

/// Extract the HEVC bitstream, inject the metadata, remux, and optionally verify.
///
/// `fps` is the source frame rate `(numerator, denominator)`; it is required to
/// give the raw injected HEVC correct timing during the remux step.
fn inject_and_mux(
    cli: &Cli,
    input: &Path,
    temp_dir: &Path,
    output_path: &str,
    json_text: &str,
    fps: (u32, u32),
) -> Result<()> {
    let metadata_json = temp_dir.join("metadata.json");
    let original_hevc = temp_dir.join("original.hevc");
    let injected_hevc = temp_dir.join("injected.hevc");

    fs::write(&metadata_json, json_text)
        .with_context(|| format!("failed to write {}", metadata_json.display()))?;

    // 1. Extract the original HEVC stream (bit-for-bit copy, no re-encode).
    println!("{}", "Extracting HEVC bitstream...".cyan());
    external::run(
        "ffmpeg extract",
        Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(input)
            .args([
                "-map",
                "0:v:0",
                "-c",
                "copy",
                "-bsf:v",
                "hevc_mp4toannexb",
                "-f",
                "hevc",
            ])
            .arg(&original_hevc),
        cli.verbose,
    )?;

    // 2. Inject the generated HDR10+ metadata as SEI.
    println!("{}", "Injecting HDR10+ metadata...".cyan());
    external::run(
        "hdr10plus_tool inject",
        Command::new("hdr10plus_tool")
            .arg("inject")
            .arg("-i")
            .arg(&original_hevc)
            .arg("-j")
            .arg(&metadata_json)
            .arg("-o")
            .arg(&injected_hevc),
        cli.verbose,
    )?;

    // 3. Remux the injected video with the original audio/subtitles/chapters.
    println!("{}", "Remuxing...".cyan());
    remux(cli, input, &injected_hevc, output_path, fps)?;

    // 4. Optional verification: extract the metadata back and report.
    if cli.verify {
        verify(&injected_hevc, temp_dir)?;
    }

    Ok(())
}

/// Remux the injected HEVC with the source audio/subtitles/chapters.
///
/// Prefers `mkvmerge` (passing `--default-duration` so the raw HEVC gets the
/// correct frame rate). If `mkvmerge` is not on `PATH`, falls back to `ffmpeg`,
/// using `-r` on the raw input so packets receive timestamps.
fn remux(
    cli: &Cli,
    input: &Path,
    injected_hevc: &Path,
    output_path: &str,
    fps: (u32, u32),
) -> Result<()> {
    let (num, den) = fps;
    if external::find_tool("mkvmerge").is_some() {
        let default_duration = format!("0:{num}/{den}fps");
        external::run(
            "mkvmerge",
            Command::new("mkvmerge")
                .arg("-o")
                .arg(output_path)
                .arg("--default-duration")
                .arg(&default_duration)
                .arg(injected_hevc)
                .arg("--no-video")
                .arg(input),
            cli.verbose,
        )
    } else {
        eprintln!(
            "{}",
            "mkvmerge not found; remuxing with ffmpeg instead.".yellow()
        );
        let rate = format!("{num}/{den}");
        external::run(
            "ffmpeg remux",
            Command::new("ffmpeg")
                .args(["-y", "-r", rate.as_str(), "-i"])
                .arg(injected_hevc)
                .arg("-i")
                .arg(input)
                .args([
                    "-map",
                    "0:v:0",
                    "-map",
                    "1:a?",
                    "-map",
                    "1:s?",
                    "-map_chapters",
                    "1",
                    "-c",
                    "copy",
                ])
                .arg(output_path),
            cli.verbose,
        )
    }
}

/// Re-extract the metadata from the injected bitstream and print a short
/// summary. We verify the injected HEVC (not the muxed MKV) because
/// `hdr10plus_tool extract` operates on a raw HEVC bitstream; the mux step does
/// not alter the SEI payload.
fn verify(injected_hevc: &Path, temp_dir: &Path) -> Result<()> {
    println!("{}", "Verifying injected metadata...".cyan());
    let verify_json = temp_dir.join("verify.json");
    external::capture_stdout(
        "hdr10plus_tool extract",
        Command::new("hdr10plus_tool")
            .arg("extract")
            .arg(injected_hevc)
            .arg("-o")
            .arg(&verify_json),
    )?;

    let text = fs::read_to_string(&verify_json)
        .with_context(|| format!("failed to read {}", verify_json.display()))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&text).context("verification JSON is not valid JSON")?;
    let scenes = parsed
        .get("SceneInfo")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if scenes == 0 {
        bail!("verification failed: no HDR10+ SceneInfo found in output");
    }
    println!(
        "{} {} frame(s) of HDR10+ metadata present.",
        "Verified:".green(),
        scenes
    );
    Ok(())
}

/// Decode the input and produce one [`FrameMeasurement`] per displayed frame,
/// the list of scene-cut frame indices, and the source frame rate
/// `(numerator, denominator)`.
fn analyze(cli: &Cli) -> Result<(Vec<FrameMeasurement>, Vec<u32>, (u32, u32))> {
    ffmpeg_next::init().context("failed to initialize FFmpeg")?;
    let mut ictx = format::input(&cli.input).context("failed to open input")?;

    let stream = ictx
        .streams()
        .best(media::Type::Video)
        .context("no video stream found")?;
    let stream_index = stream.index();
    let total_hint = {
        let n = stream.frames();
        if n > 0 {
            Some(n as u64)
        } else {
            None
        }
    };
    // Source frame rate, used later to time the remuxed stream. Fall back to
    // 24000/1001 (23.976) if the container does not report a sane value.
    let fps = {
        let r = stream.avg_frame_rate();
        let (n, d) = (r.numerator(), r.denominator());
        if n > 0 && d > 0 {
            (n as u32, d as u32)
        } else {
            (24000, 1001)
        }
    };

    let mut decoder_ctx = codec::context::Context::from_parameters(stream.parameters())
        .context("failed to build decoder context")?;
    // SAFETY: pointer is valid for the lifetime of decoder_ctx; thread_count=0
    // lets FFmpeg pick an automatic thread count.
    unsafe {
        (*decoder_ctx.as_mut_ptr()).thread_count = 0;
    }
    // SAFETY: reading the transfer characteristic enum field is a simple read.
    let trc = unsafe { color::TransferCharacteristic::from((*decoder_ctx.as_ptr()).color_trc) };
    if !matches!(
        trc,
        color::TransferCharacteristic::SMPTE2084
            | color::TransferCharacteristic::BT2020_10
            | color::TransferCharacteristic::BT2020_12
    ) {
        let name = trc.name().unwrap_or("unspecified");
        eprintln!(
            "{}",
            format!(
                "Warning: transfer characteristic is {name}, expected PQ (SMPTE2084). \
                 Measurements assume PQ-coded input."
            )
            .yellow()
        );
    }

    let mut decoder = decoder_ctx
        .decoder()
        .video()
        .context("failed to open video decoder")?;

    let downscale = match cli.downscale {
        1 | 2 | 4 => cli.downscale,
        other => {
            eprintln!("Unsupported --downscale {other}; using 1.");
            1
        }
    };
    let mut target_w = decoder.width();
    let mut target_h = decoder.height();
    if downscale > 1 {
        target_w = (target_w / downscale).max(2) & !1;
        target_h = (target_h / downscale).max(2) & !1;
    }

    let need_scaler = decoder.format() != format::Pixel::YUV420P10LE || downscale > 1;
    let mut scaler = if need_scaler {
        Some(
            software::scaling::Context::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                format::Pixel::YUV420P10LE,
                target_w,
                target_h,
                software::scaling::Flags::BILINEAR,
            )
            .context("failed to create scaling context")?,
        )
    } else {
        None
    };

    let sample_rate = cli.sample_rate.max(1);
    let mut measurements: Vec<FrameMeasurement> = Vec::new();
    let mut cuts: Vec<u32> = Vec::new();
    let mut prev_hist: Option<Vec<f64>> = None;
    let mut last_cut: Option<u32> = None;
    let mut last_measurement: Option<FrameMeasurement> = None;
    let mut frame_count: u32 = 0;

    let pb = match total_hint {
        Some(total) => {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} {msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ETA {eta}")
                    .unwrap()
                    .progress_chars("=>-"),
            );
            pb
        }
        None => {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} {msg} [{elapsed_precise}] {pos} frames")
                    .unwrap(),
            );
            pb
        }
    };
    pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
    pb.set_message("Measuring");

    let mut decoded = frame::Video::empty();
    let mut scaled = frame::Video::empty();

    let mut process = |decoded: &frame::Video,
                       measurements: &mut Vec<FrameMeasurement>,
                       frame_count: &mut u32,
                       prev_hist: &mut Option<Vec<f64>>,
                       last_cut: &mut Option<u32>,
                       last_measurement: &mut Option<FrameMeasurement>,
                       cuts: &mut Vec<u32>|
     -> Result<()> {
        let should_analyze = *frame_count % sample_rate == 0 || last_measurement.is_none();
        let m = if should_analyze {
            let analysis_frame = if let Some(sc) = scaler.as_mut() {
                sc.run(decoded, &mut scaled)
                    .context("failed to scale frame")?;
                &scaled
            } else {
                decoded
            };
            let m = measure_frame(analysis_frame);

            if let Some(prev) = prev_hist.as_ref() {
                let diff = histogram_difference(&m.coarse_histogram, prev);
                if diff > cli.scene_threshold
                    && cut_allowed(*last_cut, *frame_count, cli.min_scene_length)
                {
                    cuts.push(*frame_count);
                    *last_cut = Some(*frame_count);
                }
            }
            *prev_hist = Some(m.coarse_histogram.clone());
            *last_measurement = Some(m.clone());
            m
        } else {
            last_measurement.as_ref().unwrap().clone()
        };

        measurements.push(m);
        *frame_count += 1;
        pb.set_position(*frame_count as u64);
        Ok(())
    };

    for (stream, packet) in ictx.packets() {
        if stream.index() != stream_index {
            continue;
        }
        decoder
            .send_packet(&packet)
            .context("failed to send packet to decoder")?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            process(
                &decoded,
                &mut measurements,
                &mut frame_count,
                &mut prev_hist,
                &mut last_cut,
                &mut last_measurement,
                &mut cuts,
            )?;
        }
    }

    decoder.send_eof().context("failed to flush decoder")?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        process(
            &decoded,
            &mut measurements,
            &mut frame_count,
            &mut prev_hist,
            &mut last_cut,
            &mut last_measurement,
            &mut cuts,
        )?;
    }

    pb.finish_with_message("Measurement complete");
    Ok((measurements, cuts, fps))
}
