//! `xtask audit chart`: hand-written SVG charts of `history.json`, each in a
//! light and a dark variant. The accuracy chart draws one Wilson interval
//! and one point estimate per release; the failures chart draws one stacked
//! bar per judged batch, split into new, known and untagged failures. The
//! page colour is transparent except for the ring around each dot, which
//! takes the page's colour so the dot reads cleanly over its own line.

use super::history::{History, Release, SeedRow};
use std::fmt::Write as _;

const LEFT: f64 = 52.0;
const RIGHT: f64 = 640.0;
const FONT: &str =
    "-apple-system, BlinkMacSystemFont, &quot;Segoe UI&quot;, Helvetica, Arial, sans-serif";
/// Width kept free at the right of the categories for the value labels.
const LABEL_ROOM: f64 = 40.0;
const GOAL: f64 = 0.9;

struct Palette {
    name: &'static str,
    primary: &'static str,
    secondary: &'static str,
    muted: &'static str,
    grid: &'static str,
    series: &'static str,
    page: &'static str,
}

const LIGHT: Palette = Palette {
    name: "light",
    primary: "#0b0b0b",
    secondary: "#52514e",
    muted: "#8a8984",
    grid: "#e4e3df",
    series: "#2a78d6",
    page: "#ffffff",
};

const DARK: Palette = Palette {
    name: "dark",
    primary: "#f0efec",
    secondary: "#c3c2b7",
    muted: "#8f8e88",
    grid: "#33332f",
    series: "#3987e5",
    page: "#0d1117",
};

fn n(v: f64) -> String {
    format!("{v:.1}")
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn text(x: f64, y: f64, anchor: &str, size: u32, fill: &str, body: &str) -> String {
    format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"{anchor}\" font-size=\"{size}\" fill=\"{fill}\">{}</text>\n",
        n(x),
        n(y),
        esc(body)
    )
}

/// A direct label with a halo in the page colour, so a gridline or the goal
/// line passing behind it does not cut the digits.
fn label_text(pal: &Palette, x: f64, y: f64, size: u32, body: &str) -> String {
    let plain = text(x, y, "start", size, pal.primary, body);
    plain.replacen(
        "<text ",
        &format!(
            "<text stroke=\"{}\" stroke-width=\"4\" stroke-linejoin=\"round\" \
             paint-order=\"stroke\" ",
            pal.page
        ),
        1,
    )
}

fn open(pal: &Palette, title: &str, desc: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 720 340\" width=\"720\" \
         height=\"340\" role=\"img\" aria-labelledby=\"t d\" font-family=\"{FONT}\">\n\
         <title id=\"t\">{}</title>\n<desc id=\"d\">{}</desc>\n<!-- {} -->\n",
        esc(title),
        esc(desc),
        pal.name
    )
}

fn gridline(pal: &Palette, y: f64) -> String {
    format!(
        "<line x1=\"{}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"{}\" stroke-width=\"1\"/>\n",
        n(LEFT),
        n(RIGHT),
        pal.grid,
        y = n(y)
    )
}

