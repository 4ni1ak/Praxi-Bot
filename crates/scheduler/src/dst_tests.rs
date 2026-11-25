//! Table-driven time-zone / DST checks over fixed-offset, DST, half-hour DST,
//! half-hour offset and southern-hemisphere zones. Pure maths, no clock.

use chrono::{DateTime, NaiveDate, Timelike};
use chrono_tz::Tz;

use crate::{ScheduleSpec, parse_natural_language_with_tz};

const ZONES: [&str; 7] = [
    "Europe/Istanbul",     // fixed +03, no DST
    "Europe/London",       // DST, 1 h
    "America/New_York",    // DST, 1 h
    "Australia/Lord_Howe", // DST, 30 min
    "Asia/Kolkata",        // +05:30, no DST
    "Australia/Sydney",    // southern hemisphere DST
    "America/Sao_Paulo",   // southern hemisphere, no DST today
];

const START_MS: i64 = 1_767_225_600_000; // 2026-01-01T00:00:00Z

fn local_date_hour_min(ms: i64, tz: Tz) -> (NaiveDate, u32, u32) {
    let local = DateTime::from_timestamp_millis(ms)
        .unwrap()
        .with_timezone(&tz);
    (local.date_naive(), local.hour(), local.minute())
}

/// A daily 09:00 job fires once per local day, at 09:00 local, and time moves forward.
#[test]
fn daily_local_time_fires_once_per_local_day_at_a_stable_hour_in_every_zone() {
    for zone in ZONES {
        let tz: Tz = zone.parse().unwrap();
        let spec = ScheduleSpec::cron_tz("0 9 * * *", zone);
        let mut cursor = START_MS;
        let mut prev_day: Option<NaiveDate> = None;
        for _ in 0..400 {
            let next = spec.next_after(cursor).unwrap().unwrap();
            assert!(next > cursor, "{zone}: next_after must strictly increase");
            let (day, h, m) = local_date_hour_min(next, tz);
            assert_eq!((h, m), (9, 0), "{zone}: local time drifted on {day}");
            if let Some(p) = prev_day {
                assert_eq!(day, p.succ_opt().unwrap(), "{zone}: one run per local day");
            }
            prev_day = Some(day);
            cursor = next;
        }
    }
}

/// `Every` is a fixed number of seconds: DST never stretches or shrinks it.
#[test]
fn every_intervals_are_dst_independent() {
    let spec = ScheduleSpec::Every { seconds: 3600 };
    let first = spec.next_after(START_MS).unwrap().unwrap();
    let mut cursor = first;
    for _ in 0..24 * 400 {
        let next = spec.next_after(cursor).unwrap().unwrap();
        assert_eq!(next - cursor, 3_600_000);
        cursor = next;
    }
}

/// The 30-minute DST zone: a daily 02:15 job (inside Lord Howe's 02:00 to 02:30
/// spring gap and fall overlap) still fires at most once per day and never goes backwards.
#[test]
fn lord_howe_half_hour_transitions_fire_at_most_once_per_day() {
    let zone = "Australia/Lord_Howe";
    let tz: Tz = zone.parse().unwrap();
    let spec = ScheduleSpec::cron_tz("15 2 * * *", zone);
    let mut cursor = START_MS;
    let mut days = std::collections::HashSet::new();
    for _ in 0..400 {
        let next = spec.next_after(cursor).unwrap().unwrap();
        assert!(next > cursor);
        let (day, _, _) = local_date_hour_min(next, tz);
        assert!(days.insert(day), "fired twice on {day}");
        cursor = next;
    }
}

/// Natural language with a time zone resolves to a cron carrying that zone,
/// and the zone survives re-parsing the stored string (what an edit does).
#[test]
fn natural_language_keeps_the_zone_in_the_stored_spec() {
    for (phrase, zone, want) in [
        ("her gün 09:00", "Europe/Istanbul", "0 9 * * *"),
        (
            "every weekday at 8:15am",
            "America/New_York",
            "15 8 * * 1-5",
        ),
    ] {
        let spec = parse_natural_language_with_tz(phrase, START_MS, Some(zone))
            .unwrap_or_else(|| panic!("{phrase} did not parse"));
        let ScheduleSpec::Cron(raw) = &spec else {
            panic!("{phrase}: expected cron, got {spec:?}");
        };
        assert!(raw.contains(zone), "{raw} must carry {zone}");
        assert!(raw.contains(want), "{raw} must contain {want}");
        // The stored string is what a PUT sends back unchanged: same next run.
        let round = ScheduleSpec::Cron(raw.clone());
        assert_eq!(
            spec.next_after(START_MS).unwrap(),
            round.next_after(START_MS).unwrap()
        );
        let tz: Tz = zone.parse().unwrap();
        let next = spec.next_after(START_MS).unwrap().unwrap();
        let (_, h, _) = local_date_hour_min(next, tz);
        assert!(h == 9 || h == 8, "{phrase}: local hour {h}");
    }
}
