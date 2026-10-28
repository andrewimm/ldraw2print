//! Minifigure connector detection, fit planning and mating-surface
//! adjustment.
//!
//! # Detection
//!
//! LDraw parts build round connection features from cylinder primitives
//! (`n-4cyli.dat`, usually nested inside `stud4.dat`, `4-4cylo.dat`, …).
//! Every such instance has an exact world transform, which gives its axis,
//! radius and height without any mesh fitting. Instances that are coaxial
//! with equal radius and facing are merged into a [`Candidate`]. The facing
//! (towards or away from the axis) comes from the BFC-normalised triangles,
//! so a hat's socket wall (facing inward) is distinguishable from the
//! outside of a stud (facing outward). Non-certified geometry yields
//! `Facing::Unknown` and is never auto-selected.
//!
//! # Fit
//!
//! The mating size comes from, in order: the profile's
//! `measured_diameter_mm`, its nominal diameter × scale, or the part's own
//! modelled diameter. Clearance (fit preset) and printer compensation are
//! added for sockets / subtracted for pins. Because LDraw cylinders are
//! polygons, the *effective* diameter of a socket is across the flats
//! (`2r·cos(π/n)`) and that of a pin across the vertices; the vertex radius
//! is solved accordingly. Only vertices on the selected cylinder surface are
//! moved, radially; everything else keeps its exact position.

use crate::config::{AxisConstraint, Config, ConnectorProfile, Gender};
use crate::geometry::{triangle_normal, Vec3};
use crate::ldraw::{PrimitiveInstance, Resolved};
use crate::mesh::Mesh;
use anyhow::{bail, Result};
use serde::Serialize;
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Facing {
    /// Normals point towards the axis: the inside wall of a hole.
    Inward,
    /// Normals point away from the axis: the outside of a pin or stud.
    Outward,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CandidateSource {
    Primitive,
    Manual,
}

/// A cylindrical surface patch, in LDraw space (LDU).
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub id: usize,
    pub source: CandidateSource,
    /// Point on the axis closest to the LDraw origin.
    pub axis_origin: Vec3S,
    /// Unit axis direction (vertical axes point LDraw-up, i.e. -Y).
    pub axis_dir: Vec3S,
    pub radius_ldu: f64,
    /// Extent along the axis, measured from `axis_origin`.
    pub h_min: f64,
    pub h_max: f64,
    pub facing: Facing,
    /// Fraction of the full circle covered (1.0 = closed ring).
    pub coverage: f64,
    /// Polygon segments per full circle.
    pub segments: u32,
    pub primitives: Vec<String>,
    /// Mesh vertex indices lying on this surface.
    #[serde(skip)]
    pub vertices: Vec<u32>,
}

/// Serializable Vec3 wrapper.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Vec3S(pub [f64; 3]);
impl From<Vec3> for Vec3S {
    fn from(v: Vec3) -> Self {
        Vec3S(v.to_array())
    }
}
impl Vec3S {
    pub fn v(&self) -> Vec3 {
        Vec3::new(self.0[0], self.0[1], self.0[2])
    }
}

impl Candidate {
    pub fn diameter_ldu(&self) -> f64 {
        self.radius_ldu * 2.0
    }
    pub fn depth_ldu(&self) -> f64 {
        self.h_max - self.h_min
    }
    pub fn axis(&self) -> (Vec3, Vec3) {
        (self.axis_origin.v(), self.axis_dir.v())
    }
    pub fn point_at(&self, h: f64) -> Vec3 {
        self.axis_origin.v() + self.axis_dir.v() * h
    }
    pub fn is_vertical(&self) -> bool {
        self.axis_dir.0[1].abs() > 0.999
    }
    pub fn summary(&self) -> String {
        let o = self.point_at(self.h_min);
        format!(
            "#{}: {} Ø{:.3} LDU, depth {:.3} LDU, LDraw axis ({:.2},{:.2},{:.2}) from ({:.2},{:.2},{:.2}), coverage {:.0}%, {} segs [{}]",
            self.id,
            match self.facing {
                Facing::Inward => "hole wall (inward)",
                Facing::Outward => "pin/stud (outward)",
                Facing::Unknown => "cylinder (facing unknown)",
            },
            self.diameter_ldu(),
            self.depth_ldu(),
            self.axis_dir.0[0],
            self.axis_dir.0[1],
            self.axis_dir.0[2],
            o.x,
            o.y,
            o.z,
            self.coverage * 100.0,
            self.segments,
            self.primitives.join(", ")
        )
    }
}

