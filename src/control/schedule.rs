use std::fmt;
use std::str::FromStr;

use chrono::{Local, NaiveTime, Timelike};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Daily serving hours in this machine's local time, e.g. 19:00-08:00 (overnight is fine).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServeHours {
    start: u16,
    end: u16,
}

impl ServeHours {
    pub fn contains_now(&self) -> bool {
        let now = Local::now();
        self.contains(now.hour() as u16 * 60 + now.minute() as u16)
    }

    fn contains(&self, minute: u16) -> bool {
        if self.start <= self.end {
            (self.start..self.end).contains(&minute)
        } else {
            minute >= self.start || minute < self.end
        }
    }

    pub fn starts_at(&self) -> String {
        clock(self.start)
    }
}

fn clock(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

impl fmt::Display for ServeHours {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", clock(self.start), clock(self.end))
    }
}

impl FromStr for ServeHours {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        let (start, end) = s.trim().split_once('-').ok_or(())?;
        let minute = |t: &str| {
            NaiveTime::parse_from_str(t.trim(), "%H:%M")
                .map(|t| t.hour() as u16 * 60 + t.minute() as u16)
                .map_err(|_| ())
        };
        let hours = Self {
            start: minute(start)?,
            end: minute(end)?,
        };
        if hours.start == hours.end {
            return Err(());
        }
        Ok(hours)
    }
}

impl Serialize for ServeHours {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ServeHours {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse()
            .map_err(|_| serde::de::Error::custom(format!("invalid serving hours '{raw}'")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u16, m: u16) -> u16 {
        h * 60 + m
    }

    #[test]
    fn overnight_hours_wrap_past_midnight() {
        let hours: ServeHours = "19:00-08:00".parse().unwrap();
        assert!(hours.contains(at(23, 30)));
        assert!(hours.contains(at(3, 0)));
        assert!(hours.contains(at(19, 0)));
        assert!(!hours.contains(at(8, 0)));
        assert!(!hours.contains(at(12, 0)));
        assert_eq!(hours.to_string(), "19:00-08:00");
    }

    #[test]
    fn daytime_hours() {
        let hours: ServeHours = "9:00-17:30".parse().unwrap();
        assert!(hours.contains(at(9, 0)));
        assert!(!hours.contains(at(17, 30)));
        assert_eq!(hours.starts_at(), "09:00");
    }

    #[test]
    fn rejects_nonsense() {
        for bad in ["", "19:00", "25:00-08:00", "19:00-19:00", "7pm-8am"] {
            assert!(bad.parse::<ServeHours>().is_err(), "{bad}");
        }
    }
}
