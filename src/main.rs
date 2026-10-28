use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use ldraw2print::calibrate::{self, CalibrationRequest};
use ldraw2print::config::{Config, Gender, DEFAULT_CONFIG};
use ldraw2print::connectors::ManualConnector;
use ldraw2print::errors::{self, exit};
use ldraw2print::export::{
    paint_filaments, BinaryStl, Exporter, PaintedThreeMf, PrintModel, ThreeMf,
};
use ldraw2print::geometry::{mm_per_ldu_for_scale, Vec3};
use ldraw2print::ldraw::{search_parts, Library, PrimitiveResolution};
use ldraw2print::mesh::Mesh;
use ldraw2print::pipeline::{build, BuildOptions, BuildOutput, Placement};
use ldraw2print::preview::PreviewServer;
use ldraw2print::report;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

mod guide;

const TOP_HELP: &str = "\
QUICK START
  ldraw2print search pirate bicorne          find a part number by name
  ldraw2print inspect 3624 --connector head-stud
  ldraw2print calibrate --connector head-stud -o coupons.stl
  ldraw2print convert 3624 --connector head-stud --fit clearance --smooth 96 -o hat.stl
  ldraw2print convert 3626cp01 --color 14 --smooth 96 -o head.3mf   (painted multicolour)
  (--smooth rounds curves; if the verdict says it opened seams, drop it for that part)

SCOPE
  Converts one LDraw part (or an assembled .ldr/.mpd model, fused into one solid) to a
  printable file, with fits for minifigure connections (head stud, head body, neck,
  scarf, hand grip). It does not make print-in-place joints or articulated figures.
  Hats: head-stud = round socket on the head's top stud; head-body = slides over the
  head (bicorne family). `inspect` (no --connector) suggests profiles that match.

MORE HELP
  ldraw2print <command> --help    options and examples for one command
  ldraw2print guide               list built-in guide topics (workflow, connectors, fit, …)
  ldraw2print guide agents        automation: --json output, exit codes, pitfalls

EXIT CODES
  0 ok · 1 error · 2 usage · 3 not watertight (--strict) · 4 connector needs a decision
  (pass --connector-candidate) · 5 referenced LDraw files missing (--allow-missing)";

#[derive(Parser)]
#[command(
    name = "ldraw2print",
    version,
    about = "Convert LDraw parts into 3D-printable models (default 5:1 scale, mm, Z up)",
    long_about = "Convert LEGO LDraw parts into 3D-printable STL or multicolour 3MF, with \
minifigure connector fits (head stud, neck, hand grip), calibration coupons, curve smoothing, \
mesh diagnostics and repair, and a browser preview. Output is millimetres, Z up, default 5:1 \
scale (1 LDU = 2 mm).",
    after_help = "Run `ldraw2print help` or `--help` for the quick start, exit codes and guide topics.",
    after_long_help = TOP_HELP
)]
struct Cli {
    /// LDraw library root (contains parts/ and p/). Overrides $LDRAWDIR and the config file.
    #[arg(long, global = true, value_name = "DIR")]
    ldraw_dir: Option<PathBuf>,
    /// Configuration file (default: ./ldraw2print.toml or ~/.config/ldraw2print/config.toml).
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Primitive resolution substitution (leave at `standard`; prefer --smooth).
    #[arg(long, global = true, value_enum, value_name = "RES")]
    primitives: Option<PrimitiveResolution>,
    /// Print one JSON object on stdout instead of text (errors too). For scripts and agents.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert a part to STL or 3MF.
    #[command(after_long_help = "\
EXAMPLES
  # Plain conversion at 5:1 (writes 3624.stl)
  ldraw2print convert 3624

  # Hat with the head-stud socket fitted to your calibrated preset, rounder curves
  ldraw2print convert 3624 --connector head-stud --fit clearance --smooth 96 -o hat.stl

  # Printed part as a painted multicolour 3MF (prints the filament order to load)
  ldraw2print convert 2528ap30 --color 0 --smooth 96 -o bicorne.3mf

  # Hat with internal ribs drawn as open shells (bicorne): close them so slicers keep them
  ldraw2print convert 2528ap30 --color 0 --connector head-body --fit clearance --close-open-shells -o bicorne.3mf

  # Print upside down; refuse to write a non-watertight mesh
  ldraw2print convert 3626c --flip --strict -o head.stl

  # Top hat from the corrected copy in the repo (32 harmless open edges remain)
  ldraw2print convert examples/3878-print.dat --flip -o tophat.stl

NOTES
  Without --fit, connector geometry is exported exactly as modelled.
  If --connector can't pick a surface automatically the command exits with 4 and lists
  candidates; rerun with --connector-candidate <id>. See `ldraw2print guide connectors`.")]
    Convert {
        #[command(flatten)]
        b: BuildArgs,
        /// Output file (default: <part>.stl). A .3mf extension selects 3MF.
        #[arg(short, long, help_heading = "Output")]
        output: Option<PathBuf>,
        /// Output format (default: from the output extension, else stl).
        #[arg(long, value_enum, help_heading = "Output")]
        format: Option<Format>,
        /// How 3MF stores colours.
        #[arg(long, value_enum, default_value = "paint", help_heading = "Output")]
        color_mode: ColorMode,
        /// Fail with exit code 3 (and write nothing) unless the mesh is watertight.
        #[arg(long, help_heading = "Output")]
        strict: bool,
    },
    /// Open an interactive browser preview (runs until stopped; not for scripts).
    #[command(after_long_help = "\
EXAMPLES
  ldraw2print preview 3624 --connector head-stud --fit clearance
  ldraw2print preview 3626cp01 --color 14 --smooth 96 --port 8010 --no-open

NOTES
  The server runs until interrupted (Ctrl+C). Agents should use `inspect --json` and
  `convert` instead; the preview uses the same pipeline. The page offers Export STL and
  Export 3MF (painted). URL options: ?view=iso|front|side|top|bottom&bbox=0&refbody=1.")]
    Preview {
        #[command(flatten)]
        b: BuildArgs,
        /// Port to listen on.
        #[arg(long, default_value_t = 8008, help_heading = "Server")]
        port: u16,
        /// Address to bind (keep 127.0.0.1 unless you know you need otherwise).
        #[arg(long, default_value = "127.0.0.1", help_heading = "Server")]
        host: String,
        /// Do not open a browser automatically.
        #[arg(long, help_heading = "Server")]
        no_open: bool,
    },
    /// Report size, mesh diagnostics, colours, components and connector candidates.
    #[command(after_long_help = "\
EXAMPLES
  ldraw2print inspect 3624 --connector head-stud        connector candidates + fit plan
  ldraw2print inspect 2528ap30 --problems 50             where the open edges are
  ldraw2print --json inspect 3878 --connector head-stud   machine-readable

NOTES
  Pass the same flags you plan to give convert (--smooth, --close-open-shells, --fit, …):
  inspect runs the identical pipeline and reports what convert would write.
  inspect never writes files. With --connector and no clear match it still succeeds and
  reports `selection_error`; only convert/calibrate fail with exit code 4.")]
    Inspect {
        #[command(flatten)]
        b: BuildArgs,
        /// List up to N open / non-manifold edges with coordinates (mm) and source files.
        #[arg(long, value_name = "N", help_heading = "Output")]
        problems: Option<usize>,
    },
    /// Generate test-fit coupons (rings or pins) for a connector profile.
    #[command(after_long_help = "\
EXAMPLES
  ldraw2print calibrate --connector head-stud -o head-stud-coupons.stl
  ldraw2print calibrate --connector hand-grip --clearances 0,0.05,0.1,0.15
  ldraw2print calibrate --connector scarf --scale 3

