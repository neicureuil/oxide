//! Events by local day and their places on the day grid (`CalendarModel.swift`), and how an event reads
//! at a glance (`CalendarStatus` in `CalendarParts.swift`).
use crate::calendar_dates::{Day, local, parse_iso};
use crate::oxide::tr;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use teams_core::CalendarEventView;

/// Filled once accepted, dashed while tentative, outlined until answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Accepted,
    Tentative,
    Pending,
    Declined,
    Cancelled,
}

impl Status {
    pub fn of(event: &CalendarEventView) -> Status {
        if event.cancelled {
            return Status::Cancelled;
        }
        match event.response.as_str() {
            "Declined" => Status::Declined,
            "Tentative" | "TentativelyAccepted" => Status::Tentative,
            "NotResponded" | "None" | "" if event.can_respond => Status::Pending,
            _ => Status::Accepted,
        }
    }
    pub fn label(self) -> String {
        tr(match self {
            Status::Accepted => "ui.calendar_parts.accepted",
            Status::Tentative => "ui.calendar_parts.tentative",
            Status::Pending => "ui.calendar_parts.not_responded",
            Status::Declined => "ui.calendar_parts.declined",
            Status::Cancelled => "ui.calendar_parts.canceled",
        })
    }
    /// Declined and cancelled events are drawn greyed.
    pub fn faded(self) -> bool {
        matches!(self, Status::Declined | Status::Cancelled)
    }
}

/// The subject, or a placeholder for an event without one.
pub fn title(event: &CalendarEventView) -> String {
    if event.form.subject.trim().is_empty() {
        tr("ui.calendar_parts.no_title")
    } else {
        event.form.subject.clone()
    }
}

pub struct Entry {
    pub event: CalendarEventView,
    pub start: i64,
    pub end: i64,
}

impl Entry {
    /// All-day, or running past `day`: drawn as a banner, without a time.
    pub fn is_banner(&self, day: Day) -> bool {
        self.event.form.all_day || self.start < day.start() || self.end > day.add(1).start()
    }
}

/// Events by local day, parsed and sorted once per change of the core's list instead of once per cell.
#[derive(Default)]
pub struct Index {
    /// Every event, by start.
    pub all: Vec<Entry>,
    by_day: HashMap<Day, Vec<usize>>,
    /// Of the list it was built from, to rebuild only when that changes.
    pub key: u64,
}

pub fn key(events: &[CalendarEventView]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    events.hash(&mut h);
    h.finish()
}

impl Index {
    pub fn new(events: &[CalendarEventView]) -> Index {
        let mut all: Vec<Entry> = events
            .iter()
            .filter_map(|event| {
                Some(Entry {
                    start: parse_iso(&event.form.start)?,
                    end: parse_iso(&event.form.end)?,
                    event: event.clone(),
                })
            })
            .collect();
        all.sort_by(|a, b| a.event.form.start.cmp(&b.event.form.start));
        // An event belongs to every day it overlaps: start < next midnight and end > midnight.
        let mut by_day: HashMap<Day, Vec<usize>> = HashMap::new();
        for (i, entry) in all.iter().enumerate() {
            let mut day = Day::of(entry.start);
            while day.start() < entry.end {
                by_day.entry(day).or_default().push(i);
                day = day.add(1);
            }
        }
        Index {
            all,
            by_day,
            key: key(events),
        }
    }

    pub fn entries(&self, day: Day) -> Vec<&Entry> {
        self.by_day
            .get(&day)
            .map(|ixs| ixs.iter().map(|&i| &self.all[i]).collect())
            .unwrap_or_default()
    }
    pub fn has_events(&self, day: Day) -> bool {
        self.by_day.contains_key(&day)
    }
}

/// Points per hour of the day grid.
pub const HOUR: f32 = 56.;
pub const DAY_HEIGHT: f32 = HOUR * 24.;

/// An event block of a day column: `start`/`end` in points from midnight, and its column in its overlap group.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Index into the entries given to `layout`.
    pub ix: usize,
    pub start: f32,
    pub end: f32,
    pub column: usize,
    pub columns: usize,
}

