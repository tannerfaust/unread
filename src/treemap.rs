//! The map: a treemap of the repository. Each file is a rectangle sized by its lines of code
//! and split, along its long side, into the share a person wrote, the share AI wrote and a
//! person read, and the share AI wrote that nobody read. Nothing is rounded to a dominant
//! colour, so the orange area of the picture is exactly the unread figure printed above it.

use crate::model::Counts;
use crate::util::{esc, thousands};
use std::fmt::Write;

pub const HUMAN: &str = "#79c0ff";
pub const SEEN: &str = "#56d364";
pub const UNREAD: &str = "#ff7b39";
pub const BG: &str = "#0d1117";
/// Text drawn on top of the colours.
const INK: &str = "#0d1117";
const LABEL: &str = "#8b949e";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub struct Cell {
    pub rect: Rect,
    pub path: String,
    pub n: Counts,
}

#[derive(Default)]
pub struct Map {
    /// Files, and directories too small to show their files.
    pub cells: Vec<Cell>,
    /// Directories large enough to carry a label.
    pub labels: Vec<Cell>,
}

struct Node {
    name: String,
    path: String,
    n: Counts,
    kids: Vec<Node>,
    file: bool,
}

/// Squarified treemap (Bruls, Huizing, van Wijk). `areas` must be sorted largest first and
/// sum to the area of `r`.
pub fn squarify(areas: &[f64], mut r: Rect) -> Vec<Rect> {
    let mut out = Vec::with_capacity(areas.len());
    let mut i = 0;
    while i < areas.len() {
        let short = r.w.min(r.h).max(1e-9);
        let (mut j, mut sum, mut best) = (i, 0.0, f64::INFINITY);
        while j < areas.len() {
            let s = sum + areas[j];
            let worst = (short * short * areas[i] / (s * s))
                .max(s * s / (short * short * areas[j].max(1e-12)));
            if worst > best {
                break;
            }
            best = worst;
            sum = s;
            j += 1;
        }
        let thick = sum / short;
        if r.w >= r.h {
            let mut y = r.y;
            for a in &areas[i..j] {
                let h = a / thick;
                out.push(Rect {
                    x: r.x,
                    y,
                    w: thick,
                    h,
                });
                y += h;
            }
            r.x += thick;
            r.w -= thick;
        } else {
            let mut x = r.x;
            for a in &areas[i..j] {
                let w = a / thick;
                out.push(Rect {
                    x,
                    y: r.y,
                    w,
                    h: thick,
                });
                x += w;
            }
            r.y += thick;
            r.h -= thick;
        }
        i = j;
    }
    out
}

fn tree(files: &[(String, Counts)]) -> Node {
    let mut root = Node {
        name: String::new(),
        path: String::new(),
        n: Counts::default(),
        kids: Vec::new(),
        file: false,
    };
    for (path, n) in files.iter().filter(|(_, n)| n.code > 0) {
        let mut node = &mut root;
        node.n.merge(*n);
        let parts: Vec<&str> = path.split('/').collect();
        for (depth, part) in parts.iter().enumerate() {
            let file = depth + 1 == parts.len();
            let at = match node
                .kids
                .iter()
                .position(|k| k.name == *part && k.file == file)
            {
                Some(at) => at,
                None => {
                    let path = parts[..=depth].join("/");
                    node.kids.push(Node {
                        name: part.to_string(),
                        path,
                        n: Counts::default(),
                        kids: Vec::new(),
                        file,
                    });
                    node.kids.len() - 1
                }
            };
            node = &mut node.kids[at];
            node.n.merge(*n);
        }
    }
    collapse(&mut root);
    root
}

/// `src` holding only `app` becomes `src/app`.
fn collapse(node: &mut Node) {
    for k in &mut node.kids {
        while k.kids.len() == 1 && !k.kids[0].file {
            let only = k.kids.pop().expect("one child");
            k.name = format!("{}/{}", k.name, only.name);
            k.path = only.path;
            k.kids = only.kids;
        }
        collapse(k);
    }
}