  # After printing and testing: coupon 2 fit best
  ldraw2print calibrate --connector head-stud --record 2 --printer 'Prusa MK4' --material PLA

NOTES
  Coupon k has k notches on its rim (presets: 1 loose, 2 normal, 3 tight); a .txt summary
  is written next to the STL. Sockets produce tubes (bore = mating Ø + clearance + hole
  compensation); pins produce pins on a base.
  --record writes a calibration record (with the coupon's exact clearance) into the config
  in use, creating the user config if there is none; comments are kept. `--fit clearance`
  then uses that clearance. Pass the same --scale/--clearances as for the coupons.")]
    Calibrate {
        /// Connector profile name (see `ldraw2print profiles`).
        #[arg(long)]
        connector: String,
        /// Scale factor (default: config default_scale, normally 5).
        #[arg(long)]
        scale: Option<f64>,
        /// Comma-separated diametral clearances in mm (default: loose,normal,tight presets).
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        clearances: Option<Vec<f64>>,
        /// Take the mating diameter from this part's connector when the profile has none.
        #[arg(long)]
        part: Option<String>,
        /// Coupon wall (sockets) or base margin (pins) in mm (default: scaled to the connector).
        #[arg(long)]
        wall_mm: Option<f64>,
        /// Coupon height in mm (default: the profile's nominal depth × scale, min 3).
        #[arg(long)]
        height_mm: Option<f64>,
        /// Output STL. A summary is also written next to it with a .txt extension.
        #[arg(short, long, default_value = "calibration.stl")]
        output: PathBuf,
        /// After testing: record that coupon N fit best (1 = first/one notch). Writes a
        /// calibration record into the config instead of generating coupons. Use the same
        /// --scale and --clearances as when the coupons were made.
        #[arg(long, value_name = "N", requires_all = ["printer", "material"], help_heading = "Recording a result")]
        record: Option<usize>,
        /// Printer the coupons were printed on (for --record).
        #[arg(long, help_heading = "Recording a result")]
        printer: Option<String>,
        /// Material/filament the coupons were printed in (for --record).
        #[arg(long, help_heading = "Recording a result")]
        material: Option<String>,
        /// Date of the test (for --record; default today).
        #[arg(long, help_heading = "Recording a result")]
        date: Option<String>,
        /// Free-form notes, e.g. slicer profile (for --record).
        #[arg(long, help_heading = "Recording a result")]
        notes: Option<String>,
    },
    /// Find parts by words in their name or number (needs the LDraw library).
    #[command(after_long_help = "\
EXAMPLES
  ldraw2print search pirate bicorne
  ldraw2print search minifig head grin
  ldraw2print search scarf --limit 50
  ldraw2print search 'set 6285'                     parts listed for a set
  ldraw2print --json search 3626c

NOTES
  All words must match (case-insensitive) the part number, description, category or the
  part's LDraw keywords (themes like 'pirate', nicknames, BrickLink/Rebrickable ids, sets).
  Obsolete, moved, alias and physical-colour entries are hidden unless --all.
  Patterned parts usually share a base number with a pattern suffix (3626c → 3626cp01).")]
    Search {
        /// Words to match.
        #[arg(required = true)]
        words: Vec<String>,
        /// Maximum results to show.
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Include obsolete/moved/alias/physical-colour entries.
        #[arg(long)]
        all: bool,
    },
    /// List LDraw colour codes, optionally filtered by name or code.
    #[command(after_long_help = "\
EXAMPLES
  ldraw2print colors black
  ldraw2print colors 71
  ldraw2print --json colors

NOTES
  Use the code with --color (sets main colour 16). Printed patterns keep their own colours.")]
    Colors {
        /// Text in the colour name, or a colour code.
        query: Option<String>,
    },
    /// List connector profiles with their nominal sizes and fit settings.
    #[command(after_long_help = "\
EXAMPLES
  ldraw2print profiles
  ldraw2print --json profiles
  ldraw2print profiles --scale 3

NOTES
  Profiles come from the built-in defaults merged with your config file. Sizes are shown
  at the given scale (default: config default_scale). See `ldraw2print guide fit`.")]
    Profiles {
        /// Scale for the millimetre values shown.
        #[arg(long)]
        scale: Option<f64>,
    },
    /// Print a built-in guide topic (run without a topic to list them).
    Guide {
        /// Topic name, e.g. workflow, connectors, fit, agents.
        topic: Option<String>,
    },
    /// Manage the configuration file.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    /// Show the LDraw library in use and how to install one.
    Library,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Format {
    Stl,
    /// 3MF; see --color-mode.
    #[value(name = "3mf")]
    ThreeMf,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Format::Stl => "stl",
            Format::ThreeMf => "3mf",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ColorMode {
    /// One solid; non-base colours painted onto its faces (works for printed patterns).
    Paint,
    /// One part per colour (experimental; each colour must be a closed solid).
    Parts,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Write the default configuration to the user config path (or --path).
    Init {
        /// Where to write it (default: ~/.config/ldraw2print/config.toml).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Print the effective configuration (built-ins merged with your file).
    Show,
    /// Print the config search paths (* marks the one that exists).
    Path,
}

#[derive(Args, Clone)]
struct BuildArgs {
    /// Part number (3901, 3901.dat), library-relative name, or path to a .dat/.ldr/.mpd file.
    part: String,
    /// Scale factor relative to real LEGO (1 LDU = 0.4 mm × scale; default 5).
    #[arg(long, help_heading = "Build")]
    scale: Option<f64>,
    /// LDraw colour code for "main colour" 16 (see `ldraw2print colors`; default 71).
    #[arg(long, help_heading = "Build")]
    color: Option<u32>,
    /// Round off LDraw's 16-sided curves: N segments per circle (multiple of 16, e.g. 96). Check the verdict: on some parts it opens seams.
    #[arg(long, value_name = "N", help_heading = "Build")]
    smooth: Option<u32>,
    /// Print upside down (180° about X), e.g. hats crown-down so the socket needs no support.
    #[arg(long, help_heading = "Build")]
    flip: bool,
    /// Where to put the part.
    #[arg(long, value_enum, default_value = "bed", help_heading = "Build")]
    placement: Placement,
    /// Continue (exit 0) even if referenced files are missing; otherwise exit 5.
    #[arg(long, help_heading = "Build")]
    allow_missing: bool,

    /// Connector profile to detect/adjust (see `ldraw2print profiles`).
    #[arg(long, help_heading = "Connector & fit")]
    connector: Option<String>,
    /// Fit: none (keep modelled), loose (0.40 mm), normal (0.20), tight (0.05), or clearance (= profile default_fit). Values from config; see `profiles`.
    #[arg(long, requires = "connector", help_heading = "Connector & fit")]
    fit: Option<String>,
    /// Explicit diametral clearance in mm (overrides the fit preset).
    #[arg(
        long,
        requires = "connector",
        allow_hyphen_values = true,
        help_heading = "Connector & fit"
    )]
    clearance_mm: Option<f64>,
    /// Use this detected surface (ids from `inspect --connector`) when auto-selection can't.
    #[arg(
        long,
        requires = "connector",
        value_name = "ID",
        help_heading = "Connector & fit"
    )]
    connector_candidate: Option<usize>,
    /// Manual connector: a point on its axis, LDraw coordinates (LDU), "x,y,z".
    #[arg(long, requires_all = ["connector", "connector_radius"], allow_hyphen_values = true, value_name = "X,Y,Z", help_heading = "Connector & fit")]
    connector_axis: Option<String>,
    /// Manual connector axis direction (LDraw), default "0,-1,0" (vertical).
    #[arg(
        long,
        allow_hyphen_values = true,
        value_name = "X,Y,Z",
        help_heading = "Connector & fit"
    )]
    connector_dir: Option<String>,
    /// Manual connector radius in LDU.
    #[arg(long, value_name = "LDU", help_heading = "Connector & fit")]
    connector_radius: Option<f64>,
    /// Manual connector extent "a,b": distances along --connector-dir from the axis point (LDU), e.g. 0,4.
    #[arg(
        long,
        allow_hyphen_values = true,
        value_name = "A,B",
        help_heading = "Connector & fit"
    )]
    connector_range: Option<String>,

    /// Close separate open shells (ribs, bosses) and fuse them to the body. Only adds faces.
    #[arg(long, help_heading = "Mesh repair")]
    close_open_shells: bool,
    /// Split triangles at T-junctions (never moves vertices).
    #[arg(long, help_heading = "Mesh repair")]
    fix_t_junctions: bool,
    /// Infer facing of non-BFC-certified triangles from topology.
    #[arg(long, help_heading = "Mesh repair")]
    orient: bool,
    /// Skip removal of collapsed, needle and exact-duplicate triangles.
    #[arg(long, help_heading = "Mesh repair")]
    no_cleanup: bool,
    /// Skip the triangle–triangle intersection check (faster on big parts).
    #[arg(long, help_heading = "Mesh repair")]
    no_intersections: bool,
}

