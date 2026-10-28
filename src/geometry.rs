//! Vector/matrix math, LDraw transforms, quad triangulation and the
//! LDraw → print coordinate conversion.
//!
//! # Coordinate systems
//!
//! LDraw uses a right-handed system measured in LDraw units (LDU) where
//! **-Y is up**: +X right, +Y down, +Z towards the viewer.
//!
//! STL files, Bambu Studio and the browser preview all use a right-handed
//! **Z-up** system measured in millimetres. The mapping is
//!
//! ```text
//! print.x =  ldraw.x * mm_per_ldu
//! print.y =  ldraw.z * mm_per_ldu
//! print.z = -ldraw.y * mm_per_ldu
//! ```
//!
//! This is a proper rotation (determinant +1, a -90° turn about X), so
//! triangle winding and therefore outward normals are preserved.

use std::ops::{Add, Mul, Neg, Sub};

/// One LDraw unit in millimetres at 1:1 scale (a stud pitch is 20 LDU = 8 mm).
pub const MM_PER_LDU: f64 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }
    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn normalized(self) -> Vec3 {
        let l = self.length();
        if l > 0.0 {
            self * (1.0 / l)
        } else {
            Vec3::ZERO
        }
    }
    pub fn min(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }
    pub fn max(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
    pub fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f64> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}
impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// Affine transform `p' = m * p + t`, exactly as written on an LDraw type 1
/// line: `1 <colour> x y z a b c d e f g h i <file>` gives
/// `m = [[a,b,c],[d,e,f],[g,h,i]]` and `t = (x,y,z)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub m: [[f64; 3]; 3],
    pub t: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: Vec3::ZERO,
    };

    /// Build from the 12 numbers of an LDraw type 1 line (x y z a b c d e f g h i).
    pub fn from_ldraw(v: &[f64; 12]) -> Self {
        Transform {
            t: Vec3::new(v[0], v[1], v[2]),
            m: [[v[3], v[4], v[5]], [v[6], v[7], v[8]], [v[9], v[10], v[11]]],
        }
    }

    pub fn scale(s: f64) -> Self {
        Transform {
            m: [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]],
            t: Vec3::ZERO,
        }
    }

    pub fn translation(t: Vec3) -> Self {
        Transform {
            t,
            ..Transform::IDENTITY
        }
    }

    pub fn apply_linear(&self, p: Vec3) -> Vec3 {
        let m = &self.m;
        Vec3::new(
            m[0][0] * p.x + m[0][1] * p.y + m[0][2] * p.z,
            m[1][0] * p.x + m[1][1] * p.y + m[1][2] * p.z,
            m[2][0] * p.x + m[2][1] * p.y + m[2][2] * p.z,
        )
    }

    pub fn apply(&self, p: Vec3) -> Vec3 {
        self.apply_linear(p) + self.t
    }

    /// `self ∘ child`: first apply `child`, then `self`.
    pub fn compose(&self, child: &Transform) -> Transform {
        let mut m = [[0.0; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| self.m[i][k] * child.m[k][j]).sum();
            }
        }
        Transform {
            m,
            t: self.apply(child.t),
        }
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.m;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// Inverse transform, or None if the linear part is singular.
    pub fn inverse(&self) -> Option<Transform> {
        let d = self.determinant();
        if d.abs() < 1e-12 {
            return None;
        }
        let m = &self.m;
        let c = |r0: usize, c0: usize, r1: usize, c1: usize| {
            m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
        };
        let inv = [
            [c(1, 1, 2, 2) / d, -c(0, 1, 2, 2) / d, c(0, 1, 1, 2) / d],
            [-c(1, 0, 2, 2) / d, c(0, 0, 2, 2) / d, -c(0, 0, 1, 2) / d],
            [c(1, 0, 2, 1) / d, -c(0, 0, 2, 1) / d, c(0, 0, 1, 1) / d],
        ];
        let lin = Transform {
            m: inv,
            t: Vec3::ZERO,
        };
        Some(Transform {
            m: inv,
            t: -lin.apply(self.t),
        })
    }

    /// The transform mirrors geometry (reverses winding).
    pub fn is_mirroring(&self) -> bool {
        self.determinant() < 0.0
    }

    /// Column `j` of the linear part (image of the j-th basis vector).
    pub fn column(&self, j: usize) -> Vec3 {
        Vec3::new(self.m[0][j], self.m[1][j], self.m[2][j])
    }
}

