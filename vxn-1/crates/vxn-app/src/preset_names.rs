//! Preset / folder name sanitisation rules (ADR 0005 §5).
//!
//! Pure string rules shared by every backend so folder names and preset
//! filenames cannot drift between the desktop filesystem store
//! (`vxn-engine::preset_io`) and the web browser-storage store
//! (`vxn-web-controller`). No `std::fs`, no engine types — wasm-clean, lives in
//! `vxn-app` (E019 / 0063).

/// Sanitise a folder or preset display name: keep alphanumerics, space, `-`,
/// `_`; map everything else (path separators, `.`, etc.) to `_`. Trim;
/// empty → `"Untitled"`. Folder names and preset filenames share this rule so
/// they can't drift.
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Preset filename derived from the display name (`<sanitised>.toml`).
pub fn preset_filename(name: &str) -> String {
    format!("{}.toml", sanitize_name(name))
}

/// A unique folder name against an existing case-insensitive set. If
/// `suggested` (already sanitised, passed as `stem`) is taken it's suffixed:
/// `"New Folder"`, `"New Folder 1"`, `"New Folder 2"`, … Gaps are not filled.
pub fn unique_folder_name(stem: &str, existing_ci: &[String]) -> String {
    let stem_l = stem.to_lowercase();
    if !existing_ci.iter().any(|e| e == &stem_l) {
        return stem.to_string();
    }
    let mut n = 1;
    loop {
        let candidate = format!("{stem} {n}");
        if !existing_ci.iter().any(|e| e == &candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_is_sanitised() {
        assert_eq!(preset_filename("Mini Bass"), "Mini Bass.toml");
        assert_eq!(preset_filename("a/b:c*?"), "a_b_c__.toml");
        assert_eq!(preset_filename("   "), "Untitled.toml");
    }

    #[test]
    fn sanitise_folder_same_rule() {
        assert_eq!(sanitize_name("Pads"), "Pads");
        assert_eq!(sanitize_name("a/b"), "a_b");
        assert_eq!(sanitize_name(""), "Untitled");
        assert_eq!(sanitize_name("   "), "Untitled");
    }

    #[test]
    fn unique_folder_name_suffixes_collisions() {
        assert_eq!(unique_folder_name("New Folder", &[]), "New Folder");
        let existing = vec!["new folder".to_string()];
        assert_eq!(unique_folder_name("New Folder", &existing), "New Folder 1");
        let existing = vec!["new folder".to_string(), "new folder 1".to_string()];
        assert_eq!(unique_folder_name("New Folder", &existing), "New Folder 2");
        let existing = vec![
            "pads".to_string(),
            "pads 2".to_string(),
            "pads 1".to_string(),
        ];
        assert_eq!(unique_folder_name("Pads", &existing), "Pads 3");
    }
}
