//! Layer check: cut the print mesh the way a slicer does and find layers
//! whose outlines don't close.
//!
//! A slicer cuts each layer, chains the cut segments into loops and drops
//! any chain that doesn't close, after joining chain ends that are close
//! together (PrusaSlicer, and so Bambu Studio and OrcaSlicer, bridge gaps of
//! up to about 2 mm). Where a part is open (a petal or leaf drawn without the
//! face where it meets the body), whole regions vanish from those layers
//! even though the slicer reports no error. This finds them without a slicer.
//!
//! An open chain only matters where its area isn't already solid on that
//! layer: a crack inside solid material prints fine. The area at risk is
//! therefore the area an open chain would enclose, counting only open
//! chains that lie mostly outside the layer's closed outlines.

use crate::det::HashMap;
use crate::geometry::Vec3;
use crate::mesh::Mesh;
use serde::Serialize;

/// Open chains enclosing less than this (mm²) are ignored as noise.
const MIN_AREA_MM2: f64 = 1.0;
/// Open chains narrower than this on average (2 × area / perimeter, mm) are
/// slivers no nozzle prints faithfully either way; ignored.
const MIN_WIDTH_MM: f64 = 1.0;

#[derive(Debug, Clone, Serialize)]
pub struct LayerIssue {
    /// Height of the cut (mid-layer), mm.
    pub z_mm: f64,
    /// Outlines on this layer that don't close.
    pub open_outlines: usize,
    /// Area those outlines would enclose, outside the layer's solid, mm².
    pub area_at_risk_mm2: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerCheck {
    pub layer_height_mm: f64,
    pub closing_radius_mm: f64,
    pub layers: usize,
    /// Layers where a slicer is likely to drop part of the outline.
    pub bad_layers: Vec<LayerIssue>,
    /// Height ranges (bottom, top of layer) covered by `bad_layers`.
    pub ranges_mm: Vec<[f64; 2]>,
    pub max_area_at_risk_mm2: f64,
}

impl LayerCheck {
    pub fn is_clean(&self) -> bool {
        self.bad_layers.is_empty()
    }

    /// "z 8.6–9.8 mm, 12.0–12.4 mm" for messages.
    pub fn ranges_text(&self) -> String {
        self.ranges_mm
            .iter()
            .map(|r| format!("{:.1}–{:.1} mm", r[0], r[1]))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Cut `m` (print space, mm, Z up) at the middle of every layer. With
/// `cut_layers` false (a closed mesh, which can't have open outlines) only
/// the layer count is filled in.
pub fn check(m: &Mesh, layer_height: f64, closing_radius: f64, cut_layers: bool) -> LayerCheck {
    let mut out = LayerCheck {
        layer_height_mm: layer_height,
        closing_radius_mm: closing_radius,
        layers: 0,
        bad_layers: Vec::new(),
        ranges_mm: Vec::new(),
        max_area_at_risk_mm2: 0.0,
    };
    if m.tris.is_empty() || layer_height <= 0.0 {
        return out;
    }
    let (z0, z1) = m
        .positions
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p.z), hi.max(p.z))
        });
    out.layers = ((z1 - z0) / layer_height).ceil().max(0.0) as usize;
    if !cut_layers {
        return out;
    }
    for i in 0..out.layers {
        let z = z0 + (i as f64 + 0.5) * layer_height;
        let (closed, open) = cut(m, z, closing_radius);
        let mut n = 0;
        let mut area = 0.0;
        for c in &open {
            let a = polygon_area(c).abs();
            if a < MIN_AREA_MM2 || 2.0 * a / perimeter(c) < MIN_WIDTH_MM {
                continue;
            }
            // Mostly inside solid outlines on this layer: nothing is lost.
            let inside = c.iter().filter(|p| winding(&closed, **p) != 0).count();
            if inside * 10 >= c.len() * 9 {
                continue;
            }
            n += 1;
            area += a;
        }
        if n > 0 {
            out.max_area_at_risk_mm2 = out.max_area_at_risk_mm2.max(area);
            out.bad_layers.push(LayerIssue {
                z_mm: round3(z),
                open_outlines: n,
                area_at_risk_mm2: (area * 10.0).round() / 10.0,
            });
            let (lo, hi) = (z - layer_height / 2.0, z + layer_height / 2.0);
            match out.ranges_mm.last_mut() {
                Some(r) if (r[1] - lo).abs() < 1e-9 => r[1] = round3(hi),
                _ => out.ranges_mm.push([round3(lo), round3(hi)]),
            }
        }
    }
    out.max_area_at_risk_mm2 = (out.max_area_at_risk_mm2 * 10.0).round() / 10.0;
    out
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

