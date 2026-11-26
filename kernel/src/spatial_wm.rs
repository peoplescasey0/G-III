//! # Post Card OS — Ring-0 Spatial Window Manager (3D spatial hypervisor core)
//!
//! The spatial hypervisor owns the authoritative 3D world model for the
//! workspace: every card has a pose (position, rotation, uniform-or-anisotropic
//! scale), cards are depth ordered, and pointer input is resolved by casting a
//! real ray from a pinhole camera through the card planes instead of comparing
//! against 2D screen rectangles.
//!
//! ## Model
//! * All math is `no_std` `f32`; `sqrt`/`sin`/`cos` are implemented here because
//!   `core` does not expose them (see [`math`]).
//! * A card is a rectangle in its own local `z = 0` plane, centred on the
//!   origin, spanning `±width/2` × `±height/2` before scale is applied.
//! * A [`Pose`] maps local card space to world space:
//!   `world = position + rotation * (scale ⊙ local)`.
//! * [`World::hit_test`] returns the **nearest** hit along the ray, which is
//!   exactly the card a physical pointer would touch first. Ties fall back to
//!   the focus stack (topmost wins).
//!
//! ## Hypervisor syscall ABI
//! The dispatcher in `usermode.rs` exposes this world to ring 3 with the
//! project's `RAX`/`RDI`/`RSI`/`RDX` convention. Numbers live in
//! [`SYS_SPATIAL_CREATE_CARD`] .. [`SYS_SPATIAL_HIT_TEST`]. Descriptors are
//! `#[repr(C)]` and are copied out of user memory by the dispatcher after
//! `user_range_ok` validation; this module never sees a raw user pointer.
//!
//! ## Verification
//! The pure model has no kernel dependencies and no `unsafe`, so
//! `scripts/test_spatial_wm_host.sh` compiles this file with `rustc --test` and
//! runs the `#[cfg(test)]` suite on the host — no QEMU required.

use core::f32::consts::PI;
use core::ops::{Add, Div, Mul, Neg, Sub};

/// Maximum number of live spatial cards in one workspace.
pub const MAX_CARDS: usize = 32;

/// Length of the focus stack (bounded by the card capacity).
pub const MAX_FOCUS_DEPTH: usize = MAX_CARDS;

// ---------------------------------------------------------------------------
// Hypervisor syscall numbers and error codes
// ---------------------------------------------------------------------------

/// `SYS_SPATIAL_CREATE_CARD(*const CardDesc) -> u32 card id`
pub const SYS_SPATIAL_CREATE_CARD: u64 = 32;
/// `SYS_SPATIAL_DESTROY_CARD(card_id: u32) -> i64`
pub const SYS_SPATIAL_DESTROY_CARD: u64 = 33;
/// `SYS_SPATIAL_SET_POSE(card_id: u32, *const PoseDesc) -> i64`
pub const SYS_SPATIAL_SET_POSE: u64 = 34;
/// `SYS_SPATIAL_FOCUS_CARD(card_id: u32) -> i64`
pub const SYS_SPATIAL_FOCUS_CARD: u64 = 35;
/// `SYS_SPATIAL_HIT_TEST(*const RayDesc, *mut HitDesc) -> i64 card id`
pub const SYS_SPATIAL_HIT_TEST: u64 = 36;

/// Invalid argument (bad id, non-finite pose, zero scale, degenerate ray).
pub const EINVAL: i64 = -22;
/// No such card.
pub const ENOENT: i64 = -2;
/// Card capacity exhausted.
pub const ENOSPC: i64 = -28;
/// A user pointer argument was not a valid range in the caller's address space.
pub const EFAULT: i64 = -14;

/// Smallest usable scale magnitude; anything smaller is a degenerate pose.
const MIN_SCALE: f32 = 1.0e-4;
/// Rays more parallel than this to a card plane cannot hit it meaningfully.
const PARALLEL_EPSILON: f32 = 1.0e-6;
/// Distance tolerance used to treat two hits as coplanar (focus tie-break).
const TIE_EPSILON: f32 = 1.0e-4;

// ---------------------------------------------------------------------------
// no_std float math
// ---------------------------------------------------------------------------

/// Float helpers that `core` does not provide for `f32`.
pub mod math {
    use core::f32::consts::{FRAC_PI_2, PI};

    fn round(x: f32) -> f32 {
        if x >= 0.0 {
            (x + 0.5) as i32 as f32
        } else {
            (x - 0.5) as i32 as f32
        }
    }

    /// Newton–Raphson square root. Negative/NaN input yields `0.0`.
    pub fn sqrt(x: f32) -> f32 {
        if !(x > 0.0) {
            return 0.0;
        }
        if x.is_infinite() {
            return x;
        }
        let mut guess = if x > 1.0 { x * 0.5 } else { 1.0 };
        for _ in 0..24 {
            let next = 0.5 * (guess + x / guess);
            if (next - guess).abs() <= 1.0e-7 * next {
                return next;
            }
            guess = next;
        }
        guess
    }

    /// Sine and cosine of `radians`, range reduced to `[-π/2, π/2]` with the
    /// quadrant symmetry restored, then evaluated with a short Taylor series.
    pub fn sin_cos(radians: f32) -> (f32, f32) {
        let mut r = radians - (2.0 * PI) * round(radians / (2.0 * PI));
        let mut flip_cos = false;
        if r > FRAC_PI_2 {
            r = PI - r;
            flip_cos = true;
        } else if r < -FRAC_PI_2 {
            r = -PI - r;
            flip_cos = true;
        }
        // Truncation error over |r| ≤ π/2 is < 5e-7 for both series.
        let r2 = r * r;
        let sin = r
            * (1.0
                - r2 * (1.0 / 6.0
                    - r2 * (1.0 / 120.0
                        - r2 * (1.0 / 5040.0
                            - r2 * (1.0 / 362_880.0 - r2 / 39_916_800.0)))));
        let mut cos = 1.0
            - r2 * (1.0 / 2.0
                - r2 * (1.0 / 24.0
                    - r2 * (1.0 / 720.0
                        - r2 * (1.0 / 40_320.0 - r2 / 3_628_800.0))));
        if flip_cos {
            cos = -cos;
        }
        (sin, cos)
    }

    /// Tangent of `radians`; returns `None` where cosine vanishes.
    pub fn tan(radians: f32) -> Option<f32> {
        let (sin, cos) = sin_cos(radians);
        if cos.abs() < 1.0e-6 {
            None
        } else {
            Some(sin / cos)
        }
    }
}

// ---------------------------------------------------------------------------
// Vector / quaternion / matrix primitives
// ---------------------------------------------------------------------------

