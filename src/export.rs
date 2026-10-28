//! Export: binary STL and the JSON geometry consumed by the browser preview.
//!
//! All exporters receive a [`PrintModel`]: a mesh already in print space
//! (millimetres, Z up) that still carries per-triangle LDraw colour codes
//! and component ids. STL drops colour; a future 3MF exporter can emit one
//! object or volume per colour (Bambu Studio maps those to AMS filaments)
//! from exactly the same data.

use crate::geometry::{triangle_normal, Vec3};
use crate::ldraw::ColorTable;
use crate::mesh::Mesh;
use anyhow::Result;
use serde::Serialize;
use std::io::Write;

pub struct PrintModel<'a> {
    pub name: String,
    /// Print space: mm, Z up.
    pub mesh: &'a Mesh,
    pub colors: &'a ColorTable,
}

impl PrintModel<'_> {
    /// Distinct colour codes in first-use order (future 3MF material slots).
    pub fn color_regions(&self) -> Vec<(u32, usize)> {
        let mut out: Vec<(u32, usize)> = Vec::new();
        for t in &self.mesh.tris {
            match out.iter_mut().find(|(c, _)| *c == t.color) {
                Some(e) => e.1 += 1,
                None => out.push((t.color, 1)),
            }
        }
        out
    }
}

pub trait Exporter {
    fn extension(&self) -> &'static str;
    fn write(&self, model: &PrintModel, out: &mut dyn Write) -> Result<()>;
}

pub struct BinaryStl;

impl Exporter for BinaryStl {
    fn extension(&self) -> &'static str {
        "stl"
    }
    fn write(&self, model: &PrintModel, out: &mut dyn Write) -> Result<()> {
        let mut header = [b' '; 80];
        let text = format!("ldraw2print {} units=mm", model.name);
        let n = text.len().min(80);
        header[..n].copy_from_slice(&text.as_bytes()[..n]);
        // An STL header must not start with "solid" (ASCII STL marker).
        if header.starts_with(b"solid") {
            header[0] = b'_';
        }
        let mut buf = Vec::with_capacity(84 + model.mesh.tris.len() * 50);
        buf.extend_from_slice(&header);
        buf.extend_from_slice(&(model.mesh.tris.len() as u32).to_le_bytes());
        for t in &model.mesh.tris {
            let p = model.mesh.tri_positions(t);
            let nrm = triangle_normal(p[0], p[1], p[2]).normalized();
            for v in [nrm, p[0], p[1], p[2]] {
                for c in v.to_array() {
                    buf.extend_from_slice(&(c as f32).to_le_bytes());
                }
            }
            buf.extend_from_slice(&0u16.to_le_bytes());
        }
        out.write_all(&buf)?;
        Ok(())
    }
}

