//! The Markdown subset `CHANGELOG.md` uses: `##`/`###` headings, `- `
//! lists, paragraphs and `` `code` ``.

use crate::charts::esc;

fn inline(s: &str) -> String {
    let mut out = String::new();
    for (i, part) in esc(s).split('`').enumerate() {
        if i % 2 == 1 {
            out += &format!("<code>{part}</code>");
        } else {
            out += part;
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
        let item = l.strip_prefix("- ");
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
}
