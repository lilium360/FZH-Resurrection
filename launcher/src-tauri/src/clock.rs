#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[cfg(windows)]
fn local() -> SystemTime {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(st: *mut SystemTime);
    }
    let mut st = SystemTime::default();
    unsafe { GetLocalTime(&mut st) };
    st
}

/// Local wall-clock time as "HH:MM:SS" for the log panel.
#[cfg(windows)]
pub fn local_hms() -> String {
    let st = local();
    format!("{:02}:{:02}:{:02}", st.hour, st.minute, st.second)
}

/// Local date and time as "YYYY-MM-DD HH:MM:SS".
#[cfg(windows)]
pub fn local_date_time() -> String {
    let st = local();
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", st.year, st.month, st.day, st.hour, st.minute, st.second)
}

#[cfg(not(windows))]
pub fn local_date_time() -> String {
    let ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    iso_utc_from_ns(ns).replace('T', " ").replace('Z', "")
}

/// "YYYY-MM-DDTHH:MM:SSZ" for a time in nanoseconds since the Unix epoch.
pub fn iso_utc_from_ns(ns: u64) -> String {
    let secs = (ns / 1_000_000_000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem / 60) % 60, rem % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso_dates() {
        assert_eq!(super::iso_utc_from_ns(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::iso_utc_from_ns(951_782_400_000_000_000), "2000-02-29T00:00:00Z");
        assert_eq!(super::iso_utc_from_ns(1_795_000_000_123_000_000), "2026-11-18T11:06:40Z");
    }
}

#[cfg(not(windows))]
pub fn local_hms() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}
