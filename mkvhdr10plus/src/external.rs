//! External-tool discovery and invocation for the inject/mux chain (spec §7).
//!
//! Decoding and measurement use the FFmpeg *libraries* (via `ffmpeg-next`), so
//! the only CLI tools needed are `ffmpeg` (bitstream extraction), `mkvmerge`
//! (remux) and `hdr10plus_tool` (SEI injection / verification).

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use colored::Colorize;

/// Locate a tool on `PATH`. Returns its resolved path if found.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let locator = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    let output = Command::new(locator)
        .arg(name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(PathBuf::from)
}

/// Ensure the tools required for the end-to-end chain are present.
///
/// `ffmpeg` and `hdr10plus_tool` are mandatory. `mkvmerge` is optional: when it
/// is absent the remux step falls back to `ffmpeg`, so we only warn.
pub fn check_dependencies(json_only: bool) -> Result<()> {
    if json_only {
        return Ok(());
    }
    let required = ["ffmpeg", "hdr10plus_tool"];
    let mut missing = Vec::new();
    for tool in required {
        if find_tool(tool).is_none() {
            missing.push(tool);
        }
    }
    if !missing.is_empty() {
        for tool in &missing {
            eprintln!(
                "{}",
                format!("Error: required command '{tool}' not found in PATH.").red()
            );
        }
        bail!("missing dependencies: {}", missing.join(", "));
    }
    if find_tool("mkvmerge").is_none() {
        eprintln!(
            "{}",
            "Note: mkvmerge not found; the remux step will use ffmpeg instead.".yellow()
        );
    }
    Ok(())
}

/// Run a command to completion. With `verbose`, output is inherited so the
/// user sees live progress; otherwise it is captured and only surfaced on
/// failure.
pub fn run(label: &str, cmd: &mut Command, verbose: bool) -> Result<()> {
    if verbose {
        let status = cmd
            .status()
            .with_context(|| format!("failed to spawn {label}"))?;
        if !status.success() {
            bail!("{label} failed with status {status}");
        }
        return Ok(());
    }

    let output = cmd
        .output()
        .with_context(|| format!("failed to spawn {label}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.lines().rev().take(8).collect::<Vec<_>>().join("\n");
        bail!("{label} failed with status {}\n{tail}", output.status);
    }
    Ok(())
}

/// Run a command and capture stdout as a UTF-8 string (stderr discarded).
pub fn capture_stdout(label: &str, cmd: &mut Command) -> Result<String> {
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("failed to spawn {label}"))?;
    if !output.status.success() {
        bail!("{label} failed with status {}", output.status);
    }
    String::from_utf8(output.stdout).with_context(|| format!("{label} produced non-UTF-8 output"))
}
