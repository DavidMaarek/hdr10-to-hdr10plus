//! HDR10+ metadata JSON model (the schema consumed by `hdr10plus_tool inject`).
//!
//! See spec §4–§6. Every luminance magnitude is clamped to `[0, 100000]` before
//! serialization; we emit the Profile B identity Bezier and a non-zero
//! `TargetedSystemDisplayMaximumLuminance` so the tool accepts the file.

use serde::Serialize;

use crate::measure::{FrameMeasurement, DISTRIBUTION_INDEX, MAX_VALUE};
use crate::scene::{scene_summary, SceneLabel};

/// Profile B identity Bezier anchors (regular ramp ≈ identity, spec §5).
const IDENTITY_ANCHORS: [u32; 9] = [102, 205, 307, 410, 512, 614, 717, 819, 922];

const TOOL_NAME: &str = "mkvhdr10plus";
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize)]
pub struct Hdr10PlusJson {
    #[serde(rename = "JSONInfo")]
    pub json_info: JsonInfo,
    #[serde(rename = "SceneInfo")]
    pub scene_info: Vec<SceneInfo>,
    #[serde(rename = "SceneInfoSummary")]
    pub scene_info_summary: SceneInfoSummary,
    #[serde(rename = "ToolInfo")]
    pub tool_info: ToolInfo,
}

#[derive(Serialize)]
pub struct JsonInfo {
    #[serde(rename = "HDR10plusProfile")]
    pub hdr10plus_profile: String,
    #[serde(rename = "Version")]
    pub version: String,
}

#[derive(Serialize)]
pub struct SceneInfo {
    #[serde(rename = "BezierCurveData")]
    pub bezier_curve_data: BezierCurveData,
    #[serde(rename = "LuminanceParameters")]
    pub luminance_parameters: LuminanceParameters,
    #[serde(rename = "NumberOfWindows")]
    pub number_of_windows: u32,
    #[serde(rename = "TargetedSystemDisplayMaximumLuminance")]
    pub targeted_system_display_maximum_luminance: u32,
    #[serde(rename = "SceneFrameIndex")]
    pub scene_frame_index: u32,
    #[serde(rename = "SceneId")]
    pub scene_id: u32,
    #[serde(rename = "SequenceFrameIndex")]
    pub sequence_frame_index: u32,
}

#[derive(Serialize)]
pub struct BezierCurveData {
    #[serde(rename = "Anchors")]
    pub anchors: Vec<u32>,
    #[serde(rename = "KneePointX")]
    pub knee_point_x: u32,
    #[serde(rename = "KneePointY")]
    pub knee_point_y: u32,
}

#[derive(Serialize)]
pub struct LuminanceParameters {
    #[serde(rename = "AverageRGB")]
    pub average_rgb: u32,
    #[serde(rename = "LuminanceDistributions")]
    pub luminance_distributions: LuminanceDistributions,
    #[serde(rename = "MaxScl")]
    pub max_scl: [u32; 3],
}

#[derive(Serialize)]
pub struct LuminanceDistributions {
    #[serde(rename = "DistributionIndex")]
    pub distribution_index: Vec<u8>,
    #[serde(rename = "DistributionValues")]
    pub distribution_values: Vec<u32>,
}

#[derive(Serialize)]
pub struct SceneInfoSummary {
    #[serde(rename = "SceneFirstFrameIndex")]
    pub scene_first_frame_index: Vec<u32>,
    #[serde(rename = "SceneFrameNumbers")]
    pub scene_frame_numbers: Vec<u32>,
}

#[derive(Serialize)]
pub struct ToolInfo {
    #[serde(rename = "Tool")]
    pub tool: String,
    #[serde(rename = "Version")]
    pub version: String,
}

#[inline]
fn clamp_value(v: u32) -> u32 {
    v.min(MAX_VALUE)
}

