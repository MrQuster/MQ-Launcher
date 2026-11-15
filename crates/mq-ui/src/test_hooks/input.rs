//! Target resolution and `egui::RawInput` event sequence builders.

use egui::{DroppedFile, Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Vec2};

use crate::components::registry::{UiElementRecord, UiRegistrySnapshot};
use crate::test_hooks::protocol::TargetSpec;

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub pos: Pos2,
    pub matched_id: Option<String>,
    pub matched_label: Option<String>,
    pub was_active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct InputFrameStep {
    pub events: Vec<Event>,
    pub dropped_files: Vec<DroppedFile>,
}

/// Resolves a `TargetSpec` (`x,y`, `id`, or `label`) against the latest `UiRegistrySnapshot`.
pub fn resolve_target(
    spec: &TargetSpec,
    snapshot: Option<&UiRegistrySnapshot>,
) -> Result<ResolvedTarget, String> {
    if let (Some(x), Some(y)) = (spec.x, spec.y) {
        return Ok(ResolvedTarget {
            pos: Pos2::new(x, y),
            matched_id: None,
            matched_label: None,
            was_active: false,
        });
    }

    let snap = snapshot.ok_or_else(|| "UI registry snapshot is not available yet".to_string())?;

    if let Some(ref wanted_id) = spec.id {
        let wanted = wanted_id.trim();
        if let Some(el) = snap
            .elements
            .iter()
            .filter(|e| e.visible)
            .find(|e| e.id == wanted || e.egui_id == wanted)
            .or_else(|| {
                snap.elements
                    .iter()
                    .find(|e| e.id == wanted || e.egui_id == wanted)
            })
        {
            return element_to_target(el);
        }
        return Err(format!("Element with id '{wanted}' not found"));
    }

    if let Some(ref wanted_label) = spec.label {
        let wanted = wanted_label.trim();
        let wanted_lower = wanted.to_lowercase();

        // Preference order: exact label -> exact tech_name -> exact tooltip -> exact text -> case-insensitive exact -> substring
        let find_match = |pred: &dyn Fn(&UiElementRecord) -> bool| -> Option<&UiElementRecord> {
            snap.elements
                .iter()
                .filter(|e| e.visible && e.interactive)
                .find(|e| pred(e))
                .or_else(|| snap.elements.iter().filter(|e| e.visible).find(|e| pred(e)))
        };

        let found = find_match(&|e| e.label == wanted)
            .or_else(|| find_match(&|e| e.tech_name.as_deref() == Some(wanted)))
            .or_else(|| find_match(&|e| e.tooltip.as_deref() == Some(wanted)))
            .or_else(|| find_match(&|e| e.text == wanted))
            .or_else(|| find_match(&|e| e.label.to_lowercase() == wanted_lower))
            .or_else(|| find_match(&|e| e.label.to_lowercase().contains(&wanted_lower)))
            .or_else(|| find_match(&|e| e.text.to_lowercase().contains(&wanted_lower)));

        if let Some(el) = found {
            return element_to_target(el);
        }
        return Err(format!("Element with label '{wanted}' not found"));
    }

    Err("Target must specify id, label, or (x, y) coordinates".to_string())
}

fn element_to_target(el: &UiElementRecord) -> Result<ResolvedTarget, String> {
    if !el.visible || el.rect.w <= 0.0 || el.rect.h <= 0.0 {
        return Err(format!(
            "Element '{}' ({}) is not visible on screen",
            el.label, el.id
        ));
    }
    if !el.enabled {
        return Err(format!("Element '{}' ({}) is disabled", el.label, el.id));
    }
    Ok(ResolvedTarget {
        pos: Pos2::new(el.rect.x + el.rect.w * 0.5, el.rect.y + el.rect.h * 0.5),
        matched_id: Some(el.id.clone()),
        matched_label: Some(el.label.clone()),
        was_active: el.active,
    })
}