fn band_center(i: usize, count: usize) -> f64 {
    let band = (RIGHT - LABEL_ROOM - LEFT) / count as f64;
    LEFT + band * (i as f64 + 0.5)
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

/// Releases that have at least one judged tool.
fn scored(history: &History) -> Vec<&Release> {
    history.releases.iter().filter(|r| r.judged > 0).collect()
}

fn accuracy_desc(releases: &[&Release]) -> String {
    let mut out =
        String::from("Share of tools parsed fully correctly, per release, with the 95% interval. ");
    for r in releases {
        let _ = write!(
            out,
            "{}: {} ({} to {}, {} of {} tools). ",
            r.version,
            pct(r.correct as f64 / r.judged as f64),
            pct(r.wilson_low),
            pct(r.wilson_high),
            r.correct,
            r.judged
        );
    }
    out.push_str("Goal: 90%.");
    out
}

/// The accuracy chart for one palette.
fn accuracy_svg(history: &History, pal: &Palette) -> anyhow::Result<String> {
    let releases = scored(history);
    if releases.is_empty() {
        anyhow::bail!("no release has a judged tool; nothing to chart");
    }
    let (top, bottom) = (24.0, 262.0);
    let y_of = |v: f64| bottom - v * (bottom - top);
    let mut svg = open(
        pal,
        "Audited accuracy per release",
        &accuracy_desc(&releases),
    );
    for tick in [0.0, 0.25, 0.5, 0.75, 1.0] {
        svg.push_str(&gridline(pal, y_of(tick)));
        svg.push_str(&text(
            LEFT - 8.0,
            y_of(tick) + 4.0,
            "end",
            12,
            pal.secondary,
            &pct(tick),
        ));
    }
    let _ = writeln!(
        svg,
        "<line x1=\"{}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\" stroke=\"{}\" stroke-width=\"1.5\" \
         stroke-dasharray=\"6 4\"/>",
        n(LEFT),
        n(RIGHT),
        pal.secondary,
        y = n(y_of(GOAL))
    );
    svg.push_str(&text(
        RIGHT + 6.0,
        y_of(GOAL) + 4.0,
        "start",
        12,
        pal.secondary,
        "goal 90%",
    ));
    let points: Vec<(f64, f64)> = releases
        .iter()
        .enumerate()
        .map(|(i, r)| {
            (
                band_center(i, releases.len()),
                y_of(r.correct as f64 / r.judged as f64),
            )
        })
        .collect();
    if points.len() > 1 {
        let path: Vec<String> = points
            .iter()
            .map(|(x, y)| format!("{},{}", n(*x), n(*y)))
            .collect();
        let _ = writeln!(
            svg,
            "<polyline points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"2\" \
             stroke-opacity=\"0.4\" stroke-linejoin=\"round\"/>",
            path.join(" "),
            pal.series
        );
    }
    for (r, (x, y)) in releases.iter().zip(&points) {
        let _ = writeln!(
            svg,
            "<line x1=\"{x}\" y1=\"{}\" x2=\"{x}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"2\" \
             stroke-linecap=\"round\"/>",
            n(y_of(r.wilson_high)),
            n(y_of(r.wilson_low)),
            pal.series,
            x = n(*x)
        );
        let _ = writeln!(
            svg,
            "<circle cx=\"{x}\" cy=\"{y}\" r=\"6.5\" fill=\"{}\"/>\n<circle cx=\"{x}\" cy=\"{y}\" \
             r=\"4.5\" fill=\"{}\"/>",
            pal.page,
            pal.series,
            x = n(*x),
            y = n(*y)
        );
        svg.push_str(&text(
            *x,
            bottom + 20.0,
            "middle",
            12,
            pal.secondary,
            &r.version,
        ));
        svg.push_str(&text(
            *x,
            bottom + 35.0,
            "middle",
            11,
            pal.muted,
            &format!("n={}", r.judged),
        ));
    }
    if let (Some(last), Some((x, y))) = (releases.last(), points.last()) {
        let value = pct(last.correct as f64 / last.judged as f64);
        svg.push_str(&label_text(pal, x + 12.0, y + 4.0, 13, &value));
    }
    let eye: usize = history.releases.iter().map(|r| r.needs_eye.len()).sum();
    if eye > 0 {
        let noun = if eye == 1 { "tool" } else { "tools" };
        svg.push_str(&text(
            LEFT,
            bottom + 58.0,
            "start",
            11,
            pal.muted,
            &format!("{eye} {noun} awaiting review"),
        ));
    }
    svg.push_str("</svg>\n");
    Ok(svg)
}

/// A rounded-top rectangle path (`r` = 0 for a square top).
fn bar_path(x: f64, y: f64, w: f64, h: f64, r: f64) -> String {
    let r = r.min(h).min(w / 2.0);
    format!(
        "M{} {}V{}Q{} {} {} {}H{}Q{} {} {} {}V{}Z",
        n(x),
        n(y + h),
        n(y + r),
        n(x),
        n(y),
        n(x + r),
        n(y),
        n(x + w - r),
        n(x + w),
        n(y),
        n(x + w),
        n(y + r),
        n(y + h)
    )
}

/// Axis tick step and top tick for a largest bar of `max` failures.
fn nice_axis(max: usize) -> (usize, usize) {
    let mut step = 1usize;
    loop {
        for mult in [1usize, 2, 5] {
            let s = step * mult;
            if max.div_ceil(s) <= 4 {
                return (s, max.div_ceil(s).max(1) * s);
            }
        }
        step *= 10;
    }
}

/// Spread label centres at least `gap` apart, top to bottom.
fn spread(mut ys: Vec<(usize, f64)>, gap: f64) -> Vec<(usize, f64)> {
    ys.sort_by(|a, b| a.1.total_cmp(&b.1));
    for i in 1..ys.len() {
        if ys[i].1 - ys[i - 1].1 < gap {
            ys[i].1 = ys[i - 1].1 + gap;
        }
    }
    ys
}

fn failures_desc(rows: &[&SeedRow]) -> String {
    let mut out = String::from("Failures per judged batch, split into new, known and untagged. ");
    for s in rows {
        let _ = write!(
            out,
            "Seed {} (judged {}): {} new, {} known, {} untagged. ",
            s.seed, s.version, s.new, s.known, s.untagged
        );
    }
    out
}

fn legend(pal: &Palette, show_untagged: bool) -> String {
    let outline = format!(
        "fill=\"none\" stroke=\"{}\" stroke-width=\"1.5\"",
        pal.muted
    );
    let mut items = vec![
        ("new", format!("fill=\"{}\"", pal.series)),
        (
            "known",
            format!("fill=\"{}\" fill-opacity=\"0.35\"", pal.series),
        ),
    ];
    if show_untagged {
        items.push(("untagged", outline));
    }
    let mut svg = String::new();
    let mut x = LEFT;
    for (label, style) in items {
        let _ = writeln!(
            svg,
            "<rect x=\"{}\" y=\"8\" width=\"10\" height=\"10\" rx=\"2\" {style}/>",
            n(x)
        );
        svg.push_str(&text(x + 16.0, 17.0, "start", 12, pal.secondary, label));
        x += 24.0 + label.len() as f64 * 7.0;
    }
    svg
}

/// The failures chart for one palette.
fn failures_svg(history: &History, pal: &Palette) -> anyhow::Result<String> {
    let rows: Vec<&SeedRow> = history.seeds.iter().collect();
    if rows.is_empty() {
        anyhow::bail!("history has no judged batch; nothing to chart");
    }
    let totals: Vec<usize> = rows.iter().map(|s| s.new + s.known + s.untagged).collect();
    let (step, axis_top) = nice_axis(totals.iter().copied().max().unwrap_or(0));
    let (top, bottom) = (40.0, 262.0);
    let unit = (bottom - top) / axis_top as f64;
    let mut svg = open(pal, "Failures per judged batch", &failures_desc(&rows));
    svg.push_str(&legend(pal, rows.iter().any(|s| s.untagged > 0)));
    for tick in (0..=axis_top).step_by(step) {
        let y = bottom - tick as f64 * unit;
        svg.push_str(&gridline(pal, y));
        svg.push_str(&text(
            LEFT - 8.0,
            y + 4.0,
            "end",
            12,
            pal.secondary,
            &tick.to_string(),
        ));
    }
    let band = (RIGHT - LABEL_ROOM - LEFT) / rows.len() as f64;
    let bar_w = (band * 0.55).min(36.0);
    let mut last_labels: Vec<(usize, f64)> = Vec::new();
    for (i, s) in rows.iter().enumerate() {
        let x = band_center(i, rows.len()) - bar_w / 2.0;
        let parts = [(s.new, 0u8), (s.known, 1), (s.untagged, 2)];
        let top_part = parts.iter().rposition(|(c, _)| *c > 0);
        let mut y_bottom = bottom;
        for (idx, (count, kind)) in parts.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            let h = *count as f64 * unit - if y_bottom < bottom { 2.0 } else { 0.0 };
            let y = y_bottom - if y_bottom < bottom { 2.0 } else { 0.0 } - h;
            let r = if Some(idx) == top_part { 4.0 } else { 0.0 };
            let style = match kind {
                0 => format!("fill=\"{}\"", pal.series),
                1 => format!("fill=\"{}\" fill-opacity=\"0.35\"", pal.series),
                _ => format!(
                    "fill=\"none\" stroke=\"{}\" stroke-width=\"1.5\"",
                    pal.muted
                ),
            };
            let (px, py, pw, ph) = if *kind == 2 {
                (x + 0.75, y + 0.75, bar_w - 1.5, h - 1.5)
            } else {
                (x, y, bar_w, h)
            };
            let _ = writeln!(svg, "<path d=\"{}\" {style}/>", bar_path(px, py, pw, ph, r));
            if i + 1 == rows.len() {
                last_labels.push((idx, y + h / 2.0));
            }
            y_bottom = y;
        }
        let cx = x + bar_w / 2.0;
        svg.push_str(&text(
            cx,
            bottom + 20.0,
            "middle",
            12,
            pal.secondary,
            &s.version,
        ));
        svg.push_str(&text(
            cx,
            bottom + 35.0,
            "middle",
            11,
            pal.muted,
            &format!("seed {}", s.seed),
        ));
    }
    if let Some(s) = rows.last() {
        let x = band_center(rows.len() - 1, rows.len()) + bar_w / 2.0 + 8.0;
        for (idx, y) in spread(last_labels, 14.0) {
            let (name, count) = [("new", s.new), ("known", s.known), ("untagged", s.untagged)][idx];
            svg.push_str(&text(
                x,
                y + 4.0,
                "start",
                12,
                pal.primary,
                &format!("{name} {count}"),
            ));
        }
    }
    svg.push_str("</svg>\n");
    Ok(svg)
}

