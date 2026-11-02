//! Picks a device by the name the user typed.

use std::fmt;

/// Why no single device matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    /// No device name contains the wanted text.
    NotFound {
        /// The text the user gave.
        wanted: String,
    },
    /// Several device names contain it and none equals it.
    Ambiguous {
        /// The text the user gave.
        wanted: String,
        /// Every matching device name.
        candidates: Vec<String>,
    },
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { wanted } => write!(f, "no device matches \"{wanted}\""),
            Self::Ambiguous { wanted, candidates } => {
                write!(f, "\"{wanted}\" matches several devices: ")?;
                f.write_str(&candidates.join(", "))
            }
        }
    }
}

impl std::error::Error for SelectError {}

/// The index of the device whose name equals `wanted`, or else the only one that contains
/// it; both comparisons ignore case.
///
/// # Errors
///
/// [`SelectError`] when nothing or more than one name matches.
pub fn select<S: AsRef<str>>(wanted: &str, names: &[S]) -> Result<usize, SelectError> {
    let needle = wanted.trim().to_lowercase();
    let lowered: Vec<String> = names.iter().map(|n| n.as_ref().to_lowercase()).collect();
    if let Some(index) = lowered.iter().position(|n| *n == needle) {
        return Ok(index);
    }
    let matches: Vec<usize> = lowered
        .iter()
        .enumerate()
        .filter(|(_, n)| !needle.is_empty() && n.contains(&needle))
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(SelectError::NotFound {
            wanted: wanted.to_owned(),
        }),
        _ => Err(SelectError::Ambiguous {
            wanted: wanted.to_owned(),
            candidates: matches
                .iter()
                .map(|&i| names[i].as_ref().to_owned())
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 4] = [
        "Speakers (Focusrite USB Audio)",
        "CABLE Input (VB-Audio Virtual Cable)",
        "CABLE Output (VB-Audio Virtual Cable)",
        "Speakers (Realtek(R) Audio)",
    ];

    #[test]
    fn unique_substring_ignoring_case() {
        assert_eq!(select("focusrite", &NAMES), Ok(0));
        assert_eq!(select("cable output", &NAMES), Ok(2));
    }

    #[test]
    fn exact_name_wins_over_substrings() {
        let names = ["Line", "Line 2"];
        assert_eq!(select("line", &names), Ok(0));
    }

    #[test]
    fn several_matches_are_named() {
        let err = select("cable", &NAMES).unwrap_err();
        assert_eq!(
            err,
            SelectError::Ambiguous {
                wanted: "cable".into(),
                candidates: vec![NAMES[1].into(), NAMES[2].into()],
            }
        );
        assert!(err.to_string().contains("CABLE Output"));
    }

    #[test]
    fn nothing_or_empty_is_not_found() {
        assert!(matches!(
            select("scarlett", &NAMES),
            Err(SelectError::NotFound { .. })
        ));
        assert!(matches!(
            select("  ", &NAMES),
            Err(SelectError::NotFound { .. })
        ));
    }
}