/// Parse `[48/|8/]n-dcyli[2].dat` → (fraction of circle, segments per circle).
pub fn cylinder_primitive(name: &str) -> Option<(f64, u32)> {
    let (segs, base) = if let Some(b) = name.strip_prefix("48/") {
        (48, b)
    } else if let Some(b) = name.strip_prefix("8/") {
        (8, b)
    } else {
        (16, name)
    };
    let stem = base
        .strip_suffix("cyli.dat")
        .or_else(|| base.strip_suffix("cyli2.dat"))?;
    let (n, d) = stem.split_once('-')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    if d <= 0.0 || n <= 0.0 || n > d {
        return None;
    }
    Some((n / d, segs))
}

fn canonical_axis(d: Vec3) -> Vec3 {
    let d = d.normalized();
    let (ax, ay, az) = (d.x.abs(), d.y.abs(), d.z.abs());
    let flip = if ay >= ax && ay >= az {
        d.y > 0.0
    } else if ax >= az {
        d.x < 0.0
    } else {
        d.z < 0.0
    };
    if flip {
        -d
    } else {
        d
    }
}

fn perpendicular_basis(axis: Vec3) -> (Vec3, Vec3) {
    let helper = if axis.x.abs() < 0.9 {
        Vec3::new(1., 0., 0.)
    } else {
        Vec3::new(0., 0., 1.)
    };
    let e1 = axis.cross(helper).normalized();
    let e2 = axis.cross(e1).normalized();
    (e1, e2)
}

const BINS: usize = 72;

struct Patch {
    origin: Vec3,
    dir: Vec3,
    radius: f64,
    h0: f64,
    h1: f64,
    facing: Facing,
    bins: [bool; BINS],
    segments: u32,
    name: String,
}

fn patch_from_instance(p: &PrimitiveInstance, resolved: &Resolved) -> Option<Patch> {
    let (frac, authored) = cylinder_primitive(&p.name)?;
    let segments = p.segments.unwrap_or(authored);
    let t = &p.transform;
    let cx = t.column(0);
    let cy = t.column(1);
    let cz = t.column(2);
    let (rx, rz) = (cx.length(), cz.length());
    let height = cy.length();
    if rx < 1e-9 || height < 1e-9 || (rx - rz).abs() > 1e-3 * rx {
        return None; // elliptical or degenerate
    }
    if cx.normalized().dot(cz.normalized()).abs() > 1e-3
        || cx.normalized().dot(cy.normalized()).abs() > 1e-3
        || cz.normalized().dot(cy.normalized()).abs() > 1e-3
    {
        return None; // sheared / oblique
    }
    let dir = canonical_axis(cy);
    let base = t.t;
    let top = t.apply(Vec3::new(0., 1., 0.));
    // Axis point closest to the world origin.
    let origin = base - dir * base.dot(dir);
    let (ha, hb) = ((base - origin).dot(dir), (top - origin).dot(dir));

    let facing = if !p.oriented {
        Facing::Unknown
    } else {
        let mut score = 0.0;
        let mut area = 0.0;
        for tri in &resolved.triangles[p.tri_start..p.tri_end] {
            let n = triangle_normal(tri.v[0], tri.v[1], tri.v[2]);
            let a = n.length();
            if a == 0.0 {
                continue;
            }
            let c = (tri.v[0] + tri.v[1] + tri.v[2]) * (1.0 / 3.0);
            let radial = c - origin - dir * (c - origin).dot(dir);
            score += (n * (1.0 / a)).dot(radial.normalized()) * a;
            area += a;
        }
        if area == 0.0 {
            return None;
        }
        let s = score / area;
        if s > 0.5 {
            Facing::Outward
        } else if s < -0.5 {
            Facing::Inward
        } else {
            Facing::Unknown
        }
    };

    let (e1, e2) = perpendicular_basis(dir);
    let mut bins = [false; BINS];
    let steps = 256;
    for k in 0..=steps {
        let th = 2.0 * PI * frac * k as f64 / steps as f64;
        let w = t.apply_linear(Vec3::new(th.cos(), 0.0, th.sin()));
        let ang = w.dot(e2).atan2(w.dot(e1)).rem_euclid(2.0 * PI);
        bins[((ang / (2.0 * PI)) * BINS as f64) as usize % BINS] = true;
    }
    Some(Patch {
        origin,
        dir,
        radius: rx,
        h0: ha.min(hb),
        h1: ha.max(hb),
        facing,
        bins,
        segments,
        name: p.name.clone(),
    })
}

