//! LDraw file parsing, library lookup, recursive subfile resolution,
//! colour inheritance and BFC (back-face culling) winding rules.
//!
//! The resolver flattens a part into world-space triangles (LDU, LDraw
//! coordinates) whose winding has been normalised so that, for BFC-certified
//! geometry, the right-hand-rule normal points *out of* the solid.
//! Triangles from non-certified files (or `BFC NOCLIP` sections) keep their
//! best-guess winding but are flagged `oriented = false`.

use crate::det::HashMap;
use crate::geometry::{triangulate_quad, Transform, Vec3};
use anyhow::{bail, Context, Result};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// LDraw colour code 16: "use the colour of the referencing line".
pub const COLOR_MAIN: u32 = 16;
/// LDraw colour code 24: "use the edge colour of the current colour".
pub const COLOR_EDGE: u32 = 24;

const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winding {
    Ccw,
    Cw,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Bfc {
    Certify(Option<Winding>),
    NoCertify,
    Winding(Winding),
    Clip(Option<Winding>),
    NoClip,
    InvertNext,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Bfc(Bfc),
    SubFile {
        color: u32,
        transform: Transform,
        name: String,
    },
    Line {
        color: u32,
        v: [Vec3; 2],
    },
    Triangle {
        color: u32,
        v: [Vec3; 3],
    },
    Quad {
        color: u32,
        v: [Vec3; 4],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Part,
    Subpart,
    Primitive,
    Model,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct LDrawFile {
    pub name: String,
    pub description: String,
    pub kind: FileKind,
    /// `0 BFC CERTIFY` appeared before any drawing command.
    pub certified: bool,
    pub default_winding: Winding,
    pub commands: Vec<Command>,
    /// Non-fatal syntax problems: (line number, message).
    pub warnings: Vec<(usize, String)>,
}

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

fn parse_color(tok: &str) -> Option<u32> {
    if let Some(hex) = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else if let Some(hex) = tok.strip_prefix('#') {
        // Some tools write direct colours as #2RRGGBB.
        u32::from_str_radix(hex, 16).ok()
    } else {
        tok.parse().ok()
    }
}

fn parse_floats<const N: usize>(toks: &[&str]) -> Option<[f64; N]> {
    if toks.len() < N {
        return None;
    }
    let mut out = [0.0; N];
    for (o, t) in out.iter_mut().zip(toks) {
        *o = t.parse().ok()?;
    }
    Some(out)
}

fn vecs<const N: usize>(f: &[f64]) -> [Vec3; N] {
    std::array::from_fn(|i| Vec3::new(f[i * 3], f[i * 3 + 1], f[i * 3 + 2]))
}

/// Remainder of `line` after skipping `n` whitespace-separated tokens
/// (type 1 file names may contain spaces).
fn rest_after_tokens(line: &str, n: usize) -> &str {
    let mut s = line.trim_start();
    for _ in 0..n {
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        s = s[end..].trim_start();
    }
    s.trim_end()
}

fn parse_bfc(words: &[&str]) -> Option<Bfc> {
    let up: Vec<String> = words.iter().map(|w| w.to_ascii_uppercase()).collect();
    let w: Vec<&str> = up.iter().map(String::as_str).collect();
    let dir = |s: Option<&&str>| match s {
        Some(&"CCW") => Some(Winding::Ccw),
        Some(&"CW") => Some(Winding::Cw),
        _ => None,
    };
    match w.as_slice() {
        ["CERTIFY", rest @ ..] => Some(Bfc::Certify(dir(rest.first()))),
        ["NOCERTIFY", ..] => Some(Bfc::NoCertify),
        ["CCW", "CLIP", ..] => Some(Bfc::Clip(Some(Winding::Ccw))),
        ["CW", "CLIP", ..] => Some(Bfc::Clip(Some(Winding::Cw))),
        ["CCW", ..] => Some(Bfc::Winding(Winding::Ccw)),
        ["CW", ..] => Some(Bfc::Winding(Winding::Cw)),
        ["CLIP", rest @ ..] => Some(Bfc::Clip(dir(rest.first()))),
        ["NOCLIP", ..] => Some(Bfc::NoClip),
        ["INVERTNEXT", ..] => Some(Bfc::InvertNext),
        _ => None,
    }
}

/// Parse a single LDraw file (not MPD-aware; see [`split_mpd`]).
pub fn parse_ldraw(name: &str, text: &str) -> LDrawFile {
    let mut f = LDrawFile {
        name: name.to_string(),
        description: String::new(),
        kind: FileKind::Unknown,
        certified: false,
        default_winding: Winding::Ccw,
        commands: Vec::new(),
        warnings: Vec::new(),
    };
    let mut seen_geometry = false;
    let mut first_comment = true;
    for (idx, raw) in text.lines().enumerate() {
        let lineno = idx + 1;
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        let warn = |f: &mut LDrawFile, m: &str| f.warnings.push((lineno, m.to_string()));
        match toks[0] {
            "0" => {
                if first_comment {
                    first_comment = false;
                    let d = rest_after_tokens(line, 1);
                    if !d.starts_with("!") && !d.to_ascii_uppercase().starts_with("BFC") {
                        f.description = d.to_string();
                    }
                }
                if toks.len() >= 2 && toks[1].eq_ignore_ascii_case("BFC") {
                    match parse_bfc(&toks[2..]) {
                        Some(Bfc::Certify(dir)) => {
                            if seen_geometry {
                                warn(&mut f, "BFC CERTIFY after drawing commands ignored");
                            } else {
                                f.certified = true;
                                f.default_winding = dir.unwrap_or(Winding::Ccw);
                            }
                        }
                        Some(Bfc::NoCertify) => f.certified = false,
                        Some(cmd) => f.commands.push(Command::Bfc(cmd)),
                        None => warn(&mut f, "unrecognised BFC statement"),
                    }
                } else if toks.len() >= 3 && toks[1] == "!LDRAW_ORG" {
                    let t = toks[2].to_ascii_lowercase();
                    f.kind = if t.contains("primitive") {
                        FileKind::Primitive
                    } else if t.contains("subpart") {
                        FileKind::Subpart
                    } else if t.contains("part") || t.contains("shortcut") {
                        FileKind::Part
                    } else if t.contains("model") {
                        FileKind::Model
                    } else {
                        FileKind::Unknown
                    };
                }
            }
            "1" => {
                seen_geometry = true;
                let parsed = (|| {
                    let color = parse_color(toks.get(1)?)?;
                    let n: [f64; 12] = parse_floats(&toks[2..])?;
                    let name = rest_after_tokens(line, 14);
                    if name.is_empty() {
                        return None;
                    }
                    Some(Command::SubFile {
                        color,
                        transform: Transform::from_ldraw(&n),
                        name: name.to_string(),
                    })
                })();
                match parsed {
                    Some(c) => f.commands.push(c),
                    None => warn(&mut f, "malformed type 1 line"),
                }
            }
            "2" | "3" | "4" => {
                seen_geometry = true;
                let parsed = (|| {
                    let color = parse_color(toks.get(1)?)?;
                    Some(match toks[0] {
                        "2" => Command::Line {
                            color,
                            v: vecs::<2>(&parse_floats::<6>(&toks[2..])?),
                        },
                        "3" => Command::Triangle {
                            color,
                            v: vecs::<3>(&parse_floats::<9>(&toks[2..])?),
                        },
                        _ => Command::Quad {
                            color,
                            v: vecs::<4>(&parse_floats::<12>(&toks[2..])?),
                        },
                    })
                })();
                match parsed {
                    Some(c) => f.commands.push(c),
                    None => warn(&mut f, &format!("malformed type {} line", toks[0])),
                }
            }
            "5" => {
                // Conditional lines only matter for rendering outlines.
                seen_geometry = true;
            }
            _ => warn(&mut f, "unknown line type"),
        }
    }
    f
}

/// Split a multi-part document (`0 FILE` / `0 NOFILE`) into its files. A
/// plain LDraw file is returned as a single entry with `default_name`.
pub fn split_mpd(default_name: &str, text: &str) -> Vec<(String, String)> {
    let has_file = text.lines().any(|l| is_file_line(l).is_some());
    if !has_file {
        return vec![(default_name.to_string(), text.to_string())];
    }
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some(name) = is_file_line(line) {
            if let Some(c) = current.take() {
                out.push(c);
            }
            current = Some((name, String::new()));
        } else if line.trim().eq_ignore_ascii_case("0 NOFILE") {
            if let Some(c) = current.take() {
                out.push(c);
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(c) = current.take() {
        out.push(c);
    }
    out
}

fn is_file_line(line: &str) -> Option<String> {
    let t: Vec<&str> = line.split_whitespace().collect();
    if t.len() >= 3 && t[0] == "0" && t[1].eq_ignore_ascii_case("FILE") {
        Some(rest_after_tokens(line, 2).to_string())
    } else {
        None
    }
}

/// Normalise an LDraw reference: backslashes → '/', lower-case.
pub fn normalize_name(name: &str) -> String {
    name.trim().replace('\\', "/").to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// Colours

#[derive(Debug, Clone)]
pub struct ColorDef {
    pub code: u32,
    pub name: String,
    pub rgb: [u8; 3],
    pub edge: [u8; 3],
    pub alpha: u8,
}

#[derive(Debug, Clone, Default)]
pub struct ColorTable {
    pub colors: HashMap<u32, ColorDef>,
    /// Where the table came from (LDConfig.ldr path or "built-in").
    pub source: String,
}

fn parse_hex_rgb(s: &str) -> Option<[u8; 3]> {
    let h = s.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// A small fallback palette used only when no `LDConfig.ldr` is available.
/// Values match LDConfig.ldr; load the real file for the full palette.
const BUILTIN_COLORS: &str = "\
0 !COLOUR Black CODE 0 VALUE #1B2A34 EDGE #808080
0 !COLOUR Blue CODE 1 VALUE #1E5AA8 EDGE #333333
0 !COLOUR Green CODE 2 VALUE #00852B EDGE #333333
0 !COLOUR Red CODE 4 VALUE #B40000 EDGE #333333
0 !COLOUR Brown CODE 6 VALUE #5F3109 EDGE #1E1E1E
0 !COLOUR Light_Grey CODE 7 VALUE #8A928D EDGE #333333
0 !COLOUR Dark_Grey CODE 8 VALUE #545955 EDGE #333333
0 !COLOUR Yellow CODE 14 VALUE #FAC80A EDGE #333333
0 !COLOUR White CODE 15 VALUE #F4F4F4 EDGE #333333
0 !COLOUR Main_Colour CODE 16 VALUE #FFFF80 EDGE #333333
0 !COLOUR Edge_Colour CODE 24 VALUE #7F7F7F EDGE #333333
0 !COLOUR Tan CODE 19 VALUE #E4CD9E EDGE #333333
0 !COLOUR Reddish_Brown CODE 70 VALUE #5F3109 EDGE #1E1E1E
0 !COLOUR Light_Bluish_Grey CODE 71 VALUE #969696 EDGE #333333
0 !COLOUR Dark_Bluish_Grey CODE 72 VALUE #646464 EDGE #333333
";

impl ColorTable {
    pub fn parse(text: &str, source: &str) -> Self {
        let mut t = ColorTable {
            colors: HashMap::default(),
            source: source.to_string(),
        };
        for line in text.lines() {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.len() < 4 || toks[0] != "0" || toks[1] != "!COLOUR" {
                continue;
            }
            let name = toks[2].to_string();
            let mut code = None;
            let mut rgb = None;
            let mut edge = None;
            let mut alpha = 255u8;
            for pair in toks[3..].windows(2) {
                match (pair[0].to_ascii_uppercase().as_str(), pair[1]) {
                    ("CODE", v) => code = v.parse().ok(),
                    ("VALUE", v) => rgb = parse_hex_rgb(v),
                    ("EDGE", v) => edge = parse_hex_rgb(v),
                    ("ALPHA", v) => alpha = v.parse().unwrap_or(255),
                    _ => {}
                }
            }
            if let (Some(code), Some(rgb)) = (code, rgb) {
                t.colors.insert(
                    code,
                    ColorDef {
                        code,
                        name,
                        rgb,
                        edge: edge.unwrap_or([0x33; 3]),
                        alpha,
                    },
                );
            }
        }
        t
    }

    pub fn builtin() -> Self {
        Self::parse(BUILTIN_COLORS, "built-in fallback")
    }

    /// RGBA for a colour code, including direct colours (`0x2RRGGBB`).
    pub fn rgba(&self, code: u32) -> [u8; 4] {
        if code >> 24 == 0x2 {
            return [(code >> 16) as u8, (code >> 8) as u8, code as u8, 255];
        }
        match self.colors.get(&code) {
            Some(c) => [c.rgb[0], c.rgb[1], c.rgb[2], c.alpha],
            None => [0x80, 0x80, 0x80, 255],
        }
    }

    pub fn edge_rgb(&self, code: u32) -> [u8; 3] {
        self.colors.get(&code).map(|c| c.edge).unwrap_or([0x33; 3])
    }

    pub fn name(&self, code: u32) -> String {
        if code >> 24 == 0x2 {
            return format!("Direct #{:06X}", code & 0xFFFFFF);
        }
        self.colors
            .get(&code)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("Unknown {code}"))
    }
}

// ---------------------------------------------------------------------------
// Library lookup

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum PrimitiveResolution {
    /// Use primitives exactly as referenced by the part (recommended).
    #[default]
    Standard,
    /// Prefer 48-segment primitives from `p/48/` when they exist.
    High,
    /// Prefer 8-segment primitives from `p/8/` when they exist.
    Low,
}

/// Locates and caches LDraw files.
pub struct Library {
    /// Root of an LDraw library (contains `parts/`, `p/`, `LDConfig.ldr`).
    pub root: Option<PathBuf>,
    /// Extra directories searched first (e.g. the input file's directory).
    pub extra_dirs: Vec<PathBuf>,
    pub resolution: PrimitiveResolution,
    pub colors: ColorTable,
    cache: HashMap<String, Option<Arc<LDrawFile>>>,
    /// Files defined inline (MPD documents, in-memory fixtures).
    embedded: HashMap<String, Arc<LDrawFile>>,
}

pub struct Located {
    pub key: String,
    pub file: Arc<LDrawFile>,
    pub is_primitive: bool,
}

impl Library {
    pub fn new(root: Option<PathBuf>) -> Self {
        let colors = root
            .as_ref()
            .and_then(|r| {
                let p = r.join("LDConfig.ldr");
                fs::read_to_string(&p)
                    .ok()
                    .map(|t| ColorTable::parse(&t, &p.display().to_string()))
            })
            .unwrap_or_else(ColorTable::builtin);
        Library {
            root,
            extra_dirs: Vec::new(),
            resolution: PrimitiveResolution::Standard,
            colors,
            cache: HashMap::default(),
            embedded: HashMap::default(),
        }
    }

    /// Register an in-memory file (used for MPD sub-files and tests).
    pub fn add_embedded(&mut self, name: &str, text: &str) {
        let key = normalize_name(name);
        let f = parse_ldraw(&key, text);
        self.embedded.insert(key, Arc::new(f));
    }

    fn search_dirs(&self) -> Vec<(PathBuf, bool)> {
        // (directory, contents-are-primitives)
        let mut dirs: Vec<(PathBuf, bool)> =
            self.extra_dirs.iter().map(|d| (d.clone(), false)).collect();
        if let Some(r) = &self.root {
            dirs.push((r.join("parts"), false));
            dirs.push((r.join("p"), true));
            dirs.push((r.join("models"), false));
            for un in ["UnOfficial", "unofficial", "Unofficial"] {
                let u = r.join(un);
                if u.is_dir() {
                    dirs.push((u.join("parts"), false));
                    dirs.push((u.join("p"), true));
                    break;
                }
            }
        }
        dirs
    }

    fn candidate_names(&self, key: &str) -> Vec<String> {
        let mut v = Vec::new();
        let is_res_prefixed = key.starts_with("48/") || key.starts_with("8/");
        match self.resolution {
            PrimitiveResolution::High if !is_res_prefixed => v.push(format!("48/{key}")),
            PrimitiveResolution::Low if !is_res_prefixed => v.push(format!("8/{key}")),
            _ => {}
        }
        v.push(key.to_string());
        v
    }

    /// Find a file by LDraw reference name.
    pub fn locate(&mut self, name: &str) -> Result<Option<Located>> {
        let key = normalize_name(name);
        if let Some(f) = self.embedded.get(&key) {
            return Ok(Some(Located {
                key,
                is_primitive: f.kind == FileKind::Primitive,
                file: f.clone(),
            }));
        }
        for cand in self.candidate_names(&key) {
            let substituted = cand != key;
            for (dir, prim_dir) in self.search_dirs() {
                // Resolution substitution only applies inside primitive dirs.
                if substituted && !prim_dir {
                    continue;
                }
                let cache_key = format!("{}|{}", dir.display(), cand);
                let cached = match self.cache.get(&cache_key) {
                    Some(c) => c.clone(),
                    None => {
                        let loaded = load_case_insensitive(&dir, &cand)?
                            .map(|(_, text)| Arc::new(parse_ldraw(&cand, &text)));
                        self.cache.insert(cache_key, loaded.clone());
                        loaded
                    }
                };
                if let Some(file) = cached {
                    let is_primitive = prim_dir || file.kind == FileKind::Primitive;
                    return Ok(Some(Located {
                        key: cand,
                        file,
                        is_primitive,
                    }));
                }
            }
        }
        Ok(None)
    }

    /// Load the root file given either an explicit path or a part identifier
    /// such as `3901` / `3901.dat`. Returns the reference key to resolve.
    pub fn load_root(&mut self, input: &str) -> Result<String> {
        let path = Path::new(input);
        if path.is_file() {
            let text =
                fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            if let Some(dir) = path.parent() {
                let dir = if dir.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    dir
                };
                if !self.extra_dirs.iter().any(|d| d == dir) {
                    self.extra_dirs.insert(0, dir.to_path_buf());
                }
            }
            let fname = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let files = split_mpd(&fname, &text);
            let root_key = normalize_name(&files[0].0);
            for (n, body) in &files {
                self.add_embedded(n, body);
            }
            return Ok(root_key);
        }
        let mut name = input.to_string();
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".dat") || lower.ends_with(".ldr") || lower.ends_with(".mpd")) {
            name.push_str(".dat");
        }
        if self.locate(&name)?.is_none() {
            match &self.root {
                None => bail!(
                    "'{input}' is not a file and no LDraw library is configured.\n\
                     Set --ldraw-dir, the LDRAWDIR environment variable, or `ldraw_dir` in the config file.\n\
                     Run `ldraw2print library` for setup instructions."
                ),
                Some(r) => bail!("part '{name}' not found in LDraw library at {}", r.display()),
            }
        }
        Ok(normalize_name(&name))
    }
}

fn load_case_insensitive(dir: &Path, rel: &str) -> Result<Option<(PathBuf, String)>> {
    let p = dir.join(rel);
    if p.is_file() {
        let t = read_text(&p)?;
        return Ok(Some((p, t)));
    }
    // Case-sensitive filesystems: walk components matching case-insensitively.
    let mut cur = dir.to_path_buf();
    for comp in rel.split('/') {
        let Ok(rd) = fs::read_dir(&cur) else {
            return Ok(None);
        };
        let found = rd
            .flatten()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(comp))
            .map(|e| e.path());
        match found {
            Some(f) => cur = f,
            None => return Ok(None),
        }
    }
    if cur.is_file() {
        let t = read_text(&cur)?;
        Ok(Some((cur, t)))
    } else {
        Ok(None)
    }
}

fn read_text(p: &Path) -> Result<String> {
    let bytes = fs::read(p).with_context(|| format!("reading {}", p.display()))?;
    // LDraw files are nominally UTF-8 but older ones are Latin-1.
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

// ---------------------------------------------------------------------------
// Resolution

#[derive(Debug, Clone)]
pub struct SourceTriangle {
    /// World-space vertices (LDU, LDraw axes), wound so the normal points
    /// outwards when `oriented` is true.
    pub v: [Vec3; 3],
    /// Resolved colour code (never 16).
    pub color: u32,
    /// Winding is known from BFC certification.
    pub oriented: bool,
    /// Index into [`Resolved::files`] of the file that defined it.
    pub file: u32,
}

#[derive(Debug, Clone)]
pub struct SourceEdge {
    pub v: [Vec3; 2],
    /// Colour code; if `complement` the edge colour of that code is meant.
    pub color: u32,
    pub complement: bool,
}

/// A referenced primitive with its world transform; used for connector
/// detection (e.g. a `4-4cyli.dat` scaled to radius 6 is a stud-sized wall).
#[derive(Debug, Clone)]
pub struct PrimitiveInstance {
    pub name: String,
    pub transform: Transform,
    pub tri_start: usize,
    pub tri_end: usize,
    pub edge_start: usize,
    pub edge_end: usize,
    pub oriented: bool,
    /// Segments per full circle after smoothing (None = as authored).
    pub segments: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct MissingRef {
    pub name: String,
    pub referenced_from: String,
}

#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub root: String,
    pub description: String,
    pub triangles: Vec<SourceTriangle>,
    pub edges: Vec<SourceEdge>,
    pub primitives: Vec<PrimitiveInstance>,
    pub files: Vec<String>,
    pub missing: Vec<MissingRef>,
    pub warnings: Vec<String>,
    pub quads: usize,
    pub uncertified_files: Vec<String>,
    /// Triangles that take the top-level main colour (LDraw colour 16).
    pub main_color_tris: usize,
}

#[derive(Debug)]
pub enum ResolveError {
    Circular(Vec<String>),
    TooDeep(Vec<String>),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Circular(chain) => {
                write!(f, "circular reference: {}", chain.join(" -> "))
            }
            ResolveError::TooDeep(chain) => {
                write!(
                    f,
                    "reference nesting deeper than {MAX_DEPTH}: {}",
                    chain.join(" -> ")
                )
            }
        }
    }
}
impl std::error::Error for ResolveError {}

struct Walker<'a> {
    lib: &'a mut Library,
    out: Resolved,
    stack: Vec<String>,
    file_index: HashMap<String, u32>,
}