pub fn build_click_steps(pos: Pos2, button: PointerButton, double: bool) -> Vec<InputFrameStep> {
    let mut steps = Vec::new();
    steps.push(InputFrameStep {
        events: vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        dropped_files: Vec::new(),
    });
    steps.push(InputFrameStep {
        events: vec![Event::PointerButton {
            pos,
            button,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        dropped_files: Vec::new(),
    });
    if double {
        steps.push(InputFrameStep {
            events: vec![Event::PointerButton {
                pos,
                button,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
            dropped_files: Vec::new(),
        });
        steps.push(InputFrameStep {
            events: vec![Event::PointerButton {
                pos,
                button,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
            dropped_files: Vec::new(),
        });
    }
    // One settle frame for UI state reaction
    steps.push(InputFrameStep::default());
    steps
}

pub fn build_hover_steps(pos: Pos2) -> Vec<InputFrameStep> {
    vec![
        InputFrameStep {
            events: vec![Event::PointerMoved(pos)],
            dropped_files: Vec::new(),
        },
        InputFrameStep::default(),
    ]
}

pub fn build_type_steps(focus_pos: Option<Pos2>, text: &str, clear: bool) -> Vec<InputFrameStep> {
    let mut steps = Vec::new();
    if let Some(pos) = focus_pos {
        steps.extend(build_click_steps(pos, PointerButton::Primary, false));
    }
    if clear {
        let mods = Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        };
        steps.push(InputFrameStep {
            events: vec![Event::Key {
                key: Key::A,
                physical_key: Some(Key::A),
                pressed: true,
                repeat: false,
                modifiers: mods,
            }],
            dropped_files: Vec::new(),
        });
        steps.push(InputFrameStep {
            events: vec![
                Event::Key {
                    key: Key::A,
                    physical_key: Some(Key::A),
                    pressed: false,
                    repeat: false,
                    modifiers: mods,
                },
                Event::Key {
                    key: Key::Backspace,
                    physical_key: Some(Key::Backspace),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                },
            ],
            dropped_files: Vec::new(),
        });
        steps.push(InputFrameStep {
            events: vec![Event::Key {
                key: Key::Backspace,
                physical_key: Some(Key::Backspace),
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            dropped_files: Vec::new(),
        });
    }
    if !text.is_empty() {
        steps.push(InputFrameStep {
            events: vec![Event::Text(text.to_string())],
            dropped_files: Vec::new(),
        });
    }
    steps.push(InputFrameStep::default());
    steps
}

pub fn build_key_steps(key: Key, modifiers: Modifiers) -> Vec<InputFrameStep> {
    vec![
        InputFrameStep {
            events: vec![Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers,
            }],
            dropped_files: Vec::new(),
        },
        InputFrameStep {
            events: vec![Event::Key {
                key,
                physical_key: Some(key),
                pressed: false,
                repeat: false,
                modifiers,
            }],
            dropped_files: Vec::new(),
        },
        InputFrameStep::default(),
    ]
}

pub fn build_scroll_steps(pos: Option<Pos2>, dx: f32, dy: f32) -> Vec<InputFrameStep> {
    let mut events = Vec::new();
    if let Some(p) = pos {
        events.push(Event::PointerMoved(p));
    }
    events.push(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(dx, dy),
        modifiers: Modifiers::NONE,
    });
    vec![
        InputFrameStep {
            events,
            dropped_files: Vec::new(),
        },
        InputFrameStep::default(),
    ]
}

pub fn build_drag_steps(from: Pos2, to: Pos2, steps_count: usize) -> Vec<InputFrameStep> {
    let n = steps_count.max(1);
    let mut steps = Vec::with_capacity(n + 3);
    steps.push(InputFrameStep {
        events: vec![
            Event::PointerMoved(from),
            Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        dropped_files: Vec::new(),
    });
    for i in 1..=n {
        let t = (i as f32) / (n as f32);
        let p = from.lerp(to, t);
        steps.push(InputFrameStep {
            events: vec![Event::PointerMoved(p)],
            dropped_files: Vec::new(),
        });
    }
    steps.push(InputFrameStep {
        events: vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        dropped_files: Vec::new(),
    });
    steps.push(InputFrameStep::default());
    steps
}

pub fn parse_key(name: &str) -> Option<Key> {
    let trimmed = name.trim();
    if let Some(k) = Key::from_name(trimmed) {
        return Some(k);
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "return" | "enter" => Some(Key::Enter),
        "esc" | "escape" => Some(Key::Escape),
        "space" | " " => Some(Key::Space),
        "tab" => Some(Key::Tab),
        "backspace" => Some(Key::Backspace),
        "del" | "delete" => Some(Key::Delete),
        "up" | "arrowup" | "arrow_up" => Some(Key::ArrowUp),
        "down" | "arrowdown" | "arrow_down" => Some(Key::ArrowDown),
        "left" | "arrowleft" | "arrow_left" => Some(Key::ArrowLeft),
        "right" | "arrowright" | "arrow_right" => Some(Key::ArrowRight),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "page_up" => Some(Key::PageUp),
        "pagedown" | "page_down" => Some(Key::PageDown),
        _ => None,
    }
}

pub fn parse_modifiers(mods: &[String]) -> Modifiers {
    let mut m = Modifiers::NONE;
    for raw in mods {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => {
                m.ctrl = true;
                m.command = true;
            }
            "cmd" | "command" | "super" | "meta" => {
                m.command = true;
                m.mac_cmd = true;
            }
            "shift" => m.shift = true,
            "alt" | "option" => m.alt = true,
            _ => {}
        }
    }
    m
}