/// A 3D vector in world or local space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };
    pub const UNIT_X: Vec3 = Vec3 { x: 1.0, y: 0.0, z: 0.0 };
    pub const UNIT_Y: Vec3 = Vec3 { x: 0.0, y: 1.0, z: 0.0 };
    pub const UNIT_Z: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 1.0 };

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }

    pub fn dot(self, other: Vec3) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Vec3) -> Vec3 {
        Vec3 {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    pub fn length(self) -> f32 {
        math::sqrt(self.length_squared())
    }

    /// Unit vector, or `None` for a zero/NaN vector.
    pub fn normalized(self) -> Option<Vec3> {
        let len = self.length();
        if !(len > 0.0) || !len.is_finite() {
            return None;
        }
        Some(self / len)
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, rhs: Vec3) -> Vec3 {
        Vec3::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, rhs: Vec3) -> Vec3 {
        Vec3::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, rhs: f32) -> Vec3 {
        Vec3::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl Div<f32> for Vec3 {
    type Output = Vec3;
    fn div(self, rhs: f32) -> Vec3 {
        Vec3::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

impl Default for Vec3 {
    fn default() -> Self {
        Vec3::ZERO
    }
}

/// Unit quaternion (`x, y, z, w`) rotating local space into world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Quat { x, y, z, w }
    }

    /// Rotation of `radians` about `axis` (normalised internally).
    pub fn from_axis_angle(axis: Vec3, radians: f32) -> Quat {
        let Some(axis) = axis.normalized() else {
            return Quat::IDENTITY;
        };
        let (sin, cos) = math::sin_cos(radians * 0.5);
        Quat::new(axis.x * sin, axis.y * sin, axis.z * sin, cos)
    }

    /// Intrinsic yaw (Y), then pitch (X), then roll (Z).
    pub fn from_euler(yaw: f32, pitch: f32, roll: f32) -> Quat {
        (Quat::from_axis_angle(Vec3::UNIT_Y, yaw)
            * Quat::from_axis_angle(Vec3::UNIT_X, pitch)
            * Quat::from_axis_angle(Vec3::UNIT_Z, roll))
        .normalized()
        .unwrap_or(Quat::IDENTITY)
    }

    pub fn length(self) -> f32 {
        math::sqrt(self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w)
    }

    /// Unit quaternion, or `None` when the input cannot be normalised.
    pub fn normalized(self) -> Option<Quat> {
        let len = self.length();
        if !(len > 0.0) || !len.is_finite() {
            return None;
        }
        Some(Quat::new(self.x / len, self.y / len, self.z / len, self.w / len))
    }

    pub fn conjugate(self) -> Quat {
        Quat::new(-self.x, -self.y, -self.z, self.w)
    }

    /// Rotate `v` by this quaternion (`q * v * q⁻¹`, expanded).
    pub fn rotate_vec3(self, v: Vec3) -> Vec3 {
        let axis = Vec3::new(self.x, self.y, self.z);
        let t = axis.cross(v) * 2.0;
        v + t * self.w + axis.cross(t)
    }

    /// Rotate `v` by the inverse rotation.
    pub fn inverse_rotate_vec3(self, v: Vec3) -> Vec3 {
        self.conjugate().rotate_vec3(v)
    }

    pub fn to_matrix(self) -> Mat3 {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);
        Mat3 {
            columns: [
                Vec3::new(1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (xz - wy)),
                Vec3::new(2.0 * (xy - wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + wx)),
                Vec3::new(2.0 * (xz + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (xx + yy)),
            ],
        }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite() && self.w.is_finite()
    }
}

impl Mul for Quat {
    type Output = Quat;
    fn mul(self, rhs: Quat) -> Quat {
        Quat::new(
            self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
            self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
        )
    }
}

impl Default for Quat {
    fn default() -> Self {
        Quat::IDENTITY
    }
}

/// Column-major 3×3 rotation matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3 {
    pub columns: [Vec3; 3],
}

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3 {
        columns: [Vec3::UNIT_X, Vec3::UNIT_Y, Vec3::UNIT_Z],
    };

    pub fn mul_vec3(self, v: Vec3) -> Vec3 {
        self.columns[0] * v.x + self.columns[1] * v.y + self.columns[2] * v.z
    }

    pub fn transpose(self) -> Mat3 {
        let c = self.columns;
        Mat3 {
            columns: [
                Vec3::new(c[0].x, c[1].x, c[2].x),
                Vec3::new(c[0].y, c[1].y, c[2].y),
                Vec3::new(c[0].z, c[1].z, c[2].z),
            ],
        }
    }
}

impl Default for Mat3 {
    fn default() -> Self {
        Mat3::IDENTITY
    }
}

/// Position + rotation + scale of one card in the workspace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3 { x: 1.0, y: 1.0, z: 1.0 },
    };

    pub const fn new(position: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Pose { position, rotation, scale }
    }

    /// Uniform scale on every axis.
    pub const fn uniform(position: Vec3, rotation: Quat, scale: f32) -> Self {
        Pose {
            position,
            rotation,
            scale: Vec3 { x: scale, y: scale, z: scale },
        }
    }

    /// `world = position + rotation * (scale ⊙ local)`
    pub fn to_world(&self, local: Vec3) -> Vec3 {
        self.position
            + self.rotation.rotate_vec3(Vec3::new(
                local.x * self.scale.x,
                local.y * self.scale.y,
                local.z * self.scale.z,
            ))
    }

    /// Inverse of [`Pose::to_world`]; `None` when a scale axis is degenerate.
    pub fn to_local(&self, world: Vec3) -> Option<Vec3> {
        if self.scale.x.abs() < MIN_SCALE
            || self.scale.y.abs() < MIN_SCALE
            || self.scale.z.abs() < MIN_SCALE
        {
            return None;
        }
        let rotated = self.rotation.inverse_rotate_vec3(world - self.position);
        Some(Vec3::new(
            rotated.x / self.scale.x,
            rotated.y / self.scale.y,
            rotated.z / self.scale.z,
        ))
    }

    /// Unit normal of the card plane in world space.
    pub fn plane_normal(&self) -> Vec3 {
        self.rotation.rotate_vec3(Vec3::UNIT_Z)
    }

    pub fn is_finite(&self) -> bool {
        self.position.is_finite() && self.rotation.is_finite() && self.scale.is_finite()
    }

    /// A pose is usable when it is finite and no scale axis collapses.
    pub fn is_valid(&self) -> bool {
        self.is_finite()
            && self.scale.x.abs() >= MIN_SCALE
            && self.scale.y.abs() >= MIN_SCALE
            && self.scale.z.abs() >= MIN_SCALE
            && self.rotation.length() > 0.5
    }
}

impl Default for Pose {
    fn default() -> Self {
        Pose::IDENTITY
    }
}

// ---------------------------------------------------------------------------
// Rays and hits
// ---------------------------------------------------------------------------

/// A pointer ray: origin plus a **normalised** direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

impl Ray {
    /// Builds a ray; `None` for a degenerate or non-finite direction.
    pub fn new(origin: Vec3, direction: Vec3) -> Option<Ray> {
        if !origin.is_finite() {
            return None;
        }
        let direction = direction.normalized()?;
        Some(Ray { origin, direction })
    }

    /// Point at distance `t` along the (unit) direction.
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }
}

/// Nearest intersection of a ray with a card's rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub card_id: u32,
    /// Distance from the ray origin along the unit direction.
    pub distance: f32,
    /// World-space intersection point.
    pub point: Vec3,
    /// Intersection in card-local (unscaled) space.
    pub local: Vec3,
    /// Normalised card coordinates, `0..1` across width/height.
    pub u: f32,
    pub v: f32,
}

/// Intersection of a ray with the infinite plane through `plane_point`.
pub fn ray_plane(ray: &Ray, plane_point: Vec3, normal: Vec3) -> Option<f32> {
    let denominator = normal.dot(ray.direction);
    if denominator.abs() < PARALLEL_EPSILON {
        return None;
    }
    let t = normal.dot(plane_point - ray.origin) / denominator;
    if !t.is_finite() || t < 0.0 {
        return None;
    }
    Some(t)
}

/// Ray/rectangle intersection in 3D. Two-sided: back faces are hittable.
pub fn intersect_card(ray: &Ray, card: &SpatialCard) -> Option<Hit> {
    if !card.active || !(card.width > 0.0) || !(card.height > 0.0) {
        return None;
    }
    if !card.pose.is_valid() {
        return None;
    }
    let t = ray_plane(ray, card.pose.position, card.pose.plane_normal())?;
    let point = ray.at(t);
    let local = card.pose.to_local(point)?;
    let half_width = card.width * 0.5;
    let half_height = card.height * 0.5;
    if local.x.abs() > half_width || local.y.abs() > half_height {
        return None;
    }
    Some(Hit {
        card_id: card.id,
        distance: t,
        point,
        local,
        u: (local.x + half_width) / card.width,
        v: (local.y + half_height) / card.height,
    })
}

