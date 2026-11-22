//! A README badge. Lit when the code is read, dark when it is not.

use crate::model::Counts;
use serde_json::{json, Value};

/// (background, text) for the value half of the badge.
fn colours(unread_pct: f64) -> (&'static str, &'static str) {
    match unread_pct {
        p if p < 10.0 => ("#2ea043", "#ffffff"),
        p if p < 30.0 => ("#9e6a03", "#ffffff"),
        p if p < 50.0 => ("#bc4c00", "#ffffff"),
        p if p < 75.0 => ("#cf222e", "#ffffff"),
        _ => ("#a40e26", "#ffffff"),
    }
}

fn value(c: Counts) -> String {
    if c.code == 0 {
        "n/a".to_string()
    } else {
        format!("{:.0}%", c.unread_pct())
    }
}

pub fn svg(c: Counts) -> String {
    let value = value(c);
    let (bg, fg) = colours(c.unread_pct());
    let label_w = 52;
    let value_w = 14 + 7 * value.len();
    let total = label_w + value_w;
    let vx = label_w + value_w / 2;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{total}" height="20" role="img" aria-label="unread: {value}">
<title>unread: {value} of this code was written by an agent and never seen by a person</title>
<clipPath id="r"><rect width="{total}" height="20" rx="3"/></clipPath>
<g clip-path="url(#r)"><rect width="{label_w}" height="20" fill="#0d1117"/><rect x="{label_w}" width="{value_w}" height="20" fill="{bg}"/></g>
<g font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11" text-anchor="middle">
<text x="{lx}" y="14" fill="#e6edf3">unread</text><text x="{vx}" y="14" fill="{fg}" font-weight="bold">{value}</text>
</g></svg>
"##,
        lx = label_w / 2
    )
}

/// Payload for a shields.io endpoint badge.
pub fn shields(c: Counts) -> Value {
    let (bg, _) = colours(c.unread_pct());
    json!({"schemaVersion": 1, "label": "unread", "message": value(c), "color": bg.trim_start_matches('#')})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_states() {
        let c = Counts {
            code: 200,
            human: 50,
            seen: 0,
            unread: 150,
        };
        let s = svg(c);
        assert!(s.contains(">75%<") && s.contains("#a40e26"));
        assert_eq!(shields(c)["message"], "75%");
        assert!(svg(Counts::default()).contains(">n/a<"));
    }
}
