//! Smoothing of circular primitives by angular re-tessellation.
//!
//! LDraw draws every round feature as a 16-sided polygon (48 for `p/48/`
//! primitives). At 5:1 those facets are 10 mm wide on a hat brim. Every
//! circular primitive (cylinders, discs, rings, cones, tori, …) is a surface
//! of revolution about its local Y axis, sampled at evenly spaced angles.
//! This module verifies that property for each primitive instance from its
//! actual vertices and then re-samples it at `N` segments per circle:
//! each face spanning one angular step is replaced by `k` faces whose
//! vertices keep their exact radius and height, at finer angles. The
//! profile (e.g. a torus cross-section) is unchanged.
//!
//! Because all smoothed primitives use the same angle set, smoothed
//! neighbours still share vertices. Where a smoothed arc meets geometry
//! that was not smoothed (hand-modelled polygons, `ndis` corners), the old
//! chord and the new arc leave a thin planar sliver; [`fill_arc_cracks`]
//! closes it with a fan after welding. Faces that are not part of a verified
//! surface of revolution are never modified.

use crate::det::HashMap;
use crate::geometry::{Transform, Vec3};
use crate::ldraw::{Resolved, SourceEdge, SourceTriangle};
use crate::mesh::{Mesh, Tri};
use std::f64::consts::TAU;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SmoothReport {
    pub segments: u32,
    pub instances_smoothed: usize,
    pub instances_skipped: usize,
    /// Names of primitives left as authored (with counts).
    pub skipped: Vec<(String, usize)>,
    pub triangles_before: usize,
    pub triangles_after: usize,
    pub cracks_filled: usize,
    /// Hand-modelled triangles lying on a smoothed cylinder's facets that
    /// were re-curved onto it (e.g. printed face patterns).
    pub conformed_triangles: usize,
    /// Primitives lying flat on a smoothed cylinder, curved with it instead
    /// of being rounded in their own plane.
    pub on_surface_primitives: usize,
    /// Vertices of other geometry that touched a smoothed surface and were
    /// moved onto it, so the contact is kept.
    pub snapped_vertices: usize,
}

/// A refined arc between two original angular stations (world, LDU).
#[derive(Debug, Clone)]
pub struct Arc {
    pub a: Vec3,
    pub b: Vec3,
    /// Points strictly between `a` and `b`, ordered from `a`.
    pub interior: Vec<Vec3>,
}

const R_EPS: f64 = 1e-6;
/// How far (LDU) hand-modelled geometry may be from a facet and still be
/// treated as drawn on it.
const SURFACE_TOL_LDU: f64 = 0.015;
const ANG_EPS: f64 = 1e-4;

fn local_polar(inv: &Transform, w: Vec3) -> (f64, f64, f64) {
    let p = inv.apply(w);
    let r = (p.x * p.x + p.z * p.z).sqrt();
    let th = if r < R_EPS {
        0.0
    } else {
        p.z.atan2(p.x).rem_euclid(TAU)
    };
    (r, th, p.y)
}

/// Unwrap a face's angles to a contiguous range; returns (θa, θb) where all
/// non-axis vertices lie at one of the two, or None.
fn face_span(angles: &[Option<f64>], step: f64) -> Option<(f64, Option<f64>)> {
    let mut v: Vec<f64> = angles.iter().flatten().copied().collect();
    if v.is_empty() {
        return Some((0.0, None));
    }
    let lo = v.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if hi - lo > std::f64::consts::PI {
        for a in &mut v {
            if *a < std::f64::consts::PI {
                *a += TAU;
            }
        }
    }
    let lo = v.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if hi - lo < ANG_EPS {
        return Some((lo, None));
    }
    if ((hi - lo) - step).abs() > ANG_EPS * 10.0 {
        return None;
    }
    if v.iter()
        .any(|a| (a - lo).abs() > ANG_EPS * 10.0 && (a - hi).abs() > ANG_EPS * 10.0)
    {
        return None;
    }
    Some((lo, Some(hi)))
}

fn angle_near(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(TAU);
    d < ANG_EPS * 10.0 || TAU - d < ANG_EPS * 10.0
}

/// 1-D clustering with a small absolute + relative tolerance; returns a
/// function mapping a value to its cluster id.
fn cluster(values: impl Iterator<Item = f64>) -> impl Fn(f64) -> usize {
    let mut v: Vec<f64> = values.collect();
    v.sort_by(|a, b| a.total_cmp(b));
    let tol = |x: f64| 5e-4 + x.abs() * 2e-4;
    let mut starts: Vec<f64> = Vec::new();
    let mut last = f64::NEG_INFINITY;
    for x in v {
        if x - last > tol(x) {
            starts.push(x);
        }
        last = x;
    }
    move |x: f64| {
        starts
            .partition_point(|&s| s <= x + tol(x))
            .saturating_sub(1)
    }
}