/// `xtask audit chart`: write the four SVGs into `out_dir`.
pub fn cmd_chart(args: &super::ChartArgs) -> anyhow::Result<()> {
    let (history_path, out_dir) = (&args.history, &args.out_dir);
    let raw = std::fs::read_to_string(history_path).map_err(|e| {
        anyhow::anyhow!(
            "reading {}: {e} (run `xtask audit history` first)",
            history_path.display()
        )
    })?;
    let history: History = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("parsing {}: {e}", history_path.display()))?;
    std::fs::create_dir_all(out_dir)?;
    for pal in [&LIGHT, &DARK] {
        for (stem, svg) in [
            ("accuracy", accuracy_svg(&history, pal)?),
            ("new-failures", failures_svg(&history, pal)?),
        ] {
            let path = out_dir.join(format!("{stem}-{}.svg", pal.name));
            std::fs::write(&path, svg)
                .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str, correct: usize, judged: usize, eye: usize) -> Release {
        let (low, high) = super::super::report::wilson_interval(correct, judged);
        Release {
            version: version.to_string(),
            correct,
            judged,
            needs_eye: (0..eye)
                .map(|i| super::super::history::NeedsEye {
                    seed: 1,
                    tool: format!("t{i}"),
                    command: String::new(),
                })
                .collect(),
            wilson_low: low,
            wilson_high: high,
            seeds: vec![1],
        }
    }

    fn history() -> History {
        History {
            releases: vec![
                release("0.8.4", 20, 36, 0),
                release("0.9.0", 0, 0, 2),
                release("0.10.0", 24, 36, 0),
            ],
            seeds: vec![
                SeedRow {
                    seed: 7,
                    version: "0.9.0".into(),
                    known: 9,
                    new: 3,
                    untagged: 1,
                },
                SeedRow {
                    seed: 8,
                    version: "0.10.0".into(),
                    known: 4,
                    new: 0,
                    untagged: 0,
                },
            ],
        }
    }

    #[test]
    fn output_is_deterministic_and_themes_differ_only_in_colour() {
        let h = history();
        assert_eq!(
            accuracy_svg(&h, &LIGHT).unwrap(),
            accuracy_svg(&h, &LIGHT).unwrap()
        );
        assert_eq!(
            failures_svg(&h, &DARK).unwrap(),
            failures_svg(&h, &DARK).unwrap()
        );
        assert_ne!(
            accuracy_svg(&h, &LIGHT).unwrap(),
            accuracy_svg(&h, &DARK).unwrap()
        );
    }

    #[test]
    fn accuracy_chart_leaves_out_empty_releases_and_footnotes_needs_eye() {
        let svg = accuracy_svg(&history(), &LIGHT).unwrap();
        assert!(svg.contains(">0.8.4<") && svg.contains(">0.10.0<"));
        assert!(
            !svg.contains(">0.9.0<"),
            "a release with nothing judged has no column"
        );
        assert!(svg.contains("2 tools awaiting review"));
        assert!(svg.contains("goal 90%") && svg.contains(">67%<"));
        assert!(
            svg.contains("<title")
                && svg.contains("<desc")
                && svg.contains("viewBox=\"0 0 720 340\"")
        );
        let mut quiet = history();
        quiet.releases[1].needs_eye.clear();
        assert!(!accuracy_svg(&quiet, &LIGHT)
            .unwrap()
            .contains("awaiting review"));
    }

    #[test]
    fn failures_chart_labels_the_latest_bar_and_has_a_legend() {
        let svg = failures_svg(&history(), &LIGHT).unwrap();
        assert!(svg.contains(">known 4<") && !svg.contains(">new 0<"));
        assert!(svg.contains(">new<") && svg.contains(">known<") && svg.contains(">untagged<"));
        assert!(svg.contains("fill-opacity=\"0.35\""));
    }

    #[test]
    fn empty_histories_are_an_error() {
        let empty = History {
            releases: vec![release("0.9.0", 0, 0, 0)],
            seeds: vec![],
        };
        assert!(accuracy_svg(&empty, &LIGHT).is_err());
        assert!(failures_svg(&empty, &LIGHT).is_err());
    }

    #[test]
    fn axis_steps_stay_at_five_ticks_or_fewer() {
        assert_eq!(nice_axis(0), (1, 1));
        assert_eq!(nice_axis(4), (1, 4));
        assert_eq!(nice_axis(12), (5, 15));
        assert_eq!(nice_axis(40), (10, 40));
    }
}
