//! Resolving link targets against the page's URL.
//!
//! Close to RFC 3986 section 5.2, with two differences kept for
//! compatibility with earlier releases: dot segments are removed from the
//! whole reference, query and fragment included, and a path that collapses
//! to the root through a final `.` or `..` is written `//`.

/// Resolve `href` against `base`. A base without `://` can't be resolved
/// against, so the link is returned unchanged, as are links with a scheme of
/// their own (`mailto:`, `https:` ...) and empty links.
pub fn resolve(base: &str, href: &str) -> String {
    if href.is_empty() || has_scheme(href) {
        return href.to_string();
    }
    let Some(sep) = base.find("://") else {
        return href.to_string();
    };
    let scheme = &base[..sep];
    let rest = &base[sep + 3..];
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let prefix = &base[..sep + 3 + auth_end];
    let tail = &rest[auth_end..];
    let without_fragment = tail.split('#').next().unwrap_or("");
    let path = without_fragment.split(['?', '#']).next().unwrap_or("");

    if href.starts_with("//") {
        return format!("{scheme}:{href}");
    }
    if href.starts_with('#') {
        return format!("{prefix}{without_fragment}{href}");
    }
    if href.starts_with('?') {
        return format!("{prefix}{path}{href}");
    }
    if href.starts_with('/') {
        return format!("{prefix}{}", remove_dot_segments(href));
    }
    let dir = match path.rfind('/') {
        Some(i) => &path[..=i],
        None => "/",
    };
    format!("{prefix}{}", remove_dot_segments(&format!("{dir}{href}")))
}

/// `scheme ":"` at the start, with the scheme as RFC 3986 defines it.
fn has_scheme(s: &str) -> bool {
    let Some(colon) = s.find(':') else {
        return false;
    };
    let scheme = &s.as_bytes()[..colon];
    scheme.first().is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
}

/// `path` starts with `/`.
fn remove_dot_segments(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut ends_in_dots = false;
    for segment in path.split('/').skip(1) {
        ends_in_dots = true;
        match segment {
            "." => {}
            ".." => {
                out.pop();
            }
            _ => {
                out.push(segment);
                ends_in_dots = false;
            }
        }
    }
    let mut joined = out.join("/");
    if ends_in_dots && !joined.ends_with('/') {
        joined.push('/');
    }
    format!("/{joined}")
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn rfc3986_examples() {
        let base = "http://a/b/c/d;p?q";
        for (href, want) in [
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("g;x", "http://a/b/c/g;x"),
            (".", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../g", "http://a/b/g"),
            ("../../../g", "http://a/g"),
            ("g:h", "g:h"),
        ] {
            assert_eq!(resolve(base, href), want, "{href}");
        }
    }

    #[test]
    fn compatibility_quirks() {
        assert_eq!(resolve("http://a/b/c/d", "g?y/../x"), "http://a/b/c/x");
        assert_eq!(resolve("http://a/b/c/d", "../.."), "http://a//");
        assert_eq!(resolve("http://h", ".."), "http://h//");
        assert_eq!(resolve("not a url", "/x"), "/x");
        assert_eq!(resolve("http://h/", ""), "");
    }
}
