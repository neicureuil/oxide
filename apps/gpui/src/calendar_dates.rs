//! Local calendar arithmetic and date texts (`CalendarModel.swift`'s `CalendarDates`, `CalendarText`).
//! Days are civil dates; events are UTC instants; the system time zone converts between the two
//! (`settings_time`). Weeks start on Monday, as in the Swift app.
use crate::oxide::{english, tr, trf};

/// A local calendar day, as days since 1970-01-01.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day(pub i32);

impl Day {
    pub fn from_ymd(y: i32, m: u32, d: u32) -> Day {
        // Howard Hinnant's days_from_civil.
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = m as i32;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i32 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Day(era * 146097 + doe - 719468)
    }
    pub fn ymd(self) -> (i32, u32, u32) {
        let z = self.0 + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
        (y, m, d)
    }
    pub fn year(self) -> i32 {
        self.ymd().0
    }
    pub fn month(self) -> u32 {
        self.ymd().1
    }
    pub fn day(self) -> u32 {
        self.ymd().2
    }
    /// 0 is Sunday, as `RecurrenceDraft.weekdays`.
    pub fn weekday(self) -> u32 {
        (self.0 + 4).rem_euclid(7) as u32
    }
    pub fn add(self, days: i32) -> Day {
        Day(self.0 + days)
    }
    /// The Monday of its week.
    pub fn week(self) -> Day {
        self.add(-(((self.weekday() + 6) % 7) as i32))
    }
    pub fn month_start(self) -> Day {
        let (y, m, _) = self.ymd();
        Day::from_ymd(y, m, 1)
    }
    /// Same day `n` months later, kept inside a shorter month (Jan 31 + 1 → Feb 28).
    pub fn add_months(self, n: i32) -> Day {
        let (y, m, d) = self.ymd();
        let months = y * 12 + m as i32 - 1 + n;
        let (y, m) = (months.div_euclid(12), months.rem_euclid(12) as u32 + 1);
        Day::from_ymd(y, m, d.min(days_in_month(y, m)))
    }
    pub fn same_month(self, other: Day) -> bool {
        let (a, b) = (self.ymd(), other.ymd());
        a.0 == b.0 && a.1 == b.1
    }
    pub fn is_weekend(self) -> bool {
        matches!(self.weekday(), 0 | 6)
    }
    /// The instant at `hour:minute` local time that day (the next valid time in a DST gap).
    pub fn at(self, hour: u32, minute: u32) -> i64 {
        let (y, m, d) = self.ymd();
        crate::settings_time::from_local(y, m as i32, d as i32, hour as i32, minute as i32)
    }
    /// Local midnight.
    pub fn start(self) -> i64 {
        self.at(0, 0)
    }
    /// The local day of an instant.
    pub fn of(t: i64) -> Day {
        local(t).0
    }
    pub fn today() -> Day {
        Day::of(now())
    }
    /// `YYYY-MM-DD`, as `RecurrenceDraft.end_date`.
    pub fn iso(self) -> String {
        let (y, m, d) = self.ymd();
        format!("{y:04}-{m:02}-{d:02}")
    }
    pub fn parse_iso(s: &str) -> Option<Day> {
        let mut parts = s.split('-');
        let y = parts.next()?.parse().ok()?;
        let m = parts.next()?.parse().ok()?;
        let d = parts.next()?.parse().ok()?;
        (parts.next().is_none() && (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m))
            .then(|| Day::from_ymd(y, m, d))
    }
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 31,
    }
}

/// Day, hour and minute of an instant, in local time.
pub fn local(t: i64) -> (Day, u32, u32) {
    let l = crate::settings_time::local(t);
    (Day::from_ymd(l.year, l.month, l.day), l.hour, l.minute)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// RFC 3339 as the core sends it: `2026-11-20T08:00:00Z`, `…+01:00`, with or without fractions.
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once(['T', 't', ' '])?;
    let day = Day::parse_iso(date)?;
    let (clock, offset) = match rest.find(['Z', 'z', '+', '-']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let clock = clock.split('.').next()?;
    let mut fields = clock.split(':');
    let h: i64 = fields.next()?.parse().ok()?;
    let m: i64 = fields.next()?.parse().ok()?;
    let sec: i64 = fields.next().map_or(Some(0), |f| f.parse().ok())?;
    let offset = match offset {
        "" | "Z" | "z" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let (oh, om) = o[1..]
                .split_once(':')
                .unwrap_or((&o[1..3.min(o.len())], &o[3.min(o.len())..]));
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().unwrap_or(0) * 60)
        }
    };
    Some(day.0 as i64 * 86400 + h * 3600 + m * 60 + sec - offset)
}

