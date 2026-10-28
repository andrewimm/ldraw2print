//! The single geometry pipeline shared by `convert`, `inspect` and the
//! preview server, so the preview always shows exactly what gets exported.
//!
//! resolve LDraw → weld → safe cleanup → optional repairs → connector
//! detection / fit (LDraw space) → convert to print space (mm, Z up) →
//! placement → diagnostics.

use crate::config::Config;
use crate::connectors::{
    self, Candidate, FitPlan, FitReport, ManualConnector, ReferenceShape, Selection,
};
use crate::geometry::{ldraw_dir_to_print, ldraw_to_print, mm_per_ldu_for_scale, Vec3};
use crate::ldraw::{Library, MissingRef};
use crate::mesh::{self, DiagnosticOptions, Diagnostics, Mesh, RepairOutcome, RepairStep};
use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Lowest point on Z=0, centred in X/Y (ready for the build plate).
    Bed,
    /// Keep the LDraw origin (useful for aligning several exports).
    Ldraw,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub input: String,
    pub scale: f64,
    pub color: Option<u32>,
    pub connector: Option<String>,
    /// loose | normal | tight | clearance. None = keep modelled geometry.
    pub fit: Option<String>,
    pub clearance_mm: Option<f64>,
    pub candidate: Option<usize>,
    pub manual: Option<ManualConnector>,
    pub placement: Placement,
    pub cleanup: bool,
    pub fix_t_junctions: Option<bool>,
    pub orient: Option<bool>,
    pub allow_missing: bool,
    pub check_intersections: bool,
    /// Rotate 180° about X (print upside down), e.g. hats crown-down.
    pub flip: bool,
    /// Re-tessellate circular primitives to this many segments per circle.
    pub smooth: Option<u32>,
    /// Close separate open shells (ribs, bosses) so slicers keep them.
    pub close_open_shells: Option<bool>,
}

