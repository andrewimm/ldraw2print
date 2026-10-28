//! Indexed triangle mesh, vertex welding, topology diagnostics and a small
//! repair pipeline.
//!
//! LDraw geometry is authored for rendering: open seams, T-junctions between
//! primitives, back-to-back faces and overlapping shells are all common.
//! Diagnostics therefore distinguish *errors* (the file is very likely not
//! printable as-is) from *warnings* (slicers usually cope, but check the
//! result). A mesh is only reported watertight when every edge is shared by
//! exactly two consistently-wound triangles and the enclosed volume is
//! positive — exporting triangles successfully proves nothing.
//!
//! Repairs implement [`RepairStep`] so a more capable backend (e.g. a mesh
//! boolean library) can be slotted in later. Every built-in step either
//! removes zero-area/duplicate faces or changes connectivity/orientation
//! only; none of them moves vertices, closes holes or unions solids, so
//! connector surfaces are never altered behind the user's back.

use crate::det::{HashMap, HashSet};
use crate::geometry::{triangle_normal, Aabb, Vec3};
use crate::ldraw::SourceTriangle;
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tri {
    pub v: [u32; 3],
    pub color: u32,
    /// Winding known to be outward (BFC certified source).
    pub oriented: bool,
    /// Source file index (see `Resolved::files`).
    pub file: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub tris: Vec<Tri>,
}

/// Spatial hash used for welding and nearest-vertex lookup.
struct Welder {
    tol: f64,
    grid: HashMap<(i64, i64, i64), Vec<u32>>,
}

impl Welder {
    fn new(tol: f64) -> Self {
        Welder {
            tol: tol.max(1e-12),
            grid: HashMap::default(),
        }
    }
    fn cell(&self, p: Vec3) -> (i64, i64, i64) {
        let s = 1.0 / self.tol;
        (
            (p.x * s).floor() as i64,
            (p.y * s).floor() as i64,
            (p.z * s).floor() as i64,
        )
    }
    fn find_or_insert(&mut self, p: Vec3, positions: &mut Vec<Vec3>) -> u32 {
        let (cx, cy, cz) = self.cell(p);
        let mut best: Option<(f64, u32)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(ids) = self.grid.get(&(cx + dx, cy + dy, cz + dz)) {
                        for &i in ids {
                            let d = (positions[i as usize] - p).length();
                            if d <= self.tol && best.is_none_or(|(bd, _)| d < bd) {
                                best = Some((d, i));
                            }
                        }
                    }
                }
            }
        }
        if let Some((_, i)) = best {
            return i;
        }
        let i = positions.len() as u32;
        positions.push(p);
        self.grid.entry((cx, cy, cz)).or_default().push(i);
        i
    }
}

impl Mesh {
    /// Build an indexed mesh, merging vertices closer than `weld_tol`.
    pub fn from_source(tris: &[SourceTriangle], weld_tol: f64) -> Mesh {
        let mut m = Mesh::default();
        let mut w = Welder::new(weld_tol);
        for t in tris {
            let v = t.v.map(|p| w.find_or_insert(p, &mut m.positions));
            m.tris.push(Tri {
                v,
                color: t.color,
                oriented: t.oriented,
                file: t.file,
            });
        }
        m
    }

    pub fn tri_positions(&self, t: &Tri) -> [Vec3; 3] {
        t.v.map(|i| self.positions[i as usize])
    }

    pub fn tri_normal(&self, t: &Tri) -> Vec3 {
        let p = self.tri_positions(t);
        triangle_normal(p[0], p[1], p[2])
    }

    pub fn bbox(&self) -> Aabb {
        let mut b = Aabb::empty();
        for t in &self.tris {
            for p in self.tri_positions(t) {
                b.add(p);
            }
        }
        b
    }

    pub fn transform(&mut self, f: impl Fn(Vec3) -> Vec3) {
        for p in &mut self.positions {
            *p = f(*p);
        }
    }

    /// Sum of signed tetrahedron volumes; meaningful only for closed,
    /// consistently oriented meshes.
    pub fn signed_volume(&self) -> f64 {
        signed_volume(self, self.tris.iter())
    }

    /// Drop vertices no longer referenced by any triangle.
    pub fn compact(&mut self) {
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut pos = Vec::new();
        for t in &mut self.tris {
            for v in &mut t.v {
                if remap[*v as usize] == u32::MAX {
                    remap[*v as usize] = pos.len() as u32;
                    pos.push(self.positions[*v as usize]);
                }
                *v = remap[*v as usize];
            }
        }
        self.positions = pos;
    }

    /// Undirected edge → list of (triangle index, traversed a→b with a<b).
    pub fn edge_map(&self) -> HashMap<(u32, u32), Vec<(u32, bool)>> {
        let mut m: HashMap<(u32, u32), Vec<(u32, bool)>> =
            HashMap::with_capacity_and_hasher(self.tris.len() * 2, Default::default());
        for (ti, t) in self.tris.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t.v[k], t.v[(k + 1) % 3]);
                if a == b {
                    continue;
                }
                let key = if a < b { (a, b) } else { (b, a) };
                m.entry(key).or_default().push((ti as u32, a < b));
            }
        }
        m
    }

    /// Edge-connected components. Returns (component id per triangle, count).
    pub fn components(&self) -> (Vec<u32>, usize) {
        let n = self.tris.len();
        let mut uf = UnionFind::new(n);
        for uses in self.edge_map().values() {
            for w in uses.windows(2) {
                uf.union(w[0].0 as usize, w[1].0 as usize);
            }
        }
        let mut ids = HashMap::default();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let r = uf.find(i);
            let next = ids.len() as u32;
            out.push(*ids.entry(r).or_insert(next));
        }
        (out, ids.len())
    }
}

fn signed_volume<'a>(m: &Mesh, tris: impl Iterator<Item = &'a Tri>) -> f64 {
    tris.map(|t| {
        let p = m.tri_positions(t);
        p[0].dot(p[1].cross(p[2])) / 6.0
    })
    .sum()
}

struct UnionFind {
    parent: Vec<usize>,
}
impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

fn is_collapsed(t: &Tri) -> bool {
    t.v[0] == t.v[1] || t.v[1] == t.v[2] || t.v[0] == t.v[2]
}

fn is_degenerate(m: &Mesh, t: &Tri, tol: f64) -> bool {
    if t.v[0] == t.v[1] || t.v[1] == t.v[2] || t.v[0] == t.v[2] {
        return true;
    }
    let p = m.tri_positions(t);
    let area2 = triangle_normal(p[0], p[1], p[2]).length();
    let longest = (p[1] - p[0])
        .length()
        .max((p[2] - p[1]).length())
        .max((p[0] - p[2]).length());
    // Height of the triangle over its longest edge.
    longest == 0.0 || area2 / longest < tol
}

/// Canonical key for a triangle ignoring orientation, plus its parity.
fn tri_key(t: &Tri) -> ([u32; 3], bool) {
    let mut s = t.v;
    s.sort_unstable();
    // Orientation parity: is (v0,v1,v2) an even permutation of sorted?
    let rot = |a: [u32; 3]| [[a[0], a[1], a[2]], [a[1], a[2], a[0]], [a[2], a[0], a[1]]];
    let even = rot(t.v).contains(&s);
    (s, even)
}

// ---------------------------------------------------------------------------
// Diagnostics

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComponentInfo {
    pub id: u32,
    pub triangles: usize,
    pub bbox_min: [f64; 3],
    pub bbox_max: [f64; 3],
    pub closed: bool,
    pub signed_volume: f64,
    pub colors: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostics {
    pub triangles: usize,
    pub vertices: usize,
    pub unoriented_triangles: usize,
    pub degenerate_triangles: usize,
    pub duplicate_triangles: usize,
    pub opposite_face_pairs: usize,
    pub boundary_edges: usize,
    pub nonmanifold_edges: usize,
    pub inconsistent_edges: usize,
    pub t_junctions: usize,
    pub components: Vec<ComponentInfo>,
    /// Pairs of triangles that cross each other (None = check skipped).
    pub intersecting_pairs: Option<usize>,
    pub intersecting_component_pairs: Vec<(u32, u32)>,
    pub signed_volume: f64,
    pub closed: bool,
    pub consistently_oriented: bool,
    pub watertight: bool,
    pub issues: Vec<Issue>,
}

#[derive(Debug, Clone, Copy)]
pub struct DiagnosticOptions {
    /// Geometric tolerance in mesh units (degenerate height, T-junctions).
    pub tol: f64,
    /// Run the O(n log n) triangle–triangle intersection check.
    pub check_intersections: bool,
    /// Skip the intersection check above this many triangles.
    pub max_intersection_tris: usize,
}

impl Default for DiagnosticOptions {
    fn default() -> Self {
        DiagnosticOptions {
            tol: 1e-4,
            check_intersections: true,
            max_intersection_tris: 200_000,
        }
    }
}

impl Diagnostics {
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.severity == Severity::Error)
    }
}

