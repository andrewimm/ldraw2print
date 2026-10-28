//! Human-readable reports for `convert` and `inspect`.

use crate::connectors::Facing;
use crate::mesh::Severity;
use crate::pipeline::BuildOutput;
use std::fmt::Write;

pub fn summary(out: &BuildOutput, scale: f64, show_candidates: bool) -> String {
    let mut s = String::new();
    let d = &out.diagnostics;
    let b = out.mesh.bbox();
    let size = b.size();
    let _ = writeln!(s, "{} — {}", out.name, out.description);
    let _ = writeln!(
        s,
        "Scale {scale}:1 ({:.3} mm per LDU), Z up, millimetres",
        out.mm_per_ldu
    );
    let _ = writeln!(
        s,
        "Size  {:.2} × {:.2} × {:.2} mm (X × Y × Z)",
        size.x, size.y, size.z
    );
    let _ = writeln!(
        s,
        "Mesh  {} triangles ({} from LDraw), {} vertices, {} component(s), {} files",
        d.triangles,
        out.source_triangles,
        d.vertices,
        d.components.len(),
        out.files.len()
    );
    for r in &out.repairs {
        if r.changed > 0 {
            let _ = writeln!(s, "Repair {}", r.note);
        }
    }
    let _ = writeln!(
        s,
        "Watertight: {}{}",
        if d.watertight { "yes" } else { "NO" },
        if d.watertight {
            String::new()
        } else {
            format!(
                " (volume estimate {:.2} cm³ is unreliable)",
                d.signed_volume / 1000.0
            )
        }
    );
    if d.watertight {
        let _ = writeln!(s, "Volume {:.3} cm³", d.signed_volume / 1000.0);
    }
    for sev in [Severity::Error, Severity::Warning, Severity::Info] {
        for i in d.issues.iter().filter(|i| i.severity == sev) {
            let tag = match sev {
                Severity::Error => "error  ",
                Severity::Warning => "warning",
                Severity::Info => "info   ",
            };
            let _ = writeln!(s, "  {tag} {}", i.message);
        }
    }
    if !out.uncertified_files.is_empty() {
        let _ = writeln!(
            s,
            "  note    non-BFC-certified files: {}",
            out.uncertified_files.join(", ")
        );
    }
    for m in &out.missing {
        let _ = writeln!(s, "  MISSING {} (from {})", m.name, m.referenced_from);
    }
    for w in &out.warnings {
        let _ = writeln!(s, "  warning {w}");
    }

    let v = verdict(out);
    match v.status {
        "ready" => {
            let _ = writeln!(s, "Verdict: ready to slice");
        }
        _ => {
            let _ = writeln!(s, "Verdict: review");
            for r in &v.reasons {
                let _ = writeln!(s, "  - {r}");
            }
        }
    }
    for n in &v.notes {
        let _ = writeln!(s, "  note: {n}");
    }
    if !out.connector_suggestions.is_empty() {
        let list: Vec<String> = out
            .connector_suggestions
            .iter()
            .map(|g| {
                format!(
                    "{} (Ø{:.0} LDU{})",
                    g.profile,
                    g.diameter_ldu,
                    if g.unique {
                        ""
                    } else {
                        ", several surfaces: needs --connector-candidate"
                    }
                )
            })
            .collect();
        let _ = writeln!(
            s,
            "Connector profiles whose shape matches (geometric, check which one you mean): {}",
            list.join(", ")
        );
    }
    if let Some(c) = &out.connector {
        let _ = writeln!(s, "\nConnector profile '{}'", c.profile);
        // When selection failed, the error below already lists candidates.
        if show_candidates && c.selection_error.is_none() {
            if c.candidates.is_empty() {
                let _ = writeln!(s, "  no cylindrical primitive surfaces found");
            }
            for k in &c.candidates {
                let mark = match &c.match_reasons.get(k.id) {
                    Some(None) => "match".to_string(),
                    Some(Some(r)) => format!("no: {r}"),
                    None => String::new(),
                };
                let _ = writeln!(s, "  {}\n      → {mark}", k.summary());
            }
        }
        if let Some(e) = &c.selection_error {
            let _ = writeln!(s, "  {e}");
        }
        if let (Some(sel), Some(p)) = (&c.selection, &c.plan) {
            let k = &c.candidates[sel.candidate];
            let _ = writeln!(
                s,
                "  Surface #{} ({}): {} Ø{:.3} LDU, depth {:.3} LDU",
                sel.candidate,
                sel.how,
                match k.facing {
                    Facing::Inward => "socket",
                    Facing::Outward => "pin",
                    Facing::Unknown => "cylinder",
                },
                k.diameter_ldu(),
                k.depth_ldu()
            );
            let _ = writeln!(
                s,
                "  Modelled    Ø{:.3} mm across vertices, Ø{:.3} mm effective ({}-gon)",
                p.modelled_diameter_mm, p.modelled_effective_diameter_mm, p.segments
            );
            let _ = writeln!(
                s,
                "  Mating      Ø{:.3} mm ({})",
                p.mating_diameter_mm, p.mating_source
            );
            match &c.fit {
                Some(f) => {
                    let _ = writeln!(
                        s,
                        "  Fit         {} clearance {:.3} mm + compensation {:.3} mm → target Ø{:.3} mm effective",
                        p.fit, p.clearance_mm, p.compensation_mm, p.target_effective_diameter_mm
                    );
                    let _ = writeln!(
                        s,
                        "  Applied     moved {} vertices radially by {:+.3} mm; all other geometry unchanged",
                        f.moved_vertices, f.radial_change_mm
                    );
                }
                None => {
                    let _ = writeln!(
                        s,
                        "  Fit         not applied (modelled geometry preserved). With --fit {} the target would be Ø{:.3} mm",
                        p.fit, p.target_effective_diameter_mm
                    );
                }
            }
            let _ = writeln!(s, "  Compatibility is NOT guaranteed: print `ldraw2print calibrate --connector {}` first.", c.profile);
        }
    }
    s
}