/// Determine the angular step if the given vertex set is a sampled surface
/// of revolution: every (r, y) profile station appears at every angle.
fn revolution_step(points: &[(f64, f64, f64)]) -> Option<(f64, u32)> {
    let mut angles: Vec<f64> = points.iter().filter(|p| p.0 > R_EPS).map(|p| p.1).collect();
    if angles.len() < 2 {
        return None;
    }
    angles.sort_by(|a, b| a.total_cmp(b));
    angles.dedup_by(|a, b| (*a - *b).abs() < ANG_EPS);
    let mut step = f64::INFINITY;
    for w in angles.windows(2) {
        step = step.min(w[1] - w[0]);
    }
    let n = (TAU / step).round() as u32;
    if !matches!(n, 8 | 16 | 48) || (TAU / n as f64 - step).abs() > ANG_EPS * 10.0 {
        return None;
    }
    let step = TAU / n as f64;
    // Angles must sit on the n-gon stations.
    if angles.iter().any(|a| {
        let k = a / step;
        (k - k.round()).abs() > 1e-3
    }) {
        return None;
    }
    // Every profile station must appear at every angle. LDraw stores 4
    // decimals, so the same circle can read r = 4.0000 or 4.0001: cluster.
    let ring: Vec<&(f64, f64, f64)> = points.iter().filter(|p| p.0 > R_EPS).collect();
    let rc = cluster(ring.iter().map(|p| p.0));
    let yc = cluster(ring.iter().map(|p| p.2));
    let mut groups: HashMap<(usize, usize), Vec<f64>> = HashMap::default();
    for p in &ring {
        groups.entry((rc(p.0), yc(p.2))).or_default().push(p.1);
    }
    for th in groups.values_mut() {
        th.sort_by(|a, b| a.total_cmp(b));
        th.dedup_by(|a, b| (*a - *b).abs() < ANG_EPS);
        if th.len() != angles.len() {
            return None;
        }
    }
    Some((step, n))
}

/// A smoothed surface of revolution (cylinder, cone, torus band, disc,
/// ring), in its local frame. Its authored facets are flat trapezoids
/// between two profile stations and two authored angles; geometry drawn
/// flat on one of them (e.g. a printed face) is re-curved with it.
struct Surface {
    owner: usize,
    inv: Transform,
    /// Axis in world space (point, unit direction), for coaxiality tests.
    axis: (Vec3, Vec3),
    /// Profile segments ((r, y), (r, y)) that faces span.
    segments: Vec<((f64, f64), (f64, f64))>,
    scale: f64,
    /// Matching tolerance in local units (a fixed real-world distance).
    tol: f64,
    /// Authored angular step (e.g. 22.5° for 16 segments).
    step: f64,
    /// Smoothing factor: each authored step becomes `k` steps.
    k: u32,
}

impl Surface {
    fn tol(&self) -> f64 {
        self.tol
    }

    /// Distance from the facet-mid angle, wrapped to (-π, π].
    fn off_mid(&self, th: f64, k: i64) -> f64 {
        let mid = (k as f64 + 0.5) * self.step;
        (th - mid + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI
    }

    /// If the world triangle lies flat on one authored facet of this
    /// surface (any angle, not only where the primitive itself exists),
    /// return the facet index.
    fn facet_of(&self, v: &[Vec3; 3]) -> Option<i64> {
        let tol = self.tol();
        let n = (TAU / self.step).round() as i64;
        let polar: Vec<(f64, f64, f64)> = v.iter().map(|w| local_polar(&self.inv, *w)).collect();
        // Candidate facets per vertex (two when on a station line, all for
        // a vertex on the axis).
        let mut cand: Option<Vec<i64>> = None;
        for &(rho, th, _) in &polar {
            let ks: Vec<i64> = if rho < tol {
                (0..n).collect()
            } else {
                let f = th / self.step;
                if (f - f.round()).abs() * self.step * rho <= tol {
                    vec![(f.round() as i64 - 1).rem_euclid(n), f.round() as i64 % n]
                } else {
                    vec![f.floor() as i64 % n]
                }
            };
            cand = Some(match cand {
                None => ks,
                Some(c) => c.into_iter().filter(|k| ks.contains(k)).collect(),
            });
        }
        let cand = cand?;
        cand.into_iter().find(|&k| {
            // Chord-equivalent profile coordinates of each vertex.
            let pts: Vec<(f64, f64)> = polar
                .iter()
                .map(|&(rho, th, y)| {
                    let d = self.off_mid(th, k);
                    (rho * d.cos() / (self.step / 2.0).cos(), y)
                })
                .collect();
            if polar.iter().any(|&(rho, th, _)| {
                rho >= tol && self.off_mid(th, k).abs() > self.step / 2.0 + tol / rho
            }) {
                return false;
            }
            self.segments
                .iter()
                .any(|&(a, b)| pts.iter().all(|&p| on_segment(p, a, b, tol)))
        })
    }

    /// Cut a facet triangle at the smoothed station angles (planes through
    /// the axis) and move every vertex from the flat facet onto the true
    /// surface: same angle and height, radius scaled from chord to arc.
    fn conform(&self, v: &[Vec3; 3], k_facet: i64, xf: &Transform) -> Vec<[Vec3; 3]> {
        let local: Vec<Vec3> = v.iter().map(|w| self.inv.apply(*w)).collect();
        let mut polys = vec![local];
        let sub = self.step / self.k as f64;
        for j in 1..self.k {
            let th = k_facet as f64 * self.step + sub * j as f64;
            // Plane through the Y axis at angle th.
            let n = Vec3::new(-th.sin(), 0.0, th.cos());
            let mut next = Vec::new();
            for poly in polys {
                let (a, b) = split_polygon(&poly, n, self.tol());
                next.extend([a, b].into_iter().filter(|p| p.len() >= 3));
            }
            polys = next;
        }
        let half = (self.step / 2.0).cos();
        let onto = |p: Vec3| {
            let rho = (p.x * p.x + p.z * p.z).sqrt();
            if rho < 1e-12 {
                return xf.apply(p);
            }
            let th = p.z.atan2(p.x);
            let r = rho * self.off_mid(th.rem_euclid(TAU), k_facet).cos() / half;
            xf.apply(Vec3::new(r * th.cos(), p.y, r * th.sin()))
        };
        let mut out = Vec::new();
        for poly in polys {
            let w: Vec<Vec3> = poly.into_iter().map(onto).collect();
            for i in 1..w.len() - 1 {
                out.push([w[0], w[i], w[i + 1]]);
            }
        }
        out
    }
}

impl Surface {
    /// If the world point lies on one of this surface's authored facets,
    /// return its position on the true (smoothed) surface: same angle and
    /// height, radius scaled from chord to arc.
    fn snap(&self, w: Vec3, xf: &Transform) -> Option<Vec3> {
        let tol = self.tol();
        let (rho, th, y) = local_polar(&self.inv, w);
        if rho < tol {
            return None;
        }
        let n = (TAU / self.step).round() as i64;
        let f = th / self.step;
        let mut ks = vec![f.floor() as i64 % n];
        if (f - f.round()).abs() * self.step * rho <= tol {
            return None; // on a station line: already on the true surface
        }
        ks.retain(|&k| self.off_mid(th, k).abs() <= self.step / 2.0);
        let k = *ks.first()?;
        let d = self.off_mid(th, k);
        let r_eff = rho * d.cos() / (self.step / 2.0).cos();
        // Only curved profile segments (cylinder, cone, torus band) differ
        // between facet and true surface. On a flat disc or ring the point
        // is already on the true surface, and moving it would distort
        // whatever touches it (e.g. rib tips under a socket ceiling).
        let curved = |(a, b): &((f64, f64), (f64, f64))| (a.1 - b.1).abs() > tol;
        let on = self
            .segments
            .iter()
            .filter(|s| curved(s))
            .any(|&(a, b)| on_segment((r_eff, y), a, b, tol));
        on.then(|| xf.apply(Vec3::new(r_eff * th.cos(), y, r_eff * th.sin())))
    }
}

/// Point within `tol` of segment a–b in the (r, y) profile plane.
fn on_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64), tol: f64) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt() <= tol
}

