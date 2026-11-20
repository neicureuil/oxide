//! Local dates for the Calls screen and the settings forms: the macOS app gets them from Foundation,
//! here they come from libc's `localtime_r`/`mktime` (no date crate in this app).
use crate::oxide::{english, tr, trf};
use std::time::{SystemTime, UNIX_EPOCH};

/// macOS `struct tm`.
#[repr(C)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: *mut std::ffi::c_char,
}
impl Default for Tm {
    fn default() -> Self {
        // SAFETY: integers and a null pointer: all zeroes is a valid `tm`.
        unsafe { std::mem::zeroed() }
    }
}

// settings_time.rs and calendar_dates.rs each declare `struct tm` with the same C layout.
#[allow(clashing_extern_declarations)]
unsafe extern "C" {
    fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
    fn mktime(tm: *mut Tm) -> i64;
}

/// A moment in the Mac's time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    /// 1…12.
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// 0 is Sunday.
    pub weekday: u32,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn local(epoch: i64) -> Local {
    let mut tm = Tm::default();
    // SAFETY: both pointers are valid for the call; `localtime_r` writes only `tm`.
    unsafe { localtime_r(&epoch, &mut tm) };
    Local {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        weekday: tm.tm_wday as u32,
    }
}

/// Epoch seconds of a local wall time; out-of-range fields roll over (day 32 is the next month's 1st).
pub fn from_local(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> i64 {
    let mut tm = Tm {
        tm_year: year - 1900,
        tm_mon: month - 1,
        tm_mday: day,
        tm_hour: hour,
        tm_min: minute,
        tm_isdst: -1,
        ..Tm::default()
    };
    // SAFETY: `tm` is a valid, owned struct; `mktime` normalises it in place.
    unsafe { mktime(&mut tm) }
}

/// Start of the local day holding `epoch`.
pub fn start_of_day(epoch: i64) -> i64 {
    let l = local(epoch);
    from_local(l.year, l.month as i32, l.day as i32, 0, 0)
}

/// Epoch seconds of an ISO-8601 time as the services write it: `2026-11-20T09:30:00.123Z` or `…+01:00`.
pub fn parse_iso(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let s = text.get(from..from + len)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') {
        return None;
    }
    let (year, month, day) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (hour, minute, second) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    let mut rest = &text[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        rest = &frac[digits..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let digits: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
            if digits.len() != 4 {
                return None;
            }
            let h: i64 = digits[..2].parse().ok()?;
            let m: i64 = digits[2..].parse().ok()?;
            sign * (h * 3600 + m * 60)
        }
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

const MONTHS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];
const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub fn month_name(month: u32) -> &'static str {
    let ix = (month.clamp(1, 12) - 1) as usize;
    if english() {
        MONTHS_EN[ix]
    } else {
        MONTHS_FR[ix]
    }
}

/// "jeudi 20 novembre" / "Thursday, November 20", as Foundation's `.weekday(.wide).day().month(.wide)`.
pub fn long_date(l: Local) -> String {
    let weekday = tr(&format!("core.calendar.weekday.{}", l.weekday));
    if english() {
        format!("{weekday}, {} {}", month_name(l.month), l.day)
    } else {
        format!("{weekday} {} {}", l.day, month_name(l.month))
    }
}

/// "Today", "Yesterday", or the long date: the Calls screen's day headers.
pub fn day_title(epoch: Option<i64>, now: i64) -> String {
    let Some(epoch) = epoch else {
        return tr("ui.calls_screen.unknown_date");
    };
    let today = start_of_day(now);
    let day = start_of_day(epoch);
    if day == today {
        return tr("ui.calls_screen.today");
    }
    let l = local(today);
    if day == from_local(l.year, l.month as i32, l.day as i32 - 1, 0, 0) {
        return tr("ui.calls_screen.yesterday");
    }
    long_date(local(epoch))
}

/// "9:30 AM" / "09:30", as Foundation's shortened time in en_US and fr_FR.
pub fn short_time(l: Local) -> String {
    if english() {
        let h12 = match l.hour % 12 {
            0 => 12,
            h => h,
        };
        format!(
            "{h12}:{:02} {}",
            l.minute,
            if l.hour < 12 { "AM" } else { "PM" }
        )
    } else {
        format!("{:02}:{:02}", l.hour, l.minute)
    }
}

pub fn time_of(iso: &str) -> String {
    parse_iso(iso)
        .map(|e| short_time(local(e)))
        .unwrap_or_default()
}

/// "Today · 09:30".
pub fn date_time_of(iso: &str) -> String {
    match parse_iso(iso) {
        Some(e) => format!("{} · {}", day_title(Some(e), now()), short_time(local(e))),
        None => String::new(),
    }
}

/// "45s", "3m", "3m 05s", "1h 02m".
pub fn duration(seconds: i64) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if seconds < 60 {
        return trf(
            "ui.calls_screen.duration.seconds",
            &[("s", &seconds.to_string())],
        );
    }
    if h > 0 {
        return trf(
            "ui.calls_screen.duration.hours",
            &[("h", &h.to_string()), ("m", &format!("{m:02}"))],
        );
    }
    if s == 0 {
        trf("ui.calls_screen.duration.minutes", &[("m", &m.to_string())])
    } else {
        trf(
            "ui.calls_screen.duration.minutes_seconds",
            &[("m", &m.to_string()), ("s", &format!("{s:02}"))],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_service_times() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2026-11-20T09:30:00.123Z"), Some(1_795_167_000));
        assert_eq!(
            parse_iso("2026-11-20T10:30:00.123456789+01:00"),
            Some(1_795_167_000)
        );
        assert_eq!(parse_iso("2026-11-20T09:30:00"), Some(1_795_167_000));
        assert_eq!(parse_iso("yesterday"), None);
        assert_eq!(parse_iso(""), None);
    }

    #[test]
    fn local_round_trip() {
        let epoch = from_local(2026, 11, 20, 9, 30);
        let l = local(epoch);
        assert_eq!(
            (l.year, l.month, l.day, l.hour, l.minute),
            (2026, 11, 20, 9, 30)
        );
        // Day 0 rolls back to the last day of the previous month.
        assert_eq!(local(from_local(2026, 3, 0, 12, 0)).day, 28);
    }

    #[test]
    fn day_titles() {
        let now = from_local(2026, 11, 20, 15, 0);
        assert_eq!(
            day_title(Some(from_local(2026, 11, 20, 0, 5)), now),
            tr("ui.calls_screen.today")
        );
        assert_eq!(
            day_title(Some(from_local(2026, 11, 19, 23, 59)), now),
            tr("ui.calls_screen.yesterday")
        );
        assert_eq!(day_title(None, now), tr("ui.calls_screen.unknown_date"));
        let older = day_title(Some(from_local(2026, 11, 17, 8, 0)), now);
        assert!(older.contains("17"), "{older}");
    }

    #[test]
    fn durations() {
        let both = |s| duration(s);
        assert!(both(45).contains("45"));
        assert!(both(185).contains("05"));
        assert!(both(3720).contains("02"));
    }
}
