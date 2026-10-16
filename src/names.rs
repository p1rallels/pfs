//! Fork labels. pfs picks the number, the name is only a label.

const MAX_LEN: usize = 40;

/// `"7-Add CSV!"` → `"add-csv"`. A leading `<number>-` is dropped because
/// pfs owns the number. `None` when nothing usable is left.
pub fn clean(raw: &str) -> Option<String> {
    let mapped: String = strip_number(raw.trim())
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '.' | '_' => c,
            'A'..='Z' => c.to_ascii_lowercase(),
            _ => '-',
        })
        .collect();
    let collapsed = mapped
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .split("..")
        .collect::<Vec<_>>()
        .join(".");
    let short: String = collapsed.chars().take(MAX_LEN).collect();
    let trimmed = short
        .trim_end_matches(".lock")
        .trim_matches(|c| matches!(c, '-' | '.' | '_'));
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn strip_number(raw: &str) -> &str {
    let digits = raw.chars().take_while(char::is_ascii_digit).count();
    match (digits, raw[digits..].strip_prefix('-')) {
        (1.., Some(rest)) => rest,
        _ => raw,
    }
}

pub fn label(id: u32, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{id}-{name}"),
        None => id.to_string(),
    }
}

pub fn branch(id: u32, name: Option<&str>) -> String {
    format!("pfs/{}", label(id, name))
}

/// The fork number in a saved `pfs/<n>-…` branch name.
pub fn id_from_branch(branch: &str) -> Option<u32> {
    let rest = branch.strip_prefix("pfs/")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_names() {
        assert_eq!(clean("add-csv").as_deref(), Some("add-csv"));
        assert_eq!(clean("Add CSV export!").as_deref(), Some("add-csv-export"));
        assert_eq!(clean("7-add-csv").as_deref(), Some("add-csv"));
        assert_eq!(clean("404-page").as_deref(), Some("page"));
        assert_eq!(clean("2024").as_deref(), Some("2024"));
        assert_eq!(clean("a..b").as_deref(), Some("a.b"));
        assert_eq!(clean("x.lock").as_deref(), Some("x"));
        assert_eq!(clean("7-"), None);
        assert_eq!(clean("  "), None);
        assert_eq!(clean("---"), None);
    }

    #[test]
    fn caps_length() {
        let long = "a".repeat(80);
        assert_eq!(clean(&long).map(|name| name.len()), Some(MAX_LEN));
    }

    #[test]
    fn labels_and_branches() {
        assert_eq!(label(8, Some("add-csv")), "8-add-csv");
        assert_eq!(branch(8, None), "pfs/8");
        assert_eq!(id_from_branch("pfs/12-x"), Some(12));
        assert_eq!(id_from_branch("pfs/3"), Some(3));
        assert_eq!(id_from_branch("main"), None);
    }
}