/// One-line printability verdict shared by the text report and `--json`.
/// "ready" means the geometry should slice cleanly; it says nothing about
/// whether a connector fits (that needs calibration).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Verdict {
    /// "ready" or "review".
    pub status: &'static str,
    /// Why it needs review (empty when ready).
    pub reasons: Vec<String>,
    /// Things worth knowing that don't block slicing.
    pub notes: Vec<String>,
}

pub fn verdict(out: &BuildOutput) -> Verdict {
    let d = &out.diagnostics;
    let mut reasons = Vec::new();
    let mut notes = Vec::new();
    if !out.missing.is_empty() {
        reasons.push(format!(
            "{} referenced LDraw files are missing; geometry is incomplete",
            out.missing.len()
        ));
    }
    if let Some(i) = d.issues.iter().find(|i| i.code == "open-shells") {
        let tried = out.repairs.iter().any(|r| r.step == "close-open-shells");
        reasons.push(if tried {
            format!(
                "{} — --close-open-shells could not close them safely (see repairs); check these areas in the slicer preview",
                i.message.split(';').next().unwrap_or(&i.message)
            )
        } else {
            "separate open pieces (ribs, bosses) may be dropped by the slicer: add --close-open-shells".into()
        });
    }
    if let Some(added) = out.smoothing_open_edges_added.filter(|&a| a > 0.0) {
        reasons.push(format!("--smooth opened {added} mm of new seams on this part: build without --smooth and compare"));
    }
    if !d.watertight {
        reasons.push(format!(
            "not watertight ({} open, {} non-manifold edges); many LDraw parts still slice fine, check the slicer preview or `inspect --problems 50`",
            d.boundary_edges, d.nonmanifold_edges
        ));
    }
    if d.intersecting_pairs.unwrap_or(0) > 0 {
        notes.push("overlapping closed pieces: slicers merge them, no action needed".into());
    }
    if let Some(c) = &out.connector {
        if let Some(p) = &c.plan {
            if c.fit.is_some() && p.mating_source == "nominal" {
                notes.push(format!(
                    "fit for '{}' uses the nominal mating size, not a measurement; print calibration coupons to confirm",
                    c.profile
                ));
            }
        }
        if c.selection.is_none() {
            notes.push(format!(
                "connector '{}' was not resolved: see candidates",
                c.profile
            ));
        }
    }
    Verdict {
        status: if reasons.is_empty() {
            "ready"
        } else {
            "review"
        },
        reasons,
        notes,
    }
}