/// Unnormalised normal of triangle (a,b,c) following the right-hand rule.
pub fn triangle_normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    (b - a).cross(c - a)
}

pub fn triangle_area(a: Vec3, b: Vec3, c: Vec3) -> f64 {
    triangle_normal(a, b, c).length() * 0.5
}

/// Split an LDraw quad (vertices in perimeter order) into two triangles that
/// keep the quad's winding. LDraw requires quads to be planar and convex, in
/// which case the 0-2 diagonal is correct. Some older parts contain slightly
/// concave quads; if the 0-2 split produces triangles facing opposite ways we
/// fall back to the 1-3 diagonal.
pub fn triangulate_quad(q: [Vec3; 4]) -> [[Vec3; 3]; 2] {
    let a = [[q[0], q[1], q[2]], [q[0], q[2], q[3]]];
    let n1 = triangle_normal(q[0], q[1], q[2]);
    let n2 = triangle_normal(q[0], q[2], q[3]);
    if n1.dot(n2) >= 0.0 {
        return a;
    }
    let b = [[q[1], q[2], q[3]], [q[1], q[3], q[0]]];
    let m1 = triangle_normal(q[1], q[2], q[3]);
    let m2 = triangle_normal(q[1], q[3], q[0]);
    if m1.dot(m2) >= 0.0 {
        b
    } else {
        a
    }
}

/// Convert a point from LDraw space (LDU, -Y up) to print space (mm, Z up).
pub fn ldraw_to_print(p: Vec3, mm_per_ldu: f64) -> Vec3 {
    Vec3::new(p.x, p.z, -p.y) * mm_per_ldu
}

/// Convert a direction from LDraw space to print space (no scaling).
pub fn ldraw_dir_to_print(d: Vec3) -> Vec3 {
    Vec3::new(d.x, d.z, -d.y)
}

