//! JSON boundary between the Svelte UI and the develop worker.
//!
//! Field names are camelCase on the wire. A render that lost the race
//! against a newer one returns the string `cancelled` and no bytes.

use serde::{Deserialize, Serialize};

/// One bundled or opened stock, before it is applied to a file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockSummary {
    /// Canonical path of the `.stock.toml`. Two spellings of one file share it.
    pub id: String,
    pub path: String,
    pub name: String,
    pub model: String,
    pub revision: u32,
}

/// Slider values and the identity of the stock being edited.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockState {
    /// Same id as [`StockSummary::id`]: the canonical `.stock.toml` path.
    pub id: String,
    pub path: String,
    pub name: String,
    pub model: String,
    pub revision: u32,
    pub source: String,
    pub license: String,
    pub exposure_ev: f32,
    pub print_exposure: f32,
    pub dye_scales: Option<[f32; 3]>,
    pub grain_amount: f32,
    pub grain_available: bool,
    pub halation_enabled: bool,
    pub halation_threshold: f32,
    pub halation_radius: f32,
    pub halation_weight: [f32; 3],
}

/// The open RAW file and, once chosen, the stock applied to it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub path: String,
    pub file_name: String,
    pub sensor_width: u32,
    pub sensor_height: u32,
    pub display_width: u32,
    pub display_height: u32,
    pub orientation: String,
    /// Clockwise quarter-turns added on top of the camera tag, in `0..4`.
    pub quarter_turns: u8,
    pub stock: Option<StockState>,
}

/// A 90° turn of the open image, on top of the camera orientation.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnDirection {
    Clockwise,
    CounterClockwise,
}

/// A single Milestone 1 control. Each variant dirties only its own stage.
#[derive(Debug, Deserialize)]
#[serde(tag = "control", content = "value", rename_all = "camelCase")]
pub enum ControlEdit {
    Exposure(f32),
    PrintExposure(f32),
    DyeScales([f32; 3]),
    GrainAmount(f32),
    HalationEnabled(bool),
    HalationThreshold(f32),
    HalationRadius(f32),
    HalationRed(f32),
}

/// Fit proxy or a 100% display-space crop.
#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RenderView {
    /// Long edge of the oriented proxy. The UI sends 720 while dragging and
    /// 1440 once the pointer has been idle.
    Fit { long_edge: u32 },
    /// `exact` is the settled dye scan. A drag leaves it false.
    Hundred {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        exact: bool,
    },
}

/// One preview job. `edit` is applied before the render, on the worker.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevelopRequest {
    pub ticket: u64,
    pub compare: bool,
    pub edit: Option<ControlEdit>,
    pub view: RenderView,
}

/// Save-as fields. The path is chosen in the native dialog.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRequest {
    pub path: String,
    pub name: String,
    pub source: String,
    pub license: String,
    pub revision: u32,
}

/// Stage progress. `stage` is a [`rawstock_core::render::Stage`] name, or
/// `Opening` while a file is being decoded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub stage: String,
    pub done_rows: u32,
    pub rows: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn develop_request_reads_the_ui_payload() {
        let fit = r#"{"ticket":3,"compare":false,"edit":{"control":"printExposure","value":1.28},"view":{"kind":"fit","longEdge":720}}"#;
        let request: DevelopRequest = serde_json::from_str(fit).unwrap();
        assert_eq!(request.ticket, 3);
        assert!(!request.compare);
        assert!(
            matches!(request.edit, Some(ControlEdit::PrintExposure(value)) if (value - 1.28).abs() < 1.0e-6)
        );
        assert!(matches!(request.view, RenderView::Fit { long_edge: 720 }));

        let crop = r#"{"ticket":4,"compare":true,"edit":null,"view":{"kind":"hundred","x":10,"y":20,"width":300,"height":200,"exact":true}}"#;
        let request: DevelopRequest = serde_json::from_str(crop).unwrap();
        assert!(request.compare);
        assert!(request.edit.is_none());
        assert!(matches!(
            request.view,
            RenderView::Hundred {
                x: 10,
                y: 20,
                width: 300,
                height: 200,
                exact: true,
            }
        ));
    }
}