/// Split a convex polygon by the plane `p · n = 0`, keeping vertex order.
/// Vertices within `eps` of the plane count as on it, so a vertex that
/// almost touches a cut doesn't produce a sliver.
fn split_polygon(poly: &[Vec3], n: Vec3, eps: f64) -> (Vec<Vec3>, Vec<Vec3>) {
    let (mut neg, mut pos) = (Vec::new(), Vec::new());
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (da, db) = (a.dot(n), b.dot(n));
        if da <= eps {
            neg.push(a);
        }
        if da >= -eps {
            pos.push(a);
        }
        if (da < -eps && db > eps) || (da > eps && db < -eps) {
            let p = a + (b - a) * (da / (da - db));
            neg.push(p);
            pos.push(p);
        }
    }
    let dedup = |v: Vec<Vec3>| {
        let mut o: Vec<Vec3> = Vec::new();
        for p in v {
            if o.last().is_none_or(|q| (*q - p).length() > 1e-9) {
                o.push(p);
            }
        }
        if o.len() > 1 && (o[0] - o[o.len() - 1]).length() <= 1e-9 {
            o.pop();
        }
        o
    };
    (dedup(neg), dedup(pos))
}

fn skip(r: &mut SmoothReport, name: &str) {
    r.instances_skipped += 1;
    match r.skipped.iter_mut().find(|(n, _)| n == name) {
        Some(e) => e.1 += 1,
        None => r.skipped.push((name.to_string(), 1)),
    }
}