pub fn diagnose(m: &Mesh, opt: &DiagnosticOptions) -> Diagnostics {
    // Same threshold as the RemoveDegenerate cleanup (a tenth of the weld
    // tolerance): thin-but-real faces, such as arc slivers, are not errors.
    // Collapsed triangles (two corners welded together) are defects. Thin
    // "needles" with three distinct corners are valid geometry that often
    // closes a seam, so they are reported but do not block watertightness.
    let collapsed = m.tris.iter().filter(|t| is_collapsed(t)).count();
    let degenerate = m
        .tris
        .iter()
        .filter(|t| is_degenerate(m, t, opt.tol * 0.1))
        .count();
    let needles = degenerate - collapsed;

    let mut seen: HashMap<[u32; 3], (usize, usize)> = HashMap::default(); // (even, odd)
    for t in &m.tris {
        let (k, even) = tri_key(t);
        let e = seen.entry(k).or_default();
        if even {
            e.0 += 1
        } else {
            e.1 += 1
        }
    }
    let mut duplicate = 0;
    let mut opposite = 0;
    for (even, odd) in seen.values() {
        duplicate += even.saturating_sub(1) + odd.saturating_sub(1);
        opposite += (*even).min(*odd);
    }

    let edges = m.edge_map();
    let mut boundary = 0;
    let mut nonmanifold = 0;
    let mut inconsistent = 0;
    let mut boundary_edges = Vec::new();
    for (&(a, b), uses) in &edges {
        match uses.len() {
            1 => {
                boundary += 1;
                boundary_edges.push((a, b));
            }
            2 => {
                // Both traverse the shared edge the same way: one is flipped.
                if uses[0].1 == uses[1].1 {
                    inconsistent += 1;
                }
            }
            _ => nonmanifold += 1,
        }
    }
    let t_junctions = find_t_junctions(m, &boundary_edges, opt.tol).len();

    let (comp, ncomp) = m.components();
    let mut comps: Vec<ComponentInfo> = (0..ncomp as u32)
        .map(|id| ComponentInfo {
            id,
            triangles: 0,
            bbox_min: [f64::INFINITY; 3],
            bbox_max: [f64::NEG_INFINITY; 3],
            closed: true,
            signed_volume: 0.0,
            colors: Vec::new(),
        })
        .collect();
    for (ti, t) in m.tris.iter().enumerate() {
        let c = &mut comps[comp[ti] as usize];
        c.triangles += 1;
        for p in m.tri_positions(t) {
            for (k, v) in p.to_array().iter().enumerate() {
                c.bbox_min[k] = c.bbox_min[k].min(*v);
                c.bbox_max[k] = c.bbox_max[k].max(*v);
            }
        }
        let p = m.tri_positions(t);
        c.signed_volume += p[0].dot(p[1].cross(p[2])) / 6.0;
        if !c.colors.contains(&t.color) {
            c.colors.push(t.color);
        }
    }
    for uses in edges.values() {
        if uses.len() != 2 {
            comps[comp[uses[0].0 as usize] as usize].closed = false;
        }
    }

    let (intersecting_pairs, comp_pairs) =
        if opt.check_intersections && m.tris.len() <= opt.max_intersection_tris {
            let (n, pairs) = find_intersections(m, &comp, opt.tol);
            (Some(n), pairs)
        } else {
            (None, Vec::new())
        };

    let signed_volume = m.signed_volume();
    let unoriented = m.tris.iter().filter(|t| !t.oriented).count();
    let closed = boundary == 0 && nonmanifold == 0 && !m.tris.is_empty();
    let consistently_oriented = inconsistent == 0 && unoriented == 0;
    let scale = m.bbox().size().length().max(1e-9);
    let positive_volume = signed_volume > 1e-9 * scale.powi(3);
    let watertight = closed && consistently_oriented && positive_volume && collapsed == 0;

    let mut issues = Vec::new();
    let mut push = |severity, code, message: String| {
        issues.push(Issue {
            severity,
            code,
            message,
        })
    };
    if m.tris.is_empty() {
        push(
            Severity::Error,
            "empty",
            "mesh contains no triangles".into(),
        );
    }
    if collapsed > 0 {
        push(
            Severity::Warning,
            "degenerate",
            format!("{collapsed} collapsed triangles (two corners at the same vertex)"),
        );
    }
    if needles > 0 {
        push(
            Severity::Info,
            "needles",
            format!("{needles} needle-thin triangles (kept: removing them would open the mesh)"),
        );
    }
    if duplicate > 0 {
        push(
            Severity::Warning,
            "duplicate",
            format!("{duplicate} exact duplicate triangles"),
        );
    }
    if opposite > 0 {
        push(
            Severity::Warning,
            "opposite-pairs",
            format!("{opposite} back-to-back triangle pairs (zero-thickness walls)"),
        );
    }
    if boundary > 0 {
        push(
            Severity::Error,
            "open-boundary",
            format!("{boundary} open boundary edges — mesh is not closed"),
        );
    }
    if t_junctions > 0 {
        push(
            Severity::Info,
            "t-junctions",
            format!(
                "{t_junctions} T-junction vertices on open edges (fixable with --fix-t-junctions)"
            ),
        );
    }
    if nonmanifold > 0 {
        push(
            Severity::Error,
            "non-manifold",
            format!("{nonmanifold} non-manifold edges (shared by >2 triangles)"),
        );
    }
    if inconsistent > 0 {
        push(
            Severity::Error,
            "inconsistent-winding",
            format!("{inconsistent} edges with inconsistent triangle winding"),
        );
    }
    if unoriented > 0 {
        push(
            Severity::Warning,
            "unknown-orientation",
            format!("{unoriented} triangles come from non-BFC-certified geometry; their facing is a guess (try --orient)"),
        );
    }
    if ncomp > 1 {
        push(
            Severity::Info,
            "components",
            format!("{ncomp} disconnected components"),
        );
        let diag = |c: &ComponentInfo| {
            let d = [
                c.bbox_max[0] - c.bbox_min[0],
                c.bbox_max[1] - c.bbox_min[1],
                c.bbox_max[2] - c.bbox_min[2],
            ];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        };
        let main = comps
            .iter()
            .max_by(|a, b| diag(a).total_cmp(&diag(b)).then(b.id.cmp(&a.id)))
            .map(|c| c.id);
        let open_small = comps
            .iter()
            .filter(|c| !c.closed && Some(c.id) != main)
            .count();
        if open_small > 0 {
            push(
                Severity::Warning,
                "open-shells",
                format!("{open_small} separate open shells (e.g. ribs or bosses missing hidden faces); slicers may drop or garble them — try --close-open-shells"),
            );
        }
    }
    match intersecting_pairs {
        Some(n) if n > 0 => push(
            Severity::Warning,
            "intersections",
            format!(
                "{n} intersecting triangle pairs; {} component pairs overlap (slicers normally union overlapping shells)",
                comp_pairs.len()
            ),
        ),
        None => push(Severity::Info, "intersections-skipped", "self-intersection check skipped".into()),
        _ => {}
    }
    if closed && consistently_oriented && !positive_volume {
        push(
            Severity::Error,
            "volume",
            format!(
                "closed mesh has non-positive volume ({signed_volume:.4}); it is inverted or flat"
            ),
        );
    }
    if watertight {
        push(
            Severity::Info,
            "watertight",
            "mesh is closed, manifold, consistently oriented with positive volume".into(),
        );
    }

    Diagnostics {
        triangles: m.tris.len(),
        vertices: m.positions.len(),
        unoriented_triangles: unoriented,
        degenerate_triangles: degenerate,
        duplicate_triangles: duplicate,
        opposite_face_pairs: opposite,
        boundary_edges: boundary,
        nonmanifold_edges: nonmanifold,
        inconsistent_edges: inconsistent,
        t_junctions,
        components: comps,
        intersecting_pairs,
        intersecting_component_pairs: comp_pairs,
        signed_volume,
        closed,
        consistently_oriented,
        watertight,
        issues,
    }
}

/// Edges with exactly one adjacent triangle.
pub fn boundary_edges(m: &Mesh) -> Vec<(u32, u32)> {
    m.edge_map()
        .into_iter()
        .filter(|(_, u)| u.len() == 1)
        .map(|(k, _)| k)
        .collect()
}

/// Edges shared by more than two triangles.
pub fn nonmanifold_edges(m: &Mesh) -> Vec<(u32, u32)> {
    m.edge_map()
        .into_iter()
        .filter(|(_, u)| u.len() > 2)
        .map(|(k, _)| k)
        .collect()
}

