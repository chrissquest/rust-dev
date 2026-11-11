//! Creature names.
//!
//! The game stores its text as NUL-separated strings. The ordered short-name table for the 116
//! vivosaurs lives in entry 0 of the `text/japanese` archive, preceded by other text: the run of
//! creature names starts at the string `T-Rex` (creature 1) and is 116 long.
//!
//! Three independent things agree on this numbering, which is why it is trustworthy: this table,
//! the files `motion/battle_creature/creature_001..116.bin`, and the `3CL` slot ids.

use anyhow::{Result, bail};

/// Vivosaurs in this game.
pub const CREATURE_COUNT: usize = 116;

/// The name that starts the creature run, i.e. creature 1.
pub const ANCHOR: &str = "T-Rex";

/// Splits a text blob into its NUL-separated, non-empty strings.
pub fn strings(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (index, &byte) in data.iter().enumerate() {
        if byte == 0 {
            if index > start {
                out.push(String::from_utf8_lossy(&data[start..index]).into_owned());
            }
            start = index + 1;
        }
    }
    if start < data.len() {
        out.push(String::from_utf8_lossy(&data[start..]).into_owned());
    }
    out
}

/// Returns `count` creature names in id order, so result\[0\] is creature 1.
pub fn creature_names(text: &[u8], count: usize) -> Result<Vec<String>> {
    let all = strings(text);

    let anchor = all
        .iter()
        .position(|entry| entry == ANCHOR)
        .ok_or_else(|| anyhow::anyhow!("the creature name anchor `{ANCHOR}` was not found in the text"))?;

    if anchor + count > all.len() {
        bail!(
            "the creature name run needs {} entries from index {} but the text only has {}",
            count,
            anchor,
            all.len()
        );
    }

    Ok(all[anchor..anchor + count].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_nul_separated_strings() {
        let text = b"alpha\0\0beta\0gamma";
        assert_eq!(strings(text), vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn finds_the_run_starting_at_the_anchor() {
        let text = b"junk\0more junk\0T-Rex\0Daspleto\0Gorgo\0Tarbo\0";
        assert_eq!(
            creature_names(text, 3).unwrap(),
            vec!["T-Rex", "Daspleto", "Gorgo"]
        );
    }

    #[test]
    fn rejects_text_without_the_anchor() {
        assert!(creature_names(b"nothing here\0", 3).is_err());
    }

    #[test]
    fn rejects_a_run_that_runs_off_the_end() {
        let text = b"T-Rex\0Daspleto\0";
        assert!(creature_names(text, 5).is_err());
    }
}
