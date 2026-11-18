//! Scene camera: Bevy 0.20's `PanOrbitCamera` (right drag orbit, left drag
//! pan, wheel zoom), plus an `F` key that frames the whole graph and a
//! guard that freezes the controller while the pointer is over a UI node.
//! Renders in HDR with bloom, AgX tonemapping, MSAA and distance fog.
//! Graph changes never move the camera.
//!
//! **Moves.** Anything that moves the camera by itself (the `F` key, the agent's
//! `viewer_camera` and `viewer_show` with `focus`, restoring the user's view)
//! asks for it through [`PendingCameraMove`]. [`apply_camera_moves`] turns the
//! request into a [`CameraTween`], an eased move over a few hundred
//! milliseconds, or places the camera at once when the duration is 0.
//! **The user always wins:** as soon as `PanOrbitCamera` reports that the user
//! is orbiting, panning or zooming, a running tween is dropped without writing
//! the camera again.
//!
//! The pose arithmetic (framing, zoom, easing) is plain functions, tested
//! without a window.

use bevy::camera::Hdr;
use bevy::camera_controller::pan_orbit_camera::controller::motion::CurrentMotion;
use bevy::camera_controller::pan_orbit_camera::prelude::*;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::picking::hover::Hovered;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::view::{Msaa, Tonemapping};
use bevy::ui::Pressed;

use crate::camera_input::CustomInputPlugin;
use crate::layout::LayoutState;

/// Marks the one scene camera.
#[derive(Component)]
pub struct SceneCamera;

/// Closest and farthest a zoom may take the camera from the point it looks at.
pub const MIN_DISTANCE: f32 = 1.5;
pub const MAX_DISTANCE: f32 = 5000.0;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MeshPickingPlugin,
            DefaultPanOrbitCameraPlugins,
            CustomInputPlugin,
        ))
        .init_resource::<CameraFocus>()
        .init_resource::<PendingCameraMove>()
        .add_systems(Startup, spawn_camera)
        .add_systems(
            Update,
            (
                frame_graph_on_f,
                block_controller_over_ui,
                // After everything that can ask for a move this frame.
                (apply_camera_moves, run_camera_tween)
                    .chain()
                    .after(crate::plugin::CoreSet::Ui),
            ),
        );
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        SceneCamera,
        Camera3d::default(),
        PanOrbitCamera::default(),
        Transform::from_xyz(0.0, 8.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
        // HDR so class materials (emissive > 1) bloom, a filmic
        // tonemapper to bring them back into display range, MSAA for the
        // thin edge lines, and a light distance fog for depth.
        Hdr,
        Bloom::NATURAL,
        Tonemapping::AgX,
        Msaa::Sample4,
        DistanceFog {
            color: Color::srgb(0.02, 0.02, 0.05),
            falloff: FogFalloff::Exponential { density: 0.006 },
            ..default()
        },
    ));
}

// ── Pose arithmetic ──────────────────────────────────────────────────────────

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

/// Where the camera is and what it looks at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPose {
    pub position: Vec3,
    pub look_at: Vec3,
}

impl CameraPose {
    pub fn distance(&self) -> f32 {
        self.position.distance(self.look_at)
    }

    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.position).looking_at(self.look_at, Vec3::Y)
    }
}

/// The pose that fits `positions` in view, looking along the current viewing
/// direction (`back` is the direction from the target towards the camera).
pub fn frame_pose(back: Vec3, positions: &[Vec3], fov_y: f32) -> Option<CameraPose> {
    let (center, radius) = bounding_sphere(positions)?;
    let back = back.try_normalize().unwrap_or(Vec3::Z);
    Some(CameraPose {
        position: center + back * framing_distance(radius, fov_y),
        look_at: center,
    })
}

/// Zoom by `factor`: above 1 the camera comes closer to what it looks at, below
/// 1 it moves away. The distance stays within [`MIN_DISTANCE`], [`MAX_DISTANCE`],
/// and the target does not change.
pub fn zoom_pose(pose: CameraPose, factor: f32) -> CameraPose {
    let dir = (pose.position - pose.look_at)
        .try_normalize()
        .unwrap_or(Vec3::Z);
    let distance = (pose.distance() / factor.max(f32::EPSILON)).clamp(MIN_DISTANCE, MAX_DISTANCE);
    CameraPose {
        position: pose.look_at + dir * distance,
        look_at: pose.look_at,
    }
}

