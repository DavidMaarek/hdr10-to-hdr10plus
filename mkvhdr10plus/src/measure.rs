//! Per-frame HDR10+ luminance measurement.
//!
//! Implements section 2 of `docs/HDR10plus_writer_spec.md`:
//! decode a `yuv420p10le` frame, upsample the chroma to full resolution,
//! convert YCbCr (limited range, BT.2020 non-constant luminance) to PQ-coded
//! R'G'B', apply the ST 2084 EOTF to obtain linear light normalized to
//! `[0, 1]` (where `1.0 == 10000 nits`), then derive the per-channel `MaxScl`,
//! the `AverageRGB` of `maxRGB`, and the `maxRGB` percentile distribution.
//!
//! All output magnitudes are stored as integers scaled by `100000` and clamped
//! to `[0, 100000]`, matching what `hdr10plus_tool inject` expects.

use ffmpeg_next::frame;
use rayon::prelude::*;

/// Output scale: a normalized linear value of `1.0` maps to this integer.
pub const SCALE: f64 = 100_000.0;
/// Maximum value any luminance field may take (validation rule, spec §6).
pub const MAX_VALUE: u32 = 100_000;
/// Number of integer buckets in the fine `maxRGB` histogram (`0..=MAX_VALUE`).
const FINE_BINS: usize = MAX_VALUE as usize + 1;
/// Number of coarse buckets used only for the scene-change metric.
const COARSE_BINS: usize = 256;

/// Distribution indices fixed by the HDR10+ spec (9-value form).
pub const DISTRIBUTION_INDEX: [u8; 9] = [1, 5, 10, 25, 50, 75, 90, 95, 99];

/// Percentiles applied to each distribution slot. The slot whose
/// `DistributionIndex` is `5` is a *reserved* value that tracks the scene peak,
/// so it uses the 99.98th percentile rather than the 5th (spec §2.3).
const DISTRIBUTION_PERCENTILES: [f64; 9] = [1.0, 99.98, 10.0, 25.0, 50.0, 75.0, 90.0, 95.0, 99.0];

/// Result of analyzing a single frame.
#[derive(Clone, Debug)]
pub struct FrameMeasurement {
    /// Per-channel maxima `[R, G, B]`, scaled by `SCALE`, clamped to `MAX_VALUE`.
    pub max_scl: [u32; 3],
    /// Mean of `maxRGB`, scaled by `SCALE`, clamped to `MAX_VALUE`.
    pub average_rgb: u32,
    /// Distribution values (one per `DISTRIBUTION_INDEX` slot), scaled/clamped.
    pub distribution: [u32; 9],
    /// Normalized 256-bin histogram of `maxRGB` (sums to ~100.0), used only for
    /// scene-cut detection.
    pub coarse_histogram: Vec<f64>,
}

impl FrameMeasurement {
    /// A measurement for an all-black / empty frame.
    fn empty() -> Self {
        FrameMeasurement {
            max_scl: [0, 0, 0],
            average_rgb: 0,
            distribution: [0; 9],
            coarse_histogram: vec![0.0; COARSE_BINS],
        }
    }
}

/// ST 2084 (PQ) EOTF. Maps a PQ-coded value `e` in `[0, 1]` to linear light in
/// `[0, 1]`, where `1.0` corresponds to 10000 nits.
#[inline]
pub fn pq_eotf(e: f64) -> f64 {
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 4096.0 * 128.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 4096.0 * 32.0;
    const C3: f64 = 2392.0 / 4096.0 * 32.0;

    let e = e.clamp(0.0, 1.0);
    let p = e.powf(1.0 / M2);
    let num = (p - C1).max(0.0);
    let den = C2 - C3 * p;
    if den <= 0.0 {
        return 0.0;
    }
    (num / den).powf(1.0 / M1).clamp(0.0, 1.0)
}

/// Per-thread accumulator for the parallel row reduction.
struct Accum {
    fine: Vec<u64>,
    coarse: Vec<u64>,
    max_r: f64,
    max_g: f64,
    max_b: f64,
    sum_maxrgb: f64,
    count: u64,
}

impl Accum {
    fn new() -> Self {
        Accum {
            fine: vec![0u64; FINE_BINS],
            coarse: vec![0u64; COARSE_BINS],
            max_r: 0.0,
            max_g: 0.0,
            max_b: 0.0,
            sum_maxrgb: 0.0,
            count: 0,
        }
    }

