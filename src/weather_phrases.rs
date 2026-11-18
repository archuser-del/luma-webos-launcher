//! Context-aware, stable tech-themed weather headline selection (the "Tech" greeting style,
//! stored as `rude` for compatibility with saved preferences).
use crate::model::Preferences;
use crate::weather::Reading;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum GreetingStyle {
    Classic,
    #[default]
    Rude,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DayRecord {
    pub day: i32,
    pub category: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    ClearNice,
    Chilly,
    Cold,
    Frost,
    Hot,
    Cloudy,
    Rain,
    Thunder,
    Snow,
    Wind,
    Fog,
    Night,
    /// The same weather two or more days running (tagged by weather).
    Still,
    /// A sudden change (tagged colder, warmer or any).
    Change,
    RainSoon,
    Clearing,
    Heatwave,
    DeepFreeze,
    Monday,
    Friday,
    Weekend,
    SmallHours,
    NewYear,
    NewYearsEve,
    Valentine,
    LeapDay,
    PiDay,
    AprilFools,
    StarWars,
    Independence,
    ProgrammersDay,
    Halloween,
    Thanksgiving,
    Christmas,
    Friday13,
    /// Our own days and our weekly line (tools/couple-headlines.py).
    FirstKiss,
    Dating,
    Proposal,
    Us,
}
/// The special kinds, label first, for the headline list and its tests.
const SPECIAL_LABELS: [(&str, Category); 27] = [
    ("first_kiss", Category::FirstKiss),
    ("dating", Category::Dating),
    ("proposal", Category::Proposal),
    ("us", Category::Us),
    ("rain_soon", Category::RainSoon),
    ("clearing", Category::Clearing),
    ("heatwave", Category::Heatwave),
    ("deep_freeze", Category::DeepFreeze),
    ("monday", Category::Monday),
    ("friday", Category::Friday),
    ("weekend", Category::Weekend),
    ("small_hours", Category::SmallHours),
    ("new_year", Category::NewYear),
    ("new_years_eve", Category::NewYearsEve),
    ("valentine", Category::Valentine),
    ("leap_day", Category::LeapDay),
    ("pi_day", Category::PiDay),
    ("april_fools", Category::AprilFools),
    ("star_wars", Category::StarWars),
    ("independence", Category::Independence),
    ("programmers_day", Category::ProgrammersDay),
    ("halloween", Category::Halloween),
    ("thanksgiving", Category::Thanksgiving),
    ("christmas", Category::Christmas),
    ("friday_13", Category::Friday13),
    ("still", Category::Still),
    ("change", Category::Change),
];
impl Category {
    fn from_label(s: &str) -> Option<Self> {
        Some(match s {
            "clear_nice" => Self::ClearNice,
            "chilly" => Self::Chilly,
            "cold" => Self::Cold,
            "frost" => Self::Frost,
            "hot" => Self::Hot,
            "cloudy" => Self::Cloudy,
            "rain" => Self::Rain,
            "thunder" => Self::Thunder,
            "snow" => Self::Snow,
            "wind" => Self::Wind,
            "fog" => Self::Fog,
            "night" => Self::Night,
            other => {
                return SPECIAL_LABELS
                    .iter()
                    .find(|(label, _)| *label == other)
                    .map(|(_, category)| *category);
            }
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::ClearNice => "sunny",
            Self::Chilly => "chilly",
            Self::Cold => "cold",
            Self::Frost => "frost",
            Self::Hot => "hot",
            Self::Cloudy => "grey",
            Self::Rain => "rain",
            Self::Thunder => "storm",
            Self::Snow => "snow",
            Self::Wind => "windy",
            Self::Fog => "fog",
            Self::Night => "night",
            other => SPECIAL_LABELS
                .iter()
                .find(|(_, category)| *category == other)
                .map_or("special", |(label, _)| label),
        }
    }
    /// Weather worth remarking on when it lasts, and the tag of its streak lines.
    pub fn harsh(self) -> bool {
        self.still_tag().is_some()
    }
    fn still_tag(self) -> Option<&'static str> {
        Some(match self {
            Self::Cold | Self::Frost => "cold",
            Self::Rain => "rain",
            Self::Cloudy => "grey",
            Self::Hot => "hot",
            Self::Snow => "snow",
            Self::Fog => "fog",
            _ => return None,
        })
    }
}
#[derive(Clone, Debug)]
pub struct Template {
    pub category: Category,
    /// Which streak or change a Still/Change line is for ("rain", "colder").
    pub tag: String,
    pub title: String,
    pub subtitle: String,
}
/// Every headline Luma can show, from src/weather-headlines.tsv (written by
/// tools/generate-headlines.py from weather-phrases.md and its word lists):
/// over 10,000 lines, about 900 for each kind of weather.
pub fn templates() -> &'static [Template] {
    static TABLE: OnceLock<Vec<Template>> = OnceLock::new();
    TABLE.get_or_init(|| {
        include_str!("weather-headlines.tsv")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                let (label, tag) = parts.next()?.split_once(':').map_or_else(
                    || (line.split('\t').next().unwrap_or(""), ""),
                    |(l, t)| (l, t),
                );
                let category = Category::from_label(label)?;
                let title = parts.next()?.trim();
                // A headline may stand alone, with no subtitle.
                let subtitle = parts.next().unwrap_or("").trim();
                (!title.is_empty()).then(|| Template {
                    category,
                    tag: tag.into(),
                    title: title.into(),
                    subtitle: subtitle.into(),
                })
            })
            .collect()
    })
}