/// `2026-11-20T08:00:00Z`, as Swift's `ISO8601DateFormatter`.
pub fn iso(t: i64) -> String {
    let day = Day(t.div_euclid(86400) as i32);
    let secs = t.rem_euclid(86400);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        day.iso(),
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// The system time zone's identifier, as `TimeZone.current.identifier`.
pub fn time_zone_id() -> String {
    if let Ok(tz) = std::env::var("TZ")
        && !tz.is_empty()
    {
        return tz.trim_start_matches(':').to_owned();
    }
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| {
            let p = p.to_string_lossy().into_owned();
            p.split_once("zoneinfo/").map(|(_, id)| id.to_owned())
        })
        .unwrap_or_else(|| "UTC".to_owned())
}

// Texts, in the app's language (Swift formats with `L10n.locale`).

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
const WEEKDAYS_SHORT_FR: [&str; 7] = ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."];
const WEEKDAYS_SHORT_EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WEEKDAYS_NARROW_FR: [&str; 7] = ["D", "L", "M", "M", "J", "V", "S"];
const WEEKDAYS_NARROW_EN: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];

pub fn month_wide(m: u32) -> &'static str {
    let i = (m as usize).clamp(1, 12) - 1;
    if english() {
        MONTHS_EN[i]
    } else {
        MONTHS_FR[i]
    }
}
pub fn month_short(m: u32) -> String {
    tr(&format!("core.calendar.month.{m}"))
}
/// "jeudi" / "Thursday".
pub fn weekday_wide(weekday: u32) -> String {
    tr(&format!("core.calendar.weekday.{weekday}"))
}
pub fn weekday_short(weekday: u32) -> &'static str {
    let i = weekday as usize % 7;
    if english() {
        WEEKDAYS_SHORT_EN[i]
    } else {
        WEEKDAYS_SHORT_FR[i]
    }
}
pub fn weekday_narrow(weekday: u32) -> &'static str {
    let i = weekday as usize % 7;
    if english() {
        WEEKDAYS_NARROW_EN[i]
    } else {
        WEEKDAYS_NARROW_FR[i]
    }
}

fn capitalized(s: String) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => s,
    }
}

/// "20 nov." / "Nov 20".
pub fn day_month_short(d: Day) -> String {
    trf(
        "core.calendar.day_month",
        &[
            ("day", &d.day().to_string()),
            ("month", &month_short(d.month())),
        ],
    )
}
/// "20 nov. 2026" / "Nov 20, 2026".
pub fn day_month_short_year(d: Day) -> String {
    if english() {
        format!("{}, {}", day_month_short(d), d.year())
    } else {
        format!("{} {}", day_month_short(d), d.year())
    }
}
/// "20 novembre" / "November 20".
pub fn day_month_wide(d: Day) -> String {
    if english() {
        format!("{} {}", month_wide(d.month()), d.day())
    } else {
        format!("{} {}", d.day(), month_wide(d.month()))
    }
}
/// "jeudi 20 novembre" / "Thursday, November 20".
pub fn long_day(d: Day) -> String {
    if english() {
        format!("{}, {}", weekday_wide(d.weekday()), day_month_wide(d))
    } else {
        format!("{} {}", weekday_wide(d.weekday()), day_month_wide(d))
    }
}
/// "jeudi 20 novembre 2026" / "Thursday, November 20, 2026".
pub fn long_day_year(d: Day) -> String {
    if english() {
        format!("{}, {}", long_day(d), d.year())
    } else {
        format!("{} {}", long_day(d), d.year())
    }
}
/// "novembre 2026" / "November 2026".
pub fn month_year(d: Day) -> String {
    format!("{} {}", month_wide(d.month()), d.year())
}
/// "jeudi 20" / "Thursday 20".
pub fn weekday_day(d: Day) -> String {
    format!("{} {}", weekday_wide(d.weekday()), d.day())
}
/// "jeu. 20 nov. 2026" / "Thu, Nov 20, 2026": the editor's date fields.
pub fn short_date(d: Day) -> String {
    if english() {
        format!(
            "{}, {}",
            weekday_short(d.weekday()),
            day_month_short_year(d)
        )
    } else {
        format!("{} {}", weekday_short(d.weekday()), day_month_short_year(d))
    }
}

fn clock(hour: u32, minute: u32) -> String {
    if english() {
        let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
        format!("{h12}:{minute:02} {}", if hour < 12 { "AM" } else { "PM" })
    } else {
        format!("{hour:02}:{minute:02}")
    }
}
/// "09:30" / "9:30 AM".
pub fn time(t: i64) -> String {
    let (_, h, m) = local(t);
    clock(h, m)
}
pub fn time_of(hour: u32, minute: u32) -> String {
    clock(hour, minute)
}
/// "20 nov. 2026 à 14:00" / "Nov 20, 2026 at 2:00 PM".
pub fn date_time(t: i64) -> String {
    let d = Day::of(t);
    if english() {
        format!("{} at {}", day_month_short_year(d), time(t))
    } else {
        format!("{} à {}", day_month_short_year(d), time(t))
    }
}