    fn merge(mut self, other: Accum) -> Accum {
        for (a, b) in self.fine.iter_mut().zip(other.fine.iter()) {
            *a += *b;
        }
        for (a, b) in self.coarse.iter_mut().zip(other.coarse.iter()) {
            *a += *b;
        }
        self.max_r = self.max_r.max(other.max_r);
        self.max_g = self.max_g.max(other.max_g);
        self.max_b = self.max_b.max(other.max_b);
        self.sum_maxrgb += other.sum_maxrgb;
        self.count += other.count;
        self
    }
}

/// Read a 10-bit little-endian sample at byte offset `off` from `plane`.
#[inline]
fn sample10(plane: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([plane[off], plane[off + 1]]) & 0x03FF
}

/// Bilinearly sample a chroma plane (quarter-resolution, 4:2:0) at luma
/// coordinates `(x, y)`. Approximate center siting: chroma grid is mapped via
/// `cx = x / 2`, `cy = y / 2`. Returns the 10-bit code as `f64`.
#[inline]
fn chroma_bilinear(plane: &[u8], stride: usize, cw: usize, ch: usize, x: usize, y: usize) -> f64 {
    let fx = x as f64 * 0.5;
    let fy = y as f64 * 0.5;
    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(cw.saturating_sub(1));
    let y1 = (y0 + 1).min(ch.saturating_sub(1));
    let x0 = x0.min(cw.saturating_sub(1));
    let y0 = y0.min(ch.saturating_sub(1));
    let wx = fx - fx.floor();
    let wy = fy - fy.floor();

    let v00 = sample10(plane, y0 * stride + x0 * 2) as f64;
    let v01 = sample10(plane, y0 * stride + x1 * 2) as f64;
    let v10 = sample10(plane, y1 * stride + x0 * 2) as f64;
    let v11 = sample10(plane, y1 * stride + x1 * 2) as f64;

    let top = v00 + (v01 - v00) * wx;
    let bot = v10 + (v11 - v10) * wx;
    top + (bot - top) * wy
}

/// Analyze one decoded `yuv420p10le` frame and return its HDR10+ measurement.
pub fn measure_frame(frame: &frame::Video) -> FrameMeasurement {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    if width == 0 || height == 0 {
        return FrameMeasurement::empty();
    }

    let y_plane = frame.data(0);
    let u_plane = frame.data(1);
    let v_plane = frame.data(2);
    let y_stride = frame.stride(0);
    let u_stride = frame.stride(1);
    let v_stride = frame.stride(2);

    // Chroma plane dimensions for 4:2:0.
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);

    let coarse_scale = (COARSE_BINS - 1) as f64;

    let acc = (0..height)
        .into_par_iter()
        .fold(Accum::new, |mut acc, y| {
            let y_row = y * y_stride;
            for x in 0..width {
                let y_off = y_row + x * 2;
                if y_off + 1 >= y_plane.len() {
                    continue;
                }
                let y_code = sample10(y_plane, y_off) as f64;
                let cb_code = chroma_bilinear(u_plane, u_stride, cw, ch, x, y);
                let cr_code = chroma_bilinear(v_plane, v_stride, cw, ch, x, y);

                // Limited-range 10-bit normalization (spec §2.1 step 3).
                let yl = ((y_code - 64.0) / 876.0).clamp(0.0, 1.0);
                let cb = (cb_code - 512.0) / 896.0;
                let cr = (cr_code - 512.0) / 896.0;

                // YCbCr -> R'G'B' (BT.2020 NCL), still PQ-coded (step 4).
                let rp = (yl + 1.4746 * cr).clamp(0.0, 1.0);
                let gp = (yl - 0.16455 * cb - 0.57135 * cr).clamp(0.0, 1.0);
                let bp = (yl + 1.8814 * cb).clamp(0.0, 1.0);

                // PQ EOTF -> linear light (step 5).
                let r = pq_eotf(rp);
                let g = pq_eotf(gp);
                let b = pq_eotf(bp);

                if r > acc.max_r {
                    acc.max_r = r;
                }
                if g > acc.max_g {
                    acc.max_g = g;
                }
                if b > acc.max_b {
                    acc.max_b = b;
                }

                let max_rgb = r.max(g).max(b);
                acc.sum_maxrgb += max_rgb;
                acc.count += 1;

                let fine_idx = (max_rgb * SCALE).round() as usize;
                acc.fine[fine_idx.min(FINE_BINS - 1)] += 1;

                let coarse_idx = (max_rgb * coarse_scale).round() as usize;
                acc.coarse[coarse_idx.min(COARSE_BINS - 1)] += 1;
            }
            acc
        })
        .reduce(Accum::new, Accum::merge);

    if acc.count == 0 {
        return FrameMeasurement::empty();
    }

    let max_scl = [
        scale_clamp(acc.max_r),
        scale_clamp(acc.max_g),
        scale_clamp(acc.max_b),
    ];

    let average_rgb = scale_clamp(acc.sum_maxrgb / acc.count as f64);

    let mut distribution = [0u32; 9];
    for (slot, pct) in DISTRIBUTION_PERCENTILES.iter().enumerate() {
        distribution[slot] = percentile_from_hist(&acc.fine, acc.count, *pct).min(MAX_VALUE);
    }

    // Normalize the coarse histogram to percentages (sum ~ 100.0).
    let mut coarse_histogram = vec![0.0f64; COARSE_BINS];
    let total = acc.count as f64;
    for (dst, src) in coarse_histogram.iter_mut().zip(acc.coarse.iter()) {
        *dst = (*src as f64 / total) * 100.0;
    }

    FrameMeasurement {
        max_scl,
        average_rgb,
        distribution,
        coarse_histogram,
    }
}