/// What the camera is looking at. `PanOrbitCamera` stores no pivot, so when the
/// viewer has not set one (`focus`), take the point of the scene's centre that
/// lies on the line of sight, at least one unit ahead.
pub fn estimate_look_at(
    transform: &Transform,
    focus: Option<Vec3>,
    scene_center: Option<Vec3>,
) -> Vec3 {
    if let Some(f) = focus {
        return f;
    }
    let forward = transform.forward().as_vec3();
    let ahead = scene_center
        .map(|c| (c - transform.translation).dot(forward))
        .unwrap_or(30.0)
        .max(1.0);
    transform.translation + forward * ahead
}

/// Ease-in-out: slow at both ends. 0 at 0, 1 at 1.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The camera between `start` and `end`: position by lerp, rotation by slerp.
pub fn blend(start: &Transform, end: &Transform, t: f32) -> Transform {
    let t = ease_in_out(t);
    Transform {
        translation: start.translation.lerp(end.translation, t),
        rotation: start.rotation.slerp(end.rotation, t),
        scale: Vec3::ONE,
    }
}

// ── Moves ────────────────────────────────────────────────────────────────────

/// What the camera is looking at, when the viewer knows. A user orbit, pan or
/// zoom forgets it (the pivot is then the controller's own).
#[derive(Resource, Default, Debug)]
pub struct CameraFocus {
    pub look_at: Option<Vec3>,
}

/// A move somebody asked for. It is applied once per frame.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraMove {
    pub end: Transform,
    /// The point the camera will look at, if known.
    pub look_at: Option<Vec3>,
    pub duration_ms: u64,
}

impl CameraMove {
    pub fn to(pose: CameraPose, duration_ms: u64) -> Self {
        Self {
            end: pose.transform(),
            look_at: Some(pose.look_at),
            duration_ms,
        }
    }
}

#[derive(Resource, Default, Debug)]
pub struct PendingCameraMove(pub Option<CameraMove>);

/// An eased move in progress, on the scene camera.
#[derive(Component, Debug, Clone)]
pub struct CameraTween {
    pub start: Transform,
    pub end: Transform,
    pub look_at: Option<Vec3>,
    /// `Time::elapsed_secs` when it began, and how long it lasts.
    pub started: f32,
    pub duration: f32,
}

impl CameraTween {
    /// How far along, 0 to 1.
    pub fn progress(&self, now: f32) -> f32 {
        if self.duration <= 0.0 {
            1.0
        } else {
            ((now - self.started) / self.duration).clamp(0.0, 1.0)
        }
    }
}

/// Settle the camera at `end`: the controller's next orbit then pivots on the
/// same point.
fn settle(
    controller: &mut PanOrbitCamera,
    focus: &mut CameraFocus,
    end: &Transform,
    look_at: Option<Vec3>,
) {
    focus.look_at = look_at;
    if let Some(look_at) = look_at {
        controller.last_anchor_depth = -(end.translation.distance(look_at) as f64);
    }
}

/// Start the move that was asked for, or place the camera at once for a
/// duration of 0.
pub fn apply_camera_moves(
    mut commands: Commands,
    time: Res<Time>,
    mut pending: ResMut<PendingCameraMove>,
    mut focus: ResMut<CameraFocus>,
    mut camera: Query<(Entity, &mut Transform, &mut PanOrbitCamera), With<SceneCamera>>,
) {
    let Some(requested) = pending.0.take() else {
        return;
    };
    let Ok((entity, mut transform, mut controller)) = camera.single_mut() else {
        return;
    };
    // The user is moving the camera right now: their hand wins.
    if controller.current_motion.is_user_controlled() {
        return;
    }
    // Stop any momentum, so the controller does not push the camera on.
    controller.current_motion = CurrentMotion::Stationary;
    if requested.duration_ms == 0 {
        *transform = requested.end;
        settle(
            &mut controller,
            &mut focus,
            &requested.end,
            requested.look_at,
        );
        commands.entity(entity).remove::<CameraTween>();
        return;
    }
    commands.entity(entity).insert(CameraTween {
        start: *transform,
        end: requested.end,
        look_at: requested.look_at,
        started: time.elapsed_secs(),
        duration: requested.duration_ms as f32 / 1000.0,
    });
}

/// Advance the tween; drop it the moment the user touches the camera.
pub fn run_camera_tween(
    mut commands: Commands,
    time: Res<Time>,
    mut focus: ResMut<CameraFocus>,
    mut camera: Query<
        (
            Entity,
            &mut Transform,
            &mut PanOrbitCamera,
            Option<&CameraTween>,
        ),
        With<SceneCamera>,
    >,
) {
    let Ok((entity, mut transform, mut controller, tween)) = camera.single_mut() else {
        return;
    };
    if controller.current_motion.is_user_controlled() {
        focus.look_at = None;
        if tween.is_some() {
            commands.entity(entity).remove::<CameraTween>();
        }
        return;
    }
    let Some(tween) = tween else { return };
    let t = tween.progress(time.elapsed_secs());
    *transform = blend(&tween.start, &tween.end, t);
    if t >= 1.0 {
        let end = tween.end;
        let look_at = tween.look_at;
        settle(&mut controller, &mut focus, &end, look_at);
        commands.entity(entity).remove::<CameraTween>();
    }
}