// ---------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------

/// One workspace card as tracked by the hypervisor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialCard {
    pub id: u32,
    pub pose: Pose,
    /// Card extent in local (pre-scale) units.
    pub width: f32,
    pub height: f32,
    pub active: bool,
    pub focused: bool,
}

impl SpatialCard {
    /// Placeholder used to initialise fixed-capacity storage.
    pub const EMPTY: SpatialCard = SpatialCard {
        id: 0,
        pose: Pose::IDENTITY,
        width: 0.0,
        height: 0.0,
        active: false,
        focused: false,
    };

    pub fn new(id: u32, pose: Pose, width: f32, height: f32) -> Self {
        SpatialCard {
            id,
            pose,
            width,
            height,
            active: true,
            focused: false,
        }
    }

    /// Distance from an eye point; used for painter's-order depth sorting.
    pub fn depth_from(&self, eye: Vec3) -> f32 {
        (self.pose.position - eye).length()
    }

    /// Corner world positions in local order: `(-x,-y) (+x,-y) (+x,+y) (-x,+y)`.
    pub fn corners(&self) -> [Vec3; 4] {
        let hw = self.width * 0.5;
        let hh = self.height * 0.5;
        [
            self.pose.to_world(Vec3::new(-hw, -hh, 0.0)),
            self.pose.to_world(Vec3::new(hw, -hh, 0.0)),
            self.pose.to_world(Vec3::new(hw, hh, 0.0)),
            self.pose.to_world(Vec3::new(-hw, hh, 0.0)),
        ]
    }
}

impl Default for SpatialCard {
    fn default() -> Self {
        SpatialCard::EMPTY
    }
}

/// Allocation-free snapshot of the workspace, returned by
/// [`World::cards`] / [`active_cards`].
#[derive(Debug, Clone, Copy)]
pub struct CardList {
    cards: [SpatialCard; MAX_CARDS],
    len: usize,
}

impl CardList {
    pub const fn empty() -> Self {
        CardList { cards: [SpatialCard::EMPTY; MAX_CARDS], len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> core::slice::Iter<'_, SpatialCard> {
        self.cards[..self.len].iter()
    }

    pub fn as_slice(&self) -> &[SpatialCard] {
        &self.cards[..self.len]
    }
}

impl<'a> IntoIterator for &'a CardList {
    type Item = &'a SpatialCard;
    type IntoIter = core::slice::Iter<'a, SpatialCard>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for CardList {
    type Item = SpatialCard;
    type IntoIter = core::iter::Take<core::array::IntoIter<SpatialCard, MAX_CARDS>>;
    fn into_iter(self) -> Self::IntoIter {
        self.cards.into_iter().take(self.len)
    }
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// Pinhole camera used to turn pointer coordinates into world-space rays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub position: Vec3,
    pub rotation: Quat,
    /// Vertical field of view in radians.
    pub fov_y: f32,
}

impl Camera {
    /// The workspace camera: 1024×768, 60° vertical FOV, at `z = -700` looking
    /// down `+Z` with `+Y` up.
    pub const fn workspace() -> Camera {
        Camera {
            viewport_width: 1024.0,
            viewport_height: 768.0,
            position: Vec3::new(512.0, 384.0, -700.0),
            rotation: Quat::IDENTITY,
            fov_y: PI / 3.0,
        }
    }

    /// Forward axis (`+Z` in camera space).
    pub fn forward(&self) -> Vec3 {
        self.rotation.rotate_vec3(Vec3::UNIT_Z)
    }

    /// Right axis (`+X` in camera space).
    pub fn right(&self) -> Vec3 {
        self.rotation.rotate_vec3(Vec3::UNIT_X)
    }

    /// Up axis (`+Y` in camera space).
    pub fn up(&self) -> Vec3 {
        self.rotation.rotate_vec3(Vec3::UNIT_Y)
    }

    /// Ray through a pointer in viewport pixels (origin at the top-left, `y`
    /// growing downward). Returns `None` for a degenerate camera.
    pub fn ray_for_screen(&self, x: f32, y: f32) -> Option<Ray> {
        if !(self.viewport_width > 0.0) || !(self.viewport_height > 0.0) {
            return None;
        }
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let tan_half = math::tan(self.fov_y * 0.5)?;
        let aspect = self.viewport_width / self.viewport_height;
        let ndc_x = (x / self.viewport_width) * 2.0 - 1.0;
        let ndc_y = 1.0 - (y / self.viewport_height) * 2.0;
        let direction = self.forward()
            + self.right() * (ndc_x * tan_half * aspect)
            + self.up() * (ndc_y * tan_half);
        Ray::new(self.position, direction)
    }
}

impl Default for Camera {
    fn default() -> Self {
        Camera::workspace()
    }
}

// ---------------------------------------------------------------------------
// Pointer input
// ---------------------------------------------------------------------------

/// A pointer sample in viewport pixels. `pressed` begins a grab, `dragged`
/// continues one, and a sample with neither settles the interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerEvent {
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
    pub dragged: bool,
}

#[derive(Debug, Clone, Copy)]
struct Grab {
    card_id: u32,
    /// World anchor on the card plane when the grab started.
    anchor: Vec3,
    normal: Vec3,
    /// Card position when the grab started, for absolute (drift-free) deltas.
    start_position: Vec3,
}

// ---------------------------------------------------------------------------
// World
// ---------------------------------------------------------------------------

/// Authoritative spatial world model.
#[derive(Debug, Clone)]
pub struct World {
    cards: [Option<SpatialCard>; MAX_CARDS],
    /// Focus stack, `0..focus_len` from bottom to top.
    focus: [u32; MAX_FOCUS_DEPTH],
    focus_len: usize,
    next_id: u32,
    camera: Camera,
    grab: Option<Grab>,
    version: u64,
}

impl World {
    /// An empty world using the default workspace camera.
    pub const fn new() -> World {
        World {
            cards: [None; MAX_CARDS],
            focus: [0; MAX_FOCUS_DEPTH],
            focus_len: 0,
            next_id: 1,
            camera: Camera::workspace(),
            grab: None,
            version: 0,
        }
    }

    /// Monotonic revision, bumped on every mutation. The VM-control API uses it
    /// to tell the studio when its cached pose snapshot is stale.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Serialises the workspace as one line of JSON.
    ///
    /// This is the kernel half of the host bridge: the shell prints it as
    /// `SPATIALv1 {...}` on COM1 and `workspace-prototype/kernel-bridge.mjs`
    /// parses it. Fixed decimal precision keeps a frame byte-stable when
    /// nothing moved, and `paint_order` is reused so the host sees the same
    /// z-depth order the renderer does.
    pub fn write_json(&self, out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        core::write!(out, "{{\"v\":1,\"rev\":{},\"focus\":", self.version)?;
        match self.focused_id() {
            Some(id) => core::write!(out, "{}", id)?,
            None => out.write_str("null")?,
        }
        out.write_str(",\"cards\":[")?;
        let mut first = true;
        for card in self.paint_order().as_slice() {
            if !first {
                out.write_str(",")?;
            }
            first = false;
            let p = card.pose.position;
            let r = card.pose.rotation;
            let s = card.pose.scale;
            core::write!(
                out,
                "{{\"id\":{},\"px\":{:.3},\"py\":{:.3},\"pz\":{:.3},\"qx\":{:.5},\"qy\":{:.5},\"qz\":{:.5},\"qw\":{:.5},\"sx\":{:.3},\"sy\":{:.3},\"sz\":{:.3},\"w\":{:.1},\"h\":{:.1},\"active\":{},\"focused\":{}}}",
                card.id,
                p.x,
                p.y,
                p.z,
                r.x,
                r.y,
                r.z,
                r.w,
                s.x,
                s.y,
                s.z,
                card.width,
                card.height,
                card.active,
                card.focused
            )?;
        }
        out.write_str("]}")
    }

