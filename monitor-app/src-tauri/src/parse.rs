//! Pure parsing functions for system command output.
//!
//! These are separated from command execution so they can be unit-tested
//! with fixed fixtures (no real hardware, no shell). Every parser returns
//! Option/Result so a missing or malformed field is NEVER silently 0 (F04).

/// Extract the numeric value that follows a `key:` label in
/// `memory_pressure` / `vm_stat`-style output. The value may appear after
/// a variable number of words ("Pages wired down: 12345."), so we locate
/// the colon and parse the first token after it, stripping a trailing '.'.
///
/// Returns None if the key is absent or the value is not a number.
pub fn parse_labeled_number(text: &str, key: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some(colon_idx) = line.find(key) {
            // Ensure we matched the whole key (it ends at the colon).
            let after = &line[colon_idx + key.len()..];
            let after = after.trim_start_matches(':').trim();
            // Take the first whitespace-separated token, strip trailing '.'.
            let token = after.split_whitespace().next()?;
            let token = token.trim_end_matches('.');
            if let Ok(n) = token.parse::<u64>() {
                return Some(n);
            }
        }
    }
    None
}

/// Parse one `"key" = value` entry from an ioreg PerformanceStatistics
/// block. Keys are matched exactly (after trimming quotes) so that
/// "In use system memory (driver)" does NOT match "In use system memory".
///
/// The block may include the leading `"PerformanceStatistics" = {` prefix
/// and trailing `}`; we strip the outer braces before splitting entries.
pub fn parse_ioreg_stat<'a>(block: &'a str, exact_key: &str) -> Option<u64> {
    let start = block.find('{')?;
    let end = block.rfind('}')?;
    let inner = &block[start + 1..end];
    for part in inner.split(',') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?.trim().trim_matches('"');
        let v = kv.next().map(|s| s.trim().trim_end_matches('}'))?;
        if k == exact_key {
            return v.parse::<u64>().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEM_PRESSURE: &str = "\
System-wide memory free percentage: 42%
Pages free:          1000.
Pages active:        2000.
Pages inactive:      3000.
Pages speculative:   400.
Pages throttled:     0.
Pages wired down:    5000.
Pages purgeable:     60.
Pages used by compressor: 7000.
";

    #[test]
    fn parses_simple_field() {
        assert_eq!(parse_labeled_number(MEM_PRESSURE, "Pages active:"), Some(2000));
    }

    #[test]
    fn parses_multiword_wired() {
        // F02 regression: old code took nth(2) which is "down:" -> parse fail -> 0.
        assert_eq!(parse_labeled_number(MEM_PRESSURE, "Pages wired down:"), Some(5000));
    }

    #[test]
    fn parses_multiword_compressor() {
        // F02 regression: old code took nth(2) which is "by" -> parse fail -> 0.
        assert_eq!(parse_labeled_number(MEM_PRESSURE, "Pages used by compressor:"), Some(7000));
    }

    #[test]
    fn missing_key_is_none_not_zero() {
        assert_eq!(parse_labeled_number(MEM_PRESSURE, "Pages nonexistent:"), None);
    }

    #[test]
    fn does_not_match_prefix_of_longer_key() {
        // "Pages wired" must not match "Pages wired down:" partially.
        let text = "Pages wired down: 1234.\n";
        assert_eq!(parse_labeled_number(text, "Pages wired:"), None);
    }

    const PERF_STATS: &str = r#""PerformanceStatistics" = {"Device Utilization %"=27, "In use system memory (driver)"=0, "In use system memory"=1041350656, "Alloc system memory"=3850000000}"#;

    #[test]
    fn gpu_in_use_exact_match() {
        // F03 regression: contains("In use system memory") matches "(driver)"=0 first.
        assert_eq!(
            parse_ioreg_stat(PERF_STATS, "In use system memory"),
            Some(1041350656)
        );
    }

    #[test]
    fn gpu_driver_key_is_separate() {
        assert_eq!(
            parse_ioreg_stat(PERF_STATS, "In use system memory (driver)"),
            Some(0)
        );
    }

    #[test]
    fn gpu_utilization() {
        assert_eq!(parse_ioreg_stat(PERF_STATS, "Device Utilization %"), Some(27));
    }

    #[test]
    fn gpu_missing_key_none() {
        assert_eq!(parse_ioreg_stat(PERF_STATS, "No such key"), None);
    }
}