/// `F` frames the whole graph, at once.
fn frame_graph_on_f(
    keys: Res<ButtonInput<KeyCode>>,
    layout: Option<Res<LayoutState>>,
    mut pending: ResMut<PendingCameraMove>,
    camera: Query<(&Transform, &Projection), With<SceneCamera>>,
) {
    if !keys.just_pressed(KeyCode::KeyF) {
        return;
    }
    let Some(layout) = layout else { return };
    let Ok((transform, projection)) = camera.single() else {
        return;
    };
    let fov_y = fov_of(projection);
    if let Some(pose) = frame_pose(transform.back().as_vec3(), &layout.live_positions(), fov_y) {
        pending.0 = Some(CameraMove::to(pose, 0));
    }
}

/// The vertical field of view of a projection (a perspective one in practice).
pub fn fov_of(projection: &Projection) -> f32 {
    match projection {
        Projection::Perspective(p) => p.fov,
        _ => std::f32::consts::FRAC_PI_4,
    }
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

    const FOV: f32 = std::f32::consts::FRAC_PI_4;

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
        assert!(framing_distance(10.0, FOV) > framing_distance(2.0, FOV));
    }

    #[test]
    fn framing_a_subset_centres_on_the_subset_and_keeps_the_viewing_direction() {
        // Two far-apart groups; frame only the one at x = 100.
        let subset = [Vec3::new(100.0, 0.0, 0.0), Vec3::new(104.0, 0.0, 0.0)];
        let back = Vec3::new(0.0, 0.0, 1.0);
        let pose = frame_pose(back, &subset, FOV).unwrap();
        assert!((pose.look_at - Vec3::new(102.0, 0.0, 0.0)).length() < 1e-4);
        let dir = (pose.position - pose.look_at).normalize();
        assert!(
            (dir - back).length() < 1e-5,
            "the camera keeps looking the same way"
        );
        assert!((pose.distance() - framing_distance(2.0, FOV)).abs() < 1e-3);
        // The whole scene needs a longer view than the subset.
        let all = [Vec3::new(-100.0, 0.0, 0.0), subset[0], subset[1]];
        assert!(frame_pose(back, &all, FOV).unwrap().distance() > pose.distance());
    }

    #[test]
    fn framing_one_node_stays_at_a_sane_distance() {
        let pose = frame_pose(Vec3::Z, &[Vec3::new(5.0, 5.0, 5.0)], FOV).unwrap();
        assert!(pose.distance() >= framing_distance(1.0, FOV) - 1e-4);
        assert!(pose.distance() < 5.0, "{}", pose.distance());
        assert!(frame_pose(Vec3::Z, &[], FOV).is_none());
    }

    #[test]
    fn a_degenerate_viewing_direction_falls_back_instead_of_producing_nan() {
        let pose = frame_pose(Vec3::ZERO, &[Vec3::ONE], FOV).unwrap();
        assert!(pose.position.is_finite());
    }

    #[test]
    fn zoom_moves_along_the_line_of_sight_and_keeps_the_target() {
        let pose = CameraPose {
            position: Vec3::new(0.0, 0.0, 20.0),
            look_at: Vec3::ZERO,
        };
        let closer = zoom_pose(pose, 2.0);
        assert!((closer.distance() - 10.0).abs() < 1e-4);
        assert_eq!(closer.look_at, Vec3::ZERO);
        assert!((closer.position - Vec3::new(0.0, 0.0, 10.0)).length() < 1e-4);
        let farther = zoom_pose(pose, 0.5);
        assert!((farther.distance() - 40.0).abs() < 1e-4);
        // Zooming in and out by the same factor comes back.
        let back = zoom_pose(zoom_pose(pose, 3.0), 1.0 / 3.0);
        assert!((back.position - pose.position).length() < 1e-3);
    }

    #[test]
    fn zoom_is_kept_within_the_limits() {
        let pose = CameraPose {
            position: Vec3::new(0.0, 0.0, 20.0),
            look_at: Vec3::ZERO,
        };
        assert!((zoom_pose(pose, 100.0).distance() - MIN_DISTANCE).abs() < 1e-4);
        assert!((zoom_pose(pose, 0.0001).distance() - MAX_DISTANCE).abs() < 1e-1);
        // Standing on the target: any direction will do, and nothing is NaN.
        let on = CameraPose {
            position: Vec3::ONE,
            look_at: Vec3::ONE,
        };
        assert!(zoom_pose(on, 2.0).position.is_finite());
    }

    #[test]
    fn the_look_at_point_is_the_focus_or_the_scene_centre_on_the_line_of_sight() {
        let t = Transform::from_xyz(0.0, 0.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y);
        assert_eq!(
            estimate_look_at(&t, Some(Vec3::new(1.0, 2.0, 3.0)), None),
            Vec3::new(1.0, 2.0, 3.0)
        );
        // The scene is centred off to the side: only the part along the view counts.
        let p = estimate_look_at(&t, None, Some(Vec3::new(10.0, 0.0, 0.0)));
        assert!((p - Vec3::new(0.0, 0.0, 0.0)).length() < 1e-3, "{p:?}");
        // Nothing known: a point ahead, never behind the camera.
        let p = estimate_look_at(&t, None, None);
        assert!(p.z < 30.0);
        let behind = estimate_look_at(&t, None, Some(Vec3::new(0.0, 0.0, 100.0)));
        assert!((behind - t.translation).length() >= 1.0 - 1e-4);
    }

    #[test]
    fn easing_starts_and_ends_slowly_and_is_monotonic() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert!(ease_in_out(0.1) < 0.1, "slow at the start");
        assert!(ease_in_out(0.9) > 0.9, "slow at the end");
        let mut last = 0.0;
        for i in 0..=100 {
            let v = ease_in_out(i as f32 / 100.0);
            assert!(v >= last);
            last = v;
        }
        assert_eq!(ease_in_out(-1.0), 0.0);
        assert_eq!(ease_in_out(2.0), 1.0);
    }

    #[test]
    fn a_blend_runs_from_the_start_to_the_end_pose() {
        let start = Transform::from_xyz(0.0, 0.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y);
        let end = Transform::from_xyz(30.0, 0.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y);
        assert!((blend(&start, &end, 0.0).translation - start.translation).length() < 1e-4);
        let done = blend(&start, &end, 1.0);
        assert!((done.translation - end.translation).length() < 1e-4);
        // (`angle_between` is only good to about 1e-3 for near-equal quaternions.)
        assert!((done.forward().as_vec3() - end.forward().as_vec3()).length() < 1e-4);
        // Half way round, the rotation is half way too and the camera is not at a corner.
        let mid = blend(&start, &end, 0.5);
        assert!(
            (mid.rotation.angle_between(start.rotation) - mid.rotation.angle_between(end.rotation))
                .abs()
                < 1e-3
        );
    }

    #[test]
    fn a_tween_reports_its_progress_and_a_zero_duration_is_done_at_once() {
        let t = CameraTween {
            start: Transform::IDENTITY,
            end: Transform::IDENTITY,
            look_at: None,
            started: 10.0,
            duration: 0.5,
        };
        assert_eq!(t.progress(10.0), 0.0);
        assert!((t.progress(10.25) - 0.5).abs() < 1e-6);
        assert_eq!(t.progress(11.0), 1.0);
        assert_eq!(CameraTween { duration: 0.0, ..t }.progress(10.0), 1.0);
    }
}