/// For each boundary edge, boundary vertices lying strictly inside it.
/// Returns (edge, vertex, parameter along edge).
fn find_t_junctions(m: &Mesh, boundary: &[(u32, u32)], tol: f64) -> Vec<((u32, u32), u32, f64)> {
    if boundary.is_empty() {
        return Vec::new();
    }
    let mut verts: Vec<u32> = boundary.iter().flat_map(|&(a, b)| [a, b]).collect();
    verts.sort_unstable();
    verts.dedup();
    let mut lens: Vec<f64> = boundary
        .iter()
        .map(|&(a, b)| (m.positions[a as usize] - m.positions[b as usize]).length())
        .collect();
    lens.sort_by(|a, b| a.total_cmp(b));
    let cell = lens[lens.len() / 2].max(tol * 10.0);
    let key = |p: Vec3| {
        (
            (p.x / cell).floor() as i64,
            (p.y / cell).floor() as i64,
            (p.z / cell).floor() as i64,
        )
    };
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::default();
    for &v in &verts {
        grid.entry(key(m.positions[v as usize]))
            .or_default()
            .push(v);
    }
    let mut out = Vec::new();
    for &(a, b) in boundary {
        let pa = m.positions[a as usize];
        let pb = m.positions[b as usize];
        let d = pb - pa;
        let len2 = d.dot(d);
        if len2 == 0.0 {
            continue;
        }
        let lo = key(pa.min(pb) - Vec3::new(tol, tol, tol));
        let hi = key(pa.max(pb) + Vec3::new(tol, tol, tol));
        let ncells = ((hi.0 - lo.0 + 1) * (hi.1 - lo.1 + 1) * (hi.2 - lo.2 + 1)) as usize;
        let check = |v: u32, out: &mut Vec<_>| {
            if v == a || v == b {
                return;
            }
            let p = m.positions[v as usize];
            let t = (p - pa).dot(d) / len2;
            if t <= 1e-9 || t >= 1.0 - 1e-9 {
                return;
            }
            let dist = (pa + d * t - p).length();
            if dist <= tol {
                out.push(((a, b), v, t));
            }
        };
        if ncells > verts.len() {
            for &v in &verts {
                check(v, &mut out);
            }
        } else {
            for x in lo.0..=hi.0 {
                for y in lo.1..=hi.1 {
                    for z in lo.2..=hi.2 {
                        if let Some(vs) = grid.get(&(x, y, z)) {
                            for &v in vs {
                                check(v, &mut out);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Strict segment/triangle crossing test (touching does not count).
fn segment_hits_triangle(p: Vec3, q: Vec3, t: [Vec3; 3], eps: f64) -> bool {
    let dir = q - p;
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let h = dir.cross(e2);
    let det = e1.dot(h);
    let scale = e1.length() * e2.length() * dir.length();
    if det.abs() <= 1e-9 * scale {
        return false; // parallel / coplanar: ignored
    }
    let inv = 1.0 / det;
    let s = p - t[0];
    let u = s.dot(h) * inv;
    if u <= eps || u >= 1.0 - eps {
        return false;
    }
    let qv = s.cross(e1);
    let v = dir.dot(qv) * inv;
    if v <= eps || u + v >= 1.0 - eps {
        return false;
    }
    let tt = e2.dot(qv) * inv;
    tt > eps && tt < 1.0 - eps
}

fn tris_intersect(a: [Vec3; 3], b: [Vec3; 3], eps: f64) -> bool {
    (0..3).any(|k| segment_hits_triangle(a[k], a[(k + 1) % 3], b, eps))
        || (0..3).any(|k| segment_hits_triangle(b[k], b[(k + 1) % 3], a, eps))
}

/// Count crossing triangle pairs using a uniform grid broadphase.
fn find_intersections(m: &Mesh, comp: &[u32], tol: f64) -> (usize, Vec<(u32, u32)>) {
    let n = m.tris.len();
    if n < 2 {
        return (0, Vec::new());
    }
    let boxes: Vec<Aabb> = m
        .tris
        .iter()
        .map(|t| {
            let mut b = Aabb::empty();
            for p in m.tri_positions(t) {
                b.add(p);
            }
            b
        })
        .collect();
    let mut sizes: Vec<f64> = boxes.iter().map(|b| b.size().length()).collect();
    sizes.sort_by(|a, b| a.total_cmp(b));
    let cell = (sizes[n / 2] * 2.0).max(tol * 100.0).max(1e-9);
    let key = |v: f64| (v / cell).floor() as i64;
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::default();
    for (i, b) in boxes.iter().enumerate() {
        let (x0, y0, z0) = (key(b.min.x), key(b.min.y), key(b.min.z));
        let (x1, y1, z1) = (key(b.max.x), key(b.max.y), key(b.max.z));
        if (x1 - x0 + 1) * (y1 - y0 + 1) * (z1 - z0 + 1) > 4096 {
            // Huge triangle: register in its corner cells only.
            for c in [(x0, y0, z0), (x1, y1, z1)] {
                grid.entry(c).or_default().push(i as u32);
            }
            continue;
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    grid.entry((x, y, z)).or_default().push(i as u32);
                }
            }
        }
    }
    let mut tested: HashSet<(u32, u32)> = HashSet::default();
    let mut count = 0;
    let mut cpairs: HashSet<(u32, u32)> = HashSet::default();
    let eps = 1e-6;
    for cell in grid.values() {
        for (ii, &i) in cell.iter().enumerate() {
            for &j in &cell[ii + 1..] {
                let (a, b) = if i < j { (i, j) } else { (j, i) };
                if !boxes[a as usize].overlaps(&boxes[b as usize], tol) || !tested.insert((a, b)) {
                    continue;
                }
                let ta = &m.tris[a as usize];
                let tb = &m.tris[b as usize];
                if ta.v.iter().any(|v| tb.v.contains(v)) {
                    continue;
                }
                if tris_intersect(m.tri_positions(ta), m.tri_positions(tb), eps) {
                    count += 1;
                    let (ca, cb) = (comp[a as usize], comp[b as usize]);
                    if ca != cb {
                        cpairs.insert((ca.min(cb), ca.max(cb)));
                    }
                }
            }
        }
    }
    let mut cp: Vec<_> = cpairs.into_iter().collect();
    cp.sort_unstable();
    (count, cp)
}

// ---------------------------------------------------------------------------
// Repair pipeline

#[derive(Debug, Clone, Serialize)]
pub struct RepairOutcome {
    pub step: String,
    pub changed: usize,
    pub note: String,
}

/// One mesh-repair operation. Implementations must document whether they can
/// move vertices; built-in steps never do.
pub trait RepairStep {
    fn name(&self) -> &'static str;
    fn apply(&self, mesh: &mut Mesh) -> RepairOutcome;
}

pub struct RemoveDegenerate {
    pub tol: f64,
}
impl RepairStep for RemoveDegenerate {
    fn name(&self) -> &'static str {
        "remove-degenerate"
    }
    fn apply(&self, m: &mut Mesh) -> RepairOutcome {
        // Collapsed triangles are always removed. A needle (three distinct
        // corners, near-zero height) is kept when one of its edges is shared
        // with exactly one other triangle: deleting it would turn that edge
        // into an open boundary.
        let before = m.tris.len();
        let edges = m.edge_map();
        let uses = |a: u32, b: u32| edges.get(&(a.min(b), a.max(b))).map_or(0, |u| u.len());
        let mut kept_needles = 0;
        let tris = std::mem::take(&mut m.tris);
        m.tris = tris
            .into_iter()
            .filter(|t| {
                if is_collapsed(t) {
                    return false;
                }
                if !is_degenerate(m, t, self.tol) {
                    return true;
                }
                let safe = (0..3).all(|k| uses(t.v[k], t.v[(k + 1) % 3]) != 2);
                if !safe {
                    kept_needles += 1;
                }
                !safe
            })
            .collect();
        let n = before - m.tris.len();
        RepairOutcome {
            step: self.name().into(),
            changed: n,
            note: format!("removed {n} degenerate triangles; kept {kept_needles} needle-thin triangles that close the mesh"),
        }
    }
}

pub struct RemoveDuplicates;
impl RepairStep for RemoveDuplicates {
    fn name(&self) -> &'static str {
        "remove-duplicates"
    }
    fn apply(&self, m: &mut Mesh) -> RepairOutcome {
        let before = m.tris.len();
        let mut seen = HashSet::default();
        m.tris.retain(|t| seen.insert(tri_key(t)));
        let n = before - m.tris.len();
        RepairOutcome {
            step: self.name().into(),
            changed: n,
            note: format!("removed {n} exact duplicate triangles (same vertices, same winding)"),
        }
    }
}

/// Split triangles at vertices that lie on their open edges. This makes
/// connectivity explicit without moving any vertex.
pub struct FixTJunctions {
    pub tol: f64,
}
impl RepairStep for FixTJunctions {
    fn name(&self) -> &'static str {
        "fix-t-junctions"
    }
    fn apply(&self, m: &mut Mesh) -> RepairOutcome {
        let mut total = 0;
        for _pass in 0..16 {
            let edges = m.edge_map();
            let boundary: Vec<(u32, u32)> = edges
                .iter()
                .filter(|(_, u)| u.len() == 1)
                .map(|(k, _)| *k)
                .collect();
            let tj = find_t_junctions(m, &boundary, self.tol);
            if tj.is_empty() {
                break;
            }
            let mut on_edge: HashMap<(u32, u32), Vec<(f64, u32)>> = HashMap::default();
            for (e, v, t) in tj {
                on_edge.entry(e).or_default().push((t, v));
            }
            let mut split_tris = 0;
            let mut new_tris = Vec::with_capacity(m.tris.len());
            for t in &m.tris {
                let mut done = false;
                for k in 0..3 {
                    if done {
                        break;
                    }
                    let (a, b, c) = (t.v[k], t.v[(k + 1) % 3], t.v[(k + 2) % 3]);
                    let key = if a < b { (a, b) } else { (b, a) };
                    let Some(pts) = on_edge.get(&key) else {
                        continue;
                    };
                    let mut pts = pts.clone();
                    // Parameters are along key.0 → key.1; orient along a → b.
                    if a > b {
                        for p in &mut pts {
                            p.0 = 1.0 - p.0;
                        }
                    }
                    pts.sort_by(|x, y| x.0.total_cmp(&y.0));
                    pts.dedup_by_key(|p| p.1);
                    let mut chain = vec![a];
                    chain.extend(pts.iter().map(|p| p.1));
                    chain.push(b);
                    for w in chain.windows(2) {
                        new_tris.push(Tri {
                            v: [w[0], w[1], c],
                            ..*t
                        });
                    }
                    done = true;
                    split_tris += 1;
                }
                if !done {
                    new_tris.push(*t);
                }
            }
            m.tris = new_tris;
            total += split_tris;
            if split_tris == 0 {
                break;
            }
        }
        RepairOutcome {
            step: self.name().into(),
            changed: total,
            note: format!("split {total} triangles at T-junctions (no vertices moved)"),
        }
    }
}

/// Make winding consistent within each edge-connected component, seeded from
/// BFC-certified triangles. Fully uncertified closed components are flipped
/// to positive volume. Only reorders vertex indices.
pub struct OrientComponents;
impl RepairStep for OrientComponents {
    fn name(&self) -> &'static str {
        "orient"
    }
    fn apply(&self, m: &mut Mesh) -> RepairOutcome {
        let edges = m.edge_map();
        let mut adj: Vec<Vec<(u32, bool)>> = vec![Vec::new(); m.tris.len()]; // (neighbour, same_direction)
        for uses in edges.values() {
            if uses.len() == 2 {
                let ((t0, d0), (t1, d1)) = (uses[0], uses[1]);
                adj[t0 as usize].push((t1, d0 == d1));
                adj[t1 as usize].push((t0, d0 == d1));
            }
        }
        let n = m.tris.len();
        let mut flip = vec![false; n];
        let mut visited = vec![false; n];
        let mut flipped = 0;
        let mut conflicts = 0;
        // Seed certified triangles first so they win.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| !m.tris[i].oriented);
        for &seed in &order {
            if visited[seed] {
                continue;
            }
            let mut comp = vec![seed];
            visited[seed] = true;
            let mut q = VecDeque::from([seed]);
            let seeded = m.tris[seed].oriented;
            while let Some(i) = q.pop_front() {
                for &(j, same) in &adj[i] {
                    let want = flip[i] ^ same;
                    let j = j as usize;
                    if visited[j] {
                        if flip[j] != want {
                            conflicts += 1;
                        }
                        continue;
                    }
                    if m.tris[j].oriented && want {
                        // Never override certified winding.
                        conflicts += 1;
                        visited[j] = true;
                        q.push_back(j);
                        comp.push(j);
                        continue;
                    }
                    flip[j] = want;
                    visited[j] = true;
                    q.push_back(j);
                    comp.push(j);
                }
            }
            if !seeded {
                let vol: f64 = comp
                    .iter()
                    .map(|&i| {
                        let p = m.tri_positions(&m.tris[i]);
                        let s = p[0].dot(p[1].cross(p[2])) / 6.0;
                        if flip[i] {
                            -s
                        } else {
                            s
                        }
                    })
                    .sum();
                if vol < 0.0 {
                    for &i in &comp {
                        flip[i] = !flip[i];
                    }
                }
            }
        }
        for (t, f) in m.tris.iter_mut().zip(flip) {
            if f {
                t.v.swap(1, 2);
                flipped += 1;
            }
            // Facing is now inferred from topology; only trust it if the
            // propagation was conflict-free.
            if conflicts == 0 {
                t.oriented = true;
            }
        }
        RepairOutcome {
            step: self.name().into(),
            changed: flipped,
            note: format!(
                "flipped {flipped} triangles; {conflicts} orientation conflicts left unresolved"
            ),
        }
    }
}

/// Close small open shells that slicers would drop or mangle. LDraw often
/// leaves out the faces where a feature meets the main body (ribs inside a
/// hat, a boss on its surface) because they are never seen when rendering.
///
/// The largest component is never touched. For every other open component:
/// 1. if each open boundary loop is planar, it is capped with a flat face;
/// 2. otherwise, if every existing face lies on the component's convex hull,
///    the shell is completed to that hull (a rib that is a box with missing
///    faces becomes the full box).
///
/// Otherwise the component is left alone and reported. Only faces are added
/// (from existing vertices); no vertex moves. The closed pieces overlap the
/// main body, which slicers union.
pub struct CloseOpenShells {
    pub tol: f64,
    /// Depth (mesh units) of the hidden anchors that fuse a closed piece to
    /// the main body where they only touch; 0 disables anchors.
    pub anchor: f64,
}

impl RepairStep for CloseOpenShells {
    fn name(&self) -> &'static str {
        "close-open-shells"
    }
    fn apply(&self, m: &mut Mesh) -> RepairOutcome {
        let (comp, n) = m.components();
        if n < 2 {
            return RepairOutcome {
                step: self.name().into(),
                changed: 0,
                note: "no separate open shells".into(),
            };
        }
        let main = main_component(m, &comp, n);
        let main_tris: Vec<Tri> = m
            .tris
            .iter()
            .zip(&comp)
            .filter(|(_, &c)| c == main)
            .map(|(t, _)| *t)
            .collect();
        let edges = m.edge_map();
        // Directed boundary edges per component, as traversed by their face.
        let mut boundary: HashMap<u32, Vec<(u32, u32)>> = HashMap::default();
        for (&(a, b), uses) in &edges {
            if uses.len() == 1 {
                let (t, fwd) = uses[0];
                let c = comp[t as usize];
                if c != main {
                    boundary
                        .entry(c)
                        .or_default()
                        .push(if fwd { (a, b) } else { (b, a) });
                }
            }
        }
        let (mut capped, mut hulled, mut left) = (0, 0, 0);
        let (mut flat, mut covered_n) = (0, 0);
        let mut add: Vec<Tri> = Vec::new();
        // Face outlines created here, for anchoring: (vertex ring, template).
        let mut created: Vec<(Vec<u32>, Tri)> = Vec::new();
        let mut remove: HashSet<u32> = HashSet::default();
        // Every edge that exists so far, including caps added in this pass,
        // so no two caps (or a cap and the mesh) share a diagonal.
        let mut used: HashSet<(u32, u32)> = edges.keys().copied().collect();
        let mut comps: Vec<u32> = boundary.keys().copied().collect();
        comps.sort_unstable();
        for c in comps {
            let tris: Vec<u32> = (0..m.tris.len() as u32)
                .filter(|&t| comp[t as usize] == c)
                .collect();
            let template = m.tris[tris[0] as usize];
            // A flat open sheet has no inside: capping it would only lay a
            // second face on top (a zero-volume "solid"). Leave it.
            let first = m.tri_positions(&m.tris[tris[0] as usize]);
            let n0 = triangle_normal(first[0], first[1], first[2]).normalized();
            if n0.length() > 0.0
                && tris.iter().all(|&t| {
                    m.tri_positions(&m.tris[t as usize])
                        .iter()
                        .all(|q| (*q - first[0]).dot(n0).abs() <= self.tol * 4.0)
                })
            {
                flat += 1;
                left += 1;
                continue;
            }
            let loops = boundary_loops(&boundary[&c]);
            // An opening already covered by another piece's coplanar face
            // must not be capped (the cap would coincide with that face).
            let covered = |l: &[u32]| {
                let set: HashSet<u32> = l.iter().copied().collect();
                m.tris.iter().zip(&comp).any(|(t, &tc)| {
                    tc != c
                        && t.v.iter().filter(|v| set.contains(v)).count() >= 2
                        && covers_loop(m, l, t, self.tol)
                })
            };
            let caps: Option<Vec<[u32; 3]>> = loops.as_ref().and_then(|ls| {
                if ls.iter().any(|l| covered(l)) {
                    covered_n += 1;
                    return None;
                }
                let mut all = Vec::new();
                // Coplanar loops nested inside another are holes of one face
                // (e.g. the missing ring on top of a tube): cap them together.
                for (outer, holes) in group_nested_loops(m, ls, self.tol) {
                    all.extend(cap_planar_loop(m, &outer, &holes, self.tol, &used)?);
                }
                Some(all)
            });
            if let Some(cs) = &caps {
                for t in cs {
                    for k in 0..3 {
                        let (a, b) = (t[k], t[(k + 1) % 3]);
                        used.insert((a.min(b), a.max(b)));
                    }
                }
            }
            if let Some(caps) = caps {
                // Each outer loop is one cap face; its outline is the reversed
                // loop (anchors use the outline; holes are irrelevant there).
                for (outer, _) in group_nested_loops(m, loops.as_ref().unwrap(), self.tol) {
                    created.push((outer.iter().rev().copied().collect(), template));
                }
                add.extend(caps.into_iter().map(|v| Tri { v, ..template }));
                capped += 1;
                continue;
            }
            if let Some((hull, rings)) = convex_completion(m, &tris, self.tol) {
                // Hull faces with no existing triangle on them are new.
                for ring in rings {
                    let on_plane = |t: &u32| {
                        let v = m.tris[*t as usize].v;
                        v.iter().all(|i| ring.contains(i))
                    };
                    if !tris.iter().any(on_plane) {
                        created.push((ring, template));
                    }
                }
                remove.extend(tris.iter().copied());
                add.extend(hull.into_iter().map(|v| Tri { v, ..template }));
                hulled += 1;
                continue;
            }
            left += 1;
        }
        if !remove.is_empty() {
            let mut i = 0u32;
            m.tris.retain(|_| {
                let keep = !remove.contains(&i);
                i += 1;
                keep
            });
        }
        m.tris.extend(add);
        // Anchors: where a created face only touches the main body, add a
        // thin closed prism through it, overlapping both, so slicers fuse the
        // piece to the body instead of printing it as a separate island.
        let mut anchors = 0;
        let mut anchor_skipped = 0;
        if self.anchor > 0.0 {
            for (ring, template) in &created {
                let pts: Vec<Vec3> = ring.iter().map(|&i| m.positions[i as usize]).collect();
                let dists: Vec<f64> = pts
                    .iter()
                    .map(|p| {
                        main_tris
                            .iter()
                            .map(|t| point_tri_distance(*p, m.tri_positions(t)))
                            .fold(f64::INFINITY, f64::min)
                    })
                    .collect();
                // "Touching" within half the anchor depth: the anchor reaches
                // `anchor` into the body, so it still bridges such a gap (e.g.
                // a flat rib face against a curved wall).
                let touching = dists
                    .iter()
                    .all(|&d| d <= (self.anchor * 0.5).max(self.tol * 4.0));
                if !touching {
                    continue;
                }
                let mut n = Vec3::ZERO;
                for i in 0..pts.len() {
                    let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                    n = n + Vec3::new(
                        (a.y - b.y) * (a.z + b.z),
                        (a.z - b.z) * (a.x + b.x),
                        (a.x - b.x) * (a.y + b.y),
                    );
                }
                let n = n.normalized();
                if n.length() == 0.0 || pts.len() < 3 {
                    continue;
                }
                let inner: Vec<Vec3> = pts.iter().map(|p| *p - n * self.anchor).collect();
                let outer: Vec<Vec3> = pts.iter().map(|p| *p + n * self.anchor).collect();
                // The far side must be inside the main body's material.
                let probe =
                    outer.iter().fold(Vec3::ZERO, |s, p| s + *p) * (1.0 / outer.len() as f64);
                let inside = outer
                    .iter()
                    .chain(std::iter::once(&probe))
                    .all(|p| winding_number(m, &main_tris, *p) > 0.5);
                if !inside {
                    anchor_skipped += 1;
                    continue;
                }
                let base = m.positions.len() as u32;
                m.positions.extend(inner.iter().chain(outer.iter()));
                let k = ring.len() as u32;
                let (lo, hi) = (|i: u32| base + i, |i: u32| base + k + i);
                // Outer cap (+n), inner cap (-n), sides.
                for i in 1..k - 1 {
                    add_tri(&mut m.tris, [hi(0), hi(i), hi(i + 1)], template);
                    add_tri(&mut m.tris, [lo(0), lo(i + 1), lo(i)], template);
                }
                for i in 0..k {
                    let j = (i + 1) % k;
                    add_tri(&mut m.tris, [lo(i), lo(j), hi(j)], template);
                    add_tri(&mut m.tris, [lo(i), hi(j), hi(i)], template);
                }
                anchors += 1;
            }
        }
        RepairOutcome {
            step: self.name().into(),
            changed: capped + hulled,
            note: format!(
                "closed {} open shells ({capped} by capping flat openings, {hulled} by completing convex pieces); {left} left open{}; \
                 added {anchors} hidden anchors fusing closed pieces to the body{}",
                capped + hulled,
                if left > 0 {
                    format!(
                        " ({flat} flat sheets, {covered_n} openings covered by another piece, {} not safely closable)",
                        left - flat - covered_n.min(left - flat)
                    )
                } else {
                    String::new()
                },
                if anchor_skipped > 0 { format!(" ({anchor_skipped} skipped: would not stay inside the body)") } else { String::new() }
            ),
        }
    }
}

/// The main body: the component with the largest bounding-box diagonal.
/// (Triangle count is unstable: capping a small piece adds triangles.)
pub fn main_component(m: &Mesh, comp: &[u32], n: usize) -> u32 {
    let mut boxes = vec![Aabb::empty(); n];
    for (t, &c) in m.tris.iter().zip(comp) {
        for p in m.tri_positions(t) {
            boxes[c as usize].add(p);
        }
    }
    (0..n)
        .max_by(|&a, &b| {
            boxes[a]
                .size()
                .length()
                .total_cmp(&boxes[b].size().length())
                .then(b.cmp(&a))
        })
        .unwrap_or(0) as u32
}

fn add_tri(tris: &mut Vec<Tri>, v: [u32; 3], template: &Tri) {
    tris.push(Tri { v, ..*template });
}

fn point_tri_distance(p: Vec3, t: [Vec3; 3]) -> f64 {
    let (a, b, c) = (t[0], t[1], t[2]);
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d00, d01, d11) = (ab.dot(ab), ab.dot(ac), ac.dot(ac));
    let den = d00 * d11 - d01 * d01;
    if den > 1e-18 {
        let (d1, d2) = (ab.dot(ap), ac.dot(ap));
        let v = (d11 * d1 - d01 * d2) / den;
        let w = (d00 * d2 - d01 * d1) / den;
        if v >= 0.0 && w >= 0.0 && v + w <= 1.0 {
            return (a + ab * v + ac * w - p).length();
        }
    }
    let seg = |u: Vec3, v: Vec3| {
        let uv = v - u;
        let l = uv.dot(uv);
        let s = if l == 0.0 {
            0.0
        } else {
            ((p - u).dot(uv) / l).clamp(0.0, 1.0)
        };
        (u + uv * s - p).length()
    };
    seg(a, b).min(seg(b, c)).min(seg(c, a))
}

