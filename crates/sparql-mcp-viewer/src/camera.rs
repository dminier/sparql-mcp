//! Scene camera: Bevy 0.20's `PanOrbitCamera` (right drag orbit, left drag
//! pan, wheel zoom), plus an `F` key that frames the whole graph and a
//! guard that freezes the controller while the pointer is over a UI node.
//! Graph changes never move the camera. No system here reads `ViewerGraph`.

use bevy::camera_controller::pan_orbit_camera::prelude::*;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Pressed;

use crate::camera_input::CustomInputPlugin;
use crate::layout::LayoutState;

/// Marks the one scene camera.
#[derive(Component)]
pub struct SceneCamera;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MeshPickingPlugin,
            DefaultPanOrbitCameraPlugins,
            CustomInputPlugin,
        ))
        .add_systems(Startup, spawn_camera)
        .add_systems(Update, (frame_graph_on_f, block_controller_over_ui));
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        SceneCamera,
        Camera3d::default(),
        PanOrbitCamera::default(),
        Transform::from_xyz(0.0, 8.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Centroid and radius of the bounding sphere of every placed node.
pub fn bounding_sphere(positions: &[Vec3]) -> Option<(Vec3, f32)> {
    if positions.is_empty() {
        return None;
    }
    let center = positions.iter().copied().sum::<Vec3>() / positions.len() as f32;
    let radius = positions
        .iter()
        .map(|p| p.distance(center))
        .fold(0.0f32, f32::max);
    Some((center, radius))
}

/// Distance at which a sphere of `radius` fits a vertical field of view
/// `fov_y` (radians), with a small margin.
pub fn framing_distance(radius: f32, fov_y: f32) -> f32 {
    let r = radius.max(1.0);
    (r / (fov_y / 2.0).sin()) * 1.1
}

fn frame_graph_on_f(
    keys: Res<ButtonInput<KeyCode>>,
    layout: Option<Res<LayoutState>>,
    mut camera: Query<(&mut Transform, &Projection), With<SceneCamera>>,
) {
    if !keys.just_pressed(KeyCode::KeyF) {
        return;
    }
    let Some(layout) = layout else { return };
    let Some((center, radius)) = bounding_sphere(&layout.live_positions()) else {
        return;
    };
    let Ok((mut transform, projection)) = camera.single_mut() else {
        return;
    };
    let fov_y = match projection {
        Projection::Perspective(p) => p.fov,
        _ => std::f32::consts::FRAC_PI_4,
    };
    let back = transform.back().as_vec3();
    let distance = framing_distance(radius, fov_y);
    *transform = Transform::from_translation(center + back * distance).looking_at(center, Vec3::Y);
}

/// Freeze every motion while any UI node is hovered or pressed, so clicks
/// and scrolls over panels don't move the camera. Phase 5 panels must
/// carry `Hovered` for this to see them; until then the guard is inert.
fn block_controller_over_ui(
    hovered: Query<&Hovered, With<Node>>,
    pressed: Query<(), (With<Pressed>, With<Node>)>,
    mut cameras: Query<&mut PanOrbitCamera, With<SceneCamera>>,
) {
    let over_ui = hovered.iter().any(Hovered::get) || !pressed.is_empty();
    for mut controller in &mut cameras {
        let enabled = !over_ui;
        if controller.enabled_motion.pan != enabled {
            controller.enabled_motion.pan = enabled;
            controller.enabled_motion.orbit = enabled;
            controller.enabled_motion.zoom = enabled;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_sphere_of_symmetric_points() {
        let (c, r) =
            bounding_sphere(&[Vec3::new(-2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)]).unwrap();
        assert!(c.length() < 1e-6);
        assert!((r - 2.0).abs() < 1e-6);
    }

    #[test]
    fn bounding_sphere_of_nothing_is_none() {
        assert!(bounding_sphere(&[]).is_none());
    }

    #[test]
    fn framing_distance_grows_with_radius() {
        let fov = std::f32::consts::FRAC_PI_4;
        assert!(framing_distance(10.0, fov) > framing_distance(2.0, fov));
    }
}