/// How many shown headlines Luma remembers, so none comes back soon.
const SEEN_LIMIT: usize = 1000;

/// A short, stable fingerprint of a headline (FNV-1a), for the "shown" list.
fn fingerprint(template: &Template) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in template
        .title
        .bytes()
        .chain([0])
        .chain(template.subtitle.bytes())
    {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

pub fn weather_category(reading: Reading, hour: u8) -> Category {
    // WMO codes. Unknown codes still resolve to temperature/sky, never to a blank title.
    match reading.code {
        95 | 96 | 99 => return Category::Thunder,
        71 | 73 | 75 | 77 | 85 | 86 => return Category::Snow,
        51 | 53 | 55 | 56 | 57 | 61 | 63 | 65 | 66 | 67 | 80 | 81 | 82 => return Category::Rain,
        45 | 48 => return Category::Fog,
        _ => {}
    }
    if reading.wind_speed >= 35.0 {
        return Category::Wind;
    }
    if !reading.is_day || !(5..23).contains(&hour) {
        return Category::Night;
    }
    if reading.temperature < 0.0 {
        return Category::Frost;
    }
    if reading.temperature < 5.0 {
        return Category::Cold;
    }
    if reading.temperature >= 25.0 {
        return Category::Hot;
    }
    if matches!(reading.code, 2 | 3) {
        return Category::Cloudy;
    }
    if reading.temperature < 15.0 {
        Category::Chilly
    } else {
        Category::ClearNice
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Phrase {
    pub category: Category,
    pub title: String,
    pub subtitle: String,
}

#[derive(Default)]
pub struct Engine {
    pub current: Option<Phrase>,
    pub previous: Option<Phrase>,
    pub changed_at: Option<Instant>,
    last_category: Option<Category>,
    last_reading: Option<Reading>,
    last_update: i64,
    chosen_category: Option<Category>,
    chosen_bucket: i64,
    change_until: i64,
    old_label: String,
    new_label: String,
    /// Degrees Celsius changed (negative: colder).
    delta: f64,
}
impl Engine {
    pub fn sync(
        &mut self,
        prefs: &mut Preferences,
        reading: Option<(Reading, i64)>,
        now: i64,
        day: i32,
        hour: u8,
    ) -> bool {
        let Some((reading, updated_at)) = reading.filter(|(r, _)| r.valid()) else {
            if self.current.is_some() {
                self.previous = self.current.take();
                self.changed_at = Some(Instant::now());
            }
            return false;
        };
        let base = weather_category(reading, hour);
        let changed_reading = self.last_update != updated_at || self.last_reading != Some(reading);
        let mut save = false;
        if changed_reading {
            if let Some(old) = self.last_category
                && old != base
            {
                self.change_until = now + 3600;
                self.old_label = old.label().into();
                self.new_label = base.label().into();
                self.delta = self
                    .last_reading
                    .map_or(0., |r| reading.temperature - r.temperature);
            }
            if let Some(three_hours_ago) = reading.temperature_3h_ago
                && (reading.temperature - three_hours_ago).abs() >= 6.0
                && (now - updated_at).abs() < 3600
            {
                self.change_until = now + 3600;
                self.old_label = degrees(three_hours_ago, prefs.temperature_unit);
                self.new_label = degrees(reading.temperature, prefs.temperature_unit);
                self.delta = reading.temperature - three_hours_ago;
            }
            self.last_reading = Some(reading);
            self.last_category = Some(base);
            self.last_update = updated_at;
            if !prefs
                .weather_day_history
                .iter()
                .any(|record| record.day == day && record.category == base.label())
            {
                prefs.weather_day_history.retain(|record| record.day != day);
                prefs.weather_day_history.push(DayRecord {
                    day,
                    category: base.label().into(),
                });
                prefs.weather_day_history.sort_by_key(|r| r.day);
                if prefs.weather_day_history.len() > 14 {
                    prefs
                        .weather_day_history
                        .drain(..prefs.weather_day_history.len() - 14);
                }
                save = true;
            }
        }
        let mut run = 0;
        if base.harsh() {
            let mut d = day;
            for entry in prefs.weather_day_history.iter().rev() {
                if entry.day == d && entry.category == base.label() {
                    run += 1;
                    d -= 1;
                } else if entry.day < d {
                    break;
                }
            }
        }
        let bucket = now.div_euclid(3600);
        let date = time::Date::from_julian_day(day).ok();
        let forecast = forecast_case(reading, base);
        // Specials take turns with the weather so neither wears out: a
        // holiday every other hour, a streak two hours in three, a time of
        // the week one hour in three. A sudden change and a forecast speak
        // while they last.
        // Our own days take every other hour; a holiday on the same day
        // (New Year's Eve) the hours between.
        let ours = date.and_then(couple_day);
        let category = if now < self.change_until {
            Category::Change
        } else if let Some((day, _)) = ours
            && bucket % 2 == 0
        {
            day
        } else if let Some(holiday) = date.and_then(|d| holiday(d, hour))
            && bucket % 2 == i64::from(ours.is_some())
        {
            holiday
        } else if date.is_some_and(|d| weekly_slot(d) == (d, hour)) {
            Category::Us
        } else if run >= 2 && bucket % 3 != 2 {
            Category::Still
        } else if let Some((case, _)) = forecast {
            case
        } else if reading.temperature >= 35. && bucket % 2 == 0 {
            Category::Heatwave
        } else if reading.temperature <= -15. && bucket % 2 == 0 {
            Category::DeepFreeze
        } else if let Some(time) = date.and_then(|d| time_of_week(d, hour))
            && bucket % 3 == 0
        {
            time
        } else {
            base
        };
        if self
            .current
            .as_ref()
            .is_some_and(|p| p.category == category)
            && self.chosen_category == Some(category)
            && self.chosen_bucket == bucket
        {
            return save;
        }
        // Streak lines are for this weather; change lines say colder or
        // warmer only for a real swing (3 °C or more).
        let still_tag = base.still_tag().unwrap_or("");
        let change_tag = if self.delta <= -3. {
            "colder"
        } else if self.delta >= 3. {
            "warmer"
        } else {
            ""
        };
        let choices: Vec<_> = templates()
            .iter()
            .filter(|t| {
                t.category == category
                    && match category {
                        Category::Still => t.tag == still_tag,
                        Category::Change => t.tag == "any" || t.tag == change_tag,
                        _ => true,
                    }
            })
            .collect();
        if choices.is_empty() {
            return save;
        }
        // Headlines not shown lately; once a section has shown them all, its
        // memory starts over (keeping only the last one, so it does not repeat
        // straight away).
        let seen: std::collections::HashSet<u32> =
            prefs.weather_phrase_seen.iter().copied().collect();
        let mut fresh: Vec<_> = choices
            .iter()
            .copied()
            .filter(|t| !seen.contains(&fingerprint(t)))
            .collect();
        if fresh.is_empty() {
            let last = self
                .current
                .as_ref()
                .map(|p| (p.title.clone(), p.subtitle.clone()));
            prefs
                .weather_phrase_seen
                .retain(|seen| !choices.iter().any(|t| fingerprint(t) == *seen));
            fresh = choices
                .iter()
                .copied()
                .filter(|t| {
                    choices.len() == 1
                        || last.as_ref().is_none_or(|(title, subtitle)| {
                            &t.title != title || &t.subtitle != subtitle
                        })
                })
                .collect();
        }
        // Pseudorandom across category and elapsed time, but deterministic until the next rotation.
        let seed = (now as u64).rotate_left(17)
            ^ ((reading.temperature.to_bits() >> 16).wrapping_mul(0x9e3779b97f4a7c15))
            ^ (category as u64).wrapping_mul(0xbf58476d1ce4e5b9);
        let seed = seed ^ (seed >> 29).wrapping_mul(0x94d049bb133111eb);
        let index = (seed as usize) % fresh.len();
        let selected = fresh[index];
        prefs.weather_phrase_seen.push(fingerprint(selected));
        if prefs.weather_phrase_seen.len() > SEEN_LIMIT {
            let extra = prefs.weather_phrase_seen.len() - SEEN_LIMIT;
            prefs.weather_phrase_seen.drain(..extra);
        }
        save = true;
        let unit = prefs.temperature_unit;
        let delta = match unit {
            crate::weather::Unit::Celsius => self.delta.abs(),
            crate::weather::Unit::Fahrenheit => self.delta.abs() * 9. / 5.,
        };
        let fill = |text: &str| {
            text.replace("{N}", &run.to_string())
                .replace("{old}", &self.old_label)
                .replace("{new}", &self.new_label)
                .replace("{delta}", &format!("{delta:.0}"))
                .replace("{temp}", &degrees(reading.temperature, unit))
                .replace("{time}", &format!("{hour:02}:00"))
                .replace(
                    "{years}",
                    &match ours.map_or(0, |(_, years)| years) {
                        1 => "1 year".to_owned(),
                        years => format!("{years} years"),
                    },
                )
                .replace(
                    "{soon}",
                    &forecast.map_or_else(String::new, |(_, at)| format!("{at:02}:00")),
                )
        };
        let title = fill(&selected.title);
        let subtitle = fill(&selected.subtitle);
        self.previous = self.current.replace(Phrase {
            category,
            title,
            subtitle,
        });
        self.changed_at = Some(Instant::now());
        self.chosen_category = Some(category);
        self.chosen_bucket = bucket;
        save
    }
}

/// A temperature as shown in headlines ("21°"), in the unit set in Luma.
fn degrees(celsius: f64, unit: crate::weather::Unit) -> String {
    match unit {
        crate::weather::Unit::Celsius => format!("{celsius:.0}°"),
        crate::weather::Unit::Fahrenheit => format!("{:.0}°", celsius * 9. / 5. + 32.),
    }
}

fn wet(code: u8) -> bool {
    matches!(
        code,
        51 | 53 | 55 | 56 | 57 | 61 | 63 | 65 | 66 | 67 | 80 | 81 | 82 | 95 | 96 | 99
    )
}

/// Rain due in the next three hours while it is dry, or the end of rain
/// that is falling now, with the hour it happens.
fn forecast_case(reading: Reading, base: Category) -> Option<(Category, u8)> {
    let mut next = reading.hours.iter().take(3).flatten();
    if matches!(base, Category::Rain | Category::Thunder) {
        next.find(|h| !wet(h.code) && !matches!(h.code, 71..=77 | 85 | 86))
            .map(|h| (Category::Clearing, h.hour))
    } else if base != Category::Snow && !wet(reading.code) {
        next.find(|h| wet(h.code))
            .map(|h| (Category::RainSoon, h.hour))
    } else {
        None
    }
}

/// A holiday or a day worth a line (US calendar for Thanksgiving and the
/// Fourth of July).
fn holiday(date: time::Date, hour: u8) -> Option<Category> {
    use time::{Month, Weekday};
    Some(match (date.month(), date.day()) {
        (Month::January, 1) => Category::NewYear,
        (Month::December, 31) if hour >= 17 => Category::NewYearsEve,
        (Month::February, 14) => Category::Valentine,
        (Month::February, 29) => Category::LeapDay,
        (Month::March, 14) => Category::PiDay,
        (Month::April, 1) => Category::AprilFools,
        (Month::May, 4) => Category::StarWars,
        (Month::July, 4) => Category::Independence,
        (Month::October, 31) => Category::Halloween,
        (Month::December, 24 | 25) => Category::Christmas,
        // The fourth Thursday of November.
        (Month::November, 22..=28) if date.weekday() == Weekday::Thursday => Category::Thanksgiving,
        (_, 13) if date.weekday() == Weekday::Friday => Category::Friday13,
        // The 256th day of the year.
        _ if date.ordinal() == 256 => Category::ProgrammersDay,
        _ => return None,
    })
}

/// Our own days: the first kiss (2023-10-16), the day we started dating
/// (2024-03-27) and the proposal (2024-12-31), with the years since.
fn couple_day(date: time::Date) -> Option<(Category, i32)> {
    use time::Month;
    let (category, since) = match (date.month(), date.day()) {
        (Month::October, 16) => (Category::FirstKiss, 2023),
        (Month::March, 27) => (Category::Dating, 2024),
        (Month::December, 31) => (Category::Proposal, 2024),
        _ => return None,
    };
    let years = date.year() - since;
    (years >= 1).then_some((category, years))
}

/// The one hour each week (day and hour) for a line about us: a different
/// day and hour every week, between 10:00 and 21:59.
fn weekly_slot(date: time::Date) -> (time::Date, u8) {
    let monday = date.to_julian_day() - i32::from(date.weekday().number_days_from_monday());
    let mut z = (monday as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    let day = time::Date::from_julian_day(monday + (z % 7) as i32).unwrap_or(date);
    (day, 10 + ((z >> 8) % 12) as u8)
}

/// Monday morning, Friday evening, weekend daytime, the small hours.
fn time_of_week(date: time::Date, hour: u8) -> Option<Category> {
    use time::Weekday;
    Some(match (date.weekday(), hour) {
        (_, 1..=4) => Category::SmallHours,
        (Weekday::Monday, 5..=11) => Category::Monday,
        (Weekday::Friday, 16..=23) => Category::Friday,
        (Weekday::Saturday | Weekday::Sunday, 8..=20) => Category::Weekend,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reading(t: f64, code: u8) -> Reading {
        Reading {
            temperature: t,
            code,
            is_day: true,
            wind_speed: 0.,
            temperature_3h_ago: None,
            hours: [None; 5],
        }
    }
    #[test]
    fn all_approved_phrases_are_loaded() {
        let count = |c| templates().iter().filter(|t| t.category == c).count();
        assert!(templates().len() > 10_000, "{}", templates().len());
        assert!(count(Category::ClearNice) >= 900);
        assert!(count(Category::Still) >= 30);
        assert!(count(Category::Change) >= 15);
        for row in templates() {
            // Only a line about us may stand without a subtitle.
            assert!(!row.title.is_empty());
            assert!(!row.subtitle.is_empty() || row.category == Category::Us);
        }
    }
    #[test]
    fn priority_and_wmo_codes() {
        assert_eq!(weather_category(reading(30., 95), 12), Category::Thunder);
        assert_eq!(weather_category(reading(30., 85), 12), Category::Snow);
        assert_eq!(weather_category(reading(30., 51), 12), Category::Rain);
        assert_eq!(weather_category(reading(20., 45), 12), Category::Fog);
        assert_eq!(
            weather_category(
                Reading {
                    wind_speed: 36.,
                    ..reading(20., 0)
                },
                12
            ),
            Category::Wind
        );
        assert_eq!(weather_category(reading(-1., 0), 12), Category::Frost);
        assert_eq!(weather_category(reading(4., 0), 12), Category::Cold);
        assert_eq!(weather_category(reading(14., 0), 12), Category::Chilly);
        assert_eq!(weather_category(reading(15., 0), 12), Category::ClearNice);
        assert_eq!(weather_category(reading(25., 0), 12), Category::Hot);
        assert_eq!(weather_category(reading(20., 3), 12), Category::Cloudy);
        assert_eq!(weather_category(reading(20., 0), 0), Category::Night);
    }
    #[test]
    fn stability_and_repeats() {
        let mut prefs = Preferences::default();
        let mut engine = Engine::default();
        engine.sync(&mut prefs, Some((reading(12., 0), 100)), 101, 10, 12);
        let first = engine.current.clone().unwrap();
        engine.sync(&mut prefs, Some((reading(12., 0), 100)), 300, 10, 12);
        assert_eq!(engine.current, Some(first));
    }
    #[test]
    fn still_change_and_interpolation() {
        let mut prefs = Preferences::default();
        let mut engine = Engine::default();
        engine.sync(&mut prefs, Some((reading(3., 0), 100)), 100, 10, 12);
        engine.sync(&mut prefs, Some((reading(3., 0), 90000)), 90000, 11, 12);
        assert_eq!(engine.current.as_ref().unwrap().category, Category::Still);
        let changed = Reading {
            temperature_3h_ago: Some(3.),
            ..reading(13., 0)
        };
        engine.sync(&mut prefs, Some((changed, 90010)), 90010, 11, 12);
        assert_eq!(engine.current.as_ref().unwrap().category, Category::Change);
        assert!(!engine.current.as_ref().unwrap().subtitle.contains("{"));
    }
    #[test]
    fn headlines_do_not_repeat_while_remembered() {
        let mut prefs = Preferences::default();
        let mut engine = Engine::default();
        let total = templates()
            .iter()
            .filter(|t| t.category == Category::Fog)
            .count()
            .min(SEEN_LIMIT);
        let mut shown = std::collections::HashSet::new();
        let wednesday = day(2026, time::Month::November, 18);
        for hour in 0..total as i64 {
            let now = 1_000_000 + hour * 3600;
            engine.sync(
                &mut prefs,
                Some((reading(10., 45), now)),
                now,
                wednesday,
                12,
            );
            let phrase = engine.current.clone().unwrap();
            assert!(
                shown.insert((phrase.title, phrase.subtitle)),
                "repeat at {hour}"
            );
        }
        let before = engine.current.clone().unwrap();
        let now = 1_000_000 + total as i64 * 3600;
        engine.sync(
            &mut prefs,
            Some((reading(10., 45), now)),
            now,
            wednesday,
            12,
        );
        assert_ne!(engine.current, Some(before));
        assert!(prefs.weather_phrase_seen.len() <= SEEN_LIMIT);
    }
    fn day(year: i32, month: time::Month, day: u8) -> i32 {
        time::Date::from_calendar_date(year, month, day)
            .unwrap()
            .to_julian_day()
    }
    fn on(year: i32, month: time::Month, d: u8) -> time::Date {
        time::Date::from_calendar_date(year, month, d).unwrap()
    }
    #[test]
    fn every_special_case_and_streak_has_lines() {
        for (label, category) in SPECIAL_LABELS {
            let count = templates()
                .iter()
                .filter(|t| t.category == category)
                .count();
            assert!(count >= 3, "{label}: {count}");
            assert_eq!(Category::from_label(label), Some(category));
        }
        for base in [
            Category::Cold,
            Category::Frost,
            Category::Rain,
            Category::Cloudy,
            Category::Hot,
            Category::Snow,
            Category::Fog,
        ] {
            let tag = base.still_tag().unwrap();
            assert!(
                templates()
                    .iter()
                    .filter(|t| t.category == Category::Still && t.tag == tag)
                    .count()
                    >= 4,
                "{tag}"
            );
        }
        for tag in ["any", "colder", "warmer"] {
            assert!(
                templates()
                    .iter()
                    .any(|t| t.category == Category::Change && t.tag == tag)
            );
        }
        // No headline is left with a placeholder nothing fills.
        for t in templates() {
            let text = format!("{} {}", t.title, t.subtitle)
                .replace("{N}", "")
                .replace("{old}", "")
                .replace("{new}", "")
                .replace("{delta}", "")
                .replace("{temp}", "")
                .replace("{time}", "")
                .replace("{soon}", "")
                .replace("{years}", "");
            assert!(!text.contains('{') && !text.contains('—'), "{text}");
        }
    }
    #[test]
    fn holidays_and_times_of_the_week() {
        use time::Month::*;
        assert_eq!(holiday(on(2027, January, 1), 9), Some(Category::NewYear));
        assert_eq!(holiday(on(2026, December, 31), 12), None);
        assert_eq!(
            holiday(on(2026, December, 31), 20),
            Some(Category::NewYearsEve)
        );
        assert_eq!(
            holiday(on(2027, February, 14), 9),
            Some(Category::Valentine)
        );
        assert_eq!(holiday(on(2028, February, 29), 9), Some(Category::LeapDay));
        assert_eq!(holiday(on(2026, October, 31), 9), Some(Category::Halloween));
        assert_eq!(
            holiday(on(2026, December, 24), 9),
            Some(Category::Christmas)
        );
        // Thanksgiving moves: the fourth Thursday of November.
        assert_eq!(
            holiday(on(2026, November, 26), 9),
            Some(Category::Thanksgiving)
        );
        assert_eq!(
            holiday(on(2027, November, 25), 9),
            Some(Category::Thanksgiving)
        );
        assert_eq!(holiday(on(2026, November, 19), 9), None);
        // Friday the 13th; Programmers' Day is day 256 (13 Sept, 12 in leap years).
        assert_eq!(holiday(on(2026, November, 13), 9), Some(Category::Friday13));
        assert_eq!(
            holiday(on(2026, September, 13), 9),
            Some(Category::ProgrammersDay)
        );
        assert_eq!(
            holiday(on(2028, September, 12), 9),
            Some(Category::ProgrammersDay)
        );
        assert_eq!(holiday(on(2026, November, 18), 9), None);
        assert_eq!(
            time_of_week(on(2026, November, 16), 8),
            Some(Category::Monday)
        );
        assert_eq!(
            time_of_week(on(2026, November, 20), 19),
            Some(Category::Friday)
        );
        assert_eq!(
            time_of_week(on(2026, November, 21), 12),
            Some(Category::Weekend)
        );
        assert_eq!(
            time_of_week(on(2026, November, 18), 3),
            Some(Category::SmallHours)
        );
        assert_eq!(time_of_week(on(2026, November, 18), 12), None);
    }
    #[test]
    fn our_days_and_one_line_a_week() {
        use time::Month::*;
        assert_eq!(
            couple_day(on(2026, October, 16)),
            Some((Category::FirstKiss, 3))
        );
        assert_eq!(couple_day(on(2027, March, 27)), Some((Category::Dating, 3)));
        assert_eq!(
            couple_day(on(2025, December, 31)),
            Some((Category::Proposal, 1))
        );
        // Not before the day itself happened.
        assert_eq!(couple_day(on(2024, December, 31)), None);
        assert_eq!(couple_day(on(2026, November, 18)), None);
        // One slot a week, inside that week, 10:00 to 21:59; weeks differ.
        let mut slots = std::collections::HashSet::new();
        let start = on(2026, November, 16);
        for week in 0..52 {
            let monday = start + time::Duration::weeks(week);
            let (slot, hour) = weekly_slot(monday);
            for d in 0..7 {
                assert_eq!(weekly_slot(monday + time::Duration::days(d)), (slot, hour));
            }
            assert!((0..7).contains(&(slot - monday).whole_days()));
            assert!((10..=21).contains(&hour));
            slots.insert((slot.weekday(), hour));
        }
        assert!(slots.len() > 20, "{}", slots.len());
        // The anniversary fills {years}; on New Year's Eve the proposal and
        // the holiday take turns.
        let mut prefs = Preferences::default();
        let mut engine = Engine::default();
        let eve = on(2026, December, 31).to_julian_day();
        engine.sync(&mut prefs, Some((reading(5., 0), 7200)), 7200, eve, 20);
        let phrase = engine.current.clone().unwrap();
        assert_eq!(phrase.category, Category::Proposal);
        assert!(!format!("{} {}", phrase.title, phrase.subtitle).contains('{'));
        engine.sync(&mut prefs, Some((reading(5., 0), 10800)), 10800, eve, 21);
        assert_eq!(engine.current.unwrap().category, Category::NewYearsEve);
        // A headline can stand alone.
        assert!(
            templates()
                .iter()
                .any(|t| t.category == Category::Us && t.subtitle.is_empty())
        );
    }
    #[test]
    fn forecasts_name_the_hour_in_24_hour_time() {
        let hour = |hour, code| {
            Some(crate::weather::HourForecast {
                hour,
                temperature: 10.,
                code,
                is_day: true,
            })
        };
        let dry = Reading {
            hours: [hour(15, 2), hour(16, 61), hour(17, 61), None, None],
            ..reading(14., 2)
        };
        assert_eq!(
            forecast_case(dry, Category::Cloudy),
            Some((Category::RainSoon, 16))
        );
        let wet = Reading {
            hours: [hour(9, 61), hour(10, 3), None, None, None],
            ..reading(14., 61)
        };
        assert_eq!(
            forecast_case(wet, Category::Rain),
            Some((Category::Clearing, 10))
        );
        assert_eq!(forecast_case(reading(14., 2), Category::Cloudy), None);
        // The engine fills {soon} as "16:00".
        let mut prefs = Preferences::default();
        let mut engine = Engine::default();
        let wednesday = day(2026, time::Month::November, 18);
        engine.sync(&mut prefs, Some((dry, 7200)), 7200, wednesday, 14);
        let phrase = engine.current.unwrap();
        assert_eq!(phrase.category, Category::RainSoon);
        assert!(format!("{} {}", phrase.title, phrase.subtitle).contains("16:00"));
    }
    #[test]
    fn temperatures_follow_the_unit() {
        let mut prefs = Preferences {
            temperature_unit: crate::weather::Unit::Fahrenheit,
            ..Preferences::default()
        };
        let mut engine = Engine::default();
        let wednesday = day(2026, time::Month::November, 18);
        engine.sync(&mut prefs, Some((reading(12., 0), 100)), 100, wednesday, 12);
        let swing = Reading {
            temperature_3h_ago: Some(2.),
            ..reading(12., 0)
        };
        engine.sync(&mut prefs, Some((swing, 200)), 200, wednesday, 12);
        let phrase = engine.current.unwrap();
        assert_eq!(phrase.category, Category::Change);
        let text = format!("{} {}", phrase.title, phrase.subtitle);
        // 2 °C → 36 °F, 12 °C → 54 °F, a 10 °C swing is 18 °F.
        assert!(
            text.contains("36°")
                || text.contains("54°")
                || text.contains("18°")
                || !text.contains('°'),
            "{text}"
        );
        assert!(!text.contains("12°") && !text.contains("10°"), "{text}");
        assert_eq!(degrees(-15., crate::weather::Unit::Fahrenheit), "5°");
    }
    #[test]
    fn no_weather_is_classic_fallback() {
        let mut prefs = Preferences::default();
        let mut engine = Engine::default();
        engine.sync(&mut prefs, None, 10, 1, 12);
        assert!(engine.current.is_none());
    }
}
