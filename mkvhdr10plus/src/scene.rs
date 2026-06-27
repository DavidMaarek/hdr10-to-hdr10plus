//! Lightweight scene-cut detection over the per-frame `maxRGB` histograms.
//!
//! Mirrors the metric used by `hdr_analyzer_mvp` (chi-squared histogram
//! distance with a minimum-scene-length guard), but operates on the coarse
//! `maxRGB` histogram produced by [`crate::measure`].

/// Per-frame scene labels expected by the HDR10+ JSON (spec §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneLabel {
    /// Absolute, contiguous index in display order (`0..N-1`).
    pub sequence_frame_index: u32,
    /// Scene identifier, incremented at each cut (starts at 0).
    pub scene_id: u32,
    /// Frame index within its scene, resets to 0 at each cut.
    pub scene_frame_index: u32,
}

/// Chi-squared distance between two normalized histograms.
pub fn histogram_difference(a: &[f64], b: &[f64]) -> f64 {
    let len = a.len().min(b.len());
    let mut dist = 0.0f64;
    for i in 0..len {
        let denom = a[i] + b[i] + 1e-6;
        let diff = a[i] - b[i];
        dist += (diff * diff) / denom;
    }
    dist
}

/// Whether a candidate cut at `candidate` is far enough from the last cut.
pub fn cut_allowed(last_cut: Option<u32>, candidate: u32, min_scene_len: u32) -> bool {
    match last_cut {
        None => candidate >= min_scene_len,
        Some(prev) => candidate.saturating_sub(prev) >= min_scene_len,
    }
}

/// Assign per-frame scene labels from a list of cut frame indices.
///
/// `cuts` holds the `SequenceFrameIndex` values at which a new scene begins
/// (the first frame of each new scene). They need not be sorted.
pub fn label_frames(total_frames: u32, mut cuts: Vec<u32>) -> Vec<SceneLabel> {
    cuts.sort_unstable();
    cuts.dedup();
    cuts.retain(|&c| c > 0 && c < total_frames);

    let mut labels = Vec::with_capacity(total_frames as usize);
    let mut scene_id = 0u32;
    let mut scene_frame_index = 0u32;
    let mut cut_iter = cuts.into_iter().peekable();

    for seq in 0..total_frames {
        if let Some(&next_cut) = cut_iter.peek() {
            if seq == next_cut {
                scene_id += 1;
                scene_frame_index = 0;
                cut_iter.next();
            }
        }
        labels.push(SceneLabel {
            sequence_frame_index: seq,
            scene_id,
            scene_frame_index,
        });
        scene_frame_index += 1;
    }

    labels
}

/// Build the `SceneInfoSummary` arrays from per-frame labels (spec §4).
///
/// Returns `(scene_first_frame_index, scene_frame_numbers)`.
pub fn scene_summary(labels: &[SceneLabel]) -> (Vec<u32>, Vec<u32>) {
    let mut first_indices = Vec::new();
    for label in labels {
        if label.scene_frame_index == 0 {
            first_indices.push(label.sequence_frame_index);
        }
    }

    let mut frame_numbers = Vec::with_capacity(first_indices.len());
    for (i, &start) in first_indices.iter().enumerate() {
        let end = first_indices
            .get(i + 1)
            .copied()
            .unwrap_or(labels.len() as u32);
        frame_numbers.push(end - start);
    }

    (first_indices, frame_numbers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_histograms_zero_distance() {
        let h = vec![1.0; 256];
        assert!(histogram_difference(&h, &h).abs() < 1e-9);
    }

    #[test]
    fn disjoint_histograms_large_distance() {
        let mut a = vec![0.0; 256];
        a[0] = 100.0;
        let mut b = vec![0.0; 256];
        b[255] = 100.0;
        assert!(histogram_difference(&a, &b) > 0.5);
    }

    #[test]
    fn min_scene_length_guard() {
        assert!(!cut_allowed(Some(0), 10, 24));
        assert!(cut_allowed(Some(0), 24, 24));
        assert!(cut_allowed(None, 24, 24));
        assert!(!cut_allowed(None, 10, 24));
    }

    #[test]
    fn labels_single_scene() {
        let labels = label_frames(3, vec![]);
        assert_eq!(labels.len(), 3);
        assert!(labels.iter().all(|l| l.scene_id == 0));
        assert_eq!(labels[2].scene_frame_index, 2);
    }

    #[test]
    fn labels_with_cuts() {
        // Cuts at frame 2 and 5 over 7 frames -> scenes [0,1] [2,3,4] [5,6].
        let labels = label_frames(7, vec![2, 5]);
        let ids: Vec<u32> = labels.iter().map(|l| l.scene_id).collect();
        assert_eq!(ids, vec![0, 0, 1, 1, 1, 2, 2]);
        let sfi: Vec<u32> = labels.iter().map(|l| l.scene_frame_index).collect();
        assert_eq!(sfi, vec![0, 1, 0, 1, 2, 0, 1]);

        let (first, numbers) = scene_summary(&labels);
        assert_eq!(first, vec![0, 2, 5]);
        assert_eq!(numbers, vec![2, 3, 2]);
    }
}