/// Find cylindrical connector candidates from primitive instances, then
/// attach the mesh vertices lying on each surface.
pub fn detect_candidates(resolved: &Resolved, mesh: &Mesh) -> Vec<Candidate> {
    let patches: Vec<Patch> = resolved
        .primitives
        .iter()
        .filter_map(|p| patch_from_instance(p, resolved))
        .collect();
    let tol = 0.01;
    // Group coaxial, equal-radius, equal-facing patches.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, p) in patches.iter().enumerate() {
        let g = groups.iter_mut().find(|g| {
            let q = &patches[g[0]];
            q.facing == p.facing
                && q.dir.dot(p.dir) > 0.9999
                && (q.origin - p.origin).length() < tol
                && (q.radius - p.radius).abs() < tol
        });
        match g {
            Some(g) => g.push(i),
            None => groups.push(vec![i]),
        }
    }
    let mut out = Vec::new();
    for g in groups {
        // Split each group into runs of touching/overlapping height ranges.
        let mut idx = g.clone();
        idx.sort_by(|&a, &b| patches[a].h0.total_cmp(&patches[b].h0));
        let mut runs: Vec<Vec<usize>> = Vec::new();
        let mut run_end = f64::NEG_INFINITY;
        for i in idx {
            if runs.is_empty() || patches[i].h0 > run_end + tol {
                runs.push(vec![i]);
                run_end = patches[i].h1;
            } else {
                runs.last_mut().unwrap().push(i);
                run_end = run_end.max(patches[i].h1);
            }
        }
        for run in runs {
            let first = &patches[run[0]];
            let mut bins = [false; BINS];
            let mut names: Vec<String> = Vec::new();
            let (mut h0, mut h1) = (f64::INFINITY, f64::NEG_INFINITY);
            let mut segments = u32::MAX;
            for &i in &run {
                let p = &patches[i];
                for (b, &pb) in bins.iter_mut().zip(p.bins.iter()) {
                    *b |= pb;
                }
                h0 = h0.min(p.h0);
                h1 = h1.max(p.h1);
                segments = segments.min(p.segments);
                if !names.contains(&p.name) {
                    names.push(p.name.clone());
                }
            }
            let mut c = Candidate {
                id: 0,
                source: CandidateSource::Primitive,
                axis_origin: first.origin.into(),
                axis_dir: first.dir.into(),
                radius_ldu: first.radius,
                h_min: h0,
                h_max: h1,
                facing: first.facing,
                coverage: bins.iter().filter(|b| **b).count() as f64 / BINS as f64,
                segments,
                primitives: names,
                vertices: Vec::new(),
            };
            c.vertices = surface_vertices(mesh, &c, tol);
            out.push(c);
        }
    }
    // Largest, fully closed rings first; stable ids.
    out.sort_by(|a, b| {
        b.coverage
            .total_cmp(&a.coverage)
            .then(b.radius_ldu.total_cmp(&a.radius_ldu))
            .then(a.h_min.total_cmp(&b.h_min))
    });
    for (i, c) in out.iter_mut().enumerate() {
        c.id = i;
    }
    out
}