/// Minimal binary STL reader (used by tests and `inspect` on STL files).
pub fn read_binary_stl(bytes: &[u8]) -> Result<Vec<[Vec3; 3]>> {
    anyhow::ensure!(bytes.len() >= 84, "STL too short");
    let n = u32::from_le_bytes(bytes[80..84].try_into()?) as usize;
    anyhow::ensure!(
        bytes.len() >= 84 + n * 50,
        "STL truncated: header says {n} triangles"
    );
    let f = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as f64;
    Ok((0..n)
        .map(|i| {
            let b = 84 + i * 50 + 12;
            std::array::from_fn(|k| Vec3::new(f(b + k * 12), f(b + k * 12 + 4), f(b + k * 12 + 8)))
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Preview geometry

#[derive(Serialize)]
pub struct PreviewColor {
    pub code: u32,
    pub name: String,
    pub rgba: [u8; 4],
}

#[derive(Serialize)]
pub struct PreviewGeometry {
    /// Flat xyz per vertex, mm, Z up.
    pub positions: Vec<f32>,
    /// Three vertex indices per triangle.
    pub indices: Vec<u32>,
    /// Index into `palette` per triangle.
    pub tri_color: Vec<u16>,
    /// Component id per triangle.
    pub tri_component: Vec<u32>,
    /// Source-file index per triangle (into `files`).
    pub tri_file: Vec<u32>,
    pub palette: Vec<PreviewColor>,
    /// LDraw type 2 edge lines, flat xyz pairs, mm.
    pub edges: Vec<f32>,
    /// Open boundary edges, flat xyz pairs, mm.
    pub boundary_edges: Vec<f32>,
    /// Non-manifold edges, flat xyz pairs, mm.
    pub nonmanifold_edges: Vec<f32>,
}

pub fn preview_geometry(
    mesh: &Mesh,
    components: &[u32],
    colors: &ColorTable,
    edges_mm: &[[Vec3; 2]],
) -> PreviewGeometry {
    let mut palette: Vec<PreviewColor> = Vec::new();
    let mut tri_color = Vec::with_capacity(mesh.tris.len());
    for t in &mesh.tris {
        let idx = match palette.iter().position(|p| p.code == t.color) {
            Some(i) => i,
            None => {
                palette.push(PreviewColor {
                    code: t.color,
                    name: colors.name(t.color),
                    rgba: colors.rgba(t.color),
                });
                palette.len() - 1
            }
        };
        tri_color.push(idx as u16);
    }
    let flat = |v: &mut Vec<f32>, a: Vec3, b: Vec3| {
        for c in a.to_array().into_iter().chain(b.to_array()) {
            v.push(c as f32);
        }
    };
    let mut boundary = Vec::new();
    let mut nonmanifold = Vec::new();
    for (&(a, b), uses) in &mesh.edge_map() {
        let (pa, pb) = (mesh.positions[a as usize], mesh.positions[b as usize]);
        match uses.len() {
            1 => flat(&mut boundary, pa, pb),
            2 => {}
            _ => flat(&mut nonmanifold, pa, pb),
        }
    }
    let mut edges = Vec::with_capacity(edges_mm.len() * 6);
    for e in edges_mm {
        flat(&mut edges, e[0], e[1]);
    }
    PreviewGeometry {
        positions: mesh
            .positions
            .iter()
            .flat_map(|p| p.to_array().map(|c| c as f32))
            .collect(),
        indices: mesh.tris.iter().flat_map(|t| t.v).collect(),
        tri_color,
        tri_component: components.to_vec(),
        tri_file: mesh.tris.iter().map(|t| t.file).collect(),
        palette,
        edges,
        boundary_edges: boundary,
        nonmanifold_edges: nonmanifold,
    }
}

// ---------------------------------------------------------------------------
// 3MF (experimental)

/// 3MF core-spec export: one object made of one component (part) per LDraw
/// colour, each with a base-material display colour. Bambu Studio imports
/// this as a single object with one part per colour; assign AMS filaments
/// per part. No Bambu-specific project metadata is written.
pub struct ThreeMf;

impl Exporter for ThreeMf {
    fn extension(&self) -> &'static str {
        "3mf"
    }
    fn write(&self, model: &PrintModel, out: &mut dyn Write) -> Result<()> {
        let regions = model.color_regions();
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n",
        );
        xml.push_str(&format!(
            "  <metadata name=\"Title\">{}</metadata>\n",
            xml_escape(&model.name)
        ));
        xml.push_str("  <metadata name=\"Application\">ldraw2print</metadata>\n  <resources>\n    <basematerials id=\"1\">\n");
        for (code, _) in &regions {
            let c = model.colors.rgba(*code);
            xml.push_str(&format!(
                "      <base name=\"{}\" displaycolor=\"#{:02X}{:02X}{:02X}{:02X}\"/>\n",
                xml_escape(&format!("{} ({code})", model.colors.name(*code))),
                c[0],
                c[1],
                c[2],
                c[3]
            ));
        }
        xml.push_str("    </basematerials>\n");
        for (k, (code, _)) in regions.iter().enumerate() {
            let mut remap = vec![u32::MAX; model.mesh.positions.len()];
            let mut verts = String::new();
            let mut tris = String::new();
            let mut n = 0u32;
            for t in model.mesh.tris.iter().filter(|t| t.color == *code) {
                let mut idx = [0u32; 3];
                for (j, &v) in t.v.iter().enumerate() {
                    if remap[v as usize] == u32::MAX {
                        remap[v as usize] = n;
                        n += 1;
                        let p = model.mesh.positions[v as usize];
                        verts.push_str(&format!(
                            "          <vertex x=\"{:.6}\" y=\"{:.6}\" z=\"{:.6}\"/>\n",
                            p.x, p.y, p.z
                        ));
                    }
                    idx[j] = remap[v as usize];
                }
                tris.push_str(&format!(
                    "          <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>\n",
                    idx[0], idx[1], idx[2]
                ));
            }
            xml.push_str(&format!(
                "    <object id=\"{}\" name=\"{}\" type=\"model\" pid=\"1\" pindex=\"{k}\">\n      <mesh>\n        <vertices>\n{verts}        </vertices>\n        <triangles>\n{tris}        </triangles>\n      </mesh>\n    </object>\n",
                k + 2,
                xml_escape(&model.colors.name(*code)),
            ));
        }
        let parent = regions.len() + 2;
        xml.push_str(&format!(
            "    <object id=\"{parent}\" name=\"{}\" type=\"model\">\n      <components>\n",
            xml_escape(&model.name)
        ));
        for k in 0..regions.len() {
            xml.push_str(&format!("        <component objectid=\"{}\"/>\n", k + 2));
        }
        xml.push_str(&format!("      </components>\n    </object>\n  </resources>\n  <build>\n    <item objectid=\"{parent}\"/>\n  </build>\n</model>\n"));

        let zip = stored_zip(&[
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
            ("_rels/.rels", RELS.as_bytes()),
            ("3D/3dmodel.model", xml.as_bytes()),
        ]);
        out.write_all(&zip)?;
        Ok(())
    }
}

/// Multicolour 3MF as **one solid with painted faces**, which is how LDraw
/// printed patterns are modelled (coloured patches of one closed surface).
///
/// The most-used colour is filament 1; triangles of every other colour are
/// painted with filament 2, 3, … using:
/// * `paint_color` — Bambu Studio / OrcaSlicer per-triangle filament paint,
/// * `slic3rpe:mmu_segmentation` — PrusaSlicer's equivalent attribute,
/// * core-spec `pid`/`p1` base materials, so any 3MF viewer shows colours.
///
/// The encoding of a whole (unsubdivided) triangle painted with filament
/// `n` follows the TriangleSelector format both slicers share: `n` < 3 is a
/// single hex digit `n << 2`; larger states use an escape nibble.
pub struct PaintedThreeMf;

/// Filament assignment used by [`PaintedThreeMf`]: (colour code, triangles), filament = index + 1.
pub fn paint_filaments(model: &PrintModel) -> Vec<(u32, usize)> {
    let mut regions = model.color_regions();
    regions.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    regions
}

/// TriangleSelector code for a leaf triangle in `state` (filament index).
pub fn paint_code(state: u32) -> String {
    let hex = |n: u32| char::from_digit(n, 16).unwrap().to_ascii_uppercase();
    if state < 3 {
        hex(state << 2).to_string()
    } else {
        // Leaf with state escape (0b11 << 2), then a nibble with state − 3,
        // written most-significant nibble first.
        format!("{}{}", hex(state - 3), hex(0b1100))
    }
}

impl Exporter for PaintedThreeMf {
    fn extension(&self) -> &'static str {
        "3mf"
    }
    fn write(&self, model: &PrintModel, out: &mut dyn Write) -> Result<()> {
        let filaments = paint_filaments(model);
        anyhow::ensure!(
            filaments.len() <= 16,
            "{} colours; painted 3MF supports at most 16 filaments",
            filaments.len()
        );
        let slot = |code: u32| filaments.iter().position(|(c, _)| *c == code).unwrap();
        let mapping: Vec<String> = filaments
            .iter()
            .enumerate()
            .map(|(i, (c, n))| {
                format!(
                    "filament {}: {} ({c}), {n} triangles",
                    i + 1,
                    model.colors.name(*c)
                )
            })
            .collect();
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" xmlns:slic3rpe=\"http://schemas.slic3r.org/3mf/2017/06\">\n",
        );
        xml.push_str(&format!(
            "  <metadata name=\"Title\">{}</metadata>\n",
            xml_escape(&model.name)
        ));
        xml.push_str("  <metadata name=\"Application\">ldraw2print</metadata>\n");
        xml.push_str(&format!(
            "  <metadata name=\"Description\">{}</metadata>\n",
            xml_escape(&mapping.join("; "))
        ));
        xml.push_str("  <resources>\n    <basematerials id=\"1\">\n");
        for (code, _) in &filaments {
            let c = model.colors.rgba(*code);
            xml.push_str(&format!(
                "      <base name=\"{}\" displaycolor=\"#{:02X}{:02X}{:02X}{:02X}\"/>\n",
                xml_escape(&format!("{} ({code})", model.colors.name(*code))),
                c[0],
                c[1],
                c[2],
                c[3]
            ));
        }
        xml.push_str("    </basematerials>\n");
        xml.push_str(&format!(
            "    <object id=\"2\" name=\"{}\" type=\"model\" pid=\"1\" pindex=\"0\">\n      <mesh>\n        <vertices>\n",
            xml_escape(&model.name)
        ));
        for p in &model.mesh.positions {
            xml.push_str(&format!(
                "          <vertex x=\"{:.6}\" y=\"{:.6}\" z=\"{:.6}\"/>\n",
                p.x, p.y, p.z
            ));
        }
        xml.push_str("        </vertices>\n        <triangles>\n");
        for t in &model.mesh.tris {
            let k = slot(t.color);
            let [a, b, c] = t.v;
            if k == 0 {
                xml.push_str(&format!(
                    "          <triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>\n"
                ));
            } else {
                let code = paint_code(k as u32 + 1);
                xml.push_str(&format!(
                    "          <triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\" pid=\"1\" p1=\"{k}\" paint_color=\"{code}\" slic3rpe:mmu_segmentation=\"{code}\"/>\n"
                ));
            }
        }
        xml.push_str("        </triangles>\n      </mesh>\n    </object>\n  </resources>\n  <build>\n    <item objectid=\"2\"/>\n  </build>\n</model>\n");
        let zip = stored_zip(&[
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
            ("_rels/.rels", RELS.as_bytes()),
            ("3D/3dmodel.model", xml.as_bytes()),
        ]);
        out.write_all(&zip)?;
        Ok(())
    }
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n  <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n  <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n</Types>\n";
const RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n  <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>\n</Relationships>\n";

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, e) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *e = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Minimal uncompressed ("stored") ZIP archive, enough for 3MF packages.
fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let le16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let le32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    const DOS_DATE: u16 = (1 << 5) | 1; // 1980-01-01
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        le32(&mut out, 0x0403_4b50);
        for x in [20u16, 0, 0, 0, DOS_DATE] {
            le16(&mut out, x);
        }
        for x in [crc, data.len() as u32, data.len() as u32] {
            le32(&mut out, x);
        }
        le16(&mut out, name.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        le32(&mut central, 0x0201_4b50);
        for x in [20u16, 20, 0, 0, 0, DOS_DATE] {
            le16(&mut central, x);
        }
        for x in [crc, data.len() as u32, data.len() as u32] {
            le32(&mut central, x);
        }
        for x in [name.len() as u16, 0, 0, 0, 0] {
            le16(&mut central, x);
        }
        le32(&mut central, 0);
        le32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    le32(&mut out, 0x0605_4b50);
    for x in [0u16, 0, files.len() as u16, files.len() as u16] {
        le16(&mut out, x);
    }
    le32(&mut out, cd_size);
    le32(&mut out, cd_offset);
    le16(&mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_codes_match_bambu_files() {
        // Observed in real Bambu Studio projects: filament 1 = "4", 2 = "8".
        assert_eq!(paint_code(1), "4");
        assert_eq!(paint_code(2), "8");
        assert_eq!(paint_code(3), "0C");
        assert_eq!(paint_code(6), "3C");
    }

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn stored_zip_layout() {
        let z = stored_zip(&[("a.txt", b"hello")]);
        assert_eq!(&z[..4], &[0x50, 0x4b, 0x03, 0x04]);
        let eocd = &z[z.len() - 22..];
        assert_eq!(&eocd[..4], &[0x50, 0x4b, 0x05, 0x06]);
        assert_eq!(u16::from_le_bytes([eocd[10], eocd[11]]), 1);
    }
}