fn parse_vec(s: &str) -> Result<Vec3> {
    let v: Vec<f64> = s
        .split(',')
        .map(|x| x.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("bad vector '{s}'"))?;
    if v.len() != 3 {
        bail!("expected x,y,z, got '{s}'");
    }
    Ok(Vec3::new(v[0], v[1], v[2]))
}

impl BuildArgs {
    fn options(&self, cfg: &Config) -> Result<BuildOptions> {
        let mut o = BuildOptions::new(&self.part, self.scale.unwrap_or(cfg.default_scale));
        o.color = self.color;
        o.connector = self.connector.clone();
        o.fit = self.fit.clone();
        o.clearance_mm = self.clearance_mm;
        o.candidate = self.connector_candidate;
        o.placement = self.placement;
        o.cleanup = !self.no_cleanup;
        o.fix_t_junctions = self.fix_t_junctions.then_some(true);
        o.orient = self.orient.then_some(true);
        o.allow_missing = self.allow_missing;
        o.check_intersections = !self.no_intersections;
        o.flip = self.flip;
        o.smooth = self.smooth;
        o.close_open_shells = self.close_open_shells.then_some(true);
        if let Some(a) = &self.connector_axis {
            let range = match &self.connector_range {
                Some(r) => {
                    let v: Vec<f64> = r
                        .split(',')
                        .map(|x| x.trim().parse())
                        .collect::<Result<_, _>>()
                        .context("bad --connector-range")?;
                    if v.len() != 2 {
                        bail!("--connector-range expects a,b");
                    }
                    Some((v[0], v[1]))
                }
                None => None,
            };
            o.manual = Some(ManualConnector {
                point: parse_vec(a)?,
                dir: parse_vec(self.connector_dir.as_deref().unwrap_or("0,-1,0"))?,
                radius_ldu: self
                    .connector_radius
                    .context("--connector-radius is required with --connector-axis")?,
                range,
            });
        }
        if let Some(f) = &o.fit {
            if !matches!(
                f.as_str(),
                "none" | "loose" | "normal" | "tight" | "clearance"
            ) {
                bail!("--fit must be none, loose, normal, tight or clearance");
            }
        }
        Ok(o)
    }
}

const COMMON_LDRAW_DIRS: &[&str] = &[
    "~/ldraw",
    "~/LDraw",
    "~/Documents/ldraw",
    "~/Documents/LDraw",
    "~/Library/ldraw",
    "/Applications/ldraw",
    "/usr/share/ldraw",
    "/usr/local/share/ldraw",
    "/opt/ldraw",
    "C:\\Users\\Public\\Documents\\LDraw",
    "C:\\LDraw",
];

fn expand(p: &str) -> PathBuf {
    let home = std::env::var_os("HOME");
    match (p, home) {
        ("~", Some(h)) => PathBuf::from(h),
        (_, Some(h)) if p.starts_with("~/") => PathBuf::from(h).join(&p[2..]),
        _ => PathBuf::from(p),
    }
}

fn looks_like_library(p: &Path) -> bool {
    p.join("parts").is_dir() && p.join("p").is_dir()
}

/// Validate a user-supplied library path: expand `~`, accept a doubly
/// nested `ldraw/ldraw` (a common unzip result), and explain failures.
fn check_library(raw: &Path, source: &str) -> Result<PathBuf> {
    let d = expand(&raw.to_string_lossy());
    if looks_like_library(&d) {
        return Ok(d);
    }
    let nested = d.join("ldraw");
    if looks_like_library(&nested) {
        return Ok(nested);
    }
    if !d.exists() {
        bail!(
            "{source} points to {}, which does not exist.\n\
             The LDraw library is not installed there. Run `ldraw2print library` for download instructions.",
            d.display()
        );
    }
    if !d.is_dir() {
        bail!(
            "{source} points to {}, which is not a directory",
            d.display()
        );
    }
    bail!(
        "{source} points to {}, which exists but has no parts/ and p/ subdirectories.\n\
         Point it at the folder that directly contains parts/, p/ and LDConfig.ldr.",
        d.display()
    )
}