/// Mesh vertices of the candidate's cylinder surface: within `tol` of it and
/// used by at least one triangle lying entirely on it. Vertices of other
/// geometry that merely touch the circle (e.g. a rib corner) are excluded,
/// so a fit never drags unrelated parts along.
pub fn surface_vertices(mesh: &Mesh, c: &Candidate, tol: f64) -> Vec<u32> {
    let (o, d) = c.axis();
    let on: Vec<bool> = mesh
        .positions
        .iter()
        .map(|p| {
            let rel = *p - o;
            let h = rel.dot(d);
            let r = (rel - d * h).length();
            (r - c.radius_ldu).abs() <= tol && h >= c.h_min - tol && h <= c.h_max + tol
        })
        .collect();
    let mut used = vec![false; mesh.positions.len()];
    for t in &mesh.tris {
        if t.v.iter().all(|&v| on[v as usize]) {
            for &v in &t.v {
                used[v as usize] = true;
            }
        }
    }
    (0..mesh.positions.len() as u32)
        .filter(|&v| used[v as usize])
        .collect()
}

/// User-specified cylinder (for parts whose connector is not built from
/// primitives). Coordinates in LDraw space / LDU.
#[derive(Debug, Clone)]
pub struct ManualConnector {
    pub point: Vec3,
    pub dir: Vec3,
    pub radius_ldu: f64,
    /// Axial range relative to `point`; None = all vertices on the cylinder.
    pub range: Option<(f64, f64)>,
}

pub fn manual_candidate(mesh: &Mesh, m: &ManualConnector, id: usize) -> Result<Candidate> {
    let dir = canonical_axis(m.dir);
    let origin = m.point - dir * m.point.dot(dir);
    let off = (m.point - origin).dot(dir);
    let (h0, h1) = match m.range {
        Some((a, b)) => (off + a.min(b), off + a.max(b)),
        None => (f64::NEG_INFINITY, f64::INFINITY),
    };
    let mut c = Candidate {
        id,
        source: CandidateSource::Manual,
        axis_origin: origin.into(),
        axis_dir: dir.into(),
        radius_ldu: m.radius_ldu,
        h_min: h0,
        h_max: h1,
        facing: Facing::Unknown,
        coverage: 0.0,
        segments: 0,
        primitives: vec![],
        vertices: vec![],
    };
    let tol = 0.02_f64.max(m.radius_ldu * 0.002);
    c.vertices = surface_vertices(mesh, &c, tol);
    if c.vertices.len() < 6 {
        bail!(
            "manual connector: only {} mesh vertices lie on a cylinder of radius {} LDU around that axis{}. \
             Check the axis point, the radius, and --connector-range (distances along --connector-dir; \
             with the default up direction a socket rising 4 LDU from the point is 0,4), or omit the range.",
            c.vertices.len(),
            m.radius_ldu,
            m.range.map(|(a, b)| format!(" within range {a},{b}")).unwrap_or_default()
        );
    }
    // Derive extent, angular coverage and segment count from the vertex
    // angles: the typical gap is one segment; a much larger gap is an opening.
    let (e1, e2) = perpendicular_basis(dir);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut angles: Vec<f64> = Vec::new();
    for &v in &c.vertices {
        let rel = mesh.positions[v as usize] - origin;
        let h = rel.dot(dir);
        lo = lo.min(h);
        hi = hi.max(h);
        angles.push(rel.dot(e2).atan2(rel.dot(e1)).rem_euclid(2.0 * PI));
    }
    c.h_min = lo;
    c.h_max = hi;
    angles.sort_by(|a, b| a.total_cmp(b));
    angles.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    let mut gaps: Vec<f64> = angles.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.push(angles[0] + 2.0 * PI - angles[angles.len() - 1]);
    let mut sorted = gaps.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let typical = sorted[sorted.len() / 2].max(1e-6);
    let open: f64 = gaps.iter().filter(|g| **g > typical * 2.5).sum();
    c.coverage = (1.0 - open / (2.0 * PI)).clamp(0.0, 1.0);
    c.segments = (2.0 * PI / typical).round() as u32;
    c.facing = facing_from_mesh(mesh, &c);
    Ok(c)
}