/// The systems, in a minimal Bevy app without a window or a renderer.
#[cfg(test)]
mod system_tests {
    use super::*;
    use std::time::Duration;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<CameraFocus>()
            .init_resource::<PendingCameraMove>()
            .add_systems(Update, (apply_camera_moves, run_camera_tween).chain());
        app.world_mut().spawn((
            SceneCamera,
            PanOrbitCamera::default(),
            Transform::from_xyz(0.0, 0.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
        app
    }

    fn step(app: &mut App, ms: u64) {
        // The time plugin overwrites `Time` every frame, so the step is set
        // through the strategy it reads.
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(ms),
        ));
        app.update();
    }

    fn camera(app: &mut App) -> (Transform, bool) {
        let mut q = app
            .world_mut()
            .query_filtered::<(&Transform, Option<&CameraTween>), With<SceneCamera>>();
        let (t, tw) = q.single(app.world()).unwrap();
        (*t, tw.is_some())
    }

    fn pose_far() -> CameraPose {
        CameraPose {
            position: Vec3::new(40.0, 0.0, 0.0),
            look_at: Vec3::ZERO,
        }
    }

    #[test]
    fn a_move_with_no_duration_places_the_camera_at_once_and_sets_the_focus() {
        let mut app = app();
        app.world_mut().resource_mut::<PendingCameraMove>().0 = Some(CameraMove::to(pose_far(), 0));
        step(&mut app, 16);
        let (t, tweening) = camera(&mut app);
        assert!((t.translation - Vec3::new(40.0, 0.0, 0.0)).length() < 1e-4);
        assert!(!tweening);
        assert_eq!(
            app.world().resource::<CameraFocus>().look_at,
            Some(Vec3::ZERO)
        );
        assert!(
            app.world().resource::<PendingCameraMove>().0.is_none(),
            "the request is consumed"
        );
    }