/// (path, how it was found)
fn find_library(cli: &Cli, cfg: &Config) -> Result<Option<(PathBuf, String)>> {
    if let Some(d) = &cli.ldraw_dir {
        return Ok(Some((
            check_library(d, "--ldraw-dir")?,
            "--ldraw-dir".into(),
        )));
    }
    if let Some(d) = std::env::var_os("LDRAWDIR").filter(|v| !v.is_empty()) {
        return Ok(Some((
            check_library(Path::new(&d), "$LDRAWDIR")?,
            "$LDRAWDIR".into(),
        )));
    }
    if let Some(d) = &cfg.ldraw_dir {
        return Ok(Some((
            check_library(d, "ldraw_dir (config file)")?,
            "config file".into(),
        )));
    }
    Ok(COMMON_LDRAW_DIRS
        .iter()
        .map(|p| expand(p))
        .flat_map(|p| [p.join("ldraw"), p])
        .find(|p| looks_like_library(p))
        .map(|p| (p, "auto-detected".into())))
}

/// Where `find_library` looks, in order (for "not found" messages).
fn searched_locations(cli: &Cli, cfg: &Config) -> Vec<String> {
    let mut v = Vec::new();
    v.push(match &cli.ldraw_dir {
        Some(d) => format!("--ldraw-dir {}", d.display()),
        None => "--ldraw-dir (not given)".into(),
    });
    v.push(match std::env::var("LDRAWDIR") {
        Ok(d) if !d.is_empty() => format!("$LDRAWDIR = {d}"),
        _ => "$LDRAWDIR (not set)".into(),
    });
    v.push(match &cfg.ldraw_dir {
        Some(d) => format!("ldraw_dir in config = {}", d.display()),
        None => "ldraw_dir in config (not set)".into(),
    });
    for p in COMMON_LDRAW_DIRS {
        v.push(format!("{} (auto-detect)", expand(p).display()));
    }
    v
}

const LIBRARY_HELP: &str = "\
To install the official LDraw parts library:
  1. Download https://library.ldraw.org/library/updates/complete.zip
     (official library, ~150 MB, CC BY 2.0 / CC BY 4.0 licensed).
  2. Unzip it; it creates a folder named `ldraw` containing parts/, p/ and LDConfig.ldr.
     A common location is ~/ldraw.
  3. Point ldraw2print at it with any of:
       --ldraw-dir ~/ldraw
       export LDRAWDIR=~/ldraw
       ldraw_dir = \"/Users/you/ldraw\"   in the config file (`ldraw2print config init`)
  Optional: unofficial parts go in <ldraw>/UnOfficial/parts and <ldraw>/UnOfficial/p.
  Update with the monthly update zips from https://library.ldraw.org/updates.";

fn print_problem_edges(m: &Mesh, files: &[String], n: usize) {
    let edges = m.edge_map();
    let mut list: Vec<_> = edges
        .iter()
        .filter(|(_, u)| u.len() != 2 || u[0].1 == u[1].1)
        .collect();
    list.sort_by(|a, b| a.0.cmp(b.0));
    println!(
        "\nProblem edges ({} total, showing {}; print-frame mm after placement):",
        list.len(),
        n.min(list.len())
    );
    let fmt = |p: Vec3| format!("({:.3}, {:.3}, {:.3})", p.x, p.y, p.z);
    for ((a, b), uses) in list.into_iter().take(n) {
        let (pa, pb) = (m.positions[*a as usize], m.positions[*b as usize]);
        let srcs: Vec<&str> = uses
            .iter()
            .map(|(t, _)| files[m.tris[*t as usize].file as usize].as_str())
            .collect();
        println!(
            "  {} {} → {} len {:.3} mm  [{}]",
            match uses.len() {
                1 => "open        ",
                2 => "inconsistent",
                _ => "non-manifold",
            },
            fmt(pa),
            fmt(pb),
            (pb - pa).length(),
            srcs.join(", ")
        );
    }
}