/// Assemble the full HDR10+ JSON document from per-frame measurements and
/// matching scene labels.
///
/// `measurements` and `labels` must have the same length and be in display
/// order. `target_nits` is written verbatim as
/// `TargetedSystemDisplayMaximumLuminance` (must be non-zero for Profile B).
pub fn build(
    measurements: &[FrameMeasurement],
    labels: &[SceneLabel],
    target_nits: u32,
) -> Hdr10PlusJson {
    assert_eq!(
        measurements.len(),
        labels.len(),
        "measurements and labels must align one-to-one"
    );

    let target = target_nits.clamp(1, MAX_VALUE);

    let scene_info = measurements
        .iter()
        .zip(labels.iter())
        .map(|(m, label)| {
            let distribution_values = m
                .distribution
                .iter()
                .map(|&v| clamp_value(v))
                .collect::<Vec<_>>();
            SceneInfo {
                bezier_curve_data: BezierCurveData {
                    anchors: IDENTITY_ANCHORS.to_vec(),
                    knee_point_x: 0,
                    knee_point_y: 0,
                },
                luminance_parameters: LuminanceParameters {
                    average_rgb: clamp_value(m.average_rgb),
                    luminance_distributions: LuminanceDistributions {
                        distribution_index: DISTRIBUTION_INDEX.to_vec(),
                        distribution_values,
                    },
                    max_scl: [
                        clamp_value(m.max_scl[0]),
                        clamp_value(m.max_scl[1]),
                        clamp_value(m.max_scl[2]),
                    ],
                },
                number_of_windows: 1,
                targeted_system_display_maximum_luminance: target,
                scene_frame_index: label.scene_frame_index,
                scene_id: label.scene_id,
                sequence_frame_index: label.sequence_frame_index,
            }
        })
        .collect();

    let (scene_first_frame_index, scene_frame_numbers) = scene_summary(labels);

    Hdr10PlusJson {
        json_info: JsonInfo {
            hdr10plus_profile: "B".to_string(),
            version: "1.0".to_string(),
        },
        scene_info,
        scene_info_summary: SceneInfoSummary {
            scene_first_frame_index,
            scene_frame_numbers,
        },
        tool_info: ToolInfo {
            tool: TOOL_NAME.to_string(),
            version: TOOL_VERSION.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::label_frames;

    fn dummy_measurement() -> FrameMeasurement {
        FrameMeasurement {
            max_scl: [10568, 10238, 34997],
            average_rgb: 247,
            distribution: [14, 7319, 91, 42, 80, 192, 630, 1228, 7289],
            coarse_histogram: vec![0.0; 256],
        }
    }

    #[test]
    fn builds_expected_shape() {
        let measurements = vec![dummy_measurement(); 3];
        let labels = label_frames(3, vec![]);
        let doc = build(&measurements, &labels, 1000);

        assert_eq!(doc.json_info.hdr10plus_profile, "B");
        assert_eq!(doc.scene_info.len(), 3);
        assert_eq!(doc.tool_info.tool, "mkvhdr10plus");

        let s = &doc.scene_info[0];
        assert_eq!(s.number_of_windows, 1);
        assert_eq!(s.targeted_system_display_maximum_luminance, 1000);
        assert_eq!(
            s.luminance_parameters
                .luminance_distributions
                .distribution_index,
            vec![1, 5, 10, 25, 50, 75, 90, 95, 99]
        );
        assert_eq!(s.bezier_curve_data.anchors, IDENTITY_ANCHORS.to_vec());
    }

    #[test]
    fn clamps_oversized_values() {
        let mut m = dummy_measurement();
        m.max_scl = [999_999, 0, 0];
        m.average_rgb = 200_000;
        m.distribution[0] = 500_000;
        let labels = label_frames(1, vec![]);
        let doc = build(&[m], &labels, 1000);
        let lp = &doc.scene_info[0].luminance_parameters;
        assert_eq!(lp.max_scl[0], MAX_VALUE);
        assert_eq!(lp.average_rgb, MAX_VALUE);
        assert_eq!(lp.luminance_distributions.distribution_values[0], MAX_VALUE);
    }

    #[test]
    fn target_nits_never_zero() {
        let labels = label_frames(1, vec![]);
        let doc = build(&[dummy_measurement()], &labels, 0);
        assert_eq!(
            doc.scene_info[0].targeted_system_display_maximum_luminance,
            1
        );
    }

    #[test]
    fn serializes_pascal_case_keys() {
        let labels = label_frames(1, vec![]);
        let doc = build(&[dummy_measurement()], &labels, 400);
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"HDR10plusProfile\":\"B\""));
        assert!(json.contains("\"MaxScl\""));
        assert!(json.contains("\"SequenceFrameIndex\""));
        assert!(json.contains("\"TargetedSystemDisplayMaximumLuminance\":400"));
    }
}