/// Facing of triangles whose vertices all lie on the candidate surface.
pub fn facing_from_mesh(mesh: &Mesh, c: &Candidate) -> Facing {
    let set: crate::det::HashSet<u32> = c.vertices.iter().copied().collect();
    let (o, d) = c.axis();
    let mut score = 0.0;
    let mut area = 0.0;
    let mut all_oriented = true;
    for t in &mesh.tris {
        if !t.v.iter().all(|v| set.contains(v)) {
            continue;
        }
        let p = mesh.tri_positions(t);
        let n = triangle_normal(p[0], p[1], p[2]);
        let a = n.length();
        if a == 0.0 {
            continue;
        }
        all_oriented &= t.oriented;
        let cen = (p[0] + p[1] + p[2]) * (1.0 / 3.0);
        let radial = (cen - o) - d * (cen - o).dot(d);
        score += (n * (1.0 / a)).dot(radial.normalized()) * a;
        area += a;
    }
    if area == 0.0 || !all_oriented {
        return Facing::Unknown;
    }
    match score / area {
        s if s > 0.5 => Facing::Outward,
        s if s < -0.5 => Facing::Inward,
        _ => Facing::Unknown,
    }
}

/// Why a candidate does or does not match a profile.
pub fn match_profile(p: &ConnectorProfile, c: &Candidate) -> std::result::Result<(), String> {
    let want = match p.gender {
        Gender::Socket => Facing::Inward,
        Gender::Pin => Facing::Outward,
    };
    if c.facing == Facing::Unknown {
        return Err("surface facing unknown (non-BFC geometry)".into());
    }
    if c.facing != want {
        return Err(format!("faces {:?}, profile needs {:?}", c.facing, want).to_lowercase());
    }
    if p.axis == AxisConstraint::Vertical && !c.is_vertical() {
        return Err("axis not vertical".into());
    }
    if c.coverage < p.min_coverage {
        return Err(format!(
            "covers {:.0}% of a circle, profile needs {:.0}%",
            c.coverage * 100.0,
            p.min_coverage * 100.0
        ));
    }
    if let Some(nd) = p.nominal_diameter_ldu {
        let rel = (c.diameter_ldu() - nd).abs() / nd;
        if rel > p.match_tolerance {
            return Err(format!(
                "Ø{:.2} LDU differs from nominal Ø{nd} LDU by {:.0}%",
                c.diameter_ldu(),
                rel * 100.0
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Selection {
    pub candidate: usize,
    /// "auto" (unique profile match) or "explicit" (user chose).
    pub how: &'static str,
    pub warnings: Vec<String>,
}

/// Choose the candidate for a profile. Auto-selection only happens when
/// exactly one candidate matches; otherwise the user must choose.
pub fn select(
    profile_name: &str,
    p: &ConnectorProfile,
    cands: &[Candidate],
    explicit: Option<usize>,
) -> Result<Selection> {
    if let Some(id) = explicit {
        let Some(c) = cands.iter().find(|c| c.id == id) else {
            bail!(
                "connector candidate #{id} does not exist ({} candidates found)",
                cands.len()
            );
        };
        let mut warnings = Vec::new();
        if let Err(why) = match_profile(p, c) {
            warnings.push(format!("candidate #{id} does not match profile '{profile_name}': {why}; using it because it was chosen explicitly"));
        }
        return Ok(Selection {
            candidate: id,
            how: "explicit",
            warnings,
        });
    }
    let matches: Vec<&Candidate> = cands
        .iter()
        .filter(|c| match_profile(p, c).is_ok())
        .collect();
    let listing = || {
        if cands.is_empty() {
            "  (no cylindrical primitive surfaces found; use --connector-axis/--connector-radius to specify one)".to_string()
        } else {
            cands
                .iter()
                .map(|c| {
                    let m = match match_profile(p, c) {
                        Ok(()) => "MATCH".to_string(),
                        Err(e) => format!("no: {e}"),
                    };
                    format!("  {}\n      → {m}", c.summary())
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    match matches.len() {
        1 => Ok(Selection { candidate: matches[0].id, how: "auto", warnings: vec![] }),
        0 if cands.is_empty() => Err(crate::errors::DecisionNeeded(
            "no cylindrical connector surface was detected in this part (its socket isn't built from LDraw cylinder \
             primitives). Describe it by hand: --connector-axis X,Y,Z --connector-radius R [--connector-range A,B] \
             (LDraw coordinates; see `ldraw2print guide connectors`)."
                .to_string(),
        )
        .into()),
        0 => Err(crate::errors::DecisionNeeded(format!(
            "no connector surface in this part matches profile '{profile_name}'. Candidates:\n{}\n\
             Choose one explicitly with --connector-candidate <id>, or describe it with --connector-axis.",
            listing()
        ))
        .into()),
        n => Err(crate::errors::DecisionNeeded(format!(
            "{n} surfaces match profile '{profile_name}'; refusing to guess. Candidates:\n{}\n\
             Choose one with --connector-candidate <id>.",
            listing()
        ))
        .into()),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FitPlan {
    pub connector: String,
    pub gender: Gender,
    pub fit: String,
    /// Diametral clearance from the fit preset (or override), mm.
    pub clearance_mm: f64,
    /// Printer compensation applied, mm (diametral).
    pub compensation_mm: f64,
    /// Diameter of the mating feature on the figure, mm.
    pub mating_diameter_mm: f64,
    /// "measured", "nominal" or "part geometry".
    pub mating_source: String,
    /// Diameter of the surface as modelled in LDraw (vertex circle), mm.
    pub modelled_diameter_mm: f64,
    /// Effective diameter as modelled: across flats for a socket, across
    /// vertices for a pin.
    pub modelled_effective_diameter_mm: f64,
    /// Requested effective diameter after adjustment, mm.
    pub target_effective_diameter_mm: f64,
    /// Vertex-circle radius that achieves the target, mm.
    pub target_vertex_radius_mm: f64,
    pub segments: u32,
    pub warnings: Vec<String>,
}

pub fn plan_fit(
    cfg: &Config,
    name: &str,
    p: &ConnectorProfile,
    c: &Candidate,
    fit: &str,
    clearance_override: Option<f64>,
    scale: f64,
) -> Result<FitPlan> {
    let mm_per_ldu = crate::geometry::mm_per_ldu_for_scale(scale);
    let mut warnings = Vec::new();
    let clearance = match clearance_override {
        Some(v) => v,
        None => cfg.fit_clearance(name, fit)?,
    };
    let (mating, source) = if let Some(m) = p.measured_diameter_mm {
        if let Some(s) = p.measured_at_scale {
            if (s - scale).abs() > 1e-9 {
                warnings.push(format!(
                    "measured_diameter_mm was measured at scale {s} but converting at scale {scale}; the measurement is used as-is"
                ));
            }
        }
        (m, "measured")
    } else if let Some(nd) = p.nominal_diameter_ldu {
        if p.calibration.is_some() {
            warnings.push(format!(
                "'{name}': using the nominal Ø{nd} LDU × scale stud size with your calibrated clearance (the coupons were sized the same way)"
            ));
        } else {
            warnings.push(format!(
                "no measured_diameter_mm for '{name}': using nominal Ø{nd} LDU × scale. Measure your figure and calibrate before trusting the fit."
            ));
        }
        (nd * mm_per_ldu, "nominal")
    } else {
        warnings.push(format!("profile '{name}' has no nominal or measured diameter: using the part's own modelled diameter"));
        (c.diameter_ldu() * mm_per_ldu, "part geometry")
    };
    if let Some(rec) = &p.calibration {
        if (rec.scale - scale).abs() > 1e-9 {
            warnings.push(format!(
                "'{name}' was calibrated at {}:1 but this conversion is at {scale}:1; print new coupons at this scale",
                rec.scale
            ));
        }
    }
    let n = c.segments.max(3) as f64;
    let flats = (PI / n).cos();
    let r_mm = c.radius_ldu * mm_per_ldu;
    let (comp, effective_now, target, vertex_r) = match p.gender {
        Gender::Socket => {
            let comp = cfg.printer.hole_compensation_mm;
            let t = mating + clearance + comp;
            (comp, 2.0 * r_mm * flats, t, t / 2.0 / flats)
        }
        Gender::Pin => {
            let comp = cfg.printer.pin_compensation_mm;
            let t = mating - clearance - comp;
            (comp, 2.0 * r_mm, t, t / 2.0)
        }
    };
    if target <= 0.0 {
        bail!("fit for '{name}' produces a non-positive diameter ({target:.3} mm)");
    }
    if c.segments <= 16 {
        warnings.push(format!(
            "surface is a {}-sided polygon: across-flats and across-corners differ by {:.2} mm; the target is applied to the {} diameter",
            c.segments,
            2.0 * r_mm * (1.0 - flats),
            if p.gender == Gender::Socket { "across-flats (narrowest)" } else { "across-corners (widest)" }
        ));
    }
    Ok(FitPlan {
        connector: name.to_string(),
        gender: p.gender,
        fit: match (clearance_override, fit) {
            (Some(_), _) => "custom".to_string(),
            (None, "clearance" | "default") => format!("{} (profile default)", p.default_fit),
            (None, f) => f.to_string(),
        },
        clearance_mm: clearance,
        compensation_mm: comp,
        mating_diameter_mm: mating,
        mating_source: source.to_string(),
        modelled_diameter_mm: 2.0 * r_mm,
        modelled_effective_diameter_mm: effective_now,
        target_effective_diameter_mm: target,
        target_vertex_radius_mm: vertex_r,
        segments: c.segments,
        warnings,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct FitReport {
    pub moved_vertices: usize,
    pub radial_change_mm: f64,
    pub max_displacement_mm: f64,
}

/// Move the candidate's surface vertices radially to the planned radius.
/// The mesh is in LDraw space (LDU). Fails without modifying the mesh if
/// any adjacent triangle would flip (wall too thin for the change).
pub fn apply_fit(
    mesh: &mut Mesh,
    c: &Candidate,
    plan: &FitPlan,
    mm_per_ldu: f64,
) -> Result<FitReport> {
    let (o, d) = c.axis();
    let new_r = plan.target_vertex_radius_mm / mm_per_ldu;
    let mut moved = mesh.positions.clone();
    let mut max_disp: f64 = 0.0;
    for &v in &c.vertices {
        let p = mesh.positions[v as usize];
        let h = (p - o).dot(d);
        let axis_pt = o + d * h;
        let radial = (p - axis_pt).normalized();
        let np = axis_pt + radial * new_r;
        max_disp = max_disp.max((np - p).length());
        moved[v as usize] = np;
    }
    // Refuse only if the moving surface would pass through geometry that
    // stays put: a neighbouring (unmoved) vertex lying between the old and
    // new radius. (Triangle flips alone mis-fire on harmless slivers.)
    let set: crate::det::HashSet<u32> = c.vertices.iter().copied().collect();
    let old_r = c.radius_ldu;
    let (lo, hi) = if new_r > old_r {
        (old_r, new_r)
    } else {
        (new_r, old_r)
    };
    let tol = 1e-6_f64.max(1e-4 * old_r);
    let mut crossed: crate::det::HashSet<u32> = crate::det::HashSet::default();
    for t in &mesh.tris {
        if !t.v.iter().any(|v| set.contains(v)) {
            continue;
        }
        for &v in t.v.iter().filter(|v| !set.contains(v)) {
            let p = mesh.positions[v as usize];
            let h = (p - o).dot(d);
            let r = ((p - o) - d * h).length();
            if r > lo + tol && r < hi - tol && h >= c.h_min - tol && h <= c.h_max + tol {
                crossed.insert(v);
            }
        }
    }
    if !crossed.is_empty() {
        bail!(
            "fit adjustment would pass through {} vertices of the surrounding wall (wall too thin for a {:.3} mm radial change); not applied",
            crossed.len(),
            (new_r - c.radius_ldu) * mm_per_ldu
        );
    }
    mesh.positions = moved;
    Ok(FitReport {
        moved_vertices: c.vertices.len(),
        radial_change_mm: (new_r - c.radius_ldu) * mm_per_ldu,
        max_displacement_mm: max_disp * mm_per_ldu,
    })
}

/// Indices of triangles fully on the candidate surface (attachment region).
pub fn region_triangles(mesh: &Mesh, c: &Candidate) -> Vec<u32> {
    let set: crate::det::HashSet<u32> = c.vertices.iter().copied().collect();
    mesh.tris
        .iter()
        .enumerate()
        .filter(|(_, t)| t.v.iter().all(|v| set.contains(v)))
        .map(|(i, _)| i as u32)
        .collect()
}

/// Which end of a socket is open: returns the axial coordinate of the mouth.
/// A cap (disc) leaves a vertex on the axis at the closed end.
pub fn socket_mouth(mesh: &Mesh, c: &Candidate) -> f64 {
    let (o, d) = c.axis();
    let capped = |h: f64| {
        let ctr = o + d * h;
        mesh.positions
            .iter()
            .any(|p| (*p - ctr).length() < c.radius_ldu * 0.3)
    };
    match (capped(c.h_min), capped(c.h_max)) {
        (false, true) => c.h_min,
        (true, false) => c.h_max,
        // Default for vertical sockets: opening at the bottom.
        _ => c.h_min,
    }
}

/// Transparent reference geometry for the preview, LDraw space (LDU).
#[derive(Debug, Clone, Serialize)]
pub struct ReferenceShape {
    pub label: String,
    /// Centre of one end cap.
    pub base: Vec3S,
    /// Unit direction from base along the shape.
    pub dir: Vec3S,
    pub length: f64,
    pub radius: f64,
    /// Inner radius for tubes (0 = solid cylinder).
    pub inner_radius: f64,
}

pub fn reference_shapes(
    mesh: &Mesh,
    p: &ConnectorProfile,
    c: &Candidate,
    mating_d_ldu: f64,
) -> Vec<ReferenceShape> {
    let (_, d) = c.axis();
    let mut out = Vec::new();
    match p.gender {
        Gender::Socket => {
            let mouth = socket_mouth(mesh, c);
            // Direction from the mouth into the socket.
            let inward = if (mouth - c.h_min).abs() < 1e-9 {
                d
            } else {
                -d
            };
            let depth = p.nominal_depth_ldu.unwrap_or(c.depth_ldu());
            let base = c.point_at(mouth);
            out.push(ReferenceShape {
                label: "mating stud/post".into(),
                base: base.into(),
                dir: inward.into(),
                length: depth,
                radius: mating_d_ldu / 2.0,
                inner_radius: 0.0,
            });
            if let Some(b) = &p.reference_body {
                out.push(ReferenceShape {
                    label: "reference body".into(),
                    base: base.into(),
                    dir: (-inward).into(),
                    length: b.length_ldu,
                    radius: b.diameter_ldu / 2.0,
                    inner_radius: 0.0,
                });
            }
        }
        Gender::Pin => {
            let len = p
                .nominal_depth_ldu
                .unwrap_or(mating_d_ldu)
                .min(c.depth_ldu());
            let mid = (c.h_min + c.h_max) / 2.0;
            out.push(ReferenceShape {
                label: "mating grip".into(),
                base: c.point_at(mid - len / 2.0).into(),
                dir: d.into(),
                length: len,
                radius: mating_d_ldu / 2.0 * 1.6,
                inner_radius: mating_d_ldu / 2.0,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cylinder_primitive_names() {
        assert_eq!(cylinder_primitive("4-4cyli.dat"), Some((1.0, 16)));
        assert_eq!(cylinder_primitive("1-4cyli.dat"), Some((0.25, 16)));
        assert_eq!(cylinder_primitive("48/1-4cyli.dat"), Some((0.25, 48)));
        assert_eq!(cylinder_primitive("8/4-4cyli2.dat"), Some((1.0, 8)));
        assert_eq!(cylinder_primitive("4-4cylo.dat"), None);
        assert_eq!(cylinder_primitive("4-4disc.dat"), None);
    }

    #[test]
    fn canonical_axis_points_up_for_vertical() {
        assert_eq!(
            canonical_axis(Vec3::new(0., 4., 0.)),
            Vec3::new(0., -1., 0.)
        );
        assert_eq!(
            canonical_axis(Vec3::new(0., -2., 0.)),
            Vec3::new(0., -1., 0.)
        );
        assert_eq!(
            canonical_axis(Vec3::new(-3., 0., 0.)),
            Vec3::new(1., 0., 0.)
        );
    }
}