impl Library {
    /// Resolve `name` into flattened world geometry, with `root_color`
    /// substituted for colour 16 at the top level.
    pub fn resolve(&mut self, name: &str, root_color: u32) -> Result<Resolved> {
        let located = self
            .locate(name)?
            .with_context(|| format!("file '{name}' not found"))?;
        let mut w = Walker {
            lib: self,
            out: Resolved::default(),
            stack: Vec::new(),
            file_index: HashMap::default(),
        };
        w.out.root = located.key.clone();
        w.out.description = located.file.description.clone();
        w.walk(
            &located.key,
            &located.file,
            &Transform::IDENTITY,
            root_color,
            false,
            true,
            true,
        )?;
        Ok(w.out)
    }
}

impl Walker<'_> {
    fn file_id(&mut self, key: &str) -> u32 {
        if let Some(&i) = self.file_index.get(key) {
            return i;
        }
        let i = self.out.files.len() as u32;
        self.out.files.push(key.to_string());
        self.file_index.insert(key.to_string(), i);
        i
    }

    // Recursive state is passed explicitly; bundling it would only obscure it.
    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        key: &str,
        file: &Arc<LDrawFile>,
        xf: &Transform,
        color: u32,
        invert: bool,
        parent_oriented: bool,
        // Colour 16 here still means the top-level main colour.
        main: bool,
    ) -> Result<()> {
        if self.stack.iter().any(|k| k == key) {
            let mut chain = self.stack.clone();
            chain.push(key.to_string());
            return Err(ResolveError::Circular(chain).into());
        }
        if self.stack.len() >= MAX_DEPTH {
            return Err(ResolveError::TooDeep(self.stack.clone()).into());
        }
        self.stack.push(key.to_string());
        let fid = self.file_id(key);
        for (line, msg) in &file.warnings {
            self.out.warnings.push(format!("{key}:{line}: {msg}"));
        }
        if !file.certified
            && parent_oriented
            && !self.out.uncertified_files.iter().any(|f| f == key)
        {
            self.out.uncertified_files.push(key.to_string());
        }

        let mut winding = file.default_winding;
        let mut clip = true;
        let mut invert_next = false;
        let mirrored = xf.is_mirroring();

        let resolve_color = |c: u32| {
            if c == COLOR_MAIN || c == COLOR_EDGE {
                color
            } else {
                c
            }
        };

        for cmd in &file.commands {
            let oriented = parent_oriented && file.certified && clip;
            // Flip so that the stored winding is CCW-outward in world space.
            let flip = (winding == Winding::Cw) ^ invert ^ mirrored;
            match cmd {
                Command::Bfc(b) => match b {
                    Bfc::Winding(w) => winding = *w,
                    Bfc::Clip(w) => {
                        clip = true;
                        if let Some(w) = w {
                            winding = *w;
                        }
                    }
                    Bfc::NoClip => clip = false,
                    Bfc::InvertNext => invert_next = true,
                    Bfc::Certify(_) | Bfc::NoCertify => {}
                },
                Command::SubFile {
                    color: c,
                    transform,
                    name,
                } => {
                    let child_invert = invert ^ invert_next;
                    invert_next = false;
                    let child_color = resolve_color(*c);
                    let child_xf = xf.compose(transform);
                    match self.lib.locate(name)? {
                        None => self.out.missing.push(MissingRef {
                            name: normalize_name(name),
                            referenced_from: key.to_string(),
                        }),
                        Some(loc) => {
                            let start = self.out.triangles.len();
                            let edge_start = self.out.edges.len();
                            let child_main = main && (*c == COLOR_MAIN || *c == COLOR_EDGE);
                            self.walk(
                                &loc.key,
                                &loc.file,
                                &child_xf,
                                child_color,
                                child_invert,
                                oriented,
                                child_main,
                            )?;
                            if loc.is_primitive {
                                self.out.primitives.push(PrimitiveInstance {
                                    name: loc.key.clone(),
                                    transform: child_xf,
                                    tri_start: start,
                                    tri_end: self.out.triangles.len(),
                                    edge_start,
                                    edge_end: self.out.edges.len(),
                                    oriented: oriented && loc.file.certified,
                                    segments: None,
                                });
                            }
                        }
                    }
                }
                Command::Line { color: c, v } => {
                    invert_next = false;
                    self.out.edges.push(SourceEdge {
                        v: [xf.apply(v[0]), xf.apply(v[1])],
                        color: resolve_color(*c),
                        complement: *c == COLOR_EDGE,
                    });
                }
                Command::Triangle { color: c, v } => {
                    invert_next = false;
                    if main && (*c == COLOR_MAIN || *c == COLOR_EDGE) {
                        self.out.main_color_tris += 1;
                    }
                    self.emit(
                        [v[0], v[1], v[2]],
                        xf,
                        resolve_color(*c),
                        flip,
                        oriented,
                        fid,
                    );
                }
                Command::Quad { color: c, v } => {
                    invert_next = false;
                    self.out.quads += 1;
                    if main && (*c == COLOR_MAIN || *c == COLOR_EDGE) {
                        self.out.main_color_tris += 2;
                    }
                    for t in triangulate_quad(*v) {
                        self.emit(t, xf, resolve_color(*c), flip, oriented, fid);
                    }
                }
            }
        }
        self.stack.pop();
        Ok(())
    }

    fn emit(
        &mut self,
        v: [Vec3; 3],
        xf: &Transform,
        color: u32,
        flip: bool,
        oriented: bool,
        fid: u32,
    ) {
        let mut w = [xf.apply(v[0]), xf.apply(v[1]), xf.apply(v[2])];
        if flip {
            w.swap(1, 2);
        }
        self.out.triangles.push(SourceTriangle {
            v: w,
            color,
            oriented,
            file: fid,
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn library_lookup_ignores_case() {
        // LDraw references files in any case. On a case-sensitive filesystem
        // (Linux) this goes through the directory walk; macOS rarely does.
        let dir = std::env::temp_dir().join(format!("l2p-case-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("parts/S")).unwrap();
        std::fs::create_dir_all(dir.join("p")).unwrap();
        std::fs::write(
            dir.join("parts/S/Mixed.DAT"),
            "0 Sub\n3 16 0 0 0 1 0 0 0 0 1\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("parts/Upper.dat"),
            "0 Part\n0 BFC CERTIFY CCW\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 s\\mixed.dat\n",
        )
        .unwrap();
        let (_, text) = load_case_insensitive(&dir.join("parts"), "s/mixed.dat")
            .unwrap()
            .unwrap();
        assert!(text.starts_with("0 Sub"));
        assert!(load_case_insensitive(&dir.join("parts"), "s/missing.dat")
            .unwrap()
            .is_none());
        let mut lib = Library::new(Some(dir.clone()));
        let root = lib
            .locate("UPPER.DAT")
            .unwrap()
            .expect("found despite case");
        assert_eq!(root.file.description, "Part");
        assert_eq!(lib.resolve("upper.dat", 4).unwrap().triangles.len(), 1);
        assert!(lib.locate("s\\MIXED.dat").unwrap().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_ranks_names_and_base_parts() {
        let dir = std::env::temp_dir().join(format!("l2p-search-{}", std::process::id()));
        let parts = dir.join("parts");
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::create_dir_all(dir.join("p")).unwrap();
        let mk = |n: &str, d: &str, kw: &str| {
            std::fs::write(
                parts.join(format!("{n}.dat")),
                format!("0 {d}\n0 Name: {n}.dat\n{kw}"),
            )
            .unwrap();
        };
        mk("100", "Minifig Hair Female Long", "");
        mk("101", "Minifig Hair Male", "");
        mk("200", "Minifig Head Pig", "");
        mk("201", "Minifig Head with Hollow Stud", "");
        for k in 0..12 {
            mk(&format!("201p{k:02}"), "Minifig Head with Face Pattern", "");
        }
        mk("300", "Minifig Food Carrot", "0 !KEYWORDS torch");
        mk("301", "Minifig Torch", "");
        let s = |q: &str| {
            let t: Vec<String> = vec![q.to_string()];
            search_parts(&dir, &t, false, 3)
                .unwrap()
                .0
                .into_iter()
                .map(|h| h.part)
                .collect::<Vec<_>>()
        };
        assert_eq!(s("hair male")[0], "101.dat", "word match: male ≠ female");
        assert_eq!(s("minifig head")[0], "201.dat", "common base part first");
        assert_eq!(
            s("torch"),
            ["301.dat", "300.dat"],
            "name before keyword-only"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    use super::*;
    use crate::geometry::triangle_normal;

    fn lib_with(files: &[(&str, &str)]) -> Library {
        let mut l = Library::new(None);
        for (n, t) in files {
            l.add_embedded(n, t);
        }
        l
    }

    fn normal(t: &SourceTriangle) -> Vec3 {
        triangle_normal(t.v[0], t.v[1], t.v[2])
    }

    // Triangle in the XY plane with normal +Z.
    const UP_TRI: &str = "0 BFC CERTIFY CCW\n3 16 0 0 0 1 0 0 0 1 0\n";

    #[test]
    fn parses_triangle_and_quad() {
        let f = parse_ldraw(
            "t.dat",
            "0 Test\n0 BFC CERTIFY CCW\n3 4 0 0 0 1 0 0 0 1 0\n4 1 0 0 0 1 0 0 1 1 0 0 1 0\n2 24 0 0 0 1 1 1\n5 24 0 0 0 1 1 1 0 0 0 1 1 1\n",
        );
        assert_eq!(f.description, "Test");
        assert!(f.certified);
        assert_eq!(f.commands.len(), 3);
        let mut l = lib_with(&[(
            "t.dat",
            "0 BFC CERTIFY CCW\n3 4 0 0 0 1 0 0 0 1 0\n4 1 0 0 0 1 0 0 1 1 0 0 1 0\n",
        )]);
        let r = l.resolve("t.dat", 15).unwrap();
        assert_eq!(r.triangles.len(), 3);
        assert_eq!(r.quads, 1);
        assert_eq!(r.triangles[0].color, 4);
        assert_eq!(r.triangles[1].color, 1);
    }

    #[test]
    fn type1_filename_with_spaces_and_backslash() {
        let f = parse_ldraw("m.ldr", "1 16 0 0 0 1 0 0 0 1 0 0 0 1 S\\My Sub File.dat\n");
        match &f.commands[0] {
            Command::SubFile { name, .. } => assert_eq!(normalize_name(name), "s/my sub file.dat"),
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn nested_references_compose_transforms() {
        let mut l = lib_with(&[
            ("leaf.dat", UP_TRI),
            (
                "mid.dat",
                "0 BFC CERTIFY CCW\n1 16 10 0 0 1 0 0 0 1 0 0 0 1 leaf.dat\n",
            ),
            (
                "top.dat",
                "0 BFC CERTIFY CCW\n1 16 0 0 0 2 0 0 0 2 0 0 0 2 mid.dat\n",
            ),
        ]);
        let r = l.resolve("top.dat", 4).unwrap();
        assert_eq!(r.triangles.len(), 1);
        let t = &r.triangles[0];
        // leaf (1,0,0) → mid +10 → (11,0,0) → top ×2 → (22,0,0)
        assert!(t
            .v
            .iter()
            .any(|v| (*v - Vec3::new(22., 0., 0.)).length() < 1e-9));
        assert!(t
            .v
            .iter()
            .any(|v| (*v - Vec3::new(20., 0., 0.)).length() < 1e-9));
    }

    #[test]
    fn mirrored_reference_flips_winding_back() {
        let mut l = lib_with(&[
            ("leaf.dat", UP_TRI),
            (
                "m.dat",
                "0 BFC CERTIFY CCW\n1 16 0 0 0 1 0 0 0 1 0 0 0 -1 leaf.dat\n",
            ),
        ]);
        let r = l.resolve("m.dat", 4).unwrap();
        // Mirroring z maps the triangle onto itself but swaps which side is
        // "outside": the outward normal +Z becomes -Z.
        assert!(normal(&r.triangles[0]).z < 0.0);
        assert!(r.triangles[0].oriented);

        // Mirror in X: geometry moves to -x, normal must stay +Z after the fix.
        let mut l = lib_with(&[
            ("leaf.dat", UP_TRI),
            (
                "m.dat",
                "0 BFC CERTIFY CCW\n1 16 0 0 0 -1 0 0 0 1 0 0 0 1 leaf.dat\n",
            ),
        ]);
        let r = l.resolve("m.dat", 4).unwrap();
        assert!(normal(&r.triangles[0]).z > 0.0);
    }

    #[test]
    fn bfc_cw_invertnext_and_uncertified() {
        let mut l = lib_with(&[("cw.dat", "0 BFC CERTIFY CW\n3 16 0 0 0 1 0 0 0 1 0\n")]);
        let r = l.resolve("cw.dat", 4).unwrap();
        assert!(
            normal(&r.triangles[0]).z < 0.0,
            "CW file must be flipped to CCW"
        );

        let mut l = lib_with(&[
            ("leaf.dat", UP_TRI),
            ("inv.dat", "0 BFC CERTIFY CCW\n0 BFC INVERTNEXT\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 leaf.dat\n1 16 0 0 5 1 0 0 0 1 0 0 0 1 leaf.dat\n"),
        ]);
        let r = l.resolve("inv.dat", 4).unwrap();
        assert!(
            normal(&r.triangles[0]).z < 0.0,
            "INVERTNEXT applies to the next reference"
        );
        assert!(
            normal(&r.triangles[1]).z > 0.0,
            "and only to the next reference"
        );

        // INVERTNEXT combined with a mirror cancels out.
        let mut l = lib_with(&[
            ("leaf.dat", UP_TRI),
            (
                "im.dat",
                "0 BFC CERTIFY CCW\n0 BFC INVERTNEXT\n1 16 0 0 0 -1 0 0 0 1 0 0 0 1 leaf.dat\n",
            ),
        ]);
        let r = l.resolve("im.dat", 4).unwrap();
        assert!(normal(&r.triangles[0]).z < 0.0);

        // Uncertified parent: children are not oriented even if certified.
        let mut l = lib_with(&[
            ("leaf.dat", UP_TRI),
            ("u.dat", "1 16 0 0 0 1 0 0 0 1 0 0 0 1 leaf.dat\n"),
        ]);
        let r = l.resolve("u.dat", 4).unwrap();
        assert!(!r.triangles[0].oriented);
        assert_eq!(r.uncertified_files, vec!["u.dat".to_string()]);

        // NOCLIP section is not oriented.
        let mut l = lib_with(&[("nc.dat", "0 BFC CERTIFY CCW\n0 BFC NOCLIP\n3 16 0 0 0 1 0 0 0 1 0\n0 BFC CLIP\n3 16 0 0 0 1 0 0 0 1 0\n")]);
        let r = l.resolve("nc.dat", 4).unwrap();
        assert!(!r.triangles[0].oriented);
        assert!(r.triangles[1].oriented);
    }

    #[test]
    fn color_inheritance() {
        let mut l = lib_with(&[
            ("leaf.dat", "0 BFC CERTIFY CCW\n3 16 0 0 0 1 0 0 0 1 0\n3 2 0 0 0 1 0 0 0 1 0\n2 24 0 0 0 1 0 0\n"),
            ("mid.dat", "0 BFC CERTIFY CCW\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 leaf.dat\n1 4 0 0 0 1 0 0 0 1 0 0 0 1 leaf.dat\n"),
        ]);
        let r = l.resolve("mid.dat", 14).unwrap();
        let colors: Vec<u32> = r.triangles.iter().map(|t| t.color).collect();
        assert_eq!(colors, vec![14, 2, 4, 2]);
        assert!(r.edges[0].complement);
        assert_eq!(r.edges[0].color, 14);
        assert_eq!(r.edges[1].color, 4);
    }

    #[test]
    fn missing_and_circular() {
        let mut l = lib_with(&[(
            "a.dat",
            "1 16 0 0 0 1 0 0 0 1 0 0 0 1 nothere.dat\n3 16 0 0 0 1 0 0 0 1 0\n",
        )]);
        let r = l.resolve("a.dat", 4).unwrap();
        assert_eq!(r.missing.len(), 1);
        assert_eq!(r.missing[0].name, "nothere.dat");
        assert_eq!(r.triangles.len(), 1);

        let mut l = lib_with(&[
            ("a.dat", "1 16 0 0 0 1 0 0 0 1 0 0 0 1 b.dat\n"),
            ("b.dat", "1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.dat\n"),
        ]);
        let err = l.resolve("a.dat", 4).unwrap_err().to_string();
        assert!(err.contains("circular"), "{err}");
        assert!(err.contains("a.dat -> b.dat -> a.dat"), "{err}");
    }

    #[test]
    fn diamond_reference_is_not_circular() {
        let mut l = lib_with(&[
            ("leaf.dat", UP_TRI),
            (
                "a.dat",
                "1 16 0 0 0 1 0 0 0 1 0 0 0 1 leaf.dat\n1 16 5 0 0 1 0 0 0 1 0 0 0 1 leaf.dat\n",
            ),
        ]);
        assert_eq!(l.resolve("a.dat", 4).unwrap().triangles.len(), 2);
    }

    #[test]
    fn mpd_split() {
        let files = split_mpd("x.mpd", "0 FILE main.ldr\n1 16 0 0 0 1 0 0 0 1 0 0 0 1 sub.ldr\n0 NOFILE\n0 FILE sub.ldr\n3 16 0 0 0 1 0 0 0 1 0\n");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "main.ldr");
        assert!(files[1].1.contains("3 16"));
    }

    #[test]
    fn colour_table_parses_ldconfig_lines() {
        let t = ColorTable::parse(
            "0 !COLOUR Trans_Clear CODE 47 VALUE #FCFCFC EDGE #C3C3C3 ALPHA 128\n0 !COLOUR Red CODE 4 VALUE #C91A09 EDGE #333333\n",
            "test",
        );
        assert_eq!(t.rgba(47), [0xFC, 0xFC, 0xFC, 128]);
        assert_eq!(t.rgba(4), [0xC9, 0x1A, 0x09, 255]);
        assert_eq!(t.rgba(0x2FF8800), [0xFF, 0x88, 0x00, 255]);
        assert_eq!(t.edge_rgb(47), [0xC3, 0xC3, 0xC3]);
    }
}

// ---------------------------------------------------------------------------
// Part search

/// A part found by [`search_parts`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct PartHit {
    /// Reference name, e.g. "2528ap30.dat".
    pub part: String,
    pub description: String,
    pub category: Option<String>,
    /// LDraw `!KEYWORDS` (themes, nicknames, BrickLink/Rebrickable ids, sets).
    pub keywords: Vec<String>,
    /// Unofficial library part.
    pub unofficial: bool,
    /// Obsolete/moved/alias/physical-colour entry (hidden unless asked for).
    pub secondary: bool,
    /// Printed/pattern variants in the library (`<number>p…`). Many variants
    /// = a common base part (e.g. the standard head 3626c).
    pub variants: usize,
}

fn read_header(path: &Path) -> Option<(String, Option<String>, Vec<String>)> {
    use std::io::Read;
    let mut buf = vec![0u8; 16384];
    let n = fs::File::open(path).ok()?.read(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);
    let mut lines = text.lines();
    let first = lines.next()?.trim_start_matches('\u{feff}').trim();
    let desc = first.strip_prefix('0')?.trim().to_string();
    let category = text.lines().find_map(|l| {
        l.trim()
            .strip_prefix("0 !CATEGORY")
            .map(|c| c.trim().to_string())
    });
    let keywords = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("0 !KEYWORDS"))
        .flat_map(|k| k.split(','))
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    Some((desc, category, keywords))
}

/// Search part descriptions (and part numbers) in a library. Every term must
/// match, case-insensitively. Returns (hits, total matches before `limit`).
pub fn search_parts(
    root: &Path,
    terms: &[String],
    include_secondary: bool,
    limit: usize,
) -> Result<(Vec<PartHit>, usize)> {
    // Split each argument into words, so `search "pirate bicorne"` works too.
    let terms: Vec<String> = terms
        .iter()
        .flat_map(|t| t.split_whitespace())
        .map(str::to_lowercase)
        .collect();
    let mut dirs = vec![(root.join("parts"), false)];
    for un in ["UnOfficial", "unofficial", "Unofficial"] {
        let d = root.join(un).join("parts");
        if d.is_dir() {
            dirs.push((d, true));
            break;
        }
    }
    let mut hits = Vec::new();
    let mut all_names: Vec<String> = Vec::new();
    for (dir, unofficial) in dirs {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if !name.ends_with(".dat") {
                continue;
            }
            all_names.push(name.clone());
            let Some((description, category, keywords)) = read_header(&e.path()) else {
                continue;
            };
            let secondary = description.starts_with(['~', '=', '_', '|'])
                || description.to_lowercase().contains("moved to");
            if secondary && !include_secondary {
                continue;
            }
            let hay = format!(
                "{} {} {} {}",
                name.trim_end_matches(".dat"),
                description,
                category.as_deref().unwrap_or(""),
                keywords.join(" ")
            )
            .to_lowercase();
            if terms.iter().all(|t| hay.contains(t.as_str())) {
                hits.push(PartHit {
                    part: name,
                    description,
                    category,
                    keywords,
                    unofficial,
                    secondary,
                    variants: 0,
                });
            }
        }
    }
    // Count printed variants: names starting with "<number>p".
    all_names.sort_unstable();
    for h in &mut hits {
        let prefix = format!("{}p", h.part.trim_end_matches(".dat"));
        let start = all_names.partition_point(|n| n.as_str() < prefix.as_str());
        h.variants = all_names[start..]
            .iter()
            .take_while(|n| n.starts_with(&prefix))
            .count();
    }
    // Ranking tiers: exact part number; the words in order at the start of
    // the description ("Minifig Head …"); every word in the number or
    // description; keyword-only matches. Within a tier, common base parts
    // (many printed variants, compared in bands so 3 vs 0 doesn't outweigh a
    // plainer name; under 10 variants counts as none) first, then shorter descriptions.
    let band = |v: usize| {
        if v < 10 {
            0
        } else {
            (v as f64).log10().floor() as i32
        }
    };
    let exact = |h: &PartHit| {
        terms
            .iter()
            .any(|t| h.part.trim_end_matches(".dat") == t.as_str())
    };
    let words_of = |h: &PartHit| -> Vec<String> {
        format!("{} {}", h.part.trim_end_matches(".dat"), h.description)
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    };
    // The query words appear consecutively, as word prefixes ("hair male").
    let phrase = |h: &PartHit| {
        let w = words_of(h);
        w.windows(terms.len()).any(|win| {
            win.iter()
                .zip(&terms)
                .all(|(w, t)| w.starts_with(t.as_str()))
        })
    };
    // Every query word starts some word of the number/description
    // (so "male" doesn't match "female").
    let named = |h: &PartHit| {
        let w = words_of(h);
        terms
            .iter()
            .all(|t| w.iter().any(|w| w.starts_with(t.as_str())))
    };
    hits.sort_by(|a, b| {
        exact(b)
            .cmp(&exact(a))
            .then(phrase(b).cmp(&phrase(a)))
            .then(named(b).cmp(&named(a)))
            .then(a.secondary.cmp(&b.secondary))
            .then(band(b.variants).cmp(&band(a.variants)))
            .then(a.description.len().cmp(&b.description.len()))
            .then(a.part.cmp(&b.part))
    });
    let total = hits.len();
    hits.truncate(limit);
    Ok((hits, total))
}
