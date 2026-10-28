//! User configuration: LDraw library location, defaults, printer
//! compensation, fit presets and connector profiles.
//!
//! Lookup order for the config file: `--config`, `$LDRAW2PRINT_CONFIG`,
//! `./ldraw2print.toml`, `$XDG_CONFIG_HOME/ldraw2print/config.toml`
//! (default `~/.config/ldraw2print/config.toml`). Missing file → built-in
//! defaults (identical to `ldraw2print config init` output).

use crate::ldraw::PrimitiveResolution;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG: &str = include_str!("../examples/ldraw2print.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Root of the LDraw library (directory containing `parts/` and `p/`).
    pub ldraw_dir: Option<PathBuf>,
    #[serde(default = "default_scale")]
    pub default_scale: f64,
    /// Colour code substituted for colour 16 at the top level.
    #[serde(default = "default_color")]
    pub default_color: u32,
    #[serde(default)]
    pub primitive_resolution: PrimitiveResolution,
    #[serde(default)]
    pub mesh: MeshConfig,
    #[serde(default)]
    pub printer: PrinterConfig,
    #[serde(default = "default_presets")]
    pub fit_presets: FitPresets,
    #[serde(default)]
    pub connectors: BTreeMap<String, ConnectorProfile>,
}

fn default_scale() -> f64 {
    5.0
}
fn default_color() -> u32 {
    71
}
fn default_presets() -> FitPresets {
    FitPresets {
        loose: 0.40,
        normal: 0.20,
        tight: 0.05,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshConfig {
    /// Vertices closer than this (LDU) are merged.
    #[serde(default = "default_weld")]
    pub weld_tolerance_ldu: f64,
    #[serde(default)]
    pub fix_t_junctions: bool,
    #[serde(default)]
    pub orient: bool,
    /// Re-tessellate circular primitives to this many segments per circle
    /// (multiple of 16; 48 or 96 also handle p/48 primitives).
    #[serde(default)]
    pub smooth_segments: Option<u32>,
    /// Close separate open shells (LDraw ribs/bosses missing hidden faces).
    #[serde(default)]
    pub close_open_shells: bool,
}
fn default_weld() -> f64 {
    0.005
}
impl Default for MeshConfig {
    fn default() -> Self {
        MeshConfig {
            weld_tolerance_ldu: default_weld(),
            fix_t_junctions: false,
            orient: false,
            smooth_segments: None,
            close_open_shells: false,
        }
    }
}

/// Printer-specific dimensional compensation in millimetres, determined by
/// printing calibration pieces. Applied on top of fit clearance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrinterConfig {
    /// Added to the diameter of printed holes (FDM holes usually print small).
    #[serde(default)]
    pub hole_compensation_mm: f64,
    /// Subtracted from the diameter of printed pins.
    #[serde(default)]
    pub pin_compensation_mm: f64,
}

/// Diametral clearance in mm between mating surfaces for each preset.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FitPresets {
    pub loose: f64,
    pub normal: f64,
    pub tight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Gender {
    /// The accessory has a hole that slides over a male feature (hat on stud).
    Socket,
    /// The accessory has a pin/bar that goes into a hole (bar in hand).
    Pin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AxisConstraint {
    /// Parallel to LDraw Y (vertical when the figure stands upright).
    Vertical,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorProfile {
    #[serde(default)]
    pub description: String,
    pub gender: Gender,
    #[serde(default = "default_axis")]
    pub axis: AxisConstraint,
    /// Nominal diameter of the mating surface in LDU at 1:1 (LEGO nominal ×
    /// 1). Used for detection and as the mating size when not measured.
    pub nominal_diameter_ldu: Option<f64>,
    /// Nominal engagement depth in LDU (informational / overlay length).
    pub nominal_depth_ldu: Option<f64>,
    /// Measured diameter of the *mating* feature on your physical 5:1
    /// figure, in mm. Overrides nominal × scale when set.
    pub measured_diameter_mm: Option<f64>,
    /// Scale the measurement was taken at (warns if converting at another).
    pub measured_at_scale: Option<f64>,
    /// Per-connector override of the global fit presets.
    pub fit: Option<FitPresets>,
    #[serde(default = "default_fit_name")]
    pub default_fit: String,
    /// Relative radius tolerance used when auto-detecting this connector.
    #[serde(default = "default_match_tol")]
    pub match_tolerance: f64,
    /// Minimum fraction of a full circle the surface must cover (C-clips
    /// such as airtanks wrap only part of the neck).
    #[serde(default = "default_min_coverage")]
    pub min_coverage: f64,
    /// Optional transparent body drawn behind the reference connector in
    /// the preview (e.g. the head below the stud), LDU at 1:1.
    pub reference_body: Option<ReferenceBody>,
    /// Record of a real calibration (coupons printed and tested). Its
    /// presence is what "calibrated" means; results hold only for this
    /// printer, material and scale.
    pub calibration: Option<CalibrationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationRecord {
    pub date: String,
    pub printer: String,
    pub material: String,
    pub scale: f64,
    /// What fit best, e.g. "coupon 3 (tight, 0.05 mm) fit best".
    pub result: String,
    /// Diametral clearance (mm) of the winning coupon. When present,
    /// `--fit clearance` uses exactly this value.
    #[serde(default)]
    pub clearance_mm: Option<f64>,
    /// Slicer profile or anything else worth knowing.
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceBody {
    pub diameter_ldu: f64,
    pub length_ldu: f64,
}

fn default_axis() -> AxisConstraint {
    AxisConstraint::Vertical
}
fn default_fit_name() -> String {
    "normal".into()
}
fn default_match_tol() -> f64 {
    0.1
}
fn default_min_coverage() -> f64 {
    0.95
}

impl Config {
    pub fn builtin() -> Config {
        toml::from_str(DEFAULT_CONFIG).expect("built-in config must parse")
    }

    pub fn load(explicit: Option<&Path>) -> Result<(Config, Option<PathBuf>)> {
        let path = match explicit {
            Some(p) => {
                if !p.is_file() {
                    bail!("config file {} not found", p.display());
                }
                Some(p.to_path_buf())
            }
            None => Self::search_paths().into_iter().find(|p| p.is_file()),
        };
        match path {
            Some(p) => {
                let text = std::fs::read_to_string(&p)
                    .with_context(|| format!("reading {}", p.display()))?;
                let mut c: Config =
                    toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))?;
                // Profiles the user did not define fall back to built-ins.
                for (k, v) in Config::builtin().connectors {
                    c.connectors.entry(k).or_insert(v);
                }
                c.validate()?;
                Ok((c, Some(p)))
            }
            None => Ok((Config::builtin(), None)),
        }
    }

    pub fn search_paths() -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Ok(p) = std::env::var("LDRAW2PRINT_CONFIG") {
            v.push(PathBuf::from(p));
        }
        v.push(PathBuf::from("ldraw2print.toml"));
        v.push(Self::user_config_path());
        v
    }

    pub fn user_config_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("ldraw2print").join("config.toml")
    }

    pub fn validate(&self) -> Result<()> {
        if self.default_scale.is_nan() || self.default_scale <= 0.0 {
            bail!("default_scale must be positive");
        }
        for (name, p) in &self.connectors {
            if let Some(d) = p.nominal_diameter_ldu {
                if d <= 0.0 {
                    bail!("connector {name}: nominal_diameter_ldu must be positive");
                }
            }
            if let Some(d) = p.measured_diameter_mm {
                if d <= 0.0 {
                    bail!("connector {name}: measured_diameter_mm must be positive");
                }
            }
            self.fit_clearance(name, &p.default_fit)
                .with_context(|| format!("connector {name}: invalid default_fit"))?;
        }
        Ok(())
    }

    pub fn profile(&self, name: &str) -> Result<&ConnectorProfile> {
        self.connectors.get(name).with_context(|| {
            format!(
                "unknown connector profile '{name}'. Available: {}",
                self.connectors
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    /// Diametral clearance (mm) for a fit name: loose|normal|tight, or
    /// `clearance` meaning the profile's default preset.
    pub fn fit_clearance(&self, connector: &str, fit: &str) -> Result<f64> {
        if fit == "clearance" || fit == "default" {
            if let Some(c) = self
                .connectors
                .get(connector)
                .and_then(|p| p.calibration.as_ref())
                .and_then(|r| r.clearance_mm)
            {
                return Ok(c);
            }
        }
        let presets = self
            .connectors
            .get(connector)
            .and_then(|p| p.fit)
            .unwrap_or(self.fit_presets);
        let fit = if fit == "clearance" || fit == "default" {
            self.connectors
                .get(connector)
                .map(|p| p.default_fit.as_str())
                .unwrap_or("normal")
        } else {
            fit
        };
        Ok(match fit {
            "loose" => presets.loose,
            "normal" => presets.normal,
            "tight" => presets.tight,
            other => bail!("unknown fit '{other}' (expected loose, normal, tight or clearance)"),
        })
    }
}

/// Write a calibration record (and optionally `default_fit`) for one
/// connector into a TOML config file, preserving its comments and layout.
pub fn record_calibration(
    path: &Path,
    connector: &str,
    rec: &CalibrationRecord,
    default_fit: Option<&str>,
) -> Result<()> {
    use toml_edit::{value, DocumentMut, InlineTable, Item, Table};
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;
    if !doc.contains_key("connectors") {
        let mut t = Table::new();
        t.set_implicit(true);
        doc.insert("connectors", Item::Table(t));
    }
    let connectors = doc["connectors"]
        .as_table_mut()
        .context("[connectors] is not a table")?;
    if !connectors.contains_key(connector) {
        // Not in the file yet: start from the built-in profile so the table
        // is complete (a partial profile wouldn't load).
        let builtin = Config::builtin();
        let p = builtin.connectors.get(connector).with_context(|| {
            format!("unknown connector profile '{connector}' (not in the config or built-ins)")
        })?;
        let t: DocumentMut = toml::to_string(p)?.parse()?;
        connectors.insert(connector, Item::Table(t.as_table().clone()));
    }
    let entry = connectors[connector]
        .as_table_mut()
        .with_context(|| format!("[connectors.{connector}] is not a table"))?;
    if let Some(f) = default_fit {
        entry["default_fit"] = value(f);
    }
    let mut it = InlineTable::new();
    it.insert("date", rec.date.as_str().into());
    it.insert("printer", rec.printer.as_str().into());
    it.insert("material", rec.material.as_str().into());
    it.insert("scale", rec.scale.into());
    it.insert("result", rec.result.as_str().into());
    if let Some(c) = rec.clearance_mm {
        it.insert("clearance_mm", c.into());
    }
    if let Some(n) = &rec.notes {
        it.insert("notes", n.as_str().into());
    }
    entry["calibration"] = value(it);
    let out = doc.to_string();
    // Refuse to write something we can't read back.
    let check: Config =
        toml::from_str(&out).context("internal error: edited config doesn't parse")?;
    check.validate()?;
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

/// Today's date (UTC) as YYYY-MM-DD, without extra dependencies.
pub fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_config_parses_and_has_profiles() {
        let c = Config::builtin();
        c.validate().unwrap();
        assert_eq!(c.default_scale, 5.0);
        for name in ["head-stud", "head-body", "neck", "scarf", "hand-grip"] {
            assert!(c.connectors.contains_key(name), "{name}");
        }
        assert_eq!(c.connectors["head-stud"].gender, Gender::Socket);
        assert_eq!(c.connectors["hand-grip"].gender, Gender::Pin);
        assert!(
            c.connectors["head-stud"].measured_diameter_mm.is_none(),
            "never ship invented measurements"
        );
    }

    #[test]
    fn record_calibration_preserves_comments_and_is_used() {
        let dir = std::env::temp_dir().join(format!("l2p-cal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, DEFAULT_CONFIG).unwrap();
        let rec = CalibrationRecord {
            date: "2026-10-27".into(),
            printer: "Prusa MK4".into(),
            material: "PLA".into(),
            scale: 5.0,
            result: "coupon 2 fit best".into(),
            clearance_mm: Some(0.15),
            notes: None,
        };
        record_calibration(&path, "head-stud", &rec, None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# Dimensions come in three distinct kinds"),
            "comments preserved"
        );
        let (cfg, _) = Config::load(Some(&path)).unwrap();
        let p = &cfg.connectors["head-stud"];
        assert_eq!(p.calibration.as_ref().unwrap().printer, "Prusa MK4");
        assert_eq!(cfg.fit_clearance("head-stud", "clearance").unwrap(), 0.15);
        // Explicit presets still mean the preset.
        assert_eq!(
            cfg.fit_clearance("head-stud", "tight").unwrap(),
            cfg.fit_presets.tight
        );
        // Unknown profiles are refused.
        assert!(record_calibration(&path, "my-visor", &rec, Some("normal")).is_err());
        // A profile only known as a built-in gets a complete table.
        std::fs::write(&path, "default_scale = 5.0\n").unwrap();
        record_calibration(&path, "scarf", &rec, Some("tight")).unwrap();
        let (cfg, _) = Config::load(Some(&path)).unwrap();
        assert_eq!(cfg.connectors["scarf"].default_fit, "tight");
        assert_eq!(cfg.connectors["scarf"].nominal_diameter_ldu, Some(16.0));
        assert!(cfg.connectors["scarf"].calibration.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn today_is_iso_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }

    #[test]
    fn fit_presets_and_overrides() {
        let mut c = Config::builtin();
        assert_eq!(
            c.fit_clearance("head-stud", "loose").unwrap(),
            c.fit_presets.loose
        );
        assert_eq!(
            c.fit_clearance("head-stud", "clearance").unwrap(),
            c.fit_presets.normal
        );
        c.connectors.get_mut("head-stud").unwrap().fit = Some(FitPresets {
            loose: 1.0,
            normal: 0.5,
            tight: 0.1,
        });
        assert_eq!(c.fit_clearance("head-stud", "tight").unwrap(), 0.1);
        assert!(c.fit_clearance("head-stud", "snug").is_err());
    }
}