/// Scale a normalized linear value to the integer output domain, clamped.
#[inline]
fn scale_clamp(v: f64) -> u32 {
    let scaled = (v.clamp(0.0, 1.0) * SCALE).round();
    (scaled as u32).min(MAX_VALUE)
}

/// Nearest-rank percentile over an integer-indexed histogram. The bin index is
/// already the scaled value, so the returned index *is* the scaled percentile.
///
/// `total` is the number of samples; `pct` is in `[0, 100]`.
fn percentile_from_hist(hist: &[u64], total: u64, pct: f64) -> u32 {
    if total == 0 {
        return 0;
    }
    let pct = pct.clamp(0.0, 100.0);
    // Nearest-rank: smallest value whose cumulative count reaches the rank.
    let rank = ((pct / 100.0) * total as f64).ceil().max(1.0) as u64;
    let mut cumulative = 0u64;
    for (idx, &count) in hist.iter().enumerate() {
        cumulative += count;
        if cumulative >= rank {
            return idx as u32;
        }
    }
    (hist.len() - 1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pq_eotf_endpoints() {
        assert!(pq_eotf(0.0).abs() < 1e-9);
        assert!((pq_eotf(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pq_eotf_known_point() {
        // PQ-coded 0.5 maps to ~0.0092 of full scale (~92 nits of 10000).
        let l = pq_eotf(0.5);
        assert!(l > 0.008 && l < 0.011, "got {l}");
    }

    #[test]
    fn pq_eotf_100_nits_code() {
        // The ST 2084 PQ code for 100 nits is ~0.508; round-trip should hold.
        let l = pq_eotf(0.5081);
        assert!(
            (l * 10000.0 - 100.0).abs() < 2.0,
            "got {} nits",
            l * 10000.0
        );
    }

    #[test]
    fn pq_eotf_monotonic() {
        let mut prev = -1.0;
        for i in 0..=100 {
            let l = pq_eotf(i as f64 / 100.0);
            assert!(l >= prev, "EOTF must be monotonic");
            prev = l;
        }
    }

    #[test]
    fn percentile_nearest_rank() {
        // Histogram: 100 samples uniformly spread across indices 0..100.
        let mut hist = vec![0u64; 200];
        for v in hist.iter_mut().take(100) {
            *v = 1;
        }
        let total = 100;
        assert_eq!(percentile_from_hist(&hist, total, 50.0), 49);
        assert_eq!(percentile_from_hist(&hist, total, 99.0), 98);
        assert_eq!(percentile_from_hist(&hist, total, 1.0), 0);
        // 99.98th of 100 samples -> rank ceil(99.98) = 100 -> last filled bin.
        assert_eq!(percentile_from_hist(&hist, total, 99.98), 99);
    }

    #[test]
    fn percentile_empty() {
        let hist = vec![0u64; 10];
        assert_eq!(percentile_from_hist(&hist, 0, 50.0), 0);
    }

    #[test]
    fn scale_clamp_bounds() {
        assert_eq!(scale_clamp(0.0), 0);
        assert_eq!(scale_clamp(1.0), MAX_VALUE);
        assert_eq!(scale_clamp(2.0), MAX_VALUE);
        assert_eq!(scale_clamp(0.5), 50_000);
    }
}