impl BuildOptions {
    pub fn new(input: &str, scale: f64) -> Self {
        BuildOptions {
            input: input.to_string(),
            scale,
            color: None,
            connector: None,
            fit: None,
            clearance_mm: None,
            candidate: None,
            manual: None,
            placement: Placement::Bed,
            cleanup: true,
            fix_t_junctions: None,
            orient: None,
            allow_missing: false,
            check_intersections: true,
            flip: false,
            smooth: None,
            close_open_shells: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorOutcome {
    pub profile: String,
    /// All detected cylindrical candidates (LDraw space, LDU).
    pub candidates: Vec<Candidate>,
    pub selection: Option<Selection>,
    /// Why selection failed (the build continues without fit changes only
    /// when no fit was requested).
    pub selection_error: Option<String>,
    pub plan: Option<FitPlan>,
    pub fit: Option<FitReport>,
    /// Triangles on the attachment surface (indices into the final mesh).
    pub region_triangles: Vec<u32>,
    /// Reference shapes, print space (mm).
    pub reference: Vec<ReferenceShape>,
    /// Selected surface summary in print space.
    pub selected_mm: Option<SelectedMm>,
    /// Per candidate (by id): None if it matches the profile, else why not.
    #[serde(skip)]
    pub match_reasons: Vec<Option<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectedMm {
    pub diameter_mm: f64,
    pub across_flats_mm: f64,
    pub depth_mm: f64,
    pub axis_point_mm: [f64; 3],
    pub axis_dir: [f64; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorSuggestion {
    pub profile: String,
    /// Candidate ids that match this profile.
    pub candidates: Vec<usize>,
    /// Exactly one match: `--connector PROFILE` will select it automatically.
    pub unique: bool,
    pub diameter_ldu: f64,
}

pub struct BuildOutput {
    pub name: String,
    pub description: String,
    pub mm_per_ldu: f64,
    /// Final mesh in print space (mm, Z up).
    pub mesh: Mesh,
    pub components: Vec<u32>,
    /// LDraw edge lines in print space.
    pub edges: Vec<[Vec3; 2]>,
    pub files: Vec<String>,
    pub missing: Vec<MissingRef>,
    pub uncertified_files: Vec<String>,
    /// Source triangles drawn in the main colour (affected by --color).
    pub main_color_tris: usize,
    pub warnings: Vec<String>,
    pub repairs: Vec<RepairOutcome>,
    pub diagnostics: Diagnostics,
    pub connector: Option<ConnectorOutcome>,
    /// Without --connector: profiles whose detection finds a matching surface.
    pub connector_suggestions: Vec<ConnectorSuggestion>,
    /// With --smooth: length (mm) of new open seams smoothing created.
    pub smoothing_open_edges_added: Option<f64>,
    /// Translation applied after the LDraw → print conversion (mm).
    pub placement_offset: Vec3,
    pub source_triangles: usize,
}

pub fn build(lib: &mut Library, cfg: &Config, opt: &BuildOptions) -> Result<BuildOutput> {
    if opt.scale.is_nan() || opt.scale <= 0.0 {
        bail!("scale must be positive");
    }
    let mm = mm_per_ldu_for_scale(opt.scale);
    let root = lib.load_root(&opt.input)?;
    let color = opt.color.unwrap_or(cfg.default_color);
    let mut resolved = lib.resolve(&root, color)?;
    if !resolved.missing.is_empty() && !opt.allow_missing {
        let list: Vec<String> = resolved
            .missing
            .iter()
            .map(|m| format!("  {} (referenced from {})", m.name, m.referenced_from))
            .collect();
        return Err(crate::errors::MissingFiles(format!(
            "{} referenced file(s) could not be found; the output would be missing geometry:\n{}\n\
             Check the LDraw library path, or pass --allow-missing to continue anyway.",
            resolved.missing.len(),
            list.join("\n")
        ))
        .into());
    }
    let mut warnings = resolved.warnings.clone();
    if lib.colors.source.contains("built-in") {
        warnings.push("LDConfig.ldr not found: using a small built-in colour table".into());
    }

    let weld = cfg.mesh.weld_tolerance_ldu;
    let smooth = opt.smooth.or(cfg.mesh.smooth_segments).filter(|&n| n > 0);
    let mut smoothing = None;
    if let Some(n) = smooth {
        if n % 16 != 0 {
            bail!("--smooth must be a multiple of 16 (48 or 96 also smooth p/48 primitives)");
        }
        // Open edges before smoothing, to detect seams smoothing opens.
        let before = open_seam_length(&Mesh::from_source(&resolved.triangles, weld));
        smoothing = Some((crate::smooth::smooth_primitives(&mut resolved, n), before));
    }
    let mut mesh = Mesh::from_source(&resolved.triangles, weld);
    let mut smooth_outcomes = Vec::new();
    let mut smoothing_open_edges_added = None;
    if let Some(((rep, arcs), open_before)) = &smoothing {
        let filled = crate::smooth::fill_arc_cracks(&mut mesh, arcs, weld);
        let open_after = open_seam_length(&mesh);
        // Subdividing an existing open seam adds edges but not length; only
        // genuinely new gaps add length (allow 2% for the curve vs chord).
        let added = open_after - open_before * 1.02;
        let added_mm = (added.max(0.0) * mm * 10.0).round() / 10.0;
        smoothing_open_edges_added = Some(added_mm);
        if added_mm > 0.0 {
            warnings.push(format!(
                "--smooth opened {added_mm} mm of new seams on this part (open seam length {:.1} → {:.1} mm), where smoothed curves meet \
                 pieces that can't be smoothed ({}); compare with a build without --smooth",
                open_before * mm,
                open_after * mm,
                if rep.skipped.is_empty() { "see the smooth repair note".to_string() } else { rep.skipped.iter().take(4).map(|(n, _)| n.clone()).collect::<Vec<_>>().join(", ") }
            ));
        }
        smooth_outcomes.push(RepairOutcome {
            step: "smooth".into(),
            changed: rep.instances_smoothed,
            note: format!(
                "smoothed {} circular primitives to {} segments ({} → {} triangles), {} left as authored{}; re-curved {} hand-modelled faces lying on smoothed surfaces; moved {} touching vertices onto them; closed {filled} chord/arc slivers",
                rep.instances_smoothed,
                rep.segments,
                rep.triangles_before,
                rep.triangles_after,
                rep.instances_skipped,
                if rep.skipped.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", rep.skipped.iter().map(|(n, c)| format!("{c}× {n}")).collect::<Vec<_>>().join(", "))
                },
                rep.conformed_triangles,
                rep.snapped_vertices
            ),
        });
    }

    let mut steps: Vec<Box<dyn RepairStep>> = Vec::new();
    if opt.cleanup {
        steps.push(Box::new(mesh::RemoveDegenerate { tol: weld * 0.1 }));
        steps.push(Box::new(mesh::RemoveDuplicates));
    }
    if opt.fix_t_junctions.unwrap_or(cfg.mesh.fix_t_junctions) {
        steps.push(Box::new(mesh::FixTJunctions { tol: weld }));
    }
    if opt.orient.unwrap_or(cfg.mesh.orient) {
        steps.push(Box::new(mesh::OrientComponents));
    }
    if opt.close_open_shells.unwrap_or(cfg.mesh.close_open_shells) {
        steps.push(Box::new(mesh::CloseOpenShells {
            tol: weld,
            anchor: 0.25,
        }));
    }
    let mut repairs = smooth_outcomes;
    repairs.extend(mesh::run_repairs(&mut mesh, &steps));

    // Connector handling happens in LDraw space where primitives live.
    let mut connector = None;
    let mut connector_suggestions = Vec::new();
    if opt.connector.is_none() {
        let cands = connectors::detect_candidates(&resolved, &mesh);
        for (name, profile) in &cfg.connectors {
            let ids: Vec<usize> = cands
                .iter()
                .filter(|c| connectors::match_profile(profile, c).is_ok())
                .map(|c| c.id)
                .collect();
            if let Some(&first) = ids.first() {
                connector_suggestions.push(ConnectorSuggestion {
                    profile: name.clone(),
                    unique: ids.len() == 1,
                    diameter_ldu: cands[first].diameter_ldu(),
                    candidates: ids,
                });
            }
        }
    }
    if let Some(name) = &opt.connector {
        let profile = cfg.profile(name)?;
        let mut candidates = connectors::detect_candidates(&resolved, &mesh);
        let mut explicit = opt.candidate;
        if let Some(m) = &opt.manual {
            let id = candidates.len();
            candidates.push(connectors::manual_candidate(&mesh, m, id)?);
            explicit = Some(id);
        }
        let mut out = ConnectorOutcome {
            profile: name.clone(),
            candidates,
            selection: None,
            selection_error: None,
            plan: None,
            fit: None,
            region_triangles: vec![],
            reference: vec![],
            selected_mm: None,
            match_reasons: vec![],
        };
        out.match_reasons = out
            .candidates
            .iter()
            .map(|k| connectors::match_profile(profile, k).err())
            .collect();
        match connectors::select(name, profile, &out.candidates, explicit) {
            Ok(sel) => {
                warnings.extend(sel.warnings.iter().cloned());
                let c = out
                    .candidates
                    .iter()
                    .find(|c| c.id == sel.candidate)
                    .unwrap()
                    .clone();
                let fit_name = opt.fit.clone().filter(|f| f != "none");
                let plan_fit = fit_name.as_deref().unwrap_or("clearance");
                let plan = connectors::plan_fit(
                    cfg,
                    name,
                    profile,
                    &c,
                    plan_fit,
                    opt.clearance_mm,
                    opt.scale,
                )?;
                if fit_name.is_some() || opt.clearance_mm.is_some() {
                    warnings.extend(plan.warnings.iter().cloned());
                    out.fit = Some(connectors::apply_fit(&mut mesh, &c, &plan, mm)?);
                }
                let mating_ldu = plan.mating_diameter_mm / mm;
                out.reference = connectors::reference_shapes(&mesh, profile, &c, mating_ldu);
                out.region_triangles = connectors::region_triangles(&mesh, &c);
                let r_now = if out.fit.is_some() {
                    plan.target_vertex_radius_mm
                } else {
                    c.radius_ldu * mm
                };
                let flats = (std::f64::consts::PI / c.segments.max(3) as f64).cos();
                out.selected_mm = Some(SelectedMm {
                    diameter_mm: 2.0 * r_now,
                    across_flats_mm: 2.0 * r_now * flats,
                    depth_mm: c.depth_ldu() * mm,
                    axis_point_mm: flip_if(ldraw_to_print(c.point_at(c.h_min), mm), opt.flip)
                        .to_array(),
                    axis_dir: flip_if(ldraw_dir_to_print(c.axis_dir.v()), opt.flip).to_array(),
                });
                out.plan = Some(plan);
                out.selection = Some(sel);
            }
            Err(e) => {
                if opt.fit.as_deref().is_some_and(|f| f != "none") || opt.clearance_mm.is_some() {
                    return Err(e);
                }
                out.selection_error = Some(e.to_string());
            }
        }
        connector = Some(out);
    }

    // LDraw → print space (optionally upside down).
    let flip = opt.flip;
    let to_print = |p: Vec3| flip_if(ldraw_to_print(p, mm), flip);
    mesh.transform(to_print);
    let mut edges: Vec<[Vec3; 2]> = resolved
        .edges
        .iter()
        .map(|e| [to_print(e.v[0]), to_print(e.v[1])])
        .collect();
    let offset = match opt.placement {
        Placement::Ldraw => Vec3::ZERO,
        Placement::Bed => {
            let b = mesh.bbox();
            if b.is_empty() {
                Vec3::ZERO
            } else {
                let c = b.center();
                Vec3::new(-c.x, -c.y, -b.min.z)
            }
        }
    };
    mesh.transform(|p| p + offset);
    for e in &mut edges {
        e[0] = e[0] + offset;
        e[1] = e[1] + offset;
    }
    if let Some(c) = &mut connector {
        for s in &mut c.reference {
            s.base = (to_print(s.base.v()) + offset).into();
            s.dir = flip_if(ldraw_dir_to_print(s.dir.v()), flip).into();
            s.length *= mm;
            s.radius *= mm;
            s.inner_radius *= mm;
        }
        if let Some(s) = &mut c.selected_mm {
            let p = Vec3::new(s.axis_point_mm[0], s.axis_point_mm[1], s.axis_point_mm[2]) + offset;
            s.axis_point_mm = p.to_array();
        }
    }

    let diag_opts = DiagnosticOptions {
        tol: weld * mm,
        check_intersections: opt.check_intersections,
        ..Default::default()
    };
    let diagnostics = mesh::diagnose(&mesh, &diag_opts);
    let (components, _) = mesh.components();

    Ok(BuildOutput {
        name: root,
        description: resolved.description.clone(),
        mm_per_ldu: mm,
        mesh,
        components,
        edges,
        files: resolved.files.clone(),
        missing: resolved.missing.clone(),
        uncertified_files: resolved.uncertified_files.clone(),
        main_color_tris: resolved.main_color_tris,
        warnings,
        repairs,
        diagnostics,
        connector,
        connector_suggestions,
        placement_offset: offset,
        smoothing_open_edges_added,
        source_triangles: resolved.triangles.len(),
    })
}

/// 180° rotation about X: a proper rotation, so winding is preserved.
fn flip_if(p: Vec3, flip: bool) -> Vec3 {
    if flip {
        Vec3::new(p.x, -p.y, -p.z)
    } else {
        p
    }
}

/// Total length of open (boundary) edges, in mesh units.
fn open_seam_length(m: &Mesh) -> f64 {
    mesh::boundary_edges(m)
        .into_iter()
        .map(|(a, b)| (m.positions[a as usize] - m.positions[b as usize]).length())
        .sum()
}