fn problem_edges_json(m: &Mesh, files: &[String], n: usize) -> Value {
    let edges = m.edge_map();
    let mut list: Vec<_> = edges
        .iter()
        .filter(|(_, u)| u.len() != 2 || u[0].1 == u[1].1)
        .collect();
    list.sort_by(|a, b| a.0.cmp(b.0));
    let total = list.len();
    let shown: Vec<Value> = list
        .into_iter()
        .take(n)
        .map(|((a, b), uses)| {
            let (pa, pb) = (m.positions[*a as usize], m.positions[*b as usize]);
            json!({
                "kind": match uses.len() { 1 => "open", 2 => "inconsistent-winding", _ => "non-manifold" },
                "from_mm": pa.to_array(), "to_mm": pb.to_array(),
                "files": uses.iter().map(|(t, _)| files[m.tris[*t as usize].file as usize].clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "total": total, "edges": shown })
}

/// Compact, stable JSON summary of a build (shared by convert/inspect).
fn build_json(out: &BuildOutput, lib: &Library, opts: &BuildOptions, cfg: &Config) -> Value {
    let scale = opts.scale;
    let b = out.mesh.bbox();
    let d = &out.diagnostics;
    let connector = out.connector.as_ref().map(|c| {
        let selected = c.selection.as_ref().map(|s| {
            let k = &c.candidates[s.candidate];
            json!({
                "id": s.candidate, "how": s.how, "facing": k.facing,
                "diameter_ldu": k.diameter_ldu(), "depth_ldu": k.depth_ldu(), "segments": k.segments,
            })
        });
        let (calibrated, customized) = profile_status(cfg, &c.profile);
        json!({
            "profile": c.profile,
            "calibrated": calibrated,
            "calibration": cfg.connectors.get(&c.profile).and_then(|p| p.calibration.clone()),
            "default_fit_source": if calibrated { "calibration record" } else if customized.contains(&"default_fit") { "user config" } else { "built-in default" },
            "customized": customized,
            "selected": selected,
            "selection_error": c.selection_error,
            "plan": c.plan,
            "fit_applied": c.fit,
            "candidates": c.candidates.iter().map(|k| {
                let verdict = cfg.connectors.get(&c.profile).map(|p| ldraw2print::connectors::match_profile(p, k));
                json!({
                    "id": k.id, "facing": k.facing, "diameter_ldu": k.diameter_ldu(), "depth_ldu": k.depth_ldu(),
                    "coverage": k.coverage, "segments": k.segments, "axis_dir": k.axis_dir, "summary": k.summary(),
                    "matches_profile": matches!(verdict, Some(Ok(()))),
                    "reject_reason": verdict.and_then(|v| v.err()),
                })
            }).collect::<Vec<_>>(),
        })
    });
    let mut per_color: Vec<(u32, usize)> = Vec::new();
    for t in &out.mesh.tris {
        match per_color.iter_mut().find(|(c, _)| *c == t.color) {
            Some(e) => e.1 += 1,
            None => per_color.push((t.color, 1)),
        }
    }
    // The 20 largest pieces; component_count has the total.
    let mut cs: Vec<_> = d.components.iter().collect();
    cs.sort_by_key(|c| std::cmp::Reverse(c.triangles));
    let components: Vec<Value> = cs
        .into_iter()
        .take(20)
        .map(|c| {
            json!({
                "id": c.id, "triangles": c.triangles, "closed": c.closed,
                "size_mm": [c.bbox_max[0]-c.bbox_min[0], c.bbox_max[1]-c.bbox_min[1], c.bbox_max[2]-c.bbox_min[2]],
            })
        })
        .collect();
    let main_code = opts.color.unwrap_or(cfg.default_color);
    let main_color = json!({
        "code": main_code, "name": lib.colors.name(main_code),
        "source": if opts.color.is_some() { "--color" } else { "default" },
        // 0 → every triangle has its own colour and --color changes nothing.
        "triangles": out.main_color_tris,
    });
    json!({
        "part": { "name": out.name, "description": out.description },
        "scale": scale,
        "mm_per_ldu": out.mm_per_ldu,
        "size_mm": b.size().to_array(),
        "bbox_min_mm": b.min.to_array(),
        "bbox_max_mm": b.max.to_array(),
        "triangles": d.triangles,
        "verdict": report::verdict(out),
        "watertight": d.watertight,
        "volume_cm3": d.watertight.then_some(d.signed_volume / 1000.0),
        // Only meaningful for closed meshes; for open ones it's a rough guess.
        "volume_estimate_cm3": (!d.watertight).then_some(d.signed_volume / 1000.0),
        "component_count": d.components.len(),
        "components": components,
        "main_color": main_color,
        "connector_suggestions": out.connector_suggestions,
        "smoothing_new_seams_mm": out.smoothing_open_edges_added,
        "placement": { "mode": match opts.placement { Placement::Bed => "bed", Placement::Ldraw => "ldraw" }, "flip": opts.flip },
        "fit_summary": out.connector.as_ref().map(|c| {
            let (calibrated, _) = profile_status(cfg, &c.profile);
            json!({
                "profile": c.profile,
                "applied": c.fit.is_some(),
                "mating_source": c.plan.as_ref().map(|p| p.mating_source.clone()),
                "calibrated": calibrated,
                // "measured+calibrated" is the only state that is verified for this figure.
                "status": match (c.fit.is_some(), c.plan.as_ref().map(|p| p.mating_source.as_str()), calibrated) {
                    (false, _, _) => "not-applied",
                    (true, Some("measured"), true) => "measured+calibrated",
                    (true, _, true) => "calibrated-preset-nominal-size",
                    (true, Some("measured"), false) => "measured-uncalibrated",
                    _ => "unverified",
                },
            })
        }),
        "colors": per_color.iter().map(|(c, n)| json!({ "code": c, "name": lib.colors.name(*c), "triangles": n })).collect::<Vec<_>>(),
        "issues": d.issues,
        "warnings": out.warnings,
        "repairs": out.repairs,
        "missing_files": out.missing.iter().map(|m| json!({"name": m.name, "from": m.referenced_from})).collect::<Vec<_>>(),
        "connector": connector,
    })
}

/// Calibration status of a profile: (calibrated = a calibration record
/// exists, which settings differ from the shipped defaults).
fn profile_status(cfg: &Config, name: &str) -> (bool, Vec<&'static str>) {
    let Some(p) = cfg.connectors.get(name) else {
        return (false, vec![]);
    };
    let calibrated = p.calibration.is_some();
    let builtin = Config::builtin();
    let Some(b) = builtin.connectors.get(name) else {
        return (calibrated, vec!["custom profile"]);
    };
    let mut changed = Vec::new();
    if p.default_fit != b.default_fit {
        changed.push("default_fit");
    }
    if p.measured_diameter_mm.is_some() {
        changed.push("measured_diameter_mm");
    }
    if p.fit.is_some() {
        changed.push("fit");
    }
    (calibrated, changed)
}

fn hex(rgb: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

fn emit(json_mode: bool, v: Value) {
    if json_mode {
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    }
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // Help/version are "errors" in clap terms but exit 0.
            let code = if e.use_stderr() {
                exit::USAGE
            } else {
                exit::OK
            };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let json_mode = cli.json;
    if let Err(e) = run(cli) {
        let (code, kind) = errors::classify(&e);
        if json_mode {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"ok": false, "error": format!("{e:#}"), "kind": kind, "exit_code": code})).unwrap()
            );
        }
        eprintln!("error: {e:#}");
        std::process::exit(code);
    }
}