/// Greedy interval partitioning; each connected overlap group shares its column count.
pub fn layout(spans: &[(i64, i64)], day: Day) -> Vec<Item> {
    let next = day.add(1).start();
    let position = |t: i64| {
        if t >= next {
            return DAY_HEIGHT;
        }
        let (_, h, m) = local(t);
        (h * 60 + m) as f32 * HOUR / 60.
    };
    let mut items: Vec<Item> = spans
        .iter()
        .enumerate()
        .map(|(ix, &(s, e))| {
            let y = position(s);
            Item {
                ix,
                start: y,
                end: (y + 24.).max(position(e)),
                column: 0,
                columns: 1,
            }
        })
        .collect();
    items.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut group = 0;
    let mut ends: Vec<f32> = Vec::new();
    let mut group_end = -1f32;
    let finish = |items: &mut [Item], from: usize, to: usize, count: usize| {
        for item in &mut items[from..to] {
            item.columns = count.max(1);
        }
    };
    for i in 0..items.len() {
        if items[i].start >= group_end {
            finish(&mut items, group, i, ends.len());
            group = i;
            ends.clear();
        }
        let column = ends
            .iter()
            .position(|&e| e <= items[i].start)
            .unwrap_or(ends.len());
        if column == ends.len() {
            ends.push(items[i].end);
        } else {
            ends[column] = items[i].end;
        }
        items[i].column = column;
        group_end = ends.iter().copied().fold(0., f32::max).max(items[i].end);
    }
    let len = items.len();
    finish(&mut items, group, len, ends.len());
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(day: Day, (h1, m1): (u32, u32), (h2, m2): (u32, u32)) -> (i64, i64) {
        (day.at(h1, m1), day.at(h2, m2))
    }

    #[test]
    fn overlapping_events_share_columns_per_group() {
        let day = Day::from_ymd(2026, 11, 18);
        let spans = [
            span(day, (9, 0), (10, 0)),
            span(day, (9, 0), (11, 0)),
            span(day, (10, 0), (10, 30)),
            span(day, (14, 0), (15, 0)),
        ];
        let items = layout(&spans, day);
        let by = |ix: usize| items.iter().find(|i| i.ix == ix).unwrap().clone();
        assert_eq!((by(0).column, by(0).columns), (0, 2));
        assert_eq!((by(1).column, by(1).columns), (1, 2));
        // Starts when the first ends: reuses its column, same group.
        assert_eq!((by(2).column, by(2).columns), (0, 2));
        assert_eq!((by(3).column, by(3).columns), (0, 1));
        assert_eq!(by(0).start, 9. * HOUR);
        assert_eq!(by(1).end, 11. * HOUR);
    }

    #[test]
    fn short_events_keep_a_minimum_height_and_late_ones_stop_at_midnight() {
        let day = Day::from_ymd(2026, 11, 18);
        let items = layout(
            &[
                span(day, (8, 0), (8, 5)),
                (day.at(23, 0), day.add(1).at(2, 0)),
            ],
            day,
        );
        assert_eq!(items[0].end - items[0].start, 24.);
        assert_eq!(items[1].end, DAY_HEIGHT);
    }

    fn event(id: &str, start: i64, end: i64) -> CalendarEventView {
        CalendarEventView {
            id: id.into(),
            form: teams_core::CalendarDraft {
                start: crate::calendar_dates::iso(start),
                end: crate::calendar_dates::iso(end),
                ..Default::default()
            },
            organizer: String::new(),
            response: String::new(),
            cancelled: false,
            recurring: false,
            occurrence: false,
            recurrence_text: String::new(),
            can_edit: false,
            can_cancel: false,
            can_respond: false,
            can_delete: false,
            join_url: String::new(),
        }
    }

    #[test]
    fn events_belong_to_every_day_they_overlap() {
        let day = Day::from_ymd(2026, 11, 18);
        let index = Index::new(&[
            event("multi", day.at(14, 0), day.add(2).at(16, 0)),
            event("short", day.at(9, 0), day.at(10, 0)),
            event("midnight", day.add(3).start(), day.add(4).start()),
        ]);
        assert_eq!(index.all[0].event.id, "short");
        let ids = |d: Day| {
            index
                .entries(d)
                .iter()
                .map(|e| e.event.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(day), ["short", "multi"]);
        assert_eq!(ids(day.add(1)), ["multi"]);
        assert_eq!(ids(day.add(2)), ["multi"]);
        // Ends at midnight: not on the next day.
        assert_eq!(ids(day.add(3)), ["midnight"]);
        assert!(ids(day.add(4)).is_empty());
        let multi = &index.entries(day.add(1))[0];
        assert!(multi.is_banner(day.add(1)));
        assert!(!index.entries(day)[0].is_banner(day));
    }
}
