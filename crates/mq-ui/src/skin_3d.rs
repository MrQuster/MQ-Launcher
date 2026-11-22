//! Interactive 3D Minecraft skin and cape software renderer for `egui::Painter`.
//!
//! Features:
//! - Pure software 3D projection onto `egui::Painter` (zero GPU state or shader requirements).
//! - Depth-sorted faces (painter's algorithm) with back-face culling.
//! - Minecraft character model: Head + Hat outer layer, Torso, Left & Right Arms (Steve 4px / Slim 3px),
//!   Left & Right Legs, and optional Cape.
//! - Drag-to-rotate yaw and pitch, double-click to reset, toggleable idle walk animation.
//! - Pixel-sampled colors from `image::RgbaImage` with clean Steve/Alex fallbacks.
//! - Zero emojis, clean on Linux, Windows, macOS, and headless tests.

use egui::{pos2, Color32, Pos2, Sense, Stroke, Ui, Vec2};
use image::{Rgba, RgbaImage};

/// Interactive state for the 3D skin viewer.
#[derive(Debug, Clone)]
pub struct Skin3DState {
    /// Horizontal rotation in radians.
    pub yaw: f32,
    /// Vertical rotation in radians.
    pub pitch: f32,
    /// Alex slim 3px arm model toggle.
    pub slim: bool,
    /// Toggleable idle walk animation.
    pub animate: bool,
    /// Toggleable cape display.
    pub show_cape: bool,
}

impl Default for Skin3DState {
    fn default() -> Self {
        Self {
            yaw: -0.42,
            pitch: 0.12,
            slim: false,
            animate: false,
            show_cape: true,
        }
    }
}

/// 3D vector helper.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Vec3([f32; 3]);

impl Vec3 {
    #[inline]
    const fn new(x: f32, y: f32, z: f32) -> Self {
        Self([x, y, z])
    }

    #[inline]
    fn x(self) -> f32 {
        self.0[0]
    }

    #[inline]
    fn y(self) -> f32 {
        self.0[1]
    }

    #[inline]
    fn z(self) -> f32 {
        self.0[2]
    }

    #[inline]
    fn sub(self, other: Self) -> Self {
        Self([
            self.0[0] - other.0[0],
            self.0[1] - other.0[1],
            self.0[2] - other.0[2],
        ])
    }

    #[inline]
    fn cross(self, other: Self) -> Self {
        Self([
            self.0[1] * other.0[2] - self.0[2] * other.0[1],
            self.0[2] * other.0[0] - self.0[0] * other.0[2],
            self.0[0] * other.0[1] - self.0[1] * other.0[0],
        ])
    }

    #[inline]
    fn dot(self, other: Self) -> f32 {
        self.0[0] * other.0[0] + self.0[1] * other.0[1] + self.0[2] * other.0[2]
    }

    #[inline]
    fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    #[inline]
    fn normalize(self) -> Self {
        let len = self.length();
        if len > 1e-6 {
            Self([self.0[0] / len, self.0[1] / len, self.0[2] / len])
        } else {
            Self([0.0, 1.0, 0.0])
        }
    }
}

/// Rotates a point around X axis by `pitch`.
#[inline]
fn rotate_x(p: Vec3, pitch: f32) -> Vec3 {
    let (sin, cos) = pitch.sin_cos();
    Vec3::new(p.x(), p.y() * cos - p.z() * sin, p.y() * sin + p.z() * cos)
}

/// Rotates a point around Y axis by `yaw`.
#[inline]
fn rotate_y(p: Vec3, yaw: f32) -> Vec3 {
    let (sin, cos) = yaw.sin_cos();
    Vec3::new(p.x() * cos + p.z() * sin, p.y(), -p.x() * sin + p.z() * cos)
}

/// Rotates a point around a pivot point on the X axis.
#[inline]
fn rotate_pivot_x(p: Vec3, pivot: Vec3, angle: f32) -> Vec3 {
    let rel = p.sub(pivot);
    let rot = rotate_x(rel, angle);
    Vec3::new(
        pivot.x() + rot.x(),
        pivot.y() + rot.y(),
        pivot.z() + rot.z(),
    )
}