fn run(cli: Cli) -> Result<()> {
    let (cfg, cfg_path) = Config::load(cli.config.as_deref())?;
    let make_lib = |cli: &Cli, cfg: &Config| -> Result<Library> {
        let root = find_library(cli, cfg)?.map(|(p, _)| p);
        let mut lib = Library::new(root);
        lib.resolution = cli.primitives.unwrap_or(cfg.primitive_resolution);
        Ok(lib)
    };
    let j = cli.json;

    match &cli.cmd {
        Cmd::Convert {
            b,
            output,
            strict,
            format,
            color_mode,
        } => {
            let mut lib = make_lib(&cli, &cfg)?;
            let opts = b.options(&cfg)?;
            let out = build(&mut lib, &cfg, &opts)?;
            if !j {
                print!("{}", report::summary(&out, opts.scale, false));
            }
            if *strict && !out.diagnostics.watertight {
                return Err(errors::NotWatertight(
                    "--strict: mesh is not watertight; nothing written".into(),
                )
                .into());
            }
            let format =
                format.unwrap_or_else(|| match output.as_ref().and_then(|p| p.extension()) {
                    Some(e) if e.eq_ignore_ascii_case("3mf") => Format::ThreeMf,
                    _ => Format::Stl,
                });
            let exporter: Box<dyn Exporter> = match format {
                Format::Stl => Box::new(BinaryStl),
                Format::ThreeMf => match color_mode {
                    ColorMode::Paint => Box::new(PaintedThreeMf),
                    ColorMode::Parts => Box::new(ThreeMf),
                },
            };
            let path = output.clone().unwrap_or_else(|| {
                let stem = Path::new(&out.name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or("part".into());
                PathBuf::from(format!("{stem}.{}", exporter.extension()))
            });
            let model = PrintModel {
                name: out.name.clone(),
                mesh: &out.mesh,
                colors: &lib.colors,
            };
            {
                let mut f = std::io::BufWriter::new(
                    std::fs::File::create(&path)
                        .with_context(|| format!("creating {}", path.display()))?,
                );
                exporter.write(&model, &mut f)?;
            }
            let painted = format == Format::ThreeMf && *color_mode == ColorMode::Paint;
            let color_note = (format == Format::ThreeMf && opts.color.is_none() && out.main_color_tris > 0).then(|| {
                format!(
                    "main colour not chosen: base is LDraw {} {} (default); pass --color CODE (see `ldraw2print colors`)",
                    cfg.default_color,
                    lib.colors.name(cfg.default_color)
                )
            });
            let filaments: Vec<Value> = if painted {
                paint_filaments(&model)
                    .iter()
                    .enumerate()
                    .map(|(i, (c, n))| {
                        json!({"slot": i + 1, "code": c, "name": lib.colors.name(*c), "hex": hex(lib.colors.rgba(*c)), "triangles": n, "base": i == 0})
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if j {
                let mut v = build_json(&out, &lib, &opts, &cfg);
                let o = v.as_object_mut().unwrap();
                o.insert("ok".into(), json!(true));
                o.insert("command".into(), json!("convert"));
                o.insert(
                    "output".into(),
                    json!({
                        "path": path, "format": format.name(),
                        "color_mode": (format == Format::ThreeMf).then_some(if painted { "paint" } else { "parts" }),
                        "bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                    }),
                );
                o.insert("filaments".into(), json!(filaments));
                if let Some(n) = &color_note {
                    if let Some(w) = o.get_mut("warnings").and_then(|w| w.as_array_mut()) {
                        w.push(json!(n));
                    }
                }
                emit(true, v);
                return Ok(());
            }
            let parts = match (format, *color_mode) {
                (Format::ThreeMf, ColorMode::Parts) => {
                    format!(", {} colour part(s)", model.color_regions().len())
                }
                (Format::ThreeMf, ColorMode::Paint) => ", one solid with painted faces".to_string(),
                _ => String::new(),
            };
            println!(
                "\nWrote {} ({} triangles, millimetres{parts})",
                path.display(),
                out.mesh.tris.len()
            );
            if let Some(n) = &color_note {
                println!("note: {n}");
            }
            if painted {
                println!("Load these filaments in the slicer, in this order:");
                for f in &filaments {
                    println!(
                        "  filament {}: {} (LDraw {}, {}) — {} triangles{}",
                        f["slot"],
                        f["name"].as_str().unwrap(),
                        f["code"],
                        f["hex"].as_str().unwrap(),
                        f["triangles"],
                        if f["base"].as_bool().unwrap() {
                            ", unpainted base"
                        } else {
                            ""
                        }
                    );
                }
            }
        }
        Cmd::Preview {
            b,
            port,
            host,
            no_open,
        } => {
            let lib = make_lib(&cli, &cfg)?;
            let base = b.options(&cfg)?;
            // Fail fast on bad input before starting the server.
            let mut probe = make_lib(&cli, &cfg)?;
            probe.load_root(&base.input)?;
            PreviewServer {
                lib,
                cfg: cfg.clone(),
                base,
            }
            .serve(host, *port, !no_open)?;
        }
        Cmd::Inspect { b, problems } => {
            let mut lib = make_lib(&cli, &cfg)?;
            let opts = b.options(&cfg)?;
            let out = build(&mut lib, &cfg, &opts)?;
            if j {
                let mut v = build_json(&out, &lib, &opts, &cfg);
                let o = v.as_object_mut().unwrap();
                o.insert("ok".into(), json!(true));
                o.insert("command".into(), json!("inspect"));
                o.insert("files".into(), json!(out.files));
                o.insert("uncertified_files".into(), json!(out.uncertified_files));
                let mut diag = serde_json::to_value(&out.diagnostics)?;
                if let Some(m) = diag.as_object_mut() {
                    m.remove("issues"); // already at top level
                    m.remove("components"); // summarised at top level
                }
                o.insert("diagnostics".into(), diag);
                if let Some(n) = problems {
                    o.insert(
                        "problem_edges".into(),
                        problem_edges_json(&out.mesh, &out.files, *n),
                    );
                }
                emit(true, v);
                return Ok(());
            }
            print!("{}", report::summary(&out, opts.scale, true));
            let mut per_color: Vec<(u32, usize)> = Vec::new();
            for t in &out.mesh.tris {
                match per_color.iter_mut().find(|(c, _)| *c == t.color) {
                    Some(e) => e.1 += 1,
                    None => per_color.push((t.color, 1)),
                }
            }
            println!("\nColours:");
            for (c, n) in per_color {
                println!("  {c:>4} {:<28} {n} triangles", lib.colors.name(c));
            }
            println!("\nComponents:");
            for c in out.diagnostics.components.iter().take(30) {
                println!(
                    "  #{:<3} {:>6} tris  {}  {:.2}×{:.2}×{:.2} mm",
                    c.id,
                    c.triangles,
                    if c.closed { "closed" } else { "open  " },
                    c.bbox_max[0] - c.bbox_min[0],
                    c.bbox_max[1] - c.bbox_min[1],
                    c.bbox_max[2] - c.bbox_min[2]
                );
            }
            if out.diagnostics.components.len() > 30 {
                println!("  … {} more", out.diagnostics.components.len() - 30);
            }
            if let Some(n) = problems {
                print_problem_edges(&out.mesh, &out.files, *n);
            }
        }
        Cmd::Calibrate {
            connector,
            scale,
            clearances,
            part,
            wall_mm,
            height_mm,
            output,
            record,
            printer,
            material,
            date,
            notes,
        } => {
            let scale = scale.unwrap_or(cfg.default_scale);
            if let Some(n) = record {
                // Same coupon set as the original command, without writing files.
                let set = calibrate::generate(
                    &cfg,
                    &CalibrationRequest {
                        connector,
                        scale,
                        clearances: clearances.clone(),
                        part_diameter_mm: None,
                        wall_mm: *wall_mm,
                        height_mm: *height_mm,
                    },
                )?;
                let Some(coupon) = set.coupons.get(n.wrapping_sub(1)) else {
                    bail!("--record {n}: this coupon set has coupons 1..{} (pass the same --clearances as when printing)", set.coupons.len());
                };
                let preset = ["loose", "normal", "tight"].into_iter().find(|f| {
                    cfg.fit_clearance(connector, f)
                        .is_ok_and(|c| (c - coupon.clearance_mm).abs() < 1e-9)
                });
                let rec = ldraw2print::config::CalibrationRecord {
                    date: date.clone().unwrap_or_else(ldraw2print::config::today),
                    printer: printer.clone().unwrap_or_default(),
                    material: material.clone().unwrap_or_default(),
                    scale,
                    result: format!(
                        "coupon {n} ({}, {:+.3} mm clearance, {:.3} mm {}) fit best",
                        coupon.label,
                        coupon.clearance_mm,
                        coupon.diameter_mm,
                        if set.gender == Gender::Socket {
                            "bore"
                        } else {
                            "pin"
                        }
                    ),
                    clearance_mm: Some(coupon.clearance_mm),
                    notes: notes.clone(),
                };
                let path = match &cfg_path {
                    Some(p) => p.clone(),
                    None => {
                        let p = Config::user_config_path();
                        if let Some(d) = p.parent() {
                            std::fs::create_dir_all(d)?;
                        }
                        std::fs::write(&p, DEFAULT_CONFIG)?;
                        p
                    }
                };
                ldraw2print::config::record_calibration(&path, connector, &rec, preset)?;
                if j {
                    emit(
                        true,
                        json!({"ok": true, "command": "calibrate --record", "config_file": path, "connector": connector,
                                       "default_fit": preset, "calibration": rec}),
                    );
                } else {
                    println!("Recorded in {}:", path.display());
                    println!("  [connectors.{connector}]");
                    if let Some(p) = preset {
                        println!("  default_fit = \"{p}\"");
                    }
                    println!(
                        "  calibration: {}, {}, {}, {}:1, clearance {:+.3} mm",
                        rec.date, rec.printer, rec.material, rec.scale, coupon.clearance_mm
                    );
                    println!("`--fit clearance` now uses this clearance for '{connector}'. Check with `ldraw2print profiles`.");
                }
                return Ok(());
            }
            let mut part_diameter_mm = None;
            if let Some(p) = part {
                let mut lib = make_lib(&cli, &cfg)?;
                let mut o = BuildOptions::new(p, scale);
                o.connector = Some(connector.clone());
                o.check_intersections = false;
                let out = build(&mut lib, &cfg, &o)?;
                let c = out.connector.as_ref().unwrap();
                match (&c.selection, &c.selection_error) {
                    (Some(_), _) => {
                        part_diameter_mm = c.selected_mm.as_ref().map(|s| s.across_flats_mm)
                    }
                    (None, Some(e)) => return Err(errors::DecisionNeeded(e.clone()).into()),
                    _ => {}
                }
            }
            let set = calibrate::generate(
                &cfg,
                &CalibrationRequest {
                    connector,
                    scale,
                    clearances: clearances.clone(),
                    part_diameter_mm,
                    wall_mm: *wall_mm,
                    height_mm: *height_mm,
                },
            )?;
            let mesh = Mesh::from_source(&set.triangles, 1e-6);
            let colors = ldraw2print::ldraw::ColorTable::builtin();
            {
                let mut f = std::io::BufWriter::new(
                    std::fs::File::create(output)
                        .with_context(|| format!("creating {}", output.display()))?,
                );
                BinaryStl.write(
                    &PrintModel {
                        name: format!("calibration {connector}"),
                        mesh: &mesh,
                        colors: &colors,
                    },
                    &mut f,
                )?;
            }
            let summary = set.summary();
            let txt = output.with_extension("txt");
            std::fs::write(&txt, &summary)?;
            if j {
                let mut v = serde_json::to_value(&set)?;
                let o = v.as_object_mut().unwrap();
                o.insert("ok".into(), json!(true));
                o.insert("command".into(), json!("calibrate"));
                o.insert("output".into(), json!({ "stl": output, "summary": txt }));
                emit(true, v);
                return Ok(());
            }
            print!("{summary}");
            println!("\nWrote {} and {}", output.display(), txt.display());
        }
        Cmd::Search { words, limit, all } => {
            let Some((root, _)) = find_library(&cli, &cfg)? else {
                bail!("search needs the LDraw library. Run `ldraw2print library` for setup instructions.");
            };
            let (hits, total) = search_parts(&root, words, *all, *limit)?;
            if j {
                emit(
                    true,
                    json!({"ok": true, "command": "search", "query": words, "total": total, "hits": hits}),
                );
                return Ok(());
            }
            if hits.is_empty() {
                println!(
                    "No parts match {:?}{}.",
                    words.join(" "),
                    if *all {
                        ""
                    } else {
                        " (try --all to include obsolete/moved parts)"
                    }
                );
            }
            for h in &hits {
                println!(
                    "{:<16} {}{}{}",
                    h.part.trim_end_matches(".dat"),
                    h.description,
                    if h.variants > 0 {
                        format!(
                            "  ({} printed variant{})",
                            h.variants,
                            if h.variants == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    },
                    if h.unofficial { "  [unofficial]" } else { "" }
                );
            }
            if total > hits.len() {
                println!("… {} more (use --limit or add words)", total - hits.len());
            }
        }
        Cmd::Colors { query } => {
            let lib = make_lib(&cli, &cfg)?;
            let mut cols: Vec<_> = lib.colors.colors.values().collect();
            cols.sort_by_key(|c| c.code);
            let q = query.as_deref().map(str::to_lowercase);
            let norm = |n: &str| n.to_lowercase().replace('_', " ");
            let mut cols: Vec<_> = cols
                .into_iter()
                .filter(|c| match &q {
                    None => true,
                    Some(q) => c.code.to_string() == *q || norm(&c.name).contains(&norm(q)),
                })
                .collect();
            // Exact name (or code) matches first: `colors black` → 0 Black.
            if let Some(q) = &q {
                cols.sort_by_key(|c| !(norm(&c.name) == norm(q) || c.code.to_string() == *q));
            }
            if j {
                let list: Vec<Value> = cols
                    .iter()
                    .map(|c| json!({"code": c.code, "name": c.name, "hex": hex([c.rgb[0], c.rgb[1], c.rgb[2], 255]), "alpha": c.alpha, "edge_hex": hex([c.edge[0], c.edge[1], c.edge[2], 255])}))
                    .collect();
                emit(
                    true,
                    json!({"ok": true, "command": "colors", "source": lib.colors.source, "colors": list}),
                );
                return Ok(());
            }
            if cols.is_empty() {
                println!("No colours match.");
            }
            for c in cols {
                println!(
                    "{:>5}  {:<32} {}{}",
                    c.code,
                    c.name,
                    hex([c.rgb[0], c.rgb[1], c.rgb[2], 255]),
                    if c.alpha < 255 {
                        format!("  alpha {}", c.alpha)
                    } else {
                        String::new()
                    }
                );
            }
        }
        Cmd::Profiles { scale } => {
            let scale = scale.unwrap_or(cfg.default_scale);
            let mm = mm_per_ldu_for_scale(scale);
            let list: Vec<Value> = cfg
                .connectors
                .iter()
                .map(|(name, p)| {
                    let presets = p.fit.unwrap_or(cfg.fit_presets);
                    let (calibrated, customized) = profile_status(&cfg, name);
                    json!({
                        "name": name,
                        "calibrated": calibrated,
                        "calibration": p.calibration,
                        "customized": customized,
                        "default_fit_source": if calibrated { "calibration record" } else if customized.contains(&"default_fit") { "user config" } else { "built-in default" },
                        "description": p.description,
                        "gender": p.gender,
                        "axis": p.axis,
                        "nominal_diameter_ldu": p.nominal_diameter_ldu,
                        "nominal_diameter_mm": p.nominal_diameter_ldu.map(|d| d * mm),
                        "nominal_depth_ldu": p.nominal_depth_ldu,
                        "measured_diameter_mm": p.measured_diameter_mm,
                        "mating_diameter_mm": p.measured_diameter_mm.or(p.nominal_diameter_ldu.map(|d| d * mm)),
                        "default_fit": p.default_fit,
                        "default_clearance_mm": cfg.fit_clearance(name, "clearance").ok(),
                        "fit_presets_mm": presets,
                        "min_coverage": p.min_coverage,
                        "match_tolerance": p.match_tolerance,
                    })
                })
                .collect();
            if j {
                emit(
                    true,
                    json!({"ok": true, "command": "profiles", "scale": scale, "config_file": cfg_path, "printer": cfg.printer, "profiles": list}),
                );
                return Ok(());
            }
            println!(
                "Connector profiles at {scale}:1 ({}):\n",
                cfg_path
                    .as_ref()
                    .map(|p| format!("config {}", p.display()))
                    .unwrap_or("built-in defaults".into())
            );
            for (name, p) in &cfg.connectors {
                let mating = p
                    .measured_diameter_mm
                    .map(|d| format!("Ø{d:.3} mm measured"))
                    .or(p
                        .nominal_diameter_ldu
                        .map(|d| format!("Ø{:.3} mm nominal ({d} LDU)", d * mm)));
                let (calibrated, customized) = profile_status(&cfg, name);
                println!(
                    "  {:<11} {:<6} {:<36} fit {} ({:+.2} mm){}",
                    name,
                    match p.gender {
                        Gender::Socket => "socket",
                        Gender::Pin => "pin",
                    },
                    mating.unwrap_or("no nominal size".into()),
                    p.default_fit,
                    cfg.fit_clearance(name, "clearance").unwrap_or(0.0),
                    match (calibrated, customized.is_empty()) {
                        (true, _) => "  [calibrated]".to_string(),
                        (false, false) => "  [your setting, no calibration record]".to_string(),
                        _ => "  [built-in default]".to_string(),
                    }
                );
                if let Some(c) = &p.calibration {
                    println!(
                        "              calibrated {} on {}, {}, {}:1: {}",
                        c.date, c.printer, c.material, c.scale, c.result
                    );
                }
                println!("              {}", p.description);
            }
            println!(
                "\nPrinter compensation: holes {:+.3} mm, pins {:+.3} mm. Presets loose/normal/tight: {}/{}/{} mm.",
                cfg.printer.hole_compensation_mm, cfg.printer.pin_compensation_mm, cfg.fit_presets.loose, cfg.fit_presets.normal, cfg.fit_presets.tight
            );
        }
        Cmd::Guide { topic } => match topic.as_deref() {
            None => {
                if j {
                    emit(
                        true,
                        json!({"ok": true, "command": "guide", "topics": guide::TOPICS.iter().map(|t| json!({"name": t.name, "summary": t.summary})).collect::<Vec<_>>()}),
                    );
                    return Ok(());
                }
                println!("Guide topics (ldraw2print guide <topic>):\n");
                for t in guide::TOPICS {
                    println!("  {:<12} {}", t.name, t.summary);
                }
            }
            Some(name) => {
                let Some(t) = guide::TOPICS.iter().find(|t| t.name == name) else {
                    bail!(
                        "no guide topic '{name}'. Topics: {}",
                        guide::TOPICS
                            .iter()
                            .map(|t| t.name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                };
                if j {
                    emit(
                        true,
                        json!({"ok": true, "command": "guide", "topic": t.name, "text": t.text}),
                    );
                    return Ok(());
                }
                print!("{}", t.text);
            }
        },
        Cmd::Config { action } => match action {
            ConfigCmd::Init { path, force } => {
                let p = path.clone().unwrap_or_else(Config::user_config_path);
                if p.exists() && !force {
                    bail!("{} already exists (use --force to overwrite)", p.display());
                }
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&p, DEFAULT_CONFIG)?;
                if j {
                    emit(
                        true,
                        json!({"ok": true, "command": "config init", "path": p}),
                    );
                } else {
                    println!("Wrote {}", p.display());
                }
            }
            ConfigCmd::Show => {
                if j {
                    emit(
                        true,
                        json!({"ok": true, "command": "config show", "path": cfg_path, "config": serde_json::to_value(&cfg)?}),
                    );
                    return Ok(());
                }
                match &cfg_path {
                    Some(p) => println!("# loaded from {}", p.display()),
                    None => println!("# no config file found; built-in defaults"),
                }
                println!("{}", toml::to_string_pretty(&cfg)?);
            }
            ConfigCmd::Path => {
                if j {
                    let paths: Vec<Value> = Config::search_paths()
                        .iter()
                        .map(|p| json!({"path": p, "exists": p.is_file()}))
                        .collect();
                    emit(
                        true,
                        json!({"ok": true, "command": "config path", "paths": paths}),
                    );
                    return Ok(());
                }
                for p in Config::search_paths() {
                    println!("{} {}", if p.is_file() { "*" } else { " " }, p.display());
                }
            }
        },
        Cmd::Library => {
            let mut problem = None;
            let found = find_library(&cli, &cfg).unwrap_or_else(|e| {
                problem = Some(format!("{e:#}"));
                None
            });
            if j {
                let v = match &found {
                    Some((p, how)) => {
                        let lib = Library::new(Some(p.clone()));
                        json!({"ok": true, "command": "library", "path": p, "found_via": how,
                               "parts": std::fs::read_dir(p.join("parts")).map(|d| d.count()).unwrap_or(0),
                               "colours": lib.colors.colors.len(), "problem": problem})
                    }
                    None => {
                        json!({"ok": true, "command": "library", "path": null, "problem": problem,
                                   "searched": searched_locations(&cli, &cfg), "install": LIBRARY_HELP})
                    }
                };
                emit(true, v);
                return Ok(());
            }
            if let Some(pb) = &problem {
                println!("Problem: {pb}\n");
            }
            match found {
                Some((p, how)) => {
                    let lib = Library::new(Some(p.clone()));
                    let nparts = std::fs::read_dir(p.join("parts"))
                        .map(|d| d.count())
                        .unwrap_or(0);
                    println!("LDraw library: {} ({how})", p.display());
                    println!("  parts/ entries: {nparts}");
                    println!(
                        "  colours: {} from {}",
                        lib.colors.colors.len(),
                        lib.colors.source
                    );
                }
                None => {
                    println!("No usable LDraw library found. Looked at:");
                    for l in searched_locations(&cli, &cfg) {
                        println!("  {l}");
                    }
                }
            }
            println!("\n{LIBRARY_HELP}");
        }
    }
    Ok(())
}