/// Generalised winding number of `p` with respect to triangles (≈1 inside a
/// closed outward-oriented surface, ≈0 outside).
fn winding_number(m: &Mesh, tris: &[Tri], p: Vec3) -> f64 {
    let mut total = 0.0;
    for t in tris {
        let [a, b, c] = m.tri_positions(t).map(|q| q - p);
        let (la, lb, lc) = (a.length(), b.length(), c.length());
        let num = a.dot(b.cross(c));
        let den = la * lb * lc + a.dot(b) * lc + b.dot(c) * la + c.dot(a) * lb;
        total += 2.0 * num.atan2(den);
    }
    total / (4.0 * std::f64::consts::PI)
}

/// Chain directed boundary edges into closed loops; None if ambiguous.
fn boundary_loops(edges: &[(u32, u32)]) -> Option<Vec<Vec<u32>>> {
    let mut next: HashMap<u32, u32> = HashMap::default();
    for &(a, b) in edges {
        if next.insert(a, b).is_some() {
            return None; // vertex with two outgoing boundary edges
        }
    }
    let mut seen: HashSet<u32> = HashSet::default();
    let mut loops = Vec::new();
    for &(start, _) in edges {
        if seen.contains(&start) {
            continue;
        }
        let mut l = vec![start];
        seen.insert(start);
        let mut cur = *next.get(&start)?;
        while cur != start {
            if !seen.insert(cur) || l.len() > edges.len() {
                return None;
            }
            l.push(cur);
            cur = *next.get(&cur)?;
        }
        loops.push(l);
    }
    Some(loops)
}