    pub fn camera(&self) -> Camera {
        self.camera
    }

    fn bump(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Seeds the two demonstration cards used by the shell and self-tests.
    pub fn seed_demo(&mut self) {
        let first = SpatialCard::new(
            1,
            Pose::uniform(Vec3::new(500.0, 400.0, 0.0), Quat::IDENTITY, 1.0),
            320.0,
            200.0,
        );
        let second = SpatialCard::new(
            2,
            Pose::uniform(
                Vec3::new(900.0, 420.0, 140.0),
                Quat::from_euler(0.18, 0.0, 0.0),
                1.0,
            ),
            400.0,
            300.0,
        );
        self.cards[0] = Some(first);
        self.cards[1] = Some(second);
        self.focus_len = 0;
        // Seed order is stack order: card 1 ends up on top and focused.
        self.focus_stack_push(2);
        self.focus_stack_push(1);
        self.next_id = 3;
        self.grab = None;
        self.bump();
        self.refresh_focus_flags();
    }

    fn slot(&self, id: u32) -> Option<usize> {
        self.cards.iter().position(|c| matches!(c, Some(card) if card.id == id))
    }

    pub fn card(&self, id: u32) -> Option<&SpatialCard> {
        self.slot(id).and_then(|index| self.cards[index].as_ref())
    }

    pub fn card_mut(&mut self, id: u32) -> Option<&mut SpatialCard> {
        match self.slot(id) {
            Some(index) => self.cards[index].as_mut(),
            None => None,
        }
    }

    /// All live cards, insertion ordered.
    pub fn cards(&self) -> CardList {
        let mut list = CardList::empty();
        for card in self.cards.iter().flatten() {
            list.cards[list.len] = *card;
            list.len += 1;
        }
        list
    }

    /// Live cards sorted **farthest first** for painter's-order compositing.
    pub fn paint_order(&self) -> CardList {
        let mut list = self.cards();
        let eye = self.camera.position;
        let slice = &mut list.cards[..list.len];
        for i in 1..slice.len() {
            let mut j = i;
            while j > 0 && slice[j - 1].depth_from(eye) < slice[j].depth_from(eye) {
                slice.swap(j - 1, j);
                j -= 1;
            }
        }
        list
    }

    pub fn focused_id(&self) -> Option<u32> {
        if self.focus_len == 0 {
            None
        } else {
            Some(self.focus[self.focus_len - 1])
        }
    }

    /// Focus stack from bottom to top. Test-only: the kernel shell reports the
    /// focused card through [`World::focused_id`].
    #[cfg(test)]
    pub fn focus_order(&self) -> &[u32] {
        &self.focus[..self.focus_len]
    }

    fn focus_rank(&self, id: u32) -> Option<usize> {
        self.focus[..self.focus_len].iter().position(|&c| c == id)
    }

    fn focus_stack_remove(&mut self, id: u32) {
        if let Some(index) = self.focus_rank(id) {
            for i in index..self.focus_len - 1 {
                self.focus[i] = self.focus[i + 1];
            }
            self.focus_len -= 1;
        }
    }

    fn focus_stack_push(&mut self, id: u32) {
        self.focus_stack_remove(id);
        if self.focus_len < MAX_FOCUS_DEPTH {
            self.focus[self.focus_len] = id;
            self.focus_len += 1;
        }
    }

    fn refresh_focus_flags(&mut self) {
        let top = self.focused_id();
        for entry in self.cards.iter_mut().flatten() {
            entry.focused = Some(entry.id) == top;
        }
    }

    /// Creates a card and returns its id. `None` when the workspace is full.
    pub fn create_card(&mut self, width: f32, height: f32, pose: Pose) -> Option<u32> {
        if !(width > 0.0) || !(height > 0.0) || !width.is_finite() || !height.is_finite() {
            return None;
        }
        if !pose.is_valid() {
            return None;
        }
        let free = self.cards.iter().position(|c| c.is_none())?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.cards[free] = Some(SpatialCard::new(id, pose, width, height));
        self.focus_stack_push(id);
        self.refresh_focus_flags();
        self.bump();
        Some(id)
    }

    /// Removes a card. Returns `false` for an unknown id.
    pub fn destroy_card(&mut self, id: u32) -> bool {
        let Some(index) = self.slot(id) else {
            return false;
        };
        self.cards[index] = None;
        self.focus_stack_remove(id);
        if matches!(self.grab, Some(grab) if grab.card_id == id) {
            self.grab = None;
        }
        self.refresh_focus_flags();
        self.bump();
        true
    }

    /// Replaces a card's pose. Returns `false` for an unknown id or a pose the
    /// hypervisor refuses (non-finite values, collapsed scale).
    pub fn set_pose(&mut self, id: u32, pose: Pose) -> bool {
        if !pose.is_valid() {
            return false;
        }
        let Some(card) = self.card_mut(id) else {
            return false;
        };
        card.pose = pose;
        self.bump();
        true
    }

    /// Raises a card to the top of the focus stack, making it the input target.
    pub fn focus_card(&mut self, id: u32) -> bool {
        if self.slot(id).is_none() {
            return false;
        }
        self.focus_stack_push(id);
        self.refresh_focus_flags();
        self.bump();
        true
    }

    /// Nearest card hit by `ray`.
    pub fn hit_test(&self, ray: &Ray) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for card in self.cards.iter().flatten() {
            let Some(hit) = intersect_card(ray, card) else {
                continue;
            };
            let replace = match best {
                None => true,
                Some(current) => {
                    if (hit.distance - current.distance).abs() <= TIE_EPSILON {
                        // Coplanar overlap: the higher focus entry owns the pixel.
                        self.focus_rank(hit.card_id).unwrap_or(0)
                            > self.focus_rank(current.card_id).unwrap_or(0)
                    } else {
                        hit.distance < current.distance
                    }
                }
            };
            if replace {
                best = Some(hit);
            }
        }
        best
    }

    /// Only the focused card receives routed input, so a hit on a background
    /// card is delivered to the focused card's owner instead.
    pub fn route_input(&self, hit: &Hit) -> Option<u32> {
        match self.focused_id() {
            Some(focused) if focused == hit.card_id => Some(hit.card_id),
            Some(focused) if self.card(focused).is_some() => Some(focused),
            _ => Some(hit.card_id),
        }
    }

    /// Resolves a pointer sample into the world: focus changes, and drags use
    /// the true ray/plane displacement rather than a fixed step.
    pub fn apply_pointer(&mut self, event: PointerEvent) -> Option<u32> {
        let ray = self.camera.ray_for_screen(event.x as f32, event.y as f32)?;
        let hit = self.hit_test(&ray);

        if event.pressed {
            match hit {
                Some(hit) => {
                    self.focus_card(hit.card_id);
                    if let Some(card) = self.card(hit.card_id) {
                        self.grab = Some(Grab {
                            card_id: hit.card_id,
                            anchor: hit.point,
                            normal: card.pose.plane_normal(),
                            start_position: card.pose.position,
                        });
                    }
                }
                None => self.grab = None,
            }
        }

        if event.dragged {
            if let Some(grab) = self.grab {
                if let Some(t) = ray_plane(&ray, grab.anchor, grab.normal) {
                    let destination = ray.at(t);
                    let position = grab.start_position + (destination - grab.anchor);
                    if let Some(card) = self.card_mut(grab.card_id) {
                        card.pose.position = position;
                        self.bump();
                    }
                }
            }
        }

        if !event.pressed && !event.dragged {
            self.grab = None;
        }

        hit.map(|hit| self.route_input(&hit)).flatten()
    }