pub fn layout(files: &[(String, Counts)], area: Rect) -> Map {
    let mut map = Map::default();
    let root = tree(files);
    if root.n.code > 0 {
        place(&root, area, 0, &mut map);
    }
    map
}

fn place(node: &Node, r: Rect, depth: usize, map: &mut Map) {
    if node.file || r.w < 6.0 || r.h < 6.0 || r.w * r.h < 90.0 {
        map.cells.push(Cell {
            rect: r,
            path: node.path.clone(),
            n: node.n,
        });
        return;
    }
    let mut inner = r;
    if depth > 0 {
        // Labels carry the full path, so a directory that is mostly one subdirectory leaves
        // the label to that child instead of stacking two.
        let mostly_one_child = node
            .kids
            .iter()
            .any(|k| !k.file && u64::from(k.n.code) * 100 >= u64::from(node.n.code) * 80);
        let labelled = r.w >= 70.0 && r.h >= 44.0 && !mostly_one_child;
        let top = if labelled { 15.0 } else { 1.5 };
        inner = Rect {
            x: r.x + 1.5,
            y: r.y + top,
            w: r.w - 3.0,
            h: r.h - top - 1.5,
        };
        if labelled {
            map.labels.push(Cell {
                rect: r,
                path: node.path.clone(),
                n: node.n,
            });
        }
    }
    let mut kids: Vec<&Node> = node.kids.iter().filter(|k| k.n.code > 0).collect();
    kids.sort_by(|a, b| b.n.code.cmp(&a.n.code).then_with(|| a.name.cmp(&b.name)));
    let scale = inner.w * inner.h / f64::from(node.n.code);
    let areas: Vec<f64> = kids.iter().map(|k| f64::from(k.n.code) * scale).collect();
    for (k, kr) in kids.iter().zip(squarify(&areas, inner)) {
        place(k, kr, depth + 1, map);
    }
}

/// The three parts of a cell: (fill, rect), lit parts first.
fn parts(c: &Cell) -> Vec<(&'static str, Rect)> {
    let gap = 0.6;
    let r = Rect {
        x: c.rect.x + gap / 2.0,
        y: c.rect.y + gap / 2.0,
        w: (c.rect.w - gap).max(0.2),
        h: (c.rect.h - gap).max(0.2),
    };
    let total = f64::from(c.n.code.max(1));
    let mut out = Vec::with_capacity(3);
    let mut used = 0.0;
    for (fill, v) in [(HUMAN, c.n.human), (SEEN, c.n.seen), (UNREAD, c.n.unread)] {
        if v == 0 {
            continue;
        }
        let f = f64::from(v) / total;
        let part = if r.w >= r.h {
            Rect {
                x: r.x + r.w * used,
                y: r.y,
                w: r.w * f,
                h: r.h,
            }
        } else {
            Rect {
                x: r.x,
                y: r.y + r.h * used,
                w: r.w,
                h: r.h * f,
            }
        };
        used += f;
        out.push((fill, part));
    }
    out
}

fn rect(o: &mut String, fill: &str, r: Rect) {
    let _ = write!(
        o,
        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{fill}"/>"#,
        r.x, r.y, r.w, r.h
    );
}

