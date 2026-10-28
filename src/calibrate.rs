//! Calibration coupons: small, fast prints for test-fitting one connector
//! at several clearances before committing to a full accessory.
//!
//! * Socket profiles → a short tube whose bore is the target socket
//!   diameter; press it onto the figure's stud / neck post.
//! * Pin profiles → a pin of the target diameter on a small base; push it
//!   into the figure's hand.
//!
//! Each coupon is a closed surface of revolution (watertight by
//! construction). Coupons are identified by V-notches on their outer rim:
//! coupon *k* (1-based, in the order listed in the summary) has *k* notches.

use crate::config::{Config, Gender};
use crate::geometry::{mm_per_ldu_for_scale, Vec3};
use crate::ldraw::SourceTriangle;
use anyhow::{bail, Result};
use serde::Serialize;
use std::f64::consts::PI;

#[derive(Debug, Clone, Serialize)]
pub struct Coupon {
    pub index: usize,
    pub notches: usize,
    pub label: String,
    pub clearance_mm: f64,
    /// Effective mating diameter of the coupon (bore for sockets, pin OD).
    pub diameter_mm: f64,
    pub position_mm: [f64; 2],
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationSet {
    pub connector: String,
    pub gender: Gender,
    pub scale: f64,
    pub mating_diameter_mm: f64,
    pub mating_source: String,
    pub compensation_mm: f64,
    pub coupons: Vec<Coupon>,
    #[serde(skip)]
    pub triangles: Vec<SourceTriangle>,
}

const SEGMENTS: usize = 144;

/// Ring of a lathe profile: radius, height, whether notches cut into it.
#[derive(Clone, Copy)]
struct Ring {
    r: f64,
    z: f64,
    notched: bool,
}

fn notch_factor(angle: f64, notches: usize) -> bool {
    // Notches spaced 30° apart starting at 0°, each 8° wide.
    (0..notches).any(|k| {
        let c = (k as f64 * 30.0).to_radians();
        let d = (angle - c + PI).rem_euclid(2.0 * PI) - PI;
        d.abs() <= 4f64.to_radians()
    })
}

/// Revolve a profile around +Z. `closed_loop` joins the last ring to the
/// first (a tube); otherwise both ends are capped to the axis.
fn revolve(
    rings: &[Ring],
    closed_loop: bool,
    notches: usize,
    notch_depth: f64,
    offset: Vec3,
    color: u32,
) -> Vec<SourceTriangle> {
    let n = SEGMENTS;
    let ang = |k: usize| 2.0 * PI * k as f64 / n as f64;
    let pt = |ring: &Ring, k: usize| {
        let a = ang(k % n);
        let r = if ring.notched && notch_factor(a, notches) {
            ring.r - notch_depth
        } else {
            ring.r
        };
        offset + Vec3::new(r * a.cos(), r * a.sin(), ring.z)
    };
    let mut tris = Vec::new();
    let mut push = |v: [Vec3; 3]| {
        tris.push(SourceTriangle {
            v,
            color,
            oriented: true,
            file: 0,
        })
    };
    let pairs = if closed_loop {
        rings.len()
    } else {
        rings.len() - 1
    };
    for i in 0..pairs {
        let (a, b) = (&rings[i], &rings[(i + 1) % rings.len()]);
        for k in 0..n {
            let (p0, p1, q0, q1) = (pt(a, k), pt(a, k + 1), pt(b, k), pt(b, k + 1));
            push([p0, p1, q1]);
            push([p0, q1, q0]);
        }
    }
    if !closed_loop {
        for (ring, flip) in [(&rings[0], false), (&rings[rings.len() - 1], true)] {
            let c = offset + Vec3::new(0., 0., ring.z);
            for k in 0..n {
                let (p0, p1) = (pt(ring, k), pt(ring, k + 1));
                push(if flip { [c, p0, p1] } else { [c, p1, p0] });
            }
        }
    }
    // Make outward-facing regardless of profile direction.
    let vol: f64 = tris
        .iter()
        .map(|t| (t.v[0] - offset).dot((t.v[1] - offset).cross(t.v[2] - offset)))
        .sum();
    if vol < 0.0 {
        for t in &mut tris {
            t.v.swap(1, 2);
        }
    }
    tris
}

pub struct CalibrationRequest<'a> {
    pub connector: &'a str,
    pub scale: f64,
    /// Explicit clearances (mm); defaults to the loose/normal/tight presets.
    pub clearances: Option<Vec<f64>>,
    /// Mating diameter from a part's detected geometry (mm), used only when
    /// the profile has neither measured nor nominal diameter.
    pub part_diameter_mm: Option<f64>,
    pub wall_mm: Option<f64>,
    pub height_mm: Option<f64>,
}

