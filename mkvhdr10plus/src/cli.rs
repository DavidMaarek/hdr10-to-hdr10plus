use clap::Parser;

/// Generate HDR10+ dynamic metadata (Profile B) from an HDR10 source and inject
/// it into the original HEVC stream without re-encoding.
#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Input HDR10 video file (MKV/MP4 with a PQ HEVC stream).
    #[arg(required = true)]
    pub input: String,

    /// Output MKV path. Defaults to `<input_stem>.HDR10plus.mkv`.
    #[arg(short, long)]
    pub output: Option<String>,

    /// Only generate the metadata JSON; skip the extract/inject/mux chain.
    #[arg(long)]
    pub json_only: bool,

    /// Write the generated metadata JSON to this path (kept after the run).
    #[arg(long)]
    pub json_out: Option<String>,

    /// TargetedSystemDisplayMaximumLuminance written for every scene (Profile B
    /// requires a non-zero value). Must be in `1..=100000`.
    #[arg(long, default_value_t = 1000)]
    pub target_nits: u32,

    /// Downscale factor applied before measurement (1, 2 or 4). Speeds up
    /// analysis at a small accuracy cost.
    #[arg(long, default_value_t = 1)]
    pub downscale: u32,

    /// Analyze every Nth frame and repeat its measurement for skipped frames.
    #[arg(long, default_value_t = 1)]
    pub sample_rate: u32,

    /// Scene-cut threshold for the histogram difference metric.
    #[arg(long, default_value_t = 0.35)]
    pub scene_threshold: f64,

    /// Minimum number of frames between two scene cuts.
    #[arg(long, default_value_t = 24)]
    pub min_scene_length: u32,

    /// Keep intermediate files (extracted HEVC, injected HEVC, metadata JSON).
    #[arg(long)]
    pub keep_temp: bool,

    /// After muxing, extract the injected metadata back and report a summary.
    #[arg(long)]
    pub verify: bool,

    /// Verbose: stream external tool output to the terminal.
    #[arg(short, long)]
    pub verbose: bool,
}