    #[test]
    fn a_tween_moves_the_camera_over_its_duration_and_then_settles() {
        let mut app = app();
        app.world_mut().resource_mut::<PendingCameraMove>().0 =
            Some(CameraMove::to(pose_far(), 400));
        step(&mut app, 16);
        assert!(camera(&mut app).1, "the tween is running");
        step(&mut app, 100);
        let (mid, tweening) = camera(&mut app);
        assert!(tweening);
        assert!(
            mid.translation.distance(Vec3::new(0.0, 0.0, 30.0)) > 1.0,
            "it has started to move"
        );
        assert!(
            mid.translation.distance(Vec3::new(40.0, 0.0, 0.0)) > 1.0,
            "and has not arrived"
        );
        // (virtual time never advances more than 250 ms in one frame)
        step(&mut app, 200);
        step(&mut app, 200);
        step(&mut app, 200);
        let (end, tweening) = camera(&mut app);
        assert!(!tweening, "finished tweens are removed");
        assert!((end.translation - Vec3::new(40.0, 0.0, 0.0)).length() < 1e-3);
        assert_eq!(
            app.world().resource::<CameraFocus>().look_at,
            Some(Vec3::ZERO)
        );
        // The next user orbit pivots on the framed point.
        let mut q = app.world_mut().query::<&PanOrbitCamera>();
        let depth = q.single(app.world()).unwrap().last_anchor_depth;
        assert!((depth + 40.0).abs() < 1e-3, "{depth}");
    }

    fn user_takes_the_camera(app: &mut App) {
        let mut q = app.world_mut().query::<&mut PanOrbitCamera>();
        let mut controller = q.single_mut(app.world_mut()).unwrap();
        controller.current_motion = CurrentMotion::UserControlled {
            anchor: bevy::math::DVec3::new(0.0, 0.0, -10.0),
            motion_inputs:
                bevy::camera_controller::pan_orbit_camera::controller::inputs::MotionInputs::Zoom {
                    zoom_inputs: Default::default(),
                },
        };
    }

    #[test]
    fn the_user_taking_the_camera_stops_a_tween_without_a_last_write() {
        let mut app = app();
        app.world_mut().resource_mut::<PendingCameraMove>().0 =
            Some(CameraMove::to(pose_far(), 400));
        step(&mut app, 16);
        step(&mut app, 100);
        let (before, _) = camera(&mut app);
        user_takes_the_camera(&mut app);
        step(&mut app, 100);
        let (after, tweening) = camera(&mut app);
        assert!(!tweening, "the tween is gone");
        assert_eq!(
            before.translation, after.translation,
            "the user's camera is not written to"
        );
        assert_eq!(
            app.world().resource::<CameraFocus>().look_at,
            None,
            "the pivot is the controller's now"
        );
    }

    #[test]
    fn a_move_asked_for_while_the_user_has_the_camera_is_dropped() {
        let mut app = app();
        user_takes_the_camera(&mut app);
        app.world_mut().resource_mut::<PendingCameraMove>().0 = Some(CameraMove::to(pose_far(), 0));
        step(&mut app, 16);
        let (t, tweening) = camera(&mut app);
        assert_eq!(t.translation, Vec3::new(0.0, 0.0, 30.0));
        assert!(!tweening);
    }

    #[test]
    fn a_new_move_replaces_one_in_progress_starting_from_where_the_camera_is() {
        let mut app = app();
        app.world_mut().resource_mut::<PendingCameraMove>().0 =
            Some(CameraMove::to(pose_far(), 400));
        step(&mut app, 16);
        step(&mut app, 150);
        let (here, _) = camera(&mut app);
        app.world_mut().resource_mut::<PendingCameraMove>().0 = Some(CameraMove::to(
            CameraPose {
                position: Vec3::new(0.0, 50.0, 0.1),
                look_at: Vec3::ZERO,
            },
            400,
        ));
        step(&mut app, 16);
        let mut q = app.world_mut().query::<&CameraTween>();
        let tween = q.single(app.world()).unwrap();
        assert!(
            tween.start.translation.distance(here.translation) < 5.0,
            "no jump back to the old start"
        );
    }
}
