pub(super) fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::format_uptime;

    #[test]
    fn formats_uptime_at_useful_precision() {
        assert_eq!(format_uptime(42), "0m");
        assert_eq!(format_uptime(3_720), "1h 2m");
        assert_eq!(format_uptime(183_600), "2d 3h");
    }
}