type Pt = (f64, f64);

/// Closed loops and open chains of the cross-section at `z`. Loops run
/// counter-clockwise around solid (seen from above) for an outward-facing
/// mesh. Chains whose ends are within `closing` of each other are joined.
fn cut(m: &Mesh, z: f64, closing: f64) -> (Vec<Vec<Pt>>, Vec<Vec<Pt>>) {
    // Each crossing triangle gives a segment from one crossed edge to
    // another; chain segments through the edges they share.
    let below = |i: u32| m.positions[i as usize].z < z;
    let mut point: HashMap<(u32, u32), Pt> = HashMap::default();
    let mut next: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::default();
    let mut ends: HashMap<(u32, u32), usize> = HashMap::default();
    for t in &m.tris {
        let [a, b, c] = t.v;
        let mut start = None;
        let mut end = None;
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let (bu, bv) = (below(u), below(v));
            if bu == bv {
                continue;
            }
            let key = (u.min(v), u.max(v));
            point.entry(key).or_insert_with(|| {
                let (p, q): (Vec3, Vec3) = (m.positions[u as usize], m.positions[v as usize]);
                let s = (z - p.z) / (q.z - p.z);
                (p.x + s * (q.x - p.x), p.y + s * (q.y - p.y))
            });
            // Walking the triangle's (outward) winding, the edge that goes
            // from above to below starts the segment; that puts solid on
            // the left, so outer loops run counter-clockwise.
            if bu {
                end = Some(key);
            } else {
                start = Some(key);
            }
        }
        if let (Some(s), Some(e)) = (start, end) {
            next.entry(s).or_default().push(e);
            *ends.entry(e).or_default() += 1;
        }
    }
    let mut starts: Vec<(u32, u32)> = next.keys().copied().collect();
    starts.sort_unstable();
    // Open chains begin where nothing ends; walk those first, then cycles.
    let mut order: Vec<(u32, u32)> = starts
        .iter()
        .copied()
        .filter(|s| !ends.contains_key(s))
        .collect();
    order.extend(starts.iter().copied());
    let mut closed = Vec::new();
    let mut open: Vec<Vec<Pt>> = Vec::new();
    for s in order {
        if next.get(&s).is_none_or(|v| v.is_empty()) {
            continue;
        }
        let mut chain = vec![s];
        let mut cur = s;
        let mut is_closed = false;
        while let Some(e) = next.get_mut(&cur).and_then(|v| v.pop()) {
            if e == s {
                is_closed = true;
                break;
            }
            chain.push(e);
            cur = e;
        }
        let pts: Vec<Pt> = chain.iter().map(|k| point[k]).collect();
        if is_closed {
            closed.push(pts);
        } else {
            open.push(pts);
        }
    }
    // Join chain ends within the closing radius, nearest gap first (a
    // chain's own ends close it).
    let gap = |a: Pt, b: Pt| (a.0 - b.0).hypot(a.1 - b.1);
    loop {
        let mut best: Option<(f64, usize, usize)> = None;
        for (i, c) in open.iter().enumerate() {
            let last = *c.last().unwrap();
            for (j, d) in open.iter().enumerate() {
                if i == j && c.len() < 3 {
                    continue;
                }
                let g = gap(last, d[0]);
                if g <= closing && best.is_none_or(|(bg, _, _)| g < bg) {
                    best = Some((g, i, j));
                }
            }
        }
        let Some((_, i, j)) = best else { break };
        if i == j {
            closed.push(open.remove(i));
        } else {
            let tail = open[j].clone();
            open[i].extend(tail);
            open.remove(j);
        }
    }
    (closed, open)
}