/// Millimetres per LDU for a given model scale factor (5 → 2.0 mm/LDU).
pub fn mm_per_ldu_for_scale(scale: f64) -> f64 {
    MM_PER_LDU * scale
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn empty() -> Self {
        Aabb {
            min: Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            max: Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        }
    }
    pub fn add(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }
    pub fn size(&self) -> Vec3 {
        if self.is_empty() {
            Vec3::ZERO
        } else {
            self.max - self.min
        }
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn overlaps(&self, o: &Aabb, eps: f64) -> bool {
        !(self.is_empty() || o.is_empty())
            && self.min.x <= o.max.x + eps
            && o.min.x <= self.max.x + eps
            && self.min.y <= o.max.y + eps
            && o.min.y <= self.max.y + eps
            && self.min.z <= o.max.z + eps
            && o.min.z <= self.max.z + eps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-9
    }

    #[test]
    fn transform_apply_matches_ldraw_line_semantics() {
        // 1 16 10 20 30  1 0 0  0 1 0  0 0 1  x.dat  → pure translation
        let t = Transform::from_ldraw(&[10., 20., 30., 1., 0., 0., 0., 1., 0., 0., 0., 1.]);
        assert!(close(
            t.apply(Vec3::new(1., 2., 3.)),
            Vec3::new(11., 22., 33.)
        ));
        // rows are a b c / d e f / g h i
        let r = Transform::from_ldraw(&[0., 0., 0., 0., 0., 1., 0., 1., 0., -1., 0., 0.]);
        assert!(close(
            r.apply(Vec3::new(1., 0., 0.)),
            Vec3::new(0., 0., -1.)
        ));
        assert!(close(r.apply(Vec3::new(0., 0., 1.)), Vec3::new(1., 0., 0.)));
    }

    #[test]
    fn compose_applies_child_first() {
        let parent = Transform::from_ldraw(&[100., 0., 0., 2., 0., 0., 0., 2., 0., 0., 0., 2.]);
        let child = Transform::translation(Vec3::new(1., 0., 0.));
        let c = parent.compose(&child);
        // child: (0,0,0)->(1,0,0); parent: scale 2 + 100 → (102,0,0)
        assert!(close(c.apply(Vec3::ZERO), Vec3::new(102., 0., 0.)));
        assert!((c.determinant() - 8.0).abs() < 1e-12);
    }

    #[test]
    fn inverse_round_trips() {
        let t = Transform::from_ldraw(&[3., -4., 5., 0., 2., 0., -1., 0., 0., 0., 0., 6.]);
        let inv = t.inverse().unwrap();
        let p = Vec3::new(1.5, -2.0, 7.0);
        assert!(close(inv.apply(t.apply(p)), p));
        assert!(Transform::scale(0.0).inverse().is_none());
    }

    #[test]
    fn mirroring_detected_by_determinant() {
        let m = Transform::from_ldraw(&[0., 0., 0., -1., 0., 0., 0., 1., 0., 0., 0., 1.]);
        assert!(m.is_mirroring());
        assert!(!Transform::IDENTITY.is_mirroring());
        // Two mirrors cancel.
        assert!(!m.compose(&m).is_mirroring());
    }

    #[test]
    fn quad_triangulation_keeps_winding() {
        let q = [
            Vec3::new(0., 0., 0.),
            Vec3::new(1., 0., 0.),
            Vec3::new(1., 1., 0.),
            Vec3::new(0., 1., 0.),
        ];
        let tris = triangulate_quad(q);
        for t in tris {
            assert!(triangle_normal(t[0], t[1], t[2]).z > 0.0);
        }
        let total: f64 = tris.iter().map(|t| triangle_area(t[0], t[1], t[2])).sum();
        assert!((total - 1.0).abs() < 1e-12);
    }

    #[test]
    fn concave_quad_uses_other_diagonal() {
        // Concave at vertex 2: the 0-2 split would create a flipped triangle.
        let q = [
            Vec3::new(0., 0., 0.),
            Vec3::new(2., 0., 0.),
            Vec3::new(0.5, 0.5, 0.),
            Vec3::new(0., 2., 0.),
        ];
        for t in triangulate_quad(q) {
            assert!(triangle_normal(t[0], t[1], t[2]).z > 0.0);
        }
    }

    #[test]
    fn ldraw_to_print_is_z_up_and_scaled() {
        let mm = mm_per_ldu_for_scale(5.0);
        assert!((mm - 2.0).abs() < 1e-12);
        // LDraw "up" is -Y; it must become +Z.
        let up = ldraw_to_print(Vec3::new(0., -10., 0.), mm);
        assert!(close(up, Vec3::new(0., 0., 20.)));
        let p = ldraw_to_print(Vec3::new(1., 0., 3.), mm);
        assert!(close(p, Vec3::new(2., 6., 0.)));
    }

    #[test]
    fn ldraw_to_print_preserves_winding() {
        let (a, b, c) = (
            Vec3::new(0., 0., 0.),
            Vec3::new(1., 0., 0.),
            Vec3::new(0., 0., 1.),
        );
        let n_ld = triangle_normal(a, b, c);
        let n_pr = triangle_normal(
            ldraw_to_print(a, 1.),
            ldraw_to_print(b, 1.),
            ldraw_to_print(c, 1.),
        );
        assert!(close(ldraw_dir_to_print(n_ld), n_pr));
    }
}
