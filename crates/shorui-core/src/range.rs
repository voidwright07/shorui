//! Page range notation: `1-12, 18`, `7-`, `-3`, `odd`, `even`, `last`, `all`.

use crate::{Error, Result};

/// Parse a page range into 1-based page numbers, in the order written.
/// An empty string or `all` means every page. `10-1` counts down.
pub fn parse(spec: &str, page_count: usize) -> Result<Vec<usize>> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("all") {
        return Ok((1..=page_count).collect());
    }
    let mut out = Vec::new();
    for part in spec.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "all" => out.extend(1..=page_count),
            "odd" => out.extend((1..=page_count).filter(|p| p % 2 == 1)),
            "even" => out.extend((1..=page_count).filter(|p| p % 2 == 0)),
            "last" => {
                if page_count > 0 {
                    out.push(page_count)
                }
            }
            _ => {
                let one = |s: &str| -> Result<usize> {
                    let s = s.trim();
                    if s.eq_ignore_ascii_case("last") || s.eq_ignore_ascii_case("end") {
                        return Ok(page_count);
                    }
                    let n: usize = s.parse().map_err(|_| Error::invalid(format!("\"{part}\" is not a page range. Use something like 1-12, 18.")))?;
                    if n == 0 || n > page_count {
                        return Err(Error::invalid(format!("Page {n} does not exist. This file has {page_count} pages.")));
                    }
                    Ok(n)
                };
                if let Some((a, b)) = part.split_once('-') {
                    let a = if a.trim().is_empty() { 1 } else { one(a)? };
                    let b = if b.trim().is_empty() { page_count } else { one(b)? };
                    if a <= b {
                        out.extend(a..=b);
                    } else {
                        out.extend((b..=a).rev());
                    }
                } else {
                    out.push(one(part)?);
                }
            }
        }
    }
    if out.is_empty() {
        return Err(Error::invalid("The page range selects no pages."));
    }
    Ok(out)
}

/// Compact notation for a list of 1-based pages: `[1,2,3,7]` becomes `1-3, 7`.
pub fn format(pages: &[usize]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let start = pages[i];
        let mut end = start;
        while i + 1 < pages.len() && pages[i + 1] == end + 1 {
            end = pages[i + 1];
            i += 1;
        }
        parts.push(if end > start { format!("{start}-{end}") } else { start.to_string() });
        i += 1;
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse("", 3).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse("all", 3).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse("1-3, 7", 10).unwrap(), vec![1, 2, 3, 7]);
        assert_eq!(parse("8-", 10).unwrap(), vec![8, 9, 10]);
        assert_eq!(parse("-2", 10).unwrap(), vec![1, 2]);
        assert_eq!(parse("3-1", 10).unwrap(), vec![3, 2, 1]);
        assert_eq!(parse("odd", 5).unwrap(), vec![1, 3, 5]);
        assert_eq!(parse("even, last", 5).unwrap(), vec![2, 4, 5]);
        assert!(parse("11", 10).is_err());
        assert!(parse("abc", 10).is_err());
    }

    #[test]
    fn formats_runs() {
        assert_eq!(format(&[1, 2, 3, 7]), "1-3, 7");
        assert_eq!(format(&[5]), "5");
        assert_eq!(format(&[]), "");
    }
}