/// SVG body of a map. File names are written inside cells big enough to hold them. With
/// `hover`, every cell carries its path and counts for the report's tooltip.
pub fn svg_body(map: &Map, hover: bool) -> String {
    let mut fills = String::new();
    let mut names = String::new();
    let mut hit = String::new();
    for c in &map.cells {
        for (fill, r) in parts(c) {
            rect(&mut fills, fill, r);
        }
        let name = c.path.rsplit('/').next().unwrap_or(&c.path);
        let room = ((c.rect.w - 10.0) / 7.0) as usize;
        if c.rect.h >= 22.0 && room >= 6 {
            let _ = write!(
                names,
                r#"<text x="{:.1}" y="{:.1}">{}</text>"#,
                c.rect.x + 5.0,
                c.rect.y + 16.0,
                esc(&crate::render::term::fit(name, room))
            );
        }
        if hover {
            let _ = write!(
                hit,
                r#"<rect class="c" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" data-p="{}" data-n="{},{},{}"/>"#,
                c.rect.x,
                c.rect.y,
                c.rect.w,
                c.rect.h,
                esc(&c.path),
                c.n.human,
                c.n.seen,
                c.n.unread
            );
        }
    }
    let mut labels = String::new();
    for l in &map.labels {
        let room = ((l.rect.w - 8.0) / 6.2) as usize;
        let name = crate::render::term::fit(&l.path, room.max(3));
        let _ = write!(
            labels,
            r#"<text x="{:.1}" y="{:.1}">{}/</text>"#,
            l.rect.x + 4.0,
            l.rect.y + 11.0,
            esc(&name)
        );
    }
    const MONO: &str = "ui-monospace,SFMono-Regular,Menlo,Consolas,monospace";
    format!(
        r#"<g>{fills}</g><g font-size="12" font-weight="600" fill="{INK}" font-family="{MONO}">{names}</g><g font-size="10.5" fill="{LABEL}" font-family="{MONO}">{labels}</g><g fill="transparent">{hit}</g>"#
    )
}

/// What each colour means, in the words every view uses.
pub const MEANING: [(&str, &str); 3] = [
    (HUMAN, "Written by a person"),
    (SEEN, "Written by AI, read by a person"),
    (UNREAD, "Written by AI, never read"),
];

