//! JSON-line protocol types for the `test-hooks` QA automation harness.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    5000
}

fn default_idle_frames() -> u32 {
    2
}

fn default_drag_steps() -> usize {
    6
}

/// Optional target selector supporting ID, label/text, or logical `(x, y)` coordinates.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TargetSpec {
    pub id: Option<String>,
    pub label: Option<String>,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

/// Envelope for incoming JSON-line requests.
#[derive(Debug, Clone, Deserialize)]
pub struct RequestEnvelope {
    #[serde(default, alias = "request_id")]
    pub req_id: Option<u64>,
    #[serde(flatten)]
    pub cmd: HarnessCommand,
}

/// Supported QA harness commands.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum HarnessCommand {
    Screenshot {
        path: PathBuf,
    },
    ScreenshotRegion {
        path: PathBuf,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        #[serde(default = "default_true")]
        logical: bool,
    },
    DumpUi,
    Click {
        #[serde(default)]
        target: Option<TargetSpec>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
    },
    DoubleClick {
        #[serde(default)]
        target: Option<TargetSpec>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
    },
    RightClick {
        #[serde(default)]
        target: Option<TargetSpec>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
    },
    Hover {
        #[serde(default)]
        target: Option<TargetSpec>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
    },
    Type {
        text: String,
        #[serde(default)]
        target: Option<TargetSpec>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
        #[serde(default)]
        clear: bool,
    },
    Key {
        key: String,
        #[serde(default)]
        modifiers: Vec<String>,
    },
    Scroll {
        #[serde(default)]
        dx: f32,
        #[serde(default)]
        dy: f32,
        #[serde(default)]
        target: Option<TargetSpec>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
    },
    Drag {
        #[serde(default)]
        from: Option<TargetSpec>,
        #[serde(default)]
        to: Option<TargetSpec>,
        #[serde(default)]
        from_x: Option<f32>,
        #[serde(default)]
        from_y: Option<f32>,
        #[serde(default)]
        to_x: Option<f32>,
        #[serde(default)]
        to_y: Option<f32>,
        #[serde(default = "default_drag_steps")]
        steps: usize,
    },
    DropFile {
        path: PathBuf,
    },
    WaitIdle {
        #[serde(default = "default_timeout")]
        timeout_ms: u64,
        #[serde(default = "default_idle_frames")]
        frames: u32,
    },
    WaitFor {
        #[serde(default)]
        target: Option<TargetSpec>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        tab: Option<String>,
        #[serde(default = "default_true")]
        visible: bool,
        #[serde(default = "default_timeout")]
        timeout_ms: u64,
    },
    State,
    FrameStats {
        #[serde(default)]
        action: Option<String>,
        #[serde(default)]
        op: Option<String>,
        #[serde(default)]
        reset: bool,
    },
    Resize {
        width: f32,
        height: f32,
    },
    SetScale {
        scale: f32,
    },
    SetTheme {
        preset: String,
    },
    Goto {
        tab: String,
        #[serde(default)]
        section: Option<String>,
    },
    SetFlag {
        name: String,
        value: serde_json::Value,
    },
    Quit,
}

/// Outgoing JSON-line response.
#[derive(Debug, Clone, Serialize)]
pub struct HarnessResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

impl HarnessResponse {
    pub fn ok(id: Option<u64>, data: serde_json::Value) -> Self {
        Self {
            id,
            ok: true,
            error: None,
            data,
        }
    }

    pub fn err(id: Option<u64>, msg: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            error: Some(msg.into()),
            data: serde_json::json!({}),
        }
    }
}

pub fn merge_target(
    target: Option<TargetSpec>,
    id: Option<String>,
    label: Option<String>,
    x: Option<f32>,
    y: Option<f32>,
) -> TargetSpec {
    let base = target.unwrap_or_default();
    TargetSpec {
        id: id.or(base.id),
        label: label.or(base.label),
        x: x.or(base.x),
        y: y.or(base.y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_json_line_commands() {
        let click: RequestEnvelope =
            serde_json::from_str(r#"{"req_id":1,"cmd":"click","label":"Настройки"}"#).unwrap();
        assert_eq!(click.req_id, Some(1));
        assert!(matches!(
            click.cmd,
            HarnessCommand::Click {
                label: Some(ref lbl),
                ..
            } if lbl == "Настройки"
        ));

        let type_cmd: RequestEnvelope = serde_json::from_str(
            r#"{"cmd":"type","id":"mc_locale_search","text":"Русский","clear":true}"#,
        )
        .unwrap();
        assert_eq!(type_cmd.req_id, None);
        assert!(matches!(
            type_cmd.cmd,
            HarnessCommand::Type {
                id: Some(ref target_id),
                clear: true,
                ..
            } if target_id == "mc_locale_search"
        ));

        let stats: RequestEnvelope =
            serde_json::from_str(r#"{"cmd":"frame_stats","action":"get"}"#).unwrap();
        assert!(matches!(
            stats.cmd,
            HarnessCommand::FrameStats {
                action: Some(ref act),
                ..
            } if act == "get"
        ));
    }
}
