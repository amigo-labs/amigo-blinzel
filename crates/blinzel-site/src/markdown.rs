//! The Markdown subset `CHANGELOG.md` uses (hand-written and from
//! release-please): `##`/`###` headings, `- ` and `* ` lists, paragraphs,
//! `` `code` ``, `**bold**` and `[links](https://…)`.

use crate::charts::esc;

/// Bold and links in text outside code spans.
fn rich(s: &str) -> String {
    let mut out = String::new();
    let mut bold = false;
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("**") {
            out += if bold { "</strong>" } else { "<strong>" };
            bold = !bold;
            rest = r;
            continue;
        }
        if let Some(r) = rest.strip_prefix('[')
            && let Some((text, after)) = r.split_once("](")
            && let Some((url, tail)) = after.split_once(')')
            && url.starts_with("https://")
            && !text.contains(']')
        {
            out += &format!("<a href=\"{}\">{}</a>", esc(url), rich(text));
            rest = tail;
            continue;
        }
        // Plain text up to the next possible marker (after the first
        // character, which is no marker here; on a char boundary).
        let first = rest.chars().next().map_or(1, char::len_utf8);
        let next = rest[first..]
            .find(['*', '['])
            .map_or(rest.len(), |i| i + first);
        out += &esc(&rest[..next]);
        rest = &rest[next..];
    }
    if bold {
        out += "</strong>";
    }
    out
}

fn inline(s: &str) -> String {
    let mut out = String::new();
    for (i, part) in s.split('`').enumerate() {
        if i % 2 == 1 {
            out += &format!("<code>{}</code>", esc(part));
        } else {
            out += &rich(part);
        }
    }
    out
}

pub fn to_html(md: &str) -> String {
    let mut out = String::new();
    let mut in_list = false;
    let mut para: Vec<&str> = Vec::new();
    let flush = |out: &mut String, para: &mut Vec<&str>| {
        if !para.is_empty() {
            *out += &format!("<p>{}</p>\n", inline(&para.join(" ")));
            para.clear();
        }
    };
    for line in md.lines() {
        let l = line.trim_end();
        let item = l.strip_prefix("- ").or_else(|| l.strip_prefix("* "));
        if in_list && item.is_none() {
            out += "</ul>\n";
            in_list = false;
        }
        if let Some(h) = l.strip_prefix("### ") {
            flush(&mut out, &mut para);
            out += &format!("<h3>{}</h3>\n", inline(h));
        } else if let Some(h) = l.strip_prefix("## ") {
            flush(&mut out, &mut para);
            out += &format!("<h2>{}</h2>\n", inline(h));
        } else if l.starts_with("# ") {
            // The document title is the page title.
            flush(&mut out, &mut para);
        } else if let Some(text) = item {
            flush(&mut out, &mut para);
            if !in_list {
                out += "<ul>\n";
                in_list = true;
            }
            out += &format!("<li>{}</li>\n", inline(text));
        } else if l.is_empty() {
            flush(&mut out, &mut para);
        } else {
            para.push(l.trim());
        }
    }
    flush(&mut out, &mut para);
    if in_list {
        out += "</ul>\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_the_changelog_subset() {
        let html = to_html(
            "# Changelog\n\n## 0.1.0\n\nFirst <release>.\nSecond line.\n\n- one `code`\n- two\n\n### Fixed\nx",
        );
        assert_eq!(
            html,
            "<h2>0.1.0</h2>\n<p>First &lt;release&gt;. Second line.</p>\n<ul>\n<li>one <code>code</code></li>\n<li>two</li>\n</ul>\n<h3>Fixed</h3>\n<p>x</p>\n"
        );
    }

    #[test]
    fn reads_release_please_entries() {
        let html = to_html(
            "## [0.2.0](https://github.com/o/r/compare/v0.1.0...v0.2.0) (2026-10-27)\n\n\n### Features\n\n* **snip:** take <screenshots> ([abc1234](https://github.com/o/r/commit/abc1234))\n* a*b and [not a link](javascript:x)\n* über\n",
        );
        assert_eq!(
            html,
            "<h2><a href=\"https://github.com/o/r/compare/v0.1.0...v0.2.0\">0.2.0</a> (2026-10-27)</h2>\n<h3>Features</h3>\n<ul>\n<li><strong>snip:</strong> take &lt;screenshots&gt; (<a href=\"https://github.com/o/r/commit/abc1234\">abc1234</a>)</li>\n<li>a*b and [not a link](javascript:x)</li>\n<li>über</li>\n</ul>\n"
        );
    }
}