/// A standalone 1200x630 picture that explains itself: one sentence, one bar, a legend, and
/// the largest files. Sized for link previews, so it can be posted as it is.
pub fn card(name: &str, total: Counts, files: &[(String, Counts)]) -> String {
    let t = total;
    let pct = |v: u32| {
        if t.code == 0 {
            0.0
        } else {
            f64::from(v) * 100.0 / f64::from(t.code)
        }
    };
    let mut o = String::new();

    // The bar: the whole codebase as one strip, split three ways.
    let (x0, bar_w, bar_y, bar_h) = (40.0, 1120.0, 244.0, 44.0);
    let mut x = x0;
    for (v, (fill, _)) in [t.human, t.seen, t.unread].iter().zip(MEANING) {
        let w = if t.code == 0 {
            0.0
        } else {
            bar_w * f64::from(*v) / f64::from(t.code)
        };
        if w > 0.0 {
            let _ = write!(
                o,
                r#"<rect x="{x:.1}" y="{bar_y}" width="{w:.1}" height="{bar_h}" fill="{fill}"/>"#
            );
        }
        x += w;
    }
    let bar = format!(
        r##"<clipPath id="bar"><rect x="{x0}" y="{bar_y}" width="{bar_w}" height="{bar_h}" rx="8"/></clipPath><rect x="{x0}" y="{bar_y}" width="{bar_w}" height="{bar_h}" rx="8" fill="#21262d"/><g clip-path="url(#bar)">{o}</g>"##
    );

    // The legend: three columns, each with its share, its meaning and its line count.
    let mut legend = String::new();
    for (i, (v, (fill, label))) in [t.human, t.seen, t.unread].iter().zip(MEANING).enumerate() {
        let lx = 40.0 + i as f64 * 380.0;
        let _ = write!(
            legend,
            r##"<rect x="{lx}" y="318" width="18" height="18" rx="4" fill="{fill}"/><text x="{tx}" y="334" font-size="30" font-weight="700" fill="#e6edf3">{p:.0}%</text><text x="{lx}" y="366" font-size="19" fill="#c9d1d9">{label}</text><text x="{lx}" y="392" font-size="16" fill="#8b949e">{lines} lines</text>"##,
            tx = lx + 30.0,
            p = pct(*v),
            lines = thousands(u64::from(*v)),
        );
    }

    // The largest files, each with its own bar.
    let mut biggest: Vec<&(String, Counts)> = files.iter().filter(|(_, n)| n.code > 0).collect();
    biggest.sort_by(|a, b| b.1.code.cmp(&a.1.code).then_with(|| a.0.cmp(&b.0)));
    let mut rows = String::new();
    for (i, (path, n)) in biggest.iter().take(8).enumerate() {
        let col = (i / 4) as f64;
        let row = (i % 4) as f64;
        let fx = 40.0 + col * 580.0;
        let fy = 466.0 + row * 34.0;
        let name = crate::render::term::fit(path, 30);
        let _ = write!(
            rows,
            r##"<text x="{fx}" y="{ty}" font-size="15" fill="#c9d1d9" font-family="ui-monospace,SFMono-Regular,Menlo,Consolas,monospace">{}</text>"##,
            esc(&name),
            ty = fy + 12.0
        );
        let (bx, bw) = (fx + 300.0, 220.0);
        let mut seg = bx;
        for (v, (fill, _)) in [n.human, n.seen, n.unread].iter().zip(MEANING) {
            let w = bw * f64::from(*v) / f64::from(n.code);
            if w > 0.0 {
                let _ = write!(
                    rows,
                    r#"<rect x="{seg:.1}" y="{fy}" width="{w:.1}" height="14" fill="{fill}"/>"#
                );
            }
            seg += w;
        }
    }

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 630" width="1200" height="630" font-family="ui-sans-serif,system-ui,-apple-system,Segoe UI,Helvetica,Arial,sans-serif">
<rect width="1200" height="630" fill="{BG}"/>
<text x="40" y="58" font-size="20" fill="#8b949e"><tspan font-weight="700" fill="#e6edf3">unread</tspan>  ·  {name}</text>
<text x="36" y="160" font-size="96" font-weight="800" fill="{UNREAD}" letter-spacing="-2">{unread_pct:.0}%</text>
<text x="40" y="208" font-size="27" fill="#e6edf3">of this code was written by AI and never read by a person.</text>
{bar}
{legend}
<text x="40" y="440" font-size="14" font-weight="600" fill="#8b949e" letter-spacing="1.5">LARGEST FILES</text>
{rows}
<text x="40" y="612" font-size="14" fill="#6e7681">Measured with unread, code coverage for human eyes</text>
</svg>
"##,
        name = esc(name),
        unread_pct = pct(t.unread),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(code: u32, unread: u32) -> Counts {
        Counts {
            code,
            human: code - unread,
            seen: 0,
            unread,
        }
    }

    #[test]
    fn squarify_fills_the_rectangle() {
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 600.0,
            h: 400.0,
        };
        let sizes = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let scale = r.w * r.h / sizes.iter().sum::<f64>();
        let areas: Vec<f64> = sizes.iter().map(|s| s * scale).collect();
        let rects = squarify(&areas, r);
        assert_eq!(rects.len(), sizes.len());
        for (rect, area) in rects.iter().zip(&areas) {
            assert!((rect.w * rect.h - area).abs() < 1e-6);
            assert!(
                rect.x >= -1e-9
                    && rect.y >= -1e-9
                    && rect.x + rect.w <= 600.0 + 1e-6
                    && rect.y + rect.h <= 400.0 + 1e-6
            );
        }
    }

    #[test]
    fn lit_area_equals_coverage() {
        let files = vec![
            ("src/a.rs".to_string(), counts(300, 300)),
            ("src/b.rs".to_string(), counts(100, 0)),
            ("lib/deep/only/c.rs".to_string(), counts(100, 50)),
        ];
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: 500.0,
            h: 500.0,
        };
        let map = layout(&files, area);
        assert_eq!(map.cells.len(), 3);
        let (mut lit, mut all) = (0.0, 0.0);
        for c in &map.cells {
            for (fill, r) in parts(c) {
                all += r.w * r.h;
                if fill != UNREAD {
                    lit += r.w * r.h;
                }
            }
        }
        // 150 of 500 lines are lit; padding between cells costs a little accuracy.
        assert!((lit / all - 0.30).abs() < 0.02, "lit share {}", lit / all);
    }

    #[test]
    fn single_child_directories_collapse() {
        let files = vec![
            ("a/b/c/x.rs".to_string(), counts(10, 0)),
            ("a/b/c/y.rs".to_string(), counts(10, 0)),
        ];
        let root = tree(&files);
        assert_eq!(root.kids.len(), 1);
        assert_eq!(root.kids[0].name, "a/b/c");
        assert_eq!(root.kids[0].kids.len(), 2);
    }
}
