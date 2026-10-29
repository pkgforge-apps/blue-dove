//! Duration formatting shared by the UI.

use std::time::Duration;

/// Formats a playback position as `m:ss`, or `h:mm:ss` from an hour on.
pub fn duration(value: Duration) -> String {
    let seconds = value.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let rest = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{minutes}:{rest:02}")
    }
}

/// Formats a length, showing `--:--` until the decoder reports one.
///
/// A game rip has no length in its tags, so its row says that rather than
/// claiming the track is empty.
pub fn length(value: Duration) -> String {
    if value.is_zero() {
        "--:--".to_string()
    } else {
        duration(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_short_durations() {
        assert_eq!(duration(Duration::ZERO), "0:00");
        assert_eq!(duration(Duration::from_secs(7)), "0:07");
        assert_eq!(duration(Duration::from_secs(65)), "1:05");
        assert_eq!(duration(Duration::from_secs(3599)), "59:59");
    }

    #[test]
    fn an_unknown_length_is_dashes() {
        assert_eq!(length(Duration::ZERO), "--:--");
        assert_eq!(length(Duration::from_secs(65)), "1:05");
    }

    #[test]
    fn formats_long_durations() {
        assert_eq!(duration(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(duration(Duration::from_secs(3725)), "1:02:05");
    }
}