/// Re-tessellate circular primitives in `resolved` to `segments` per circle.
/// Returns the report and the arcs needed for crack filling.
pub fn smooth_primitives(resolved: &mut Resolved, segments: u32) -> (SmoothReport, Vec<Arc>) {
    let mut report = SmoothReport {
        segments,
        triangles_before: resolved.triangles.len(),
        ..Default::default()
    };
    let ntri = resolved.triangles.len();
    let nedge = resolved.edges.len();
    // Innermost primitive owning each triangle / edge.
    let mut order: Vec<usize> = (0..resolved.primitives.len()).collect();
    order.sort_by_key(|&i| {
        let p = &resolved.primitives[i];
        std::cmp::Reverse((p.tri_end - p.tri_start) + (p.edge_end - p.edge_start))
    });
    let mut tri_owner = vec![usize::MAX; ntri];
    let mut edge_owner = vec![usize::MAX; nedge];
    for &i in &order {
        let p = &resolved.primitives[i];
        tri_owner[p.tri_start..p.tri_end]
            .iter_mut()
            .for_each(|o| *o = i);
        edge_owner[p.edge_start..p.edge_end]
            .iter_mut()
            .for_each(|o| *o = i);
    }
    let mut owned_tris: HashMap<usize, Vec<usize>> = HashMap::default();
    for (t, &o) in tri_owner.iter().enumerate() {
        if o != usize::MAX {
            owned_tris.entry(o).or_default().push(t);
        }
    }
    let mut owned_edges: HashMap<usize, Vec<usize>> = HashMap::default();
    for (e, &o) in edge_owner.iter().enumerate() {
        if o != usize::MAX {
            owned_edges.entry(o).or_default().push(e);
        }
    }

    // Per owning instance: (inverse, step, factor) if smoothable.
    let mut plan: HashMap<usize, (Transform, f64, u32)> = HashMap::default();
    let mut owners: Vec<usize> = owned_tris
        .keys()
        .chain(owned_edges.keys())
        .copied()
        .collect();
    owners.sort_unstable();
    owners.dedup();
    for &i in &owners {
        let p = &resolved.primitives[i];
        let Some(inv) = p.transform.inverse() else {
            skip(&mut report, &p.name);
            continue;
        };
        let mut pts = Vec::new();
        for &t in owned_tris.get(&i).map(Vec::as_slice).unwrap_or(&[]) {
            pts.extend(
                resolved.triangles[t]
                    .v
                    .iter()
                    .map(|w| local_polar(&inv, *w)),
            );
        }
        for &e in owned_edges.get(&i).map(Vec::as_slice).unwrap_or(&[]) {
            pts.extend(resolved.edges[e].v.iter().map(|w| local_polar(&inv, *w)));
        }
        let ok = revolution_step(&pts).and_then(|(step, n)| {
            (segments.is_multiple_of(n) && segments > n).then_some((step, segments / n))
        });
        // Every owned face must span exactly one step (or be radial).
        let ok = ok.filter(|(step, _)| {
            owned_tris
                .get(&i)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .all(|&t| {
                    let a: Vec<Option<f64>> = resolved.triangles[t]
                        .v
                        .iter()
                        .map(|w| {
                            let (r, th, _) = local_polar(&inv, *w);
                            (r > R_EPS).then_some(th)
                        })
                        .collect();
                    face_span(&a, *step).is_some()
                })
        });
        match ok {
            Some((step, k)) => {
                plan.insert(i, (inv, step, k));
                report.instances_smoothed += 1;
            }
            None => skip(&mut report, &p.name),
        }
    }

    // Smoothed surfaces of revolution. Hand-modelled faces (and primitives
    // such as an eye disc) drawn flat on their authored facets get re-curved
    // with them.
    // Iterate primitives in index order so results never depend on map order.
    let mut planned: Vec<(&usize, &(Transform, f64, u32))> = plan.iter().collect();
    planned.sort_by_key(|(i, _)| **i);
    let surfaces: Vec<Surface> = planned
        .into_iter()
        .filter_map(|(&i, (inv, step, k))| {
            let tris = owned_tris.get(&i)?;
            let xf = &resolved.primitives[i].transform;
            let mut segments: Vec<((f64, f64), (f64, f64))> = Vec::new();
            let mut scale: f64 = 0.0;
            for &t in tris {
                let mut st: Vec<(f64, f64)> = Vec::new();
                for w in &resolved.triangles[t].v {
                    let (r, _, y) = local_polar(inv, *w);
                    scale = scale.max(r).max(y.abs());
                    if !st
                        .iter()
                        .any(|q| (q.0 - r).abs() < 1e-3 && (q.1 - y).abs() < 1e-3)
                    {
                        st.push((r, y));
                    }
                }
                for a in 0..st.len() {
                    for b in a + 1..st.len() {
                        let seg = (st[a], st[b]);
                        let same = |x: &((f64, f64), (f64, f64))| {
                            let c = |p: (f64, f64), q: (f64, f64)| {
                                (p.0 - q.0).abs() < 1e-3 && (p.1 - q.1).abs() < 1e-3
                            };
                            (c(x.0, seg.0) && c(x.1, seg.1)) || (c(x.0, seg.1) && c(x.1, seg.0))
                        };
                        if !segments.iter().any(same) {
                            segments.push(seg);
                        }
                    }
                }
            }
            (!segments.is_empty()).then(|| Surface {
                owner: i,
                inv: *inv,
                axis: (xf.t, xf.column(1).normalized()),
                segments,
                scale,
                // 0.015 LDU in world units: LDraw's rounded coordinates put
                // pattern vertices up to ~0.006 LDU off the facet.
                tol: SURFACE_TOL_LDU
                    / xf.column(0)
                        .length()
                        .max(xf.column(1).length())
                        .max(xf.column(2).length())
                        .max(1e-9),
                step: *step,
                k: *k,
            })
        })
        .collect();
    // Primitives drawn flat on another (non-coaxial) smoothed surface, like
    // a disc for an eye in a face print, must follow that surface rather
    // than be rounded in their own plane: un-plan them so they are
    // conformed like the faces around them.
    let coaxial = |a: &Surface, b: &Surface| {
        let (pa, da) = a.axis;
        let (pb, db) = b.axis;
        let rel = pb - pa;
        da.dot(db).abs() > 0.9999 && (rel - da * rel.dot(da)).length() < 1e-3 * a.scale.max(1.0)
    };
    let on_surface: Vec<usize> = surfaces
        .iter()
        .filter(|me| {
            let tris = owned_tris.get(&me.owner).map(Vec::as_slice).unwrap_or(&[]);
            !tris.is_empty()
                && tris.iter().all(|&t| {
                    surfaces.iter().any(|s| {
                        s.owner != me.owner
                            && !coaxial(s, me)
                            && s.facet_of(&resolved.triangles[t].v).is_some()
                    })
                })
        })
        .map(|s| s.owner)
        .collect();
    for i in &on_surface {
        plan.remove(i);
        report.instances_smoothed -= 1;
        report.on_surface_primitives += 1;
    }
    let surfaces: Vec<Surface> = surfaces
        .into_iter()
        .filter(|s| !on_surface.contains(&s.owner))
        .collect();
    let mut conform: HashMap<usize, (usize, i64)> = HashMap::default();
    if !surfaces.is_empty() {
        for (t, owner) in tri_owner.iter().enumerate() {
            if plan.contains_key(owner) {
                continue;
            }
            let hit = surfaces
                .iter()
                .enumerate()
                .find_map(|(c, s)| s.facet_of(&resolved.triangles[t].v).map(|k| (c, k)));
            if let Some(h) = hit {
                conform.insert(t, h);
            }
        }
    }

    // Rebuild triangle list.
    let mut arcs: HashMap<(i64, i64, i64, i64, i64, i64), Arc> = HashMap::default();
    let q = |v: Vec3| {
        (
            (v.x * 1e4).round() as i64,
            (v.y * 1e4).round() as i64,
            (v.z * 1e4).round() as i64,
        )
    };
    let mut new_tris: Vec<SourceTriangle> = Vec::with_capacity(ntri * 2);
    let mut tri_map = vec![0usize; ntri + 1];
    for t in 0..ntri {
        tri_map[t] = new_tris.len();
        let src = &resolved.triangles[t];
        if let Some(&(c, k_facet)) = conform.get(&t) {
            let surf = &surfaces[c];
            let xf = &resolved.primitives[surf.owner].transform;
            let pieces = surf.conform(&src.v, k_facet, xf);
            report.conformed_triangles += 1;
            new_tris.extend(
                pieces
                    .into_iter()
                    .map(|v| SourceTriangle { v, ..src.clone() }),
            );
            continue;
        }
        let Some((inv, step, k)) = plan.get(&tri_owner[t]) else {
            // Geometry merely touching a smoothed surface (e.g. a rib whose
            // edge runs along a cone wall) follows it, so contact is kept.
            let mut v = src.v;
            for p in &mut v {
                if let Some(q) = surfaces
                    .iter()
                    .find_map(|s| s.snap(*p, &resolved.primitives[s.owner].transform))
                {
                    *p = q;
                    report.snapped_vertices += 1;
                }
            }
            new_tris.push(SourceTriangle { v, ..src.clone() });
            continue;
        };
        let xf = &resolved.primitives[tri_owner[t]].transform;
        let polar: Vec<(f64, f64, f64)> = src.v.iter().map(|w| local_polar(inv, *w)).collect();
        let angles: Vec<Option<f64>> = polar.iter().map(|p| (p.0 > R_EPS).then_some(p.1)).collect();
        let (lo, hi) = face_span(&angles, *step).unwrap();
        if hi.is_none() {
            new_tris.push(src.clone());
            continue;
        }
        let sub = step / *k as f64;
        let at =
            |p: &(f64, f64, f64), th: f64| xf.apply(Vec3::new(p.0 * th.cos(), p.2, p.0 * th.sin()));
        for j in 0..*k {
            let (p0, p1) = (lo + sub * j as f64, lo + sub * (j + 1) as f64);
            let v = std::array::from_fn(|n| {
                let p = &polar[n];
                if p.0 <= R_EPS {
                    src.v[n]
                } else if angle_near(p.1, lo) {
                    at(p, p0)
                } else {
                    at(p, p1)
                }
            });
            new_tris.push(SourceTriangle { v, ..src.clone() });
        }
        // Record arcs on the face's circles for crack filling.
        for p in polar.iter().filter(|p| p.0 > R_EPS) {
            let a = at(p, lo);
            let b = at(p, lo + step);
            let (qa, qb) = (q(a), q(b));
            let (lo_q, hi_q) = if qa <= qb { (qa, qb) } else { (qb, qa) };
            let key = (lo_q.0, lo_q.1, lo_q.2, hi_q.0, hi_q.1, hi_q.2);
            arcs.entry(key).or_insert_with(|| Arc {
                a,
                b,
                interior: (1..*k).map(|j| at(p, lo + sub * j as f64)).collect(),
            });
        }
    }
    tri_map[ntri] = new_tris.len();

    let mut new_edges: Vec<SourceEdge> = Vec::with_capacity(nedge * 2);
    let mut edge_map = vec![0usize; nedge + 1];
    for e in 0..nedge {
        edge_map[e] = new_edges.len();
        let src = &resolved.edges[e];
        let Some((inv, step, k)) = plan.get(&edge_owner[e]) else {
            new_edges.push(src.clone());
            continue;
        };
        let xf = &resolved.primitives[edge_owner[e]].transform;
        let polar: Vec<(f64, f64, f64)> = src.v.iter().map(|w| local_polar(inv, *w)).collect();
        let angles: Vec<Option<f64>> = polar.iter().map(|p| (p.0 > R_EPS).then_some(p.1)).collect();
        match face_span(&angles, *step) {
            Some((lo, Some(_))) if polar.iter().all(|p| p.0 > R_EPS) => {
                let sub = step / *k as f64;
                let at = |p: &(f64, f64, f64), th: f64| {
                    xf.apply(Vec3::new(p.0 * th.cos(), p.2, p.0 * th.sin()))
                };
                for j in 0..*k {
                    let (p0, p1) = (lo + sub * j as f64, lo + sub * (j + 1) as f64);
                    let v = std::array::from_fn(|n| {
                        if angle_near(polar[n].1, lo) {
                            at(&polar[n], p0)
                        } else {
                            at(&polar[n], p1)
                        }
                    });
                    new_edges.push(SourceEdge { v, ..src.clone() });
                }
            }
            _ => new_edges.push(src.clone()),
        }
    }
    edge_map[nedge] = new_edges.len();

    for (i, p) in resolved.primitives.iter_mut().enumerate() {
        p.tri_start = tri_map[p.tri_start];
        p.tri_end = tri_map[p.tri_end];
        p.edge_start = edge_map[p.edge_start];
        p.edge_end = edge_map[p.edge_end];
        // Connector detection reads cylinder leaves, which own their faces.
        if plan.contains_key(&i) {
            p.segments = Some(segments);
        }
    }
    resolved.triangles = new_tris;
    resolved.edges = new_edges;
    report.triangles_after = resolved.triangles.len();
    (report, {
        let mut v: Vec<(_, Arc)> = arcs.into_iter().collect();
        v.sort_by_key(|a| a.0);
        v.into_iter().map(|(_, a)| a).collect()
    })
}