    // -- hypervisor syscalls (pure; the dispatcher validates user pointers) --

    /// `SYS_SPATIAL_CREATE_CARD`
    pub fn sys_create_card(&mut self, desc: CardDesc) -> i64 {
        let Some(pose) = desc.pose.to_pose() else {
            return EINVAL;
        };
        match self.create_card(desc.width, desc.height, pose) {
            Some(id) => id as i64,
            None => {
                if self.cards.iter().all(|c| c.is_some()) {
                    ENOSPC
                } else {
                    EINVAL
                }
            }
        }
    }

    /// `SYS_SPATIAL_DESTROY_CARD`
    pub fn sys_destroy_card(&mut self, id: u64) -> i64 {
        let Ok(id) = u32::try_from(id) else {
            return EINVAL;
        };
        if self.destroy_card(id) {
            0
        } else {
            ENOENT
        }
    }

    /// `SYS_SPATIAL_SET_POSE`
    pub fn sys_set_pose(&mut self, id: u64, desc: PoseDesc) -> i64 {
        let Ok(id) = u32::try_from(id) else {
            return EINVAL;
        };
        let Some(pose) = desc.to_pose() else {
            return EINVAL;
        };
        if self.card(id).is_none() {
            return ENOENT;
        }
        if self.set_pose(id, pose) {
            0
        } else {
            EINVAL
        }
    }

    /// `SYS_SPATIAL_FOCUS_CARD`
    pub fn sys_focus_card(&mut self, id: u64) -> i64 {
        let Ok(id) = u32::try_from(id) else {
            return EINVAL;
        };
        if self.focus_card(id) {
            0
        } else {
            ENOENT
        }
    }

    /// `SYS_SPATIAL_HIT_TEST`
    pub fn sys_hit_test(&self, desc: RayDesc) -> Result<Hit, i64> {
        let origin = desc.origin_vec();
        let direction = desc.direction_vec();
        if !origin.is_finite() || !direction.is_finite() {
            return Err(EINVAL);
        }
        let ray = Ray::new(origin, direction).ok_or(EINVAL)?;
        self.hit_test(&ray).ok_or(ENOENT)
    }
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

// ---------------------------------------------------------------------------
// Syscall descriptors (shared with ring 3)
// ---------------------------------------------------------------------------

/// `#[repr(C)]` pose descriptor: `rotation` is `(x, y, z, w)`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PoseDesc {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl PoseDesc {
    /// Validated conversion; normalises the quaternion and rejects degenerate
    /// or non-finite input.
    pub fn to_pose(&self) -> Option<Pose> {
        let position = Vec3::new(self.position[0], self.position[1], self.position[2]);
        let rotation = Quat::new(
            self.rotation[0],
            self.rotation[1],
            self.rotation[2],
            self.rotation[3],
        )
        .normalized()?;
        let scale = Vec3::new(self.scale[0], self.scale[1], self.scale[2]);
        let pose = Pose { position, rotation, scale };
        if pose.is_valid() {
            Some(pose)
        } else {
            None
        }
    }
}

/// `#[repr(C)]` card creation descriptor.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CardDesc {
    pub width: f32,
    pub height: f32,
    pub pose: PoseDesc,
}

/// `#[repr(C)]` pointer ray descriptor.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RayDesc {
    pub origin: [f32; 3],
    pub direction: [f32; 3],
}

impl RayDesc {
    fn origin_vec(&self) -> Vec3 {
        Vec3::new(self.origin[0], self.origin[1], self.origin[2])
    }

    fn direction_vec(&self) -> Vec3 {
        Vec3::new(self.direction[0], self.direction[1], self.direction[2])
    }
}

/// `#[repr(C)]` hit result written back to ring 3.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HitDesc {
    pub card_id: u32,
    pub reserved: u32,
    pub distance: f32,
    pub point: [f32; 3],
    pub local: [f32; 3],
    pub u: f32,
    pub v: f32,
}

impl HitDesc {
    pub fn from_hit(hit: &Hit) -> HitDesc {
        HitDesc {
            card_id: hit.card_id,
            reserved: 0,
            distance: hit.distance,
            point: [hit.point.x, hit.point.y, hit.point.z],
            local: [hit.local.x, hit.local.y, hit.local.z],
            u: hit.u,
            v: hit.v,
        }
    }
}

// ---------------------------------------------------------------------------
// Kernel-global workspace
// ---------------------------------------------------------------------------

#[cfg(not(test))]
mod global {
    use super::World;
    use spin::Mutex;

    /// The single hypervisor-owned workspace.
    ///
    /// Critical sections are short, allocation-free and never perform I/O or
    /// drop to user mode, so this lock can never be held across a synchronous
    /// exception or an IRQ-handled structure.
    pub static WORLD: Mutex<World> = Mutex::new(World::new());
}

#[cfg(not(test))]
pub use global::WORLD;

/// Runs `f` against the global workspace.
#[cfg(not(test))]
pub fn with_world<R>(f: impl FnOnce(&mut World) -> R) -> R {
    let mut world = WORLD.lock();
    f(&mut world)
}

/// Runs `f` against a copy of the global workspace.
///
/// Used by the serial wire frame: the copy is taken under the world lock and
/// released immediately, so formatting can write the UART without holding the
/// hypervisor spinlock (the lock is never held across I/O or an exception).
#[cfg(not(test))]
pub fn with_world_copy<R>(f: impl FnOnce(&World) -> R) -> R {
    let copy = WORLD.lock().clone();
    f(&copy)
}

/// Live workspace formatted as the `SPATIALv1` JSON line the host bridge reads.
///
/// `Display` runs inside the caller's `serial_println!`; the world is copied
/// under its lock first (`with_world_copy`), so only the serial writer's own
/// lock is held while bytes go to COM1.
#[cfg(not(test))]
pub struct WireSnapshot;

#[cfg(not(test))]
impl core::fmt::Display for WireSnapshot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        with_world_copy(|world| world.write_json(f))
    }
}

/// Resets the workspace and seeds the demonstration cards.
#[cfg(not(test))]
pub fn init_spatial_workspace() {
    with_world(|world| {
        *world = World::new();
        world.seed_demo();
    });
}

/// Focused card id, if any.
#[cfg(not(test))]
pub fn focused_card_id() -> Option<u32> {
    with_world(|world| world.focused_id())
}

/// Snapshot of the live cards.
#[cfg(not(test))]
pub fn active_cards() -> CardList {
    with_world(|world| world.cards())
}

/// Cards in painter's order (farthest first).
#[cfg(not(test))]
pub fn paint_order() -> CardList {
    with_world(|world| world.paint_order())
}

/// Workspace revision.
#[cfg(not(test))]
pub fn world_version() -> u64 {
    with_world(|world| world.version())
}

/// Builds the pointer ray for a viewport coordinate.
#[cfg(not(test))]
pub fn ray_for_screen(x: i32, y: i32) -> Option<Ray> {
    with_world(|world| world.camera().ray_for_screen(x as f32, y as f32))
}

/// Hit-tests a pointer sample and returns the routed (nearest, focus-aware) id.
#[cfg(not(test))]
pub fn hit_test_spatial_cards(pointer: PointerEvent) -> Option<u32> {
    let ray = ray_for_screen(pointer.x, pointer.y)?;
    with_world(|world| world.hit_test(&ray).map(|hit| hit.card_id))
}