/// Represents a single face of a body part with texture UV coordinates.
struct PartFace {
    /// 3D quad corners in local space: [top-left, top-right, bottom-right, bottom-left].
    corners: [Vec3; 4],
    /// Texture coordinate origin in skin texture: (u_min, v_min).
    uv_min: (u32, u32),
    /// Texture dimensions (pixels wide, pixels high).
    uv_size: (u32, u32),
    /// Whether this face belongs to an outer transparent layer (hat, jacket).
    is_outer: bool,
}

/// Evaluates a 3D Minecraft preview widget inside `ui`.
pub fn render_skin_3d_preview(
    ui: &mut Ui,
    state: &mut Skin3DState,
    skin_rgba: Option<&RgbaImage>,
    size: Vec2,
) {
    let (rect, response) = ui.allocate_exact_size(size, Sense::drag());

    // Drag-to-rotate interaction
    if response.dragged() {
        let delta = response.drag_delta();
        state.yaw += delta.x * 0.015;
        state.pitch = (state.pitch + delta.y * 0.015).clamp(-1.25, 1.25);
    }
    if response.double_clicked() {
        state.yaw = -0.42;
        state.pitch = 0.12;
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(if response.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }

    if state.animate {
        ui.ctx().request_repaint();
    }

    let painter = ui.painter_at(rect);

    // Subtle dark gradient background / floor shadow
    let center = rect.center();
    let bg_color = egui::Color32::from_rgba_unmultiplied(18, 22, 28, 240);
    painter.rect_filled(rect, 8.0, bg_color);

    // Floor shadow circle
    let shadow_scale = (size.y / 38.0).min(size.x / 26.0);
    let shadow_center = pos2(center.x, center.y + 16.0 * shadow_scale);
    painter.circle_filled(
        shadow_center,
        10.0 * shadow_scale,
        Color32::from_black_alpha(45),
    );

    // Animation timing
    let anim_time = if state.animate {
        ui.input(|i| i.time) as f32 * 3.5
    } else {
        0.0
    };
    let leg_swing = if state.animate {
        anim_time.sin() * 0.45
    } else {
        0.0
    };
    let arm_swing = if state.animate {
        anim_time.sin() * 0.45
    } else {
        0.0
    };
    let head_sway = if state.animate {
        (anim_time * 0.5).sin() * 0.06
    } else {
        0.0
    };
    let cape_swing = if state.animate {
        0.18 + (anim_time * 1.5).cos().abs() * 0.12
    } else {
        0.18
    };

    let mut faces: Vec<PartFace> = Vec::with_capacity(64);
    build_character_faces(&mut faces, state.slim, state.show_cape);

    // Light direction (from upper right front)
    let light_dir = Vec3::new(0.35, 0.75, 0.55).normalize();

    // Perspective projection helper
    let cam_dist = 140.0;
    let proj = |pt: Vec3| -> (Pos2, f32) {
        let persp = cam_dist / (cam_dist + pt.z()).max(10.0);
        let px = center.x + pt.x() * shadow_scale * persp;
        let py = center.y - (pt.y() - 15.5) * shadow_scale * persp;
        (pos2(px, py), pt.z())
    };

    // Prepare faces projected to camera space with joint rotations applied
    struct RenderFace {
        corners_3d: [Vec3; 4],
        center_z: f32,
        normal: Vec3,
        uv_min: (u32, u32),
        uv_size: (u32, u32),
        is_outer: bool,
    }

    let mut render_faces: Vec<RenderFace> = Vec::with_capacity(faces.len());

    for face in &faces {
        // Transform vertices by part-specific joint animations
        let animated_corners = [
            apply_joint_animation(
                face.corners[0],
                leg_swing,
                arm_swing,
                head_sway,
                cape_swing,
                state.slim,
            ),
            apply_joint_animation(
                face.corners[1],
                leg_swing,
                arm_swing,
                head_sway,
                cape_swing,
                state.slim,
            ),
            apply_joint_animation(
                face.corners[2],
                leg_swing,
                arm_swing,
                head_sway,
                cape_swing,
                state.slim,
            ),
            apply_joint_animation(
                face.corners[3],
                leg_swing,
                arm_swing,
                head_sway,
                cape_swing,
                state.slim,
            ),
        ];

        // Global character rotation: pitch then yaw
        let cam_corners = [
            rotate_y(rotate_x(animated_corners[0], state.pitch), state.yaw),
            rotate_y(rotate_x(animated_corners[1], state.pitch), state.yaw),
            rotate_y(rotate_x(animated_corners[2], state.pitch), state.yaw),
            rotate_y(rotate_x(animated_corners[3], state.pitch), state.yaw),
        ];

        // Compute normal in camera space (CCW winding: v01 x v03)
        let edge1 = cam_corners[1].sub(cam_corners[0]);
        let edge2 = cam_corners[3].sub(cam_corners[0]);
        let normal = edge1.cross(edge2).normalize();

        // Back-face culling: if normal points away from camera, discard
        if normal.z() <= 0.001 {
            continue;
        }

        let avg_z =
            (cam_corners[0].z() + cam_corners[1].z() + cam_corners[2].z() + cam_corners[3].z())
                * 0.25;

        render_faces.push(RenderFace {
            corners_3d: cam_corners,
            center_z: avg_z,
            normal,
            uv_min: face.uv_min,
            uv_size: face.uv_size,
            is_outer: face.is_outer,
        });
    }

    // Sort back-to-front (painter's algorithm)
    render_faces.sort_by(|a, b| {
        a.center_z
            .partial_cmp(&b.center_z)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Render sorted faces pixel by pixel
    for face in render_faces {
        let (u_min, v_min) = face.uv_min;
        let (u_w, v_h) = face.uv_size;
        if u_w == 0 || v_h == 0 {
            continue;
        }

        // Lighting calculation based on normal dot light_dir
        let dot = face.normal.dot(light_dir).max(0.0);
        let shade = (0.55 + 0.45 * dot).clamp(0.42, 1.0);

        let p00 = face.corners_3d[0];
        let p10 = face.corners_3d[1];
        let p11 = face.corners_3d[2];
        let p01 = face.corners_3d[3];

        for vy in 0..v_h {
            let v_ratio_0 = vy as f32 / v_h as f32;
            let v_ratio_1 = (vy + 1) as f32 / v_h as f32;

            for ux in 0..u_w {
                let u_ratio_0 = ux as f32 / u_w as f32;
                let u_ratio_1 = (ux + 1) as f32 / u_w as f32;

                let px = u_min + ux;
                let py = v_min + vy;

                let color = sample_skin_color(skin_rgba, px, py, state.slim, face.is_outer);
                let Some(base_col) = color else {
                    continue; // Skip transparent pixel (especially in hat layer)
                };

                let lit_col = Color32::from_rgba_unmultiplied(
                    (base_col.r() as f32 * shade).clamp(0.0, 255.0) as u8,
                    (base_col.g() as f32 * shade).clamp(0.0, 255.0) as u8,
                    (base_col.b() as f32 * shade).clamp(0.0, 255.0) as u8,
                    base_col.a(),
                );

                // Bilinear interpolation of 3D quad corners for this sub-pixel
                let corner_3d = |u: f32, v: f32| -> Vec3 {
                    let top = Vec3::new(
                        p00.x() + (p10.x() - p00.x()) * u,
                        p00.y() + (p10.y() - p00.y()) * u,
                        p00.z() + (p10.z() - p00.z()) * u,
                    );
                    let bot = Vec3::new(
                        p01.x() + (p11.x() - p01.x()) * u,
                        p01.y() + (p11.y() - p01.y()) * u,
                        p01.z() + (p11.z() - p01.z()) * u,
                    );
                    Vec3::new(
                        top.x() + (bot.x() - top.x()) * v,
                        top.y() + (bot.y() - top.y()) * v,
                        top.z() + (bot.z() - top.z()) * v,
                    )
                };

                let c00 = proj(corner_3d(u_ratio_0, v_ratio_0)).0;
                let c10 = proj(corner_3d(u_ratio_1, v_ratio_0)).0;
                let c11 = proj(corner_3d(u_ratio_1, v_ratio_1)).0;
                let c01 = proj(corner_3d(u_ratio_0, v_ratio_1)).0;

                painter.add(egui::Shape::convex_polygon(
                    vec![c00, c10, c11, c01],
                    lit_col,
                    Stroke::NONE,
                ));
            }
        }
    }
}

/// Applies joint pivots and rotation angles (head, arms, legs, cape).
fn apply_joint_animation(
    p: Vec3,
    leg_swing: f32,
    arm_swing: f32,
    head_sway: f32,
    cape_swing: f32,
    slim: bool,
) -> Vec3 {
    // Head: y in [24.0..32.0], pivot at neck (0, 24, 0)
    if p.y() >= 23.0 && p.y() <= 33.0 && p.z().abs() <= 5.5 {
        return rotate_pivot_x(p, Vec3::new(0.0, 24.0, 0.0), head_sway);
    }

    // Legs: y in [0.0..12.0], pivot at hip y=12
    if p.y() <= 12.0 {
        if p.x() < 0.0 {
            // Right leg
            return rotate_pivot_x(p, Vec3::new(-2.0, 12.0, 0.0), leg_swing);
        } else {
            // Left leg
            return rotate_pivot_x(p, Vec3::new(2.0, 12.0, 0.0), -leg_swing);
        }
    }

    // Arms: y in [12.0..24.0], pivot at shoulder y=22
    let arm_boundary = if slim { 3.5 } else { 3.8 };
    if p.y() >= 12.0 && p.y() <= 24.5 {
        if p.x() < -arm_boundary {
            // Right arm
            return rotate_pivot_x(p, Vec3::new(-5.0, 22.0, 0.0), -arm_swing);
        } else if p.x() > arm_boundary {
            // Left arm
            return rotate_pivot_x(p, Vec3::new(5.0, 22.0, 0.0), arm_swing);
        }
    }

    // Cape: behind torso (z <= -2.0, y in [6.0..24.0]), pivot at upper shoulders
    if p.z() <= -2.0 && p.y() >= 6.0 && p.y() <= 24.5 {
        return rotate_pivot_x(p, Vec3::new(0.0, 23.5, -2.0), cape_swing);
    }

    p
}

/// Samples pixel color from skin texture with Steve/Alex fallback.
fn sample_skin_color(
    skin: Option<&RgbaImage>,
    px: u32,
    py: u32,
    slim: bool,
    is_outer: bool,
) -> Option<Color32> {
    if let Some(img) = skin {
        let (w, h) = img.dimensions();
        if px < w && py < h {
            let Rgba([r, g, b, a]) = img.get_pixel(px, py);
            if *a < 20 {
                return None; // Transparent pixel
            }
            if is_outer && *a < 180 {
                return None;
            }
            return Some(Color32::from_rgba_unmultiplied(*r, *g, *b, *a));
        }
    }

    // Outer layers have no default geometry if skin is absent
    if is_outer {
        return None;
    }

    // Fallback procedural Steve / Alex palette
    Some(fallback_palette(px, py, slim))
}

/// Procedural Minecraft default palette (Steve / Alex) based on skin UV coordinates.
fn fallback_palette(px: u32, py: u32, slim: bool) -> Color32 {
    if (8..16).contains(&px) && (8..16).contains(&py) {
        // Face
        if (12..14).contains(&py) && (px == 9 || px == 10 || px == 13 || px == 14) {
            // Eyes
            if px == 10 || px == 13 {
                Color32::from_rgb(46, 56, 150) // Blue / green pupil
            } else {
                Color32::from_rgb(235, 235, 235) // White
            }
        } else if py >= 13 && (px == 11 || px == 12) {
            // Mouth / beard
            Color32::from_rgb(74, 46, 28)
        } else if py < 10 {
            // Hair
            Color32::from_rgb(74, 46, 28)
        } else {
            // Skin
            if slim {
                Color32::from_rgb(229, 172, 138)
            } else {
                Color32::from_rgb(181, 123, 87)
            }
        }
    } else if (px < 32 && py < 16) || ((40..48).contains(&px) && py < 8) {
        // Head hair / sides
        Color32::from_rgb(74, 46, 28)
    } else if (16..32).contains(&py) {
        if (16..40).contains(&px) {
            // Torso / shirt
            if py >= 28 {
                // Belt / waist
                Color32::from_rgb(46, 56, 117)
            } else if slim {
                Color32::from_rgb(92, 123, 56) // Alex green shirt
            } else {
                Color32::from_rgb(0, 158, 155) // Steve cyan shirt
            }
        } else if (40..60).contains(&px) {
            // Right arm
            if py < 24 {
                if slim {
                    Color32::from_rgb(92, 123, 56)
                } else {
                    Color32::from_rgb(0, 158, 155)
                }
            } else if slim {
                Color32::from_rgb(229, 172, 138)
            } else {
                Color32::from_rgb(181, 123, 87)
            }
        } else if px < 16 {
            // Right leg (upper = pants, lower = boots)
            if py >= 28 {
                Color32::from_rgb(60, 60, 60)
            } else {
                Color32::from_rgb(46, 56, 117)
            }
        } else {
            Color32::from_rgb(100, 100, 100)
        }
    } else if py >= 48 {
        // Left arm / left leg (64x64)
        if px >= 32 {
            // Left arm
            if py < 56 {
                if slim {
                    Color32::from_rgb(92, 123, 56)
                } else {
                    Color32::from_rgb(0, 158, 155)
                }
            } else if slim {
                Color32::from_rgb(229, 172, 138)
            } else {
                Color32::from_rgb(181, 123, 87)
            }
        } else {
            // Left leg
            if py >= 60 {
                Color32::from_rgb(60, 60, 60)
            } else {
                Color32::from_rgb(46, 56, 117)
            }
        }
    } else {
        // Cape / accents
        Color32::from_rgb(42, 60, 92)
    }
}

/// Helper to add a 6-sided cuboid to the faces list.
#[allow(clippy::too_many_arguments)]
fn add_cuboid(
    faces: &mut Vec<PartFace>,
    min: Vec3,
    max: Vec3,
    uv_base: (u32, u32),
    w: u32,
    h: u32,
    d: u32,
    is_outer: bool,
) {
    let (u0, v0) = uv_base;
    let (x0, y0, z0) = (min.x(), min.y(), min.z());
    let (x1, y1, z1) = (max.x(), max.y(), max.z());

    // Front face (z = max.z)
    faces.push(PartFace {
        corners: [
            Vec3::new(x0, y1, z1),
            Vec3::new(x1, y1, z1),
            Vec3::new(x1, y0, z1),
            Vec3::new(x0, y0, z1),
        ],
        uv_min: (u0 + d, v0 + d),
        uv_size: (w, h),
        is_outer,
    });

    // Back face (z = min.z)
    faces.push(PartFace {
        corners: [
            Vec3::new(x1, y1, z0),
            Vec3::new(x0, y1, z0),
            Vec3::new(x0, y0, z0),
            Vec3::new(x1, y0, z0),
        ],
        uv_min: (u0 + d + w + d, v0 + d),
        uv_size: (w, h),
        is_outer,
    });

    // Right face (x = min.x, character's right side)
    faces.push(PartFace {
        corners: [
            Vec3::new(x0, y1, z0),
            Vec3::new(x0, y1, z1),
            Vec3::new(x0, y0, z1),
            Vec3::new(x0, y0, z0),
        ],
        uv_min: (u0, v0 + d),
        uv_size: (d, h),
        is_outer,
    });

    // Left face (x = max.x, character's left side)
    faces.push(PartFace {
        corners: [
            Vec3::new(x1, y1, z1),
            Vec3::new(x1, y1, z0),
            Vec3::new(x1, y0, z0),
            Vec3::new(x1, y0, z1),
        ],
        uv_min: (u0 + d + w, v0 + d),
        uv_size: (d, h),
        is_outer,
    });

    // Top face (y = max.y)
    faces.push(PartFace {
        corners: [
            Vec3::new(x0, y1, z0),
            Vec3::new(x1, y1, z0),
            Vec3::new(x1, y1, z1),
            Vec3::new(x0, y1, z1),
        ],
        uv_min: (u0 + d, v0),
        uv_size: (w, d),
        is_outer,
    });

    // Bottom face (y = min.y)
    faces.push(PartFace {
        corners: [
            Vec3::new(x0, y0, z1),
            Vec3::new(x1, y0, z1),
            Vec3::new(x1, y0, z0),
            Vec3::new(x0, y0, z0),
        ],
        uv_min: (u0 + d + w, v0),
        uv_size: (w, d),
        is_outer,
    });
}

/// Builds all 3D character cuboids (Head, Torso, Arms, Legs, Hat layer, optional Cape).
fn build_character_faces(faces: &mut Vec<PartFace>, slim: bool, show_cape: bool) {
    // 1. Head (8x8x8): centered at [0, 24..32, 0]
    add_cuboid(
        faces,
        Vec3::new(-4.0, 24.0, -4.0),
        Vec3::new(4.0, 32.0, 4.0),
        (0, 0),
        8,
        8,
        8,
        false,
    );

    // 2. Head Hat layer (8x8x8 + 0.5 expand)
    add_cuboid(
        faces,
        Vec3::new(-4.4, 23.6, -4.4),
        Vec3::new(4.4, 32.4, 4.4),
        (32, 0),
        8,
        8,
        8,
        true,
    );

    // 3. Torso (8x12x4): centered at [0, 12..24, 0]
    add_cuboid(
        faces,
        Vec3::new(-4.0, 12.0, -2.0),
        Vec3::new(4.0, 24.0, 2.0),
        (16, 16),
        8,
        12,
        4,
        false,
    );

    // 4. Arms
    let arm_w: f32 = if slim { 3.0 } else { 4.0 };
    let arm_uv_w: u32 = if slim { 3 } else { 4 };

    // Right Arm: x in [-4 - arm_w .. -4]
    add_cuboid(
        faces,
        Vec3::new(-4.0 - arm_w, 12.0, -2.0),
        Vec3::new(-4.0, 24.0, 2.0),
        (40, 16),
        arm_uv_w,
        12,
        4,
        false,
    );

    // Left Arm: x in [4 .. 4 + arm_w]
    add_cuboid(
        faces,
        Vec3::new(4.0, 12.0, -2.0),
        Vec3::new(4.0 + arm_w, 24.0, 2.0),
        (32, 48),
        arm_uv_w,
        12,
        4,
        false,
    );

    // 5. Legs (4x12x4): y in [0..12]
    // Right Leg: x in [-4..0]
    add_cuboid(
        faces,
        Vec3::new(-4.0, 0.0, -2.0),
        Vec3::new(0.0, 12.0, 2.0),
        (0, 16),
        4,
        12,
        4,
        false,
    );

    // Left Leg: x in [0..4]
    add_cuboid(
        faces,
        Vec3::new(0.0, 0.0, -2.0),
        Vec3::new(4.0, 12.0, 2.0),
        (16, 48),
        4,
        12,
        4,
        false,
    );

    // 6. Cape (10x16x1) angled behind torso
    if show_cape {
        add_cuboid(
            faces,
            Vec3::new(-5.0, 7.5, -3.0),
            Vec3::new(5.0, 23.5, -2.0),
            (0, 0),
            10,
            16,
            1,
            false,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skin_3d_state_defaults() {
        let state = Skin3DState::default();
        assert!(!state.slim);
        assert!(!state.animate);
        assert!(state.show_cape);
        assert!(state.yaw.is_finite());
        assert!(state.pitch.is_finite());
    }

    #[test]
    fn test_vec3_operations() {
        let a = Vec3::new(1.0, 0.0, 0.0);
        let b = Vec3::new(0.0, 1.0, 0.0);
        let c = a.cross(b);
        assert_eq!(c, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(a.dot(b), 0.0);
        assert_eq!(c.length(), 1.0);
    }

    #[test]
    fn test_build_character_faces_face_count() {
        let mut faces_steve = Vec::new();
        build_character_faces(&mut faces_steve, false, true);
        // Head(6) + Hat(6) + Torso(6) + R_Arm(6) + L_Arm(6) + R_Leg(6) + L_Leg(6) + Cape(6) = 48
        assert_eq!(faces_steve.len(), 48);

        let mut faces_no_cape = Vec::new();
        build_character_faces(&mut faces_no_cape, true, false);
        assert_eq!(faces_no_cape.len(), 42);
    }

    #[test]
    fn test_zero_emojis_in_source() {
        let content = include_str!("skin_3d.rs");
        for ch in content.chars() {
            let u = ch as u32;
            let is_emoji = (0x1F300..=0x1FAFF).contains(&u)
                || (0x2600..=0x27BF).contains(&u)
                || (0xFE00..=0xFE0F).contains(&u);
            assert!(
                !is_emoji,
                "Forbidden emoji character found in skin_3d.rs: {ch:?} (U+{u:04X})"
            );
        }
    }
}
