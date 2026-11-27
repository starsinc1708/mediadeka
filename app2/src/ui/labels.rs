//! Shared Russian declension and date formatting for interface labels.

pub(in crate::ui) fn counted(count: usize, one: &str, few: &str, many: &str) -> String {
    let form = match (count % 100, count % 10) {
        (11..=14, _) => many,
        (_, 1) => one,
        (_, 2..=4) => few,
        _ => many,
    };
    format!("{count} {form}")
}

pub(in crate::ui) fn track_count(count: usize) -> String {
    counted(count, "трек", "трека", "треков")
}

pub(in crate::ui) fn connection_count(count: usize) -> String {
    counted(count, "подключение", "подключения", "подключений")
}

const MONTHS: [&str; 12] = [
    "янв.",
    "февр.",
    "мар.",
    "апр.",
    "мая",
    "июня",
    "июля",
    "авг.",
    "сент.",
    "окт.",
    "нояб.",
    "дек.",
];

pub(in crate::ui) fn month_name(month: u32) -> Option<&'static str> {
    MONTHS
        .get(usize::try_from(month).ok()?.checked_sub(1)?)
        .copied()
}

/// Format an ISO date (`YYYY-MM-DD…`) as «24 нояб. 2026». Other values pass through.
pub(in crate::ui) fn readable_date(value: &str) -> String {
    use chrono::{Datelike, NaiveDate};
    value
        .get(..10)
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .map_or_else(
            || value.to_owned(),
            |date| {
                format!(
                    "{} {} {}",
                    date.day(),
                    MONTHS[date.month0() as usize],
                    date.year()
                )
            },
        )
}

/// Format local calendar fields from SQLite as «24 нояб. 2026, 14:05».
pub(in crate::ui) fn readable_date_time(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
) -> String {
    let month = month_name(month).unwrap_or("");
    format!("{day} {month} {year}, {hour:02}:{minute:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_use_the_shared_russian_months() {
        assert_eq!(readable_date("2026-11-24"), "24 нояб. 2026");
        assert_eq!(readable_date("2026-01-05T12:00"), "5 янв. 2026");
        assert_eq!(readable_date("not a date"), "not a date");
        assert_eq!(
            readable_date_time(2026, 11, 24, 14, 5),
            "24 нояб. 2026, 14:05"
        );
        assert_eq!(readable_date_time(2026, 9, 1, 9, 0), "1 сент. 2026, 09:00");
    }
}