/// An hour of the day grid, in the catalog's `hour_format` ("HH:00" → "09:00", "h a" → "9 AM").
pub fn hour_label(hour: u32) -> String {
    let format = tr("ui.calendar_screen.hour_format");
    let mut out = String::new();
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let run = chars[i..].iter().take_while(|&&x| x == c).count();
        match c {
            'H' => {
                out += &if run >= 2 {
                    format!("{hour:02}")
                } else {
                    hour.to_string()
                }
            }
            'h' => {
                let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
                out += &if run >= 2 {
                    format!("{h12:02}")
                } else {
                    h12.to_string()
                };
            }
            'm' => out += "00",
            'a' => out += if hour < 12 { "AM" } else { "PM" },
            _ => out.extend(std::iter::repeat_n(c, run)),
        }
        i += run;
    }
    out
}

/// "45 min", "1 h 30" / "1h 30m".
pub fn duration(seconds: i64) -> String {
    let minutes = ((seconds as f64 / 60.).round() as i64).max(0);
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let rest = minutes % 60;
    let h = (minutes / 60).to_string();
    if rest == 0 {
        trf("ui.calendar_parts.duration_hours", &[("h", &h)])
    } else {
        trf(
            "ui.calendar_parts.duration_hours_minutes",
            &[("h", &h), ("m", &format!("{rest:02}"))],
        )
    }
}

/// "Dans 12 min", "En cours", "Demain à 10:30"…
pub fn relative(start: i64, end: i64, now: i64) -> String {
    if start <= now && now < end {
        return tr("ui.calendar_parts.now");
    }
    let minutes = (start - now) / 60;
    if minutes < 1 {
        return tr("ui.calendar_parts.starting_now");
    }
    if minutes < 60 {
        return trf(
            "ui.calendar_parts.in_min",
            &[("minutes", &minutes.to_string())],
        );
    }
    let t = time(start);
    let today = Day::of(now);
    let day = Day::of(start);
    if day == today {
        return trf("ui.calendar_parts.today_at", &[("time", &t)]);
    }
    if day == today.add(1) {
        return trf("ui.calendar_parts.tomorrow_at", &[("time", &t)]);
    }
    weekday_day(day) + &trf("ui.calendar_parts.at", &[("time", &t)])
}

/// "Aujourd’hui · jeudi 20 novembre", "Samedi 22 novembre".
pub fn day_title(day: Day) -> String {
    let date = long_day(day);
    let today = Day::today();
    if day == today {
        return trf("ui.calendar_parts.today", &[("date", &date)]);
    }
    if day == today.add(1) {
        return trf("ui.calendar_parts.tomorrow", &[("date", &date)]);
    }
    capitalized(date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_days_round_trip() {
        assert_eq!(Day::from_ymd(1970, 1, 1), Day(0));
        assert_eq!(Day(0).weekday(), 4); // a Thursday
        for n in [-800_000, -1, 0, 59, 10_000, 20_412, 800_000] {
            let (y, m, d) = Day(n).ymd();
            assert_eq!(Day::from_ymd(y, m, d), Day(n));
        }
        assert_eq!(Day::from_ymd(2026, 11, 20).weekday(), 5);
    }

    #[test]
    fn weeks_start_on_monday_and_months_clamp() {
        let friday = Day::from_ymd(2026, 11, 20);
        assert_eq!(friday.week().ymd(), (2026, 11, 16));
        let sunday = Day::from_ymd(2026, 11, 22);
        assert_eq!(sunday.week().ymd(), (2026, 11, 16));
        assert_eq!(
            Day::from_ymd(2026, 1, 31).add_months(1).ymd(),
            (2026, 2, 28)
        );
        assert_eq!(
            Day::from_ymd(2024, 1, 31).add_months(1).ymd(),
            (2024, 2, 29)
        );
        assert_eq!(
            Day::from_ymd(2026, 1, 15).add_months(-1).ymd(),
            (2025, 12, 15)
        );
        assert_eq!(Day::parse_iso("2026-02-29"), None);
        assert_eq!(
            Day::parse_iso("2026-12-21").map(Day::iso).as_deref(),
            Some("2026-12-21")
        );
    }

    #[test]
    fn instants_parse_in_every_form_the_core_sends() {
        let z = parse_iso("2026-10-25T00:30:00Z").unwrap();
        assert_eq!(parse_iso("2026-10-25T02:30:00+02:00"), Some(z));
        assert_eq!(parse_iso("2026-10-24T19:30:00-05:00"), Some(z));
        assert_eq!(parse_iso("2026-10-25T00:30:00.000Z"), Some(z));
        assert_eq!(parse_iso("2026-10-25T00:30:00.1234567"), Some(z));
        assert_eq!(iso(z), "2026-10-25T00:30:00Z");
        assert_eq!(parse_iso("nope"), None);
    }

    #[test]
    fn local_days_and_times_agree() {
        let day = Day::from_ymd(2026, 11, 20);
        let nine = day.at(9, 30);
        assert_eq!(local(nine), (day, 9, 30));
        assert_eq!(Day::of(day.start()), day);
        assert_eq!(Day::of(day.add(1).start() - 1), day);
    }
}