pub fn generate(cfg: &Config, req: &CalibrationRequest) -> Result<CalibrationSet> {
    let p = cfg.profile(req.connector)?;
    let mm = mm_per_ldu_for_scale(req.scale);
    let (mating, source) = match (p.measured_diameter_mm, p.nominal_diameter_ldu, req.part_diameter_mm) {
        (Some(m), _, _) => (m, "measured"),
        (None, Some(n), _) => (n * mm, "nominal"),
        (None, None, Some(d)) => (d, "part geometry"),
        _ => bail!("profile '{}' has no measured or nominal diameter; pass --part to measure it from a part", req.connector),
    };
    let entries: Vec<(String, f64)> = match &req.clearances {
        Some(v) if !v.is_empty() => v.iter().map(|c| (format!("{c:+.2} mm"), *c)).collect(),
        _ => ["loose", "normal", "tight"]
            .iter()
            .map(|f| Ok((f.to_string(), cfg.fit_clearance(req.connector, f)?)))
            .collect::<Result<_>>()?,
    };
    let depth = p.nominal_depth_ldu.map(|d| d * mm).unwrap_or(mating * 0.4);
    let height = req.height_mm.unwrap_or(depth.max(3.0));
    let wall = req.wall_mm.unwrap_or((mating * 0.15).max(2.4));
    let notch_depth = (wall * 0.35).min(1.2);
    let flats = (PI / SEGMENTS as f64).cos();

    let mut coupons = Vec::new();
    let mut triangles = Vec::new();
    let mut x = 0.0;
    for (i, (label, clearance)) in entries.iter().enumerate() {
        let notches = i + 1;
        let (tris, d_eff, footprint) = match p.gender {
            Gender::Socket => {
                let comp = cfg.printer.hole_compensation_mm;
                let bore = mating + clearance + comp;
                let ri = bore / 2.0 / flats;
                let ro = ri + wall;
                let rings = [
                    Ring {
                        r: ri,
                        z: 0.0,
                        notched: false,
                    },
                    Ring {
                        r: ro,
                        z: 0.0,
                        notched: true,
                    },
                    Ring {
                        r: ro,
                        z: height,
                        notched: true,
                    },
                    Ring {
                        r: ri,
                        z: height,
                        notched: false,
                    },
                ];
                (
                    revolve(
                        &rings,
                        true,
                        notches,
                        notch_depth,
                        Vec3::new(x + ro, 0., 0.),
                        4,
                    ),
                    bore,
                    2.0 * ro,
                )
            }
            Gender::Pin => {
                let comp = cfg.printer.pin_compensation_mm;
                let pin = mating - clearance - comp;
                if pin <= 0.0 {
                    bail!("clearance {clearance} mm leaves no pin");
                }
                let rp = pin / 2.0;
                let rb = rp + wall.max(rp);
                let hb = 2.0_f64.max(wall * 0.8);
                let rings = [
                    Ring {
                        r: rb,
                        z: 0.0,
                        notched: true,
                    },
                    Ring {
                        r: rb,
                        z: hb,
                        notched: true,
                    },
                    Ring {
                        r: rp,
                        z: hb,
                        notched: false,
                    },
                    Ring {
                        r: rp,
                        z: hb + height.max(pin),
                        notched: false,
                    },
                ];
                (
                    revolve(
                        &rings,
                        false,
                        notches,
                        notch_depth,
                        Vec3::new(x + rb, 0., 0.),
                        4,
                    ),
                    pin,
                    2.0 * rb,
                )
            }
        };
        coupons.push(Coupon {
            index: i,
            notches,
            label: label.clone(),
            clearance_mm: *clearance,
            diameter_mm: d_eff,
            position_mm: [x + footprint / 2.0, 0.0],
        });
        triangles.extend(tris);
        x += footprint + 5.0;
    }
    Ok(CalibrationSet {
        connector: req.connector.to_string(),
        gender: p.gender,
        scale: req.scale,
        mating_diameter_mm: mating,
        mating_source: source.to_string(),
        compensation_mm: match p.gender {
            Gender::Socket => cfg.printer.hole_compensation_mm,
            Gender::Pin => cfg.printer.pin_compensation_mm,
        },
        coupons,
        triangles,
    })
}