/// Full hit detail for a pointer sample.
#[cfg(not(test))]
pub fn hit_test_detailed(pointer: PointerEvent) -> Option<Hit> {
    let ray = ray_for_screen(pointer.x, pointer.y)?;
    with_world(|world| world.hit_test(&ray))
}

/// Applies a pointer sample (focus, grab, real 3D drag).
#[cfg(not(test))]
pub fn handle_spatial_input(pointer: PointerEvent) {
    with_world(|world| {
        world.apply_pointer(pointer);
    });
}

// ---------------------------------------------------------------------------
// Host tests: `scripts/test_spatial_wm_host.sh` (no QEMU required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1.0e-4;

    fn close(actual: f32, expected: f32, eps: f32, what: &str) {
        assert!(
            (actual - expected).abs() <= eps,
            "{what}: expected {expected}, got {actual} (eps {eps})"
        );
    }

    fn world_with(cards: &[(u32, Pose, f32, f32)]) -> World {
        let mut world = World::new();
        world.focus_len = 0;
        for (index, (id, pose, width, height)) in cards.iter().enumerate() {
            world.cards[index] = Some(SpatialCard::new(*id, *pose, *width, *height));
            world.focus_stack_push(*id);
        }
        world.refresh_focus_flags();
        world.next_id = cards.iter().map(|c| c.0).max().unwrap_or(0) + 1;
        world
    }

    fn centered(z: f32) -> Pose {
        Pose::uniform(Vec3::new(0.0, 0.0, z), Quat::IDENTITY, 1.0)
    }

    #[test]
    fn sqrt_matches_reference_values() {
        close(math::sqrt(0.0), 0.0, 0.0, "sqrt(0)");
        close(math::sqrt(1.0), 1.0, EPS, "sqrt(1)");
        close(math::sqrt(2.0), 1.414_213_6, EPS, "sqrt(2)");
        close(math::sqrt(9.0), 3.0, EPS, "sqrt(9)");
        close(math::sqrt(1.0e6), 1000.0, 1.0e-2, "sqrt(1e6)");
        close(math::sqrt(-4.0), 0.0, 0.0, "sqrt(-4) degrades to 0");
        assert!(math::sqrt(f32::INFINITY).is_infinite());
    }

    #[test]
    fn sin_cos_matches_quadrants() {
        let cases = [
            (0.0_f32, 0.0_f32, 1.0_f32),
            (PI / 2.0, 1.0, 0.0),
            (PI, 0.0, -1.0),
            (-PI / 2.0, -1.0, 0.0),
            (PI / 6.0, 0.5, 0.866_025_4),
            (-3.0 * PI / 4.0, -0.707_106_8, -0.707_106_8),
        ];
        for (angle, sin, cos) in cases {
            let (actual_sin, actual_cos) = math::sin_cos(angle);
            close(actual_sin, sin, 2.0e-5, "sin");
            close(actual_cos, cos, 2.0e-5, "cos");
        }
    }

    #[test]
    fn tan_is_none_at_singularities() {
        assert!(math::tan(PI / 2.0).is_none());
        close(math::tan(0.0).expect("tan(0)"), 0.0, EPS, "tan(0)");
        close(math::tan(PI / 4.0).expect("tan(pi/4)"), 1.0, 1.0e-3, "tan(pi/4)");
    }

    #[test]
    fn quaternion_rotates_x_into_z_for_quarter_turn() {
        let quat = Quat::from_axis_angle(Vec3::UNIT_Y, PI / 2.0);
        let rotated = quat.rotate_vec3(Vec3::UNIT_X);
        close(rotated.x, 0.0, 1.0e-5, "x");
        close(rotated.z, -1.0, 1.0e-5, "z");
        // Inverse rotation returns the original axis.
        let back = quat.inverse_rotate_vec3(rotated);
        close(back.x, 1.0, 1.0e-5, "back x");
        close(back.z, 0.0, 1.0e-5, "back z");
    }

    #[test]
    fn quaternion_matrix_and_rotation_agree() {
        let quat = Quat::from_euler(0.4, -0.25, 0.1);
        let v = Vec3::new(0.3, -1.2, 2.0);
        let by_quat = quat.rotate_vec3(v);
        let by_matrix = quat.to_matrix().mul_vec3(v);
        close(by_quat.x, by_matrix.x, 1.0e-5, "matrix x");
        close(by_quat.y, by_matrix.y, 1.0e-5, "matrix y");
        close(by_quat.z, by_matrix.z, 1.0e-5, "matrix z");
        // The transpose inverts the rotation.
        let back = quat.to_matrix().transpose().mul_vec3(by_quat);
        close(back.x, v.x, 1.0e-5, "transpose x");
        close(back.y, v.y, 1.0e-5, "transpose y");
        close(back.z, v.z, 1.0e-5, "transpose z");
    }

    #[test]
    fn pose_world_local_round_trip() {
        let pose = Pose::new(
            Vec3::new(12.0, -4.0, 7.0),
            Quat::from_euler(0.6, 0.2, -0.3),
            Vec3::new(2.0, 0.5, 1.5),
        );
        let local = Vec3::new(-40.0, 25.0, 0.0);
        let world = pose.to_world(local);
        let back = pose.to_local(world).expect("valid pose");
        close(back.x, local.x, 1.0e-3, "local x");
        close(back.y, local.y, 1.0e-3, "local y");
        close(back.z, local.z, 1.0e-3, "local z");
    }

    #[test]
    fn degenerate_pose_is_rejected() {
        let mut pose = Pose::uniform(Vec3::ZERO, Quat::IDENTITY, 1.0);
        assert!(pose.is_valid());
        pose.scale = Vec3::new(1.0, 0.0, 1.0);
        assert!(!pose.is_valid());
        assert!(pose.to_local(Vec3::ZERO).is_none());

        let nan = Pose {
            position: Vec3::new(f32::NAN, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::new(1.0, 1.0, 1.0),
        };
        assert!(!nan.is_valid());
    }

    #[test]
    fn camera_center_ray_hits_the_plane_center() {
        let camera = Camera::workspace();
        let ray = camera.ray_for_screen(512.0, 384.0).expect("center ray");
        let t = ray_plane(&ray, Vec3::ZERO, Vec3::UNIT_Z).expect("plane hit");
        let point = ray.at(t);
        close(point.x, 512.0, 1.0e-2, "center x");
        close(point.y, 384.0, 1.0e-2, "center y");
        close(point.z, 0.0, 1.0e-2, "center z");
    }

    #[test]
    fn ray_hits_card_center_and_reports_uv() {
        let card = SpatialCard::new(7, centered(0.0), 320.0, 200.0);
        let ray = Ray::new(Vec3::new(0.0, 0.0, -500.0), Vec3::UNIT_Z).expect("ray");
        let hit = intersect_card(&ray, &card).expect("center hit");
        assert_eq!(hit.card_id, 7);
        close(hit.distance, 500.0, EPS, "distance");
        close(hit.point.z, 0.0, EPS, "point z");
        close(hit.u, 0.5, EPS, "u");
        close(hit.v, 0.5, EPS, "v");
    }

    #[test]
    fn ray_misses_beyond_card_bounds_and_behind_the_origin() {
        let card = SpatialCard::new(1, centered(0.0), 100.0, 100.0);
        let outside = Ray::new(Vec3::new(51.0, 0.0, -100.0), Vec3::UNIT_Z).expect("ray");
        assert!(intersect_card(&outside, &card).is_none());
        let edge = Ray::new(Vec3::new(50.0, 49.0, -100.0), Vec3::UNIT_Z).expect("ray");
        assert!(intersect_card(&edge, &card).is_some(), "edge is inside");
        let behind = Ray::new(Vec3::new(0.0, 0.0, 50.0), Vec3::UNIT_Z).expect("ray");
        assert!(intersect_card(&behind, &card).is_none(), "behind is rejected");
    }

    #[test]
    fn parallel_and_degenerate_rays_never_hit() {
        let card = SpatialCard::new(1, centered(0.0), 100.0, 100.0);
        let parallel = Ray::new(Vec3::new(0.0, 0.0, 0.0), Vec3::UNIT_X).expect("ray");
        assert!(intersect_card(&parallel, &card).is_none());
        assert!(Ray::new(Vec3::ZERO, Vec3::ZERO).is_none(), "zero direction");
        assert!(
            Ray::new(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::UNIT_Z).is_none(),
            "non-finite origin"
        );
    }

    #[test]
    fn back_face_is_hittable() {
        let card = SpatialCard::new(1, centered(0.0), 100.0, 100.0);
        let from_front = Ray::new(Vec3::new(0.0, 0.0, 100.0), -Vec3::UNIT_Z).expect("ray");
        let hit = intersect_card(&from_front, &card).expect("back-face hit");
        close(hit.distance, 100.0, EPS, "distance");
    }

    #[test]
    fn rotation_changes_what_the_ray_can_hit() {
        // A card yawed 90° presents its thin edge to a head-on ray.
        let pose = Pose::uniform(Vec3::ZERO, Quat::from_axis_angle(Vec3::UNIT_Y, PI / 2.0), 1.0);
        let card = SpatialCard::new(1, pose, 200.0, 100.0);
        let head_on = Ray::new(Vec3::new(0.0, 0.0, -300.0), Vec3::UNIT_Z).expect("ray");
        assert!(
            intersect_card(&head_on, &card).is_none(),
            "edge-on card cannot be hit head-on"
        );

        // Approaching along -X now hits it squarely.
        let side = Ray::new(Vec3::new(-300.0, 0.0, 0.0), Vec3::UNIT_X).expect("ray");
        let hit = intersect_card(&side, &card).expect("side hit");
        close(hit.distance, 300.0, 1.0e-3, "distance");
        close(hit.u, 0.5, 1.0e-3, "u");
    }

    #[test]
    fn scale_changes_the_world_footprint() {
        let small = SpatialCard::new(
            1,
            Pose::uniform(Vec3::ZERO, Quat::IDENTITY, 0.5),
            100.0,
            100.0,
        );
        let ray = Ray::new(Vec3::new(40.0, 0.0, -100.0), Vec3::UNIT_Z).expect("ray");
        assert!(
            intersect_card(&ray, &small).is_none(),
            "40 > 25 after 0.5 scale"
        );

        let large = SpatialCard::new(
            1,
            Pose::uniform(Vec3::ZERO, Quat::IDENTITY, 2.0),
            100.0,
            100.0,
        );
        let hit = intersect_card(&ray, &large).expect("scaled card hit");
        close(hit.local.x, 20.0, 1.0e-3, "local x is unscaled space");
    }

    #[test]
    fn nearest_card_wins_overlapping_hits() {
        let near = (1, centered(0.0), 200.0, 200.0);
        let far = (2, centered(200.0), 200.0, 200.0);
        let world = world_with(&[far, near]);
        let ray = Ray::new(Vec3::new(0.0, 0.0, -500.0), Vec3::UNIT_Z).expect("ray");
        let hit = world.hit_test(&ray).expect("hit");
        assert_eq!(hit.card_id, 1, "nearest card must win");
        close(hit.distance, 500.0, EPS, "distance");
    }

    #[test]
    fn coplanar_overlap_breaks_ties_by_focus_stack() {
        let a = (1, centered(0.0), 200.0, 200.0);
        let b = (2, centered(0.0), 200.0, 200.0);
        let mut world = world_with(&[a, b]);
        let ray = Ray::new(Vec3::new(0.0, 0.0, -500.0), Vec3::UNIT_Z).expect("ray");
        assert_eq!(world.hit_test(&ray).expect("hit").card_id, 2);
        assert!(world.focus_card(1));
        assert_eq!(
            world.hit_test(&ray).expect("hit").card_id,
            1,
            "focus raises the topmost card"
        );
    }

    #[test]
    fn focus_stack_is_ordered_and_bounded() {
        let mut world = world_with(&[
            (1, centered(0.0), 10.0, 10.0),
            (2, centered(0.0), 10.0, 10.0),
            (3, centered(0.0), 10.0, 10.0),
        ]);
        assert_eq!(world.focus_order(), &[1, 2, 3]);
        assert_eq!(world.focused_id(), Some(3));
        assert!(world.focus_card(1));
        assert_eq!(world.focus_order(), &[2, 3, 1]);
        assert_eq!(world.focused_id(), Some(1));
        assert!(world.card(1).expect("card 1").focused);
        assert!(!world.card(3).expect("card 3").focused);
        assert!(!world.focus_card(99), "unknown card cannot be focused");
    }

    #[test]
    fn input_is_routed_to_the_focused_card() {
        let mut world = world_with(&[(1, centered(0.0), 100.0, 100.0)]);
        world.focus_card(1);
        let ray = Ray::new(Vec3::new(0.0, 0.0, -100.0), Vec3::UNIT_Z).expect("ray");
        let hit = world.hit_test(&ray).expect("hit");
        assert_eq!(world.route_input(&hit), Some(1));
    }

    #[test]
    fn drag_uses_the_real_ray_plane_delta() {
        // 1024×768 camera at z = -700: one screen pixel is ≈1.0524 world units
        // on the z = 0 plane (60° vertical FOV).
        let mut world = World::new();
        world.seed_demo();
        let before = world.card(1).expect("card 1").pose.position;

        world.apply_pointer(PointerEvent { x: 500, y: 400, pressed: true, dragged: false });
        world.apply_pointer(PointerEvent { x: 560, y: 430, pressed: false, dragged: true });

        let after = world.card(1).expect("card 1").pose.position;
        let dx = after.x - before.x;
        let dy = after.y - before.y;
        close(dx, 60.0 * 1.0524, 1.0, "drag dx");
        close(dy, -30.0 * 1.0524, 1.0, "drag dy happens upward");
        close(after.z, before.z, EPS, "drag stays on the card plane");
        assert!(dx > 50.0, "delta is measured from the pointer, not a constant");
    }

    #[test]
    fn drag_does_not_accumulate_between_samples() {
        let mut world = World::new();
        world.seed_demo();
        world.apply_pointer(PointerEvent { x: 500, y: 400, pressed: true, dragged: false });
        world.apply_pointer(PointerEvent { x: 560, y: 430, pressed: false, dragged: true });
        let once = world.card(1).expect("card 1").pose.position;
        // Re-sending the same drag sample is idempotent (absolute, not additive).
        world.apply_pointer(PointerEvent { x: 560, y: 430, pressed: false, dragged: true });
        let twice = world.card(1).expect("card 1").pose.position;
        close(twice.x, once.x, EPS, "x is stable");
        close(twice.y, once.y, EPS, "y is stable");
    }

    #[test]
    fn press_on_empty_space_keeps_focus_and_clears_the_grab() {
        let mut world = World::new();
        world.seed_demo();
        world.focus_card(1);
        world.apply_pointer(PointerEvent { x: 60, y: 700, pressed: true, dragged: false });
        assert_eq!(world.focused_id(), Some(1), "empty click does not unfocus");
        world.apply_pointer(PointerEvent { x: 600, y: 400, pressed: false, dragged: true });
        let card = world.card(1).expect("card 1");
        close(card.pose.position.x, 500.0, EPS, "no drag without a grab");
    }

    #[test]
    fn demo_workspace_resolves_pointer_rays() {
        let mut world = World::new();
        world.seed_demo();
        // Card 1 covers the centre of the viewport.
        assert_eq!(world.hit_test(&world.camera().ray_for_screen(500.0, 400.0).expect("ray")).map(|h| h.card_id), Some(1));
        // Card 2 sits to the right and further away.
        assert_eq!(world.hit_test(&world.camera().ray_for_screen(880.0, 350.0).expect("ray")).map(|h| h.card_id), Some(2));
        // Bottom-left is empty.
        assert!(world.hit_test(&world.camera().ray_for_screen(60.0, 700.0).expect("ray")).is_none());
    }

    #[test]
    fn syscall_creates_updates_focuses_and_destroys() {
        let mut world = World::new();
        let pose = PoseDesc {
            position: [10.0, 20.0, 30.0],
            rotation: [0.0, 0.0, 0.0, 2.0], // unnormalised, must be accepted
            scale: [1.0, 1.0, 1.0],
        };
        let id = world.sys_create_card(CardDesc { width: 100.0, height: 50.0, pose });
        assert!(id > 0, "create returns a card id, got {id}");
        let id = id as u32;
        close(
            world.card(id).expect("created card").pose.rotation.length(),
            1.0,
            1.0e-5,
            "quaternion normalised",
        );

        let moved = PoseDesc { position: [5.0, 5.0, 5.0], ..pose };
        assert_eq!(world.sys_set_pose(id as u64, moved), 0);
        close(world.card(id).expect("card").pose.position.x, 5.0, EPS, "set_pose");

        assert_eq!(world.sys_focus_card(id as u64), 0);
        assert_eq!(world.focused_id(), Some(id));
        assert_eq!(world.sys_destroy_card(id as u64), 0);
        assert!(world.card(id).is_none());
        assert_eq!(world.sys_destroy_card(id as u64), ENOENT);
    }

    #[test]
    fn syscall_rejects_invalid_input() {
        let mut world = World::new();
        let bad_pose = PoseDesc {
            position: [f32::NAN, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        };
        assert_eq!(
            world.sys_create_card(CardDesc { width: 10.0, height: 10.0, pose: bad_pose }),
            EINVAL
        );
        assert_eq!(
            world.sys_create_card(CardDesc {
                width: 0.0,
                height: 10.0,
                pose: PoseDesc { scale: [1.0, 1.0, 1.0], rotation: [0.0, 0.0, 0.0, 1.0], position: [0.0; 3] },
            }),
            EINVAL,
            "zero width is rejected as EINVAL"
        );
        assert_eq!(world.sys_focus_card(42), ENOENT);
        assert_eq!(world.sys_set_pose(42, bad_pose), EINVAL);
        assert_eq!(world.sys_destroy_card(u64::MAX), EINVAL);

        let degenerate_ray = RayDesc { origin: [0.0; 3], direction: [0.0; 3] };
        assert_eq!(world.sys_hit_test(degenerate_ray).unwrap_err(), EINVAL);
        let no_hit = RayDesc { origin: [0.0, 0.0, -10.0], direction: [0.0, 0.0, 1.0] };
        assert_eq!(world.sys_hit_test(no_hit).unwrap_err(), ENOENT);
    }

    #[test]
    fn syscall_hit_test_returns_full_hit() {
        let mut world = World::new();
        let pose = PoseDesc {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 2.0, 2.0],
        };
        let id = world.sys_create_card(CardDesc { width: 100.0, height: 100.0, pose }) as u32;
        let ray = RayDesc { origin: [0.0, 0.0, -80.0], direction: [0.0, 0.0, 1.0] };
        let hit = world.sys_hit_test(ray).expect("hit");
        assert_eq!(hit.card_id, id);
        close(hit.distance, 80.0, EPS, "distance");
        let desc = HitDesc::from_hit(&hit);
        assert_eq!(desc.card_id, id);
        close(desc.point[2], 0.0, EPS, "point z");
        close(desc.u, 0.5, EPS, "u");
    }

    #[test]
    fn capacity_is_enforced() {
        let mut world = World::new();
        let pose = PoseDesc {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        };
        for _ in 0..MAX_CARDS {
            assert!(world.sys_create_card(CardDesc { width: 10.0, height: 10.0, pose }) > 0);
        }
        assert_eq!(
            world.sys_create_card(CardDesc { width: 10.0, height: 10.0, pose }),
            ENOSPC
        );
        assert_eq!(world.cards().len(), MAX_CARDS);
    }

    #[test]
    fn paint_order_is_farthest_first() {
        let mut world = world_with(&[
            (1, centered(-50.0), 10.0, 10.0),
            (2, centered(300.0), 10.0, 10.0),
            (3, centered(100.0), 10.0, 10.0),
        ]);
        world.camera = Camera::workspace(); // eye at z = -700
        let order: Vec<u32> = world.paint_order().as_slice().iter().map(|c| c.id).collect();
        assert_eq!(order, vec![2, 3, 1], "z-depth order, farthest first");
    }

    #[test]
    fn card_list_iterates_live_cards() {
        let mut world = World::new();
        world.seed_demo();
        let list = world.cards();
        assert_eq!(list.len(), 2);
        let ids: Vec<u32> = list.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(list.into_iter().count(), 2);
    }

    #[test]
    fn corners_form_the_card_rectangle() {
        let card = SpatialCard::new(
            1,
            Pose::uniform(Vec3::new(10.0, 20.0, 30.0), Quat::IDENTITY, 1.0),
            200.0,
            100.0,
        );
        let corners = card.corners();
        close(corners[0].x, -90.0, EPS, "corner 0 x");
        close(corners[2].y, 70.0, EPS, "corner 2 y");
        close(corners[3].z, 30.0, EPS, "corner 3 z");
    }

    struct FixedBuf {
        data: [u8; 1024],
        len: usize,
    }

    impl FixedBuf {
        fn new() -> FixedBuf {
            FixedBuf {
                data: [0; 1024],
                len: 0,
            }
        }

        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.data[..self.len]).expect("wire frame is utf8")
        }
    }

    impl core::fmt::Write for FixedBuf {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            if self.len + bytes.len() > self.data.len() {
                return Err(core::fmt::Error);
            }
            self.data[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
            Ok(())
        }
    }

    #[test]
    fn wire_frame_is_json_with_cards_and_revision() {
        let mut world = World::new();
        world.seed_demo();
        world.focus_card(2);
        let mut buf = FixedBuf::new();
        world.write_json(&mut buf).expect("frame fits");
        let json = buf.as_str();
        assert!(json.starts_with("{\"v\":1,\"rev\":"), "{json}");
        assert_eq!(json.matches("\"id\":").count(), 2, "{json}");
        assert!(json.contains("\"focus\":2"), "{json}");
        assert!(json.contains("\"px\":500.000"), "{json}");
        assert!(json.contains("\"w\":320.0"), "{json}");
        assert!(json.ends_with("]}"), "{json}");
        assert_eq!(json.matches('"').count() % 2, 0, "balanced quotes: {json}");
    }

    #[test]
    fn wire_frame_reports_an_empty_world() {
        let world = World::new();
        let mut buf = FixedBuf::new();
        world.write_json(&mut buf).expect("frame fits");
        let json = buf.as_str();
        assert!(json.contains("\"focus\":null"), "{json}");
        assert!(json.ends_with("\"cards\":[]}"), "{json}");
    }

    #[test]
    fn version_tracks_mutations() {
        let mut world = World::new();
        let start = world.version();
        world.seed_demo();
        assert!(world.version() > start);
        let after_seed = world.version();
        world.focus_card(1);
        assert!(world.version() > after_seed);
    }
}