/// Newell normal (unnormalised) and centroid of a vertex loop.
fn loop_plane(m: &Mesh, l: &[u32]) -> (Vec3, Vec3) {
    let p: Vec<Vec3> = l.iter().map(|&i| m.positions[i as usize]).collect();
    let mut n = Vec3::ZERO;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        n = n + Vec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    (
        n,
        p.iter().fold(Vec3::ZERO, |s, q| s + *q) * (1.0 / p.len() as f64),
    )
}

fn plane_basis(n: Vec3) -> (Vec3, Vec3) {
    let u = if n.x.abs() < 0.9 {
        Vec3::new(1., 0., 0.)
    } else {
        Vec3::new(0., 1., 0.)
    }
    .cross(n)
    .normalized();
    (u, n.cross(u))
}

fn point_in_poly(c: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.1 > c.1) != (b.1 > c.1) && c.0 < (b.0 - a.0) * (c.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

/// Pair each boundary loop with the coplanar loops nested inside it (holes).
/// Loops are returned in a deterministic order.
fn group_nested_loops(m: &Mesh, loops: &[Vec<u32>], tol: f64) -> Vec<(Vec<u32>, Vec<Vec<u32>>)> {
    let planes: Vec<(Vec3, Vec3)> = loops.iter().map(|l| loop_plane(m, l)).collect();
    let mut hole_of: Vec<Option<usize>> = vec![None; loops.len()];
    for (h, hl) in loops.iter().enumerate() {
        let (nh, ch) = planes[h];
        for (o, ol) in loops.iter().enumerate() {
            if o == h || ol.len() < 3 {
                continue;
            }
            let (no, co) = planes[o];
            let (nh1, no1) = (nh.normalized(), no.normalized());
            // A hole's boundary runs the opposite way round to its outer loop.
            if nh1.dot(no1) > -0.999 || (ch - co).dot(no1).abs() > tol * 4.0 {
                continue;
            }
            let (u, w) = plane_basis(no1);
            let to2 = |q: Vec3| ((q - co).dot(u), (q - co).dot(w));
            let outer2: Vec<(f64, f64)> =
                ol.iter().map(|&i| to2(m.positions[i as usize])).collect();
            if hl
                .iter()
                .all(|&i| point_in_poly(to2(m.positions[i as usize]), &outer2))
            {
                hole_of[h] = Some(o);
                break;
            }
        }
    }
    let mut out = Vec::new();
    for (o, ol) in loops.iter().enumerate() {
        if hole_of[o].is_some() {
            continue;
        }
        let holes = (0..loops.len())
            .filter(|&h| hole_of[h] == Some(o))
            .map(|h| loops[h].clone())
            .collect();
        out.push((ol.clone(), holes));
    }
    out.sort_by_key(|(l, _)| *l.iter().min().unwrap_or(&0));
    out
}

/// Triangulate a planar boundary loop with optional hole loops (ear
/// clipping on a bridged outline), wound opposite to the boundary so the cap
/// faces outwards. None if not planar or not simple, or if a new edge would
/// coincide with an existing one.
fn cap_planar_loop(
    m: &Mesh,
    l: &[u32],
    holes: &[Vec<u32>],
    tol: f64,
    existing: &HashSet<(u32, u32)>,
) -> Option<Vec<[u32; 3]>> {
    if l.len() < 3 {
        return None;
    }
    let (nrm, ctr) = loop_plane(m, l);
    let nrm = nrm.normalized();
    if nrm.length() == 0.0 {
        return None;
    }
    let all: Vec<u32> = l.iter().chain(holes.iter().flatten()).copied().collect();
    let extent = all
        .iter()
        .map(|&i| (m.positions[i as usize] - ctr).length())
        .fold(0.0, f64::max);
    if all
        .iter()
        .any(|&i| (m.positions[i as usize] - ctr).dot(nrm).abs() > tol.max(1e-3 * extent))
    {
        return None;
    }
    // Cap orientation: reversed loops, in 2D with the cap's normal (-nrm).
    let (u, w) = plane_basis(-nrm);
    let pt2 = |i: u32| {
        let q = m.positions[i as usize] - ctr;
        (q.dot(u), q.dot(w))
    };
    let mut ring: Vec<u32> = l.iter().rev().copied().collect();
    // Bridge each hole (largest u first) to a visible outline vertex.
    let mut hs: Vec<Vec<u32>> = holes
        .iter()
        .map(|h| h.iter().rev().copied().collect())
        .collect();
    hs.sort_by(|a, b| {
        let mx = |h: &Vec<u32>| {
            h.iter()
                .map(|&i| pt2(i).0)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        mx(b).total_cmp(&mx(a))
    });
    let seg_cross = |a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)| {
        let o = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
            (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
        };
        let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
        ((d1 > 1e-12 && d2 < -1e-12) || (d1 < -1e-12 && d2 > 1e-12))
            && ((d3 > 1e-12 && d4 < -1e-12) || (d3 < -1e-12 && d4 > 1e-12))
    };
    for h in hs {
        let (hi, _) = h
            .iter()
            .enumerate()
            .max_by(|a, b| pt2(*a.1).0.total_cmp(&pt2(*b.1).0))?;
        let hp = pt2(h[hi]);
        let edges_of = |r: &[u32]| -> Vec<((f64, f64), (f64, f64))> {
            (0..r.len())
                .map(|k| (pt2(r[k]), pt2(r[(k + 1) % r.len()])))
                .collect()
        };
        let mut blockers = edges_of(&ring);
        blockers.extend(edges_of(&h));
        let mut best: Option<(f64, usize)> = None;
        for (k, &o) in ring.iter().enumerate() {
            let op = pt2(o);
            let d = (op.0 - hp.0).powi(2) + (op.1 - hp.1).powi(2);
            if best.is_some_and(|(bd, _)| d >= bd) {
                continue;
            }
            if blockers.iter().any(|&(a, b)| seg_cross(hp, op, a, b)) {
                continue;
            }
            best = Some((d, k));
        }
        let (_, k) = best?;
        let mut merged: Vec<u32> = ring[..=k].to_vec();
        merged.extend(h[hi..].iter().chain(h[..hi].iter()));
        merged.push(h[hi]);
        merged.push(ring[k]);
        merged.extend_from_slice(&ring[k + 1..]);
        ring = merged;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut idx: Vec<usize> = (0..ring.len()).collect();
    let mut out = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 {
        guard += 1;
        if guard > 100_000 {
            return None;
        }
        let k = idx.len();
        // Among valid ears, clip the best-shaped one (largest smallest
        // angle) so caps have no slivers that a later fit could flip.
        let mut best: Option<(f64, usize)> = None;
        for i in 0..k {
            let (ia, ib, ic) = (idx[(i + k - 1) % k], idx[i], idx[(i + 1) % k]);
            let (va, vb, vc) = (ring[ia], ring[ib], ring[ic]);
            if va == vb || vb == vc || va == vc {
                continue;
            }
            let (a, b, c) = (pt2(va), pt2(vb), pt2(vc));
            if cross(a, b, c) <= 1e-12 {
                continue; // reflex or degenerate corner
            }
            let inside = idx.iter().any(|&j| {
                let vj = ring[j];
                if vj == va || vj == vb || vj == vc {
                    return false; // includes bridge duplicates of the corners
                }
                let q = pt2(vj);
                cross(a, b, q) >= 0.0 && cross(b, c, q) >= 0.0 && cross(c, a, q) >= 0.0
            });
            if inside {
                continue;
            }
            // The new diagonal must not coincide with an existing edge.
            if idx.len() > 3 && existing.contains(&(va.min(vc), va.max(vc))) {
                continue;
            }
            let angle = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
                let (u, v) = ((q.0 - p.0, q.1 - p.1), (r.0 - p.0, r.1 - p.1));
                let (lu, lv) = (
                    (u.0 * u.0 + u.1 * u.1).sqrt(),
                    (v.0 * v.0 + v.1 * v.1).sqrt(),
                );
                if lu == 0.0 || lv == 0.0 {
                    0.0
                } else {
                    ((u.0 * v.0 + u.1 * v.1) / (lu * lv))
                        .clamp(-1.0, 1.0)
                        .acos()
                }
            };
            let quality = angle(a, b, c).min(angle(b, c, a)).min(angle(c, a, b));
            if best.is_none_or(|(q, _)| quality > q) {
                best = Some((quality, i));
            }
        }
        let Some((_, i)) = best else {
            return None; // not a simple polygon
        };
        let k = idx.len();
        let (ia, ib, ic) = (idx[(i + k - 1) % k], idx[i], idx[(i + 1) % k]);
        out.push([ring[ia], ring[ib], ring[ic]]);
        idx.remove(i);
    }
    let (va, vb, vc) = (ring[idx[0]], ring[idx[1]], ring[idx[2]]);
    if va != vb && vb != vc && va != vc {
        out.push([va, vb, vc]);
    }
    Some(out)
}

/// Does triangle `t` lie in the plane of loop `l` and inside its outline?
fn covers_loop(m: &Mesh, l: &[u32], t: &Tri, tol: f64) -> bool {
    let p: Vec<Vec3> = l.iter().map(|&i| m.positions[i as usize]).collect();
    let mut n = Vec3::ZERO;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        n = n + Vec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    let n = n.normalized();
    if n.length() == 0.0 {
        return false;
    }
    let ctr = p.iter().fold(Vec3::ZERO, |s, q| s + *q) * (1.0 / p.len() as f64);
    let tp = m.tri_positions(t);
    if tp.iter().any(|q| (*q - ctr).dot(n).abs() > tol * 4.0) {
        return false;
    }
    // Point-in-polygon for the triangle's centroid, in the loop's plane.
    let u = if n.x.abs() < 0.9 {
        Vec3::new(1., 0., 0.)
    } else {
        Vec3::new(0., 1., 0.)
    }
    .cross(n)
    .normalized();
    let w = n.cross(u);
    let to2 = |q: Vec3| ((q - ctr).dot(u), (q - ctr).dot(w));
    let c = to2((tp[0] + tp[1] + tp[2]) * (1.0 / 3.0));
    let poly: Vec<(f64, f64)> = p.iter().map(|q| to2(*q)).collect();
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.1 > c.1) != (b.1 > c.1) && c.0 < (b.0 - a.0) * (c.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    inside
}

/// Convex hull of a small component's vertices, if every existing face lies
/// on the hull (so completing it changes nothing that already exists).
/// Hull triangles plus each hull face's vertex ring.
type HullCompletion = (Vec<[u32; 3]>, Vec<Vec<u32>>);

fn convex_completion(m: &Mesh, tris: &[u32], tol: f64) -> Option<HullCompletion> {
    let mut verts: Vec<u32> = tris.iter().flat_map(|&t| m.tris[t as usize].v).collect();
    verts.sort_unstable();
    verts.dedup();
    if verts.len() < 4 || verts.len() > 64 {
        return None;
    }
    let p = |i: u32| m.positions[i as usize];
    // Supporting planes through every triple with all points on one side.
    let mut planes: Vec<(Vec3, f64)> = Vec::new();
    for a in 0..verts.len() {
        for b in a + 1..verts.len() {
            for c in b + 1..verts.len() {
                let n = (p(verts[b]) - p(verts[a])).cross(p(verts[c]) - p(verts[a]));
                if n.length() < 1e-12 {
                    continue;
                }
                let n = n.normalized();
                let d = n.dot(p(verts[a]));
                let side: Vec<f64> = verts.iter().map(|&v| n.dot(p(v)) - d).collect();
                let (lo, hi) = (
                    side.iter().cloned().fold(0.0, f64::min),
                    side.iter().cloned().fold(0.0, f64::max),
                );
                let plane = if hi <= tol {
                    (n, d)
                } else if lo >= -tol {
                    (-n, -d)
                } else {
                    continue;
                };
                if !planes
                    .iter()
                    .any(|q| q.0.dot(plane.0) > 1.0 - 1e-9 && (q.1 - plane.1).abs() <= tol)
                {
                    planes.push(plane);
                }
            }
        }
    }
    // Every existing face must lie on some hull plane.
    for &t in tris {
        let v = m.tris[t as usize].v;
        if !planes
            .iter()
            .any(|(n, d)| v.iter().all(|&i| (n.dot(p(i)) - d).abs() <= tol))
        {
            return None;
        }
    }
    let area = |t: &[u32; 3]| triangle_normal(p(t[0]), p(t[1]), p(t[2])).length() * 0.5;
    let mut out = Vec::new();
    let mut rings = Vec::new();
    for (n, d) in &planes {
        let on: Vec<u32> = verts
            .iter()
            .copied()
            .filter(|&v| (n.dot(p(v)) - d).abs() <= tol)
            .collect();
        if on.len() < 3 {
            continue;
        }
        // Order the face's points by angle around their centroid (convex).
        let ctr = on.iter().fold(Vec3::ZERO, |s, &v| s + p(v)) * (1.0 / on.len() as f64);
        let u = (p(on[0]) - ctr).normalized();
        let w = n.cross(u);
        let mut ring: Vec<(f64, u32)> = on
            .iter()
            .map(|&v| ((p(v) - ctr).dot(w).atan2((p(v) - ctr).dot(u)), v))
            .collect();
        ring.sort_by(|a, b| a.0.total_cmp(&b.0));
        for k in 1..ring.len() - 1 {
            out.push([ring[0].1, ring[k].1, ring[k + 1].1]);
        }
        rings.push(ring.iter().map(|r| r.1).collect());
    }
    // Only complete pieces that are mostly there already (a rib missing a
    // face or two), never invent a solid from an open sheet.
    let have: f64 = tris.iter().map(|&t| area(&m.tris[t as usize].v)).sum();
    let hull: f64 = out.iter().map(area).sum();
    (hull > 0.0 && have >= MIN_HULL_COVERAGE * hull).then_some((out, rings))
}

/// Existing faces must cover at least this share of the convex hull's area
/// for a shell to be completed.
const MIN_HULL_COVERAGE: f64 = 0.6;

pub fn run_repairs(m: &mut Mesh, steps: &[Box<dyn RepairStep>]) -> Vec<RepairOutcome> {
    let out = steps.iter().map(|s| s.apply(m)).collect();
    m.compact();
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn cube_tris(size: f64, origin: Vec3) -> Vec<SourceTriangle> {
        let s = size;
        let p = |x: f64, y: f64, z: f64| origin + Vec3::new(x * s, y * s, z * s);
        let v = [
            p(0., 0., 0.),
            p(1., 0., 0.),
            p(1., 1., 0.),
            p(0., 1., 0.),
            p(0., 0., 1.),
            p(1., 0., 1.),
            p(1., 1., 1.),
            p(0., 1., 1.),
        ];
        // Outward CCW faces.
        let faces = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [1, 2, 6, 5],
            [0, 4, 7, 3],
        ];
        let mut out = Vec::new();
        for f in faces {
            for t in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
                out.push(SourceTriangle {
                    v: t.map(|i| v[i]),
                    color: 4,
                    oriented: true,
                    file: 0,
                });
            }
        }
        out
    }

    #[test]
    fn closed_cube_is_watertight() {
        let m = Mesh::from_source(&cube_tris(2.0, Vec3::ZERO), 1e-6);
        assert_eq!(m.positions.len(), 8);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert!(d.watertight, "{:?}", d.issues);
        assert!((d.signed_volume - 8.0).abs() < 1e-9);
        assert_eq!(d.components.len(), 1);
        assert_eq!(d.boundary_edges, 0);
    }

    #[test]
    fn open_and_inverted_meshes_are_not_watertight() {
        let mut tris = cube_tris(1.0, Vec3::ZERO);
        tris.pop();
        let d = diagnose(
            &Mesh::from_source(&tris, 1e-6),
            &DiagnosticOptions::default(),
        );
        assert!(!d.watertight);
        assert_eq!(d.boundary_edges, 3);
        assert!(d.errors().any(|i| i.code == "open-boundary"));

        let inv: Vec<_> = cube_tris(1.0, Vec3::ZERO)
            .into_iter()
            .map(|mut t| {
                t.v.swap(1, 2);
                t
            })
            .collect();
        let d = diagnose(
            &Mesh::from_source(&inv, 1e-6),
            &DiagnosticOptions::default(),
        );
        assert!(!d.watertight);
        assert!(d.errors().any(|i| i.code == "volume"));
    }

    #[test]
    fn welding_merges_near_vertices() {
        let mut tris = cube_tris(1.0, Vec3::ZERO);
        tris[0].v[0].x += 1e-5;
        let m = Mesh::from_source(&tris, 1e-3);
        assert_eq!(m.positions.len(), 8);
        let m = Mesh::from_source(&tris, 1e-7);
        assert_eq!(m.positions.len(), 9);
    }

    #[test]
    fn detects_and_removes_degenerate_and_duplicates() {
        let mut tris = cube_tris(1.0, Vec3::ZERO);
        tris.push(tris[0].clone());
        let mut flipped = tris[1].clone();
        flipped.v.swap(1, 2);
        tris.push(flipped);
        let a = Vec3::new(5., 0., 0.);
        tris.push(SourceTriangle {
            v: [a, Vec3::new(6., 0., 0.), Vec3::new(7., 0., 0.)],
            color: 4,
            oriented: true,
            file: 0,
        });
        let mut m = Mesh::from_source(&tris, 1e-6);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert_eq!(d.duplicate_triangles, 1);
        assert_eq!(d.opposite_face_pairs, 1);
        assert_eq!(d.degenerate_triangles, 1);
        assert!(d.nonmanifold_edges > 0);
        run_repairs(
            &mut m,
            &[
                Box::new(RemoveDegenerate { tol: 1e-6 }),
                Box::new(RemoveDuplicates),
            ],
        );
        assert_eq!(m.tris.len(), 13); // 12 + the opposite face (kept: not an exact duplicate)
    }

    #[test]
    fn components_and_intersections() {
        let mut tris = cube_tris(1.0, Vec3::ZERO);
        tris.extend(cube_tris(1.0, Vec3::new(0.5, 0.3, 0.4)));
        tris.extend(cube_tris(1.0, Vec3::new(10., 0., 0.)));
        let d = diagnose(
            &Mesh::from_source(&tris, 1e-6),
            &DiagnosticOptions::default(),
        );
        assert_eq!(d.components.len(), 3);
        assert!(d.intersecting_pairs.unwrap() > 0);
        assert_eq!(d.intersecting_component_pairs, vec![(0, 1)]);
        assert!(d.components.iter().all(|c| c.closed));
    }

    #[test]
    fn t_junction_fix_closes_mesh_without_moving_vertices() {
        // A cube whose top face is split into two quads along x=0.5 while
        // the side faces are not: classic LDraw T-junction.
        let mut tris: Vec<SourceTriangle> = cube_tris(1.0, Vec3::ZERO)
            .into_iter()
            .filter(|t| !t.v.iter().all(|p| p.z == 1.0))
            .collect();
        let p = |x: f64, y: f64| Vec3::new(x, y, 1.0);
        for q in [
            [p(0., 0.), p(0.5, 0.), p(0.5, 1.), p(0., 1.)],
            [p(0.5, 0.), p(1., 0.), p(1., 1.), p(0.5, 1.)],
        ] {
            for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                tris.push(SourceTriangle {
                    v: t,
                    color: 4,
                    oriented: true,
                    file: 0,
                });
            }
        }
        let mut m = Mesh::from_source(&tris, 1e-6);
        let before: Vec<Vec3> = m.positions.clone();
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert!(!d.watertight);
        assert_eq!(d.t_junctions, 2);
        run_repairs(&mut m, &[Box::new(FixTJunctions { tol: 1e-6 })]);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert!(d.watertight, "{:?}", d.issues);
        let mut after = m.positions.clone();
        let mut b = before;
        let k = |v: &Vec3| (v.x, v.y, v.z);
        after.sort_by(|a, c| k(a).partial_cmp(&k(c)).unwrap());
        b.sort_by(|a, c| k(a).partial_cmp(&k(c)).unwrap());
        assert_eq!(after, b);
    }

    #[test]
    fn close_open_shells_caps_and_completes() {
        // Main body: closed cube. Shell A: cube missing its top (planar hole).
        // Shell B: cube missing top and one side (non-planar hole, convex).
        let mut tris = cube_tris(10.0, Vec3::new(0., 0., -20.));
        let a: Vec<SourceTriangle> = cube_tris(1.0, Vec3::new(2., 2., 0.))
            .into_iter()
            .filter(|t| !t.v.iter().all(|p| (p.z - 1.0).abs() < 1e-9))
            .collect();
        let b: Vec<SourceTriangle> = cube_tris(1.0, Vec3::new(5., 2., 0.))
            .into_iter()
            .filter(|t| {
                !t.v.iter().all(|p| (p.z - 1.0).abs() < 1e-9)
                    && !t.v.iter().all(|p| (p.x - 6.0).abs() < 1e-9)
            })
            .collect();
        tris.extend(a);
        tris.extend(b);
        let mut m = Mesh::from_source(&tris, 1e-6);
        assert!(!diagnose(&m, &DiagnosticOptions::default()).closed);
        let before: Vec<Vec3> = m.positions.clone();
        let out = run_repairs(
            &mut m,
            &[Box::new(CloseOpenShells {
                tol: 1e-6,
                anchor: 0.0,
            })],
        );
        assert_eq!(out[0].changed, 2, "{}", out[0].note);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert!(d.watertight, "{:?}", d.issues);
        assert_eq!(d.components.len(), 3);
        assert!((d.signed_volume - (1000.0 + 1.0 + 1.0)).abs() < 1e-9);
        assert_eq!(
            m.positions.len(),
            before.len(),
            "no vertices added or moved"
        );
    }

    #[test]
    fn anchors_fuse_touching_pieces_and_stay_inside() {
        // Main body: closed 10-cube. A rib box sits on its top face but is
        // missing its bottom (the touching face): it gets completed, then
        // anchored down into the body.
        let mut tris = cube_tris(10.0, Vec3::ZERO);
        let rib: Vec<SourceTriangle> = cube_tris(1.0, Vec3::new(4., 4., 10.))
            .into_iter()
            .filter(|t| !t.v.iter().all(|p| (p.z - 10.0).abs() < 1e-9))
            .collect();
        tris.extend(rib);
        let mut m = Mesh::from_source(&tris, 1e-6);
        let n_old = m.positions.len();
        let out = run_repairs(
            &mut m,
            &[Box::new(CloseOpenShells {
                tol: 1e-6,
                anchor: 0.25,
            })],
        );
        assert!(
            out[0].note.contains("added 1 hidden anchors"),
            "{}",
            out[0].note
        );
        assert!(m.positions.len() > n_old);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert!(d.components.iter().all(|c| c.closed), "{:?}", d.issues);
        // The anchor overlaps both: it reaches below the body's top face.
        assert!(m.positions[n_old..].iter().any(|p| p.z < 10.0 - 0.2));
        // Nothing pokes outside the body + rib envelope.
        let b = m.bbox();
        assert!(b.min.z >= -1e-9 && b.max.z <= 11.0 + 1e-9);
    }

    #[test]
    fn anchor_never_pokes_through_a_thin_body() {
        // Main body: a 0.1-thick plate. Anchor depth 0.25 would poke out of
        // its underside, so it must be skipped (the piece is still closed).
        let mut tris: Vec<SourceTriangle> = cube_tris(1.0, Vec3::ZERO)
            .into_iter()
            .map(|mut t| {
                for v in &mut t.v {
                    *v = Vec3::new(v.x * 10.0, v.y * 10.0, v.z * 0.1);
                }
                t
            })
            .collect();
        tris.extend(
            cube_tris(1.0, Vec3::new(4., 4., 0.1))
                .into_iter()
                .filter(|t| !t.v.iter().all(|p| (p.z - 0.1).abs() < 1e-9)),
        );
        let mut m = Mesh::from_source(&tris, 1e-6);
        let out = run_repairs(
            &mut m,
            &[Box::new(CloseOpenShells {
                tol: 1e-6,
                anchor: 0.25,
            })],
        );
        assert!(
            out[0].note.contains("added 0 hidden anchors") && out[0].note.contains("1 skipped"),
            "{}",
            out[0].note
        );
        assert!(m.bbox().min.z >= -1e-9, "nothing below the plate");
        assert!(diagnose(&m, &DiagnosticOptions::default())
            .components
            .iter()
            .all(|c| c.closed));
    }

    #[test]
    fn close_open_shells_leaves_non_convex_open_pieces() {
        // An open L-shaped strip (two faces of an L-prism) has no planar loop
        // and isn't on its convex hull: must be left alone.
        let mut tris = cube_tris(10.0, Vec3::new(0., 0., -20.));
        let q = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            vec![
                SourceTriangle {
                    v: [a, b, c],
                    color: 4,
                    oriented: true,
                    file: 0,
                },
                SourceTriangle {
                    v: [a, c, d],
                    color: 4,
                    oriented: true,
                    file: 0,
                },
            ]
        };
        let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
        tris.extend(q(
            v(0., 0., 0.),
            v(2., 0., 0.),
            v(2., 0., 1.),
            v(0., 0., 1.),
        ));
        tris.extend(q(
            v(2., 0., 0.),
            v(2., 2., 0.5),
            v(2., 2., 1.5),
            v(2., 0., 1.),
        ));
        let mut m = Mesh::from_source(&tris, 1e-6);
        let n0 = m.tris.len();
        let out = run_repairs(
            &mut m,
            &[Box::new(CloseOpenShells {
                tol: 1e-6,
                anchor: 0.0,
            })],
        );
        assert_eq!(out[0].changed, 0, "{}", out[0].note);
        assert_eq!(m.tris.len(), n0);
    }

    #[test]
    fn orient_fixes_uncertified_inverted_cube() {
        let tris: Vec<_> = cube_tris(1.0, Vec3::ZERO)
            .into_iter()
            .enumerate()
            .map(|(i, mut t)| {
                t.oriented = false;
                if i % 3 == 0 {
                    t.v.swap(1, 2);
                }
                t
            })
            .collect();
        let mut m = Mesh::from_source(&tris, 1e-6);
        assert!(diagnose(&m, &DiagnosticOptions::default()).inconsistent_edges > 0);
        run_repairs(&mut m, &[Box::new(OrientComponents)]);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert_eq!(d.inconsistent_edges, 0);
        assert!(d.signed_volume > 0.0);
    }
}