impl CalibrationSet {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Calibration set for '{}' ({:?}) at {}:1\nMating diameter: {:.3} mm ({}), printer compensation {:.3} mm\n\n",
            self.connector, self.gender, self.scale, self.mating_diameter_mm, self.mating_source, self.compensation_mm
        );
        s.push_str("notches  fit        clearance  effective Ø\n");
        for c in &self.coupons {
            s.push_str(&format!(
                "{:>7}  {:<9}  {:>+8.3}  {:>9.3} mm{}\n",
                c.notches,
                c.label,
                c.clearance_mm,
                c.diameter_mm,
                if self.gender == Gender::Socket {
                    " bore"
                } else {
                    " pin"
                }
            ));
        }
        s.push_str(&format!(
            "\nPrint, then test each coupon on your figure. Record the winner, e.g. if coupon 2 fit best:\n\n  \
             ldraw2print calibrate --connector {c} --scale {sc}{cl} --record 2 --printer \"…\" --material \"…\"\n\n\
             `--fit clearance` then uses that coupon's clearance. If every coupon is uniformly too\n\
             tight or loose, adjust [printer] compensation in the config instead.\n",
            c = self.connector,
            sc = self.scale,
            cl = if self.coupons.iter().map(|c| c.label.as_str()).eq(["loose", "normal", "tight"]) {
                String::new()
            } else {
                format!(" --clearances {}", self.coupons.iter().map(|c| format!("{}", c.clearance_mm)).collect::<Vec<_>>().join(","))
            }
        ));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{diagnose, DiagnosticOptions, Mesh};

    #[test]
    fn socket_coupons_are_watertight_with_expected_bore() {
        let cfg = Config::builtin();
        let set = generate(
            &cfg,
            &CalibrationRequest {
                connector: "head-stud",
                scale: 5.0,
                clearances: None,
                part_diameter_mm: None,
                wall_mm: None,
                height_mm: None,
            },
        )
        .unwrap();
        assert_eq!(set.coupons.len(), 3);
        assert_eq!(set.mating_source, "nominal");
        assert!((set.mating_diameter_mm - 24.0).abs() < 1e-9);
        assert!((set.coupons[0].diameter_mm - (24.0 + cfg.fit_presets.loose)).abs() < 1e-9);
        let m = Mesh::from_source(&set.triangles, 1e-6);
        let d = diagnose(&m, &DiagnosticOptions::default());
        assert!(d.watertight, "{:?}", d.issues);
        assert_eq!(d.components.len(), 3);

        // Bore across flats equals the requested diameter.
        let c0 = &set.coupons[0];
        let ctr = Vec3::new(c0.position_mm[0], 0., 0.);
        let min_r = m
            .positions
            .iter()
            .filter(|p| (p.x - ctr.x).abs() < 20.0)
            .map(|p| ((p.x - ctr.x).powi(2) + p.y.powi(2)).sqrt())
            .fold(f64::INFINITY, f64::min);
        let flats = (PI / SEGMENTS as f64).cos();
        assert!((2.0 * min_r * flats - c0.diameter_mm).abs() < 1e-6);
    }

    #[test]
    fn pin_coupons_are_watertight() {
        let cfg = Config::builtin();
        let set = generate(
            &cfg,
            &CalibrationRequest {
                connector: "hand-grip",
                scale: 5.0,
                clearances: Some(vec![0.0, 0.1]),
                part_diameter_mm: None,
                wall_mm: None,
                height_mm: None,
            },
        )
        .unwrap();
        assert_eq!(set.coupons.len(), 2);
        assert!((set.coupons[1].diameter_mm - 15.9).abs() < 1e-9);
        let d = diagnose(
            &Mesh::from_source(&set.triangles, 1e-6),
            &DiagnosticOptions::default(),
        );
        assert!(d.watertight, "{:?}", d.issues);
    }
}