fn perimeter(p: &[Pt]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % n]);
            (a.0 - b.0).hypot(a.1 - b.1)
        })
        .sum()
}

fn polygon_area(p: &[Pt]) -> f64 {
    let n = p.len();
    0.5 * (0..n)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f64>()
}

/// Winding number of `q` with respect to the closed loops.
fn winding(loops: &[Vec<Pt>], q: Pt) -> i32 {
    let mut w = 0;
    for l in loops {
        let n = l.len();
        for i in 0..n {
            let (a, b) = (l[i], l[(i + 1) % n]);
            let side = (b.0 - a.0) * (q.1 - a.1) - (q.0 - a.0) * (b.1 - a.1);
            if a.1 <= q.1 {
                if b.1 > q.1 && side > 0.0 {
                    w += 1;
                }
            } else if b.1 <= q.1 && side < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::tests::cube_tris;

    #[test]
    fn closed_box_has_no_bad_layers_and_ccw_outlines() {
        let m = Mesh::from_source(&cube_tris(10.0, Vec3::ZERO), 1e-6);
        let r = check(&m, 0.2, 2.0, true);
        assert_eq!(r.layers, 50);
        assert!(r.is_clean(), "{:?}", r.bad_layers);
        let (closed, open) = cut(&m, 5.0, 2.0);
        assert!(open.is_empty());
        assert_eq!(closed.len(), 1);
        assert!(
            (polygon_area(&closed[0]) - 100.0).abs() < 1e-9,
            "outer loop is CCW"
        );
    }

    #[test]
    fn open_box_side_is_reported_with_its_height_range() {
        // A box missing one side face between z 0 and 10: every layer's
        // outline is open (a U shape), enclosing the whole 10 × 10 square.
        let tris: Vec<_> = cube_tris(10.0, Vec3::ZERO)
            .into_iter()
            .filter(|t| !t.v.iter().all(|p| p.x.abs() < 1e-9))
            .collect();
        let m = Mesh::from_source(&tris, 1e-6);
        let r = check(&m, 0.5, 2.0, true);
        assert_eq!(r.bad_layers.len(), 20);
        assert_eq!(r.ranges_mm, vec![[0.0, 10.0]]);
        assert!((r.max_area_at_risk_mm2 - 100.0).abs() < 1e-6);
        assert!(r.ranges_text().contains("0.0–10.0 mm"));
    }

    #[test]
    fn open_piece_inside_solid_is_not_at_risk() {
        // A small open box (missing a side) entirely inside a big closed box:
        // the slicer drops it, but that area is solid anyway.
        let mut tris = cube_tris(20.0, Vec3::ZERO);
        tris.extend(
            cube_tris(4.0, Vec3::new(8.0, 8.0, 8.0))
                .into_iter()
                .filter(|t| !t.v.iter().all(|p| (p.x - 8.0).abs() < 1e-9)),
        );
        let m = Mesh::from_source(&tris, 1e-6);
        assert!(check(&m, 0.5, 2.0, true).is_clean());
    }

    #[test]
    fn gaps_within_the_closing_radius_are_closed() {
        // Two open half-boxes facing each other across a 0.01 mm gap: each
        // layer is two U-shaped chains whose ends almost meet.
        let half = |x0: f64, open_x: f64| -> Vec<crate::ldraw::SourceTriangle> {
            crate::mesh::tests::cube_tris(1.0, Vec3::ZERO)
                .into_iter()
                .map(|mut t| {
                    for p in &mut t.v {
                        *p = Vec3::new(x0 + p.x * 5.0, p.y * 10.0, p.z * 10.0);
                    }
                    t
                })
                .filter(|t| !t.v.iter().all(|p| (p.x - open_x).abs() < 1e-9))
                .collect()
        };
        let mut tris = half(0.0, 5.0);
        tris.extend(half(5.01, 5.01));
        let m = Mesh::from_source(&tris, 1e-6);
        assert!(check(&m, 1.0, 0.049, true).is_clean());
        assert!(check(&m, 1.0, 2.0, true).is_clean());
        assert_eq!(check(&m, 1.0, 0.001, true).bad_layers.len(), 10);
    }
}