/// Close slivers between smoothed arcs and unsmoothed chords: for an open
/// chord edge a–b whose arc points are all open too, add a fan. Only adds
/// faces; never moves vertices.
pub fn fill_arc_cracks(mesh: &mut Mesh, arcs: &[Arc], tol: f64) -> usize {
    if arcs.is_empty() {
        return 0;
    }
    let cell = tol.max(1e-9) * 4.0;
    let key = |p: Vec3| {
        (
            (p.x / cell).floor() as i64,
            (p.y / cell).floor() as i64,
            (p.z / cell).floor() as i64,
        )
    };
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::default();
    for (i, p) in mesh.positions.iter().enumerate() {
        grid.entry(key(*p)).or_default().push(i as u32);
    }
    let find = |p: Vec3| -> Option<u32> {
        let (x, y, z) = key(p);
        let mut best: Option<(f64, u32)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &i in grid.get(&(x + dx, y + dy, z + dz)).into_iter().flatten() {
                        let d = (mesh.positions[i as usize] - p).length();
                        if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, i));
                        }
                    }
                }
            }
        }
        best.map(|b| b.1)
    };
    let edges = mesh.edge_map();
    let uses = |a: u32, b: u32| {
        edges
            .get(&(a.min(b), a.max(b)))
            .map(|u| u.len())
            .unwrap_or(0)
    };
    let mut added = Vec::new();
    let mut filled = 0;
    // The same chord can be recorded twice under keys that differ in the
    // last decimal; fill each welded chord once.
    let mut done = crate::det::HashSet::default();
    for arc in arcs {
        let (Some(a), Some(b)) = (find(arc.a), find(arc.b)) else {
            continue;
        };
        let Some(pts) = arc
            .interior
            .iter()
            .map(|p| find(*p))
            .collect::<Option<Vec<u32>>>()
        else {
            continue;
        };
        if a == b || uses(a, b) != 1 || !done.insert((a.min(b), a.max(b))) {
            continue;
        }
        let mut chain = vec![a];
        chain.extend(&pts);
        chain.push(b);
        if chain.windows(2).any(|w| uses(w[0], w[1]) != 1) {
            continue;
        }
        // Direction the existing chord face traverses a→b?
        let (ti, fwd) = edges[&(a.min(b), a.max(b))][0];
        let chord_tri: Tri = mesh.tris[ti as usize];
        let a_to_b = (fwd && a < b) || (!fwd && a > b);
        // New faces must traverse the chord the other way: y→x where x→y is used.
        let (x, y, arc_from_x): (u32, u32, Vec<u32>) = if a_to_b {
            (a, b, pts.clone())
        } else {
            (b, a, pts.iter().rev().copied().collect())
        };
        let mut prev = x;
        for &p in &arc_from_x {
            added.push(Tri {
                v: [y, prev, p],
                ..chord_tri
            });
            prev = p;
        }
        filled += 1;
    }
    mesh.tris.extend(added);
    filled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldraw::Library;
    use crate::mesh::{diagnose, DiagnosticOptions};

    fn cyl16() -> String {
        let mut s = String::from("0 BFC CERTIFY CCW\n");
        for k in 0..16 {
            let (a0, a1) = (TAU * k as f64 / 16.0, TAU * (k + 1) as f64 / 16.0);
            let (c0, s0, c1, s1) = (a0.cos(), a0.sin(), a1.cos(), a1.sin());
            s.push_str(&format!(
                "4 16 {c0} 0 {s0} {c0} 1 {s0} {c1} 1 {s1} {c1} 0 {s1}\n"
            ));
        }
        s
    }
    fn disc16(flip: bool) -> String {
        let mut s = String::from("0 BFC CERTIFY CCW\n");
        for k in 0..16 {
            let (a0, a1) = (TAU * k as f64 / 16.0, TAU * (k + 1) as f64 / 16.0);
            let (p0, p1) = ((a0.cos(), a0.sin()), (a1.cos(), a1.sin()));
            let (u, v) = if flip { (p1, p0) } else { (p0, p1) };
            s.push_str(&format!("3 16 0 0 0 {} 0 {} {} 0 {}\n", u.0, u.1, v.0, v.1));
        }
        s
    }

    fn lib() -> Library {
        let mut l = Library::new(None);
        l.add_embedded("cyl.dat", &format!("0 !LDRAW_ORG Primitive\n{}", cyl16()));
        l.add_embedded(
            "disc.dat",
            &format!("0 !LDRAW_ORG Primitive\n{}", disc16(false)),
        );
        l
    }

    #[test]
    fn smoothed_closed_cylinder_stays_watertight_and_rounder() {
        let mut l = lib();
        l.add_embedded(
            "can.dat",
            "0 BFC CERTIFY CCW\n1 16 0 -10 0 5 0 0 0 10 0 0 0 5 cyl.dat\n1 16 0 -10 0 5 0 0 0 1 0 0 0 5 disc.dat\n1 16 0 0 0 5 0 0 0 -1 0 0 0 5 disc.dat\n",
        );
        let mut r = l.resolve("can.dat", 4).unwrap();
        let base = Mesh::from_source(&r.triangles, 1e-4);
        let v0 = base.signed_volume();
        let (rep, _) = smooth_primitives(&mut r, 64);
        assert_eq!(rep.instances_smoothed, 3);
        let m = Mesh::from_source(&r.triangles, 1e-4);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert!(d.watertight, "{:?}", d.issues);
        // 64-gon area is closer to the true circle (π r² h ≈ 785.4).
        assert!(d.signed_volume > v0 && d.signed_volume < std::f64::consts::PI * 250.0);
        // Every rim vertex stays on the r = 5 circle.
        for p in &m.positions {
            let r = (p.x * p.x + p.z * p.z).sqrt();
            assert!(r < 1e-6 || (r - 5.0).abs() < 1e-6);
        }
    }

    #[test]
    fn crack_between_smoothed_and_raw_geometry_is_filled() {
        let mut l = lib();
        // Wall is a primitive (smoothed); caps are raw 16-gon fans (not smoothed).
        let mut caps = String::new();
        for k in 0..16 {
            let (a0, a1) = (TAU * k as f64 / 16.0, TAU * (k + 1) as f64 / 16.0);
            let (c0, s0, c1, s1) = (
                5.0 * a0.cos(),
                5.0 * a0.sin(),
                5.0 * a1.cos(),
                5.0 * a1.sin(),
            );
            caps.push_str(&format!("3 16 0 -10 0 {c0} -10 {s0} {c1} -10 {s1}\n"));
            caps.push_str(&format!("3 16 0 0 0 {c1} 0 {s1} {c0} 0 {s0}\n"));
        }
        l.add_embedded(
            "mixed.dat",
            &format!("0 BFC CERTIFY CCW\n1 16 0 -10 0 5 0 0 0 10 0 0 0 5 cyl.dat\n{caps}"),
        );
        let mut r = l.resolve("mixed.dat", 4).unwrap();
        assert!(
            diagnose(
                &Mesh::from_source(&r.triangles, 1e-4),
                &DiagnosticOptions::default()
            )
            .watertight
        );
        let (_, arcs) = smooth_primitives(&mut r, 48);
        let mut m = Mesh::from_source(&r.triangles, 1e-4);
        assert!(!diagnose(&m, &DiagnosticOptions::default()).watertight);
        let n = fill_arc_cracks(&mut m, &arcs, 1e-4);
        assert_eq!(n, 32);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert!(d.watertight, "{:?}", d.issues);
    }

    #[test]
    fn faces_drawn_on_cylinder_facets_are_recurved() {
        // Back half of a can is a primitive; the front half is hand-drawn on
        // the 16-gon facets in two rows (like a face print), with a small
        // disc primitive lying flat on one facet (like an eye) plus the
        // facet's remainder drawn around it. Caps are full disc primitives.
        let mut l = lib();
        let mut half = String::from("0 !LDRAW_ORG Primitive\n0 BFC CERTIFY CCW\n");
        for k in 0..8 {
            let (a0, a1) = (TAU * k as f64 / 16.0, TAU * (k + 1) as f64 / 16.0);
            let (c0, s0, c1, s1) = (a0.cos(), a0.sin(), a1.cos(), a1.sin());
            half.push_str(&format!(
                "4 16 {c0} 0 {s0} {c0} 1 {s0} {c1} 1 {s1} {c1} 0 {s1}\n"
            ));
        }
        l.add_embedded("halfcyl.dat", &half);
        let r = 5.0;
        let pt = |k: usize, y: f64| {
            let a = TAU * k as f64 / 16.0;
            format!("{} {y} {}", r * a.cos(), r * a.sin())
        };
        let mut front = String::new();
        for k in 8..16 {
            for (y0, y1) in [(0.0, -5.0), (-5.0, -10.0)] {
                if k == 12 && y0 == 0.0 {
                    continue; // this panel holds the eye
                }
                // Same winding as the primitive: lower y first, then angle.
                front.push_str(&format!(
                    "4 16 {} {} {} {}\n",
                    pt(k, y1),
                    pt(k, y0),
                    pt(k + 1, y0),
                    pt(k + 1, y1)
                ));
            }
        }
        // Eye panel (facet 12, y 0..-5): disc of radius 1 at its centre, plus
        // a fan from the panel corners to the disc rim.
        let (a0, a1) = (TAU * 12.0 / 16.0, TAU * 13.0 / 16.0);
        let c0 = Vec3::new(r * a0.cos(), 0.0, r * a0.sin());
        let c1 = Vec3::new(r * a1.cos(), 0.0, r * a1.sin());
        let centre = Vec3::new((c0.x + c1.x) / 2.0, -2.5, (c0.z + c1.z) / 2.0);
        let u = (c1 - c0).normalized(); // along the facet
        let out = Vec3::new(centre.x, 0.0, centre.z).normalized(); // facet normal
        let w = Vec3::new(0.0, -1.0, 0.0);
        // Eye radius; must fit inside the facet (half-width 5·sin 11.25° ≈ 0.98).
        const EYE: f64 = 0.6;
        let rim = |j: usize| {
            let a = TAU * j as f64 / 16.0;
            centre + u * (EYE * a.cos()) + w * (EYE * a.sin())
        };
        let corners = [c0, c1, c1 + w * 5.0, c0 + w * 5.0];
        let f = |p: Vec3| format!("{} {} {}", p.x, p.y, p.z);
        // Disc lying in the facet: local x → u, local z → w, local y → -out
        // (the disc's normal is local -Y, so it faces outwards).
        front.push_str(&format!(
            "1 16 {} {} {} {} {} {} {} {} {} {} {} {} disc.dat\n",
            centre.x,
            centre.y,
            centre.z,
            EYE * u.x,
            -out.x,
            EYE * w.x,
            EYE * u.y,
            -out.y,
            EYE * w.y,
            EYE * u.z,
            -out.z,
            EYE * w.z
        ));
        for j in 0..16 {
            // Corner owning this rim segment: quadrant of the rim angle.
            let q = [2usize, 3, 0, 1][j / 4];
            let tri = [corners[q], rim(j), rim(j + 1)];
            let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
            let tri = if n.dot(out) > 0.0 {
                tri
            } else {
                [tri[0], tri[2], tri[1]]
            };
            front.push_str(&format!("3 0 {} {} {}\n", f(tri[0]), f(tri[1]), f(tri[2])));
            if j % 4 == 3 {
                let next = corners[(q + 1) % 4];
                let tri = [corners[q], rim(j + 1), next];
                let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
                let tri = if n.dot(out) > 0.0 {
                    tri
                } else {
                    [tri[0], tri[2], tri[1]]
                };
                front.push_str(&format!("3 0 {} {} {}\n", f(tri[0]), f(tri[1]), f(tri[2])));
            }
        }
        l.add_embedded(
            "printed.dat",
            &format!(
                "0 BFC CERTIFY CCW\n1 16 0 -10 0 5 0 0 0 5 0 0 0 5 halfcyl.dat\n1 16 0 -5 0 5 0 0 0 5 0 0 0 5 halfcyl.dat\n\
                 1 16 0 -10 0 5 0 0 0 1 0 0 0 5 disc.dat\n1 16 0 0 0 5 0 0 0 -1 0 0 0 5 disc.dat\n{front}"
            ),
        );
        let mut res = l.resolve("printed.dat", 14).unwrap();
        let before = diagnose(
            &Mesh::from_source(&res.triangles, 1e-4),
            &DiagnosticOptions::default(),
        );
        assert!(
            before.watertight,
            "fixture must start closed: {:?}",
            before.issues
        );
        let (rep, arcs) = smooth_primitives(&mut res, 96);
        assert_eq!(
            rep.on_surface_primitives, 1,
            "the eye disc follows the cylinder"
        );
        assert!(rep.conformed_triangles > 0);
        let mut m = Mesh::from_source(&res.triangles, 1e-4);
        fill_arc_cracks(&mut m, &arcs, 1e-4);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert!(d.watertight, "{:?}", d.issues);
        // Every side vertex, printed or not, is on the true cylinder.
        for p in &m.positions {
            let rr = (p.x * p.x + p.z * p.z).sqrt();
            if p.y < -1e-6 && p.y > -10.0 + 1e-6 {
                assert!((rr - 5.0).abs() < 1e-3, "vertex {p:?} at r = {rr}");
            }
        }
        // Black stays black.
        assert!(m.tris.iter().any(|t| t.color == 0));
    }

    #[test]
    fn non_revolution_primitive_is_left_alone() {
        let mut l = Library::new(None);
        l.add_embedded("sq.dat", "0 !LDRAW_ORG Primitive\n0 BFC CERTIFY CCW\n4 16 1 0 1 1 0 -1 -1 0 -1 -1 0 1\n4 16 1 0 0.4142 1 0 1 0.3 0 0.2 0.5 0 0.1\n");
        l.add_embedded(
            "p.dat",
            "0 BFC CERTIFY CCW\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 sq.dat\n",
        );
        let mut r = l.resolve("p.dat", 4).unwrap();
        let before = r.triangles.len();
        let (rep, _) = smooth_primitives(&mut r, 64);
        assert_eq!(rep.instances_smoothed, 0);
        assert_eq!(r.triangles.len(), before);
    }
}
