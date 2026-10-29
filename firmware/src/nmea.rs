//! The two NMEA 0183 sentences the GPS driver needs: RMC (fix, position, speed, time) and GGA
//! (satellites, altitude). Talker ids are ignored, so `$GN` works as well as `$GP`.

const KNOTS_TO_KILOMETERS_PER_HOUR: f32 = 1.852;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sentence {
    Rmc(Rmc),
    Gga(Gga),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub latitude: f32,
    pub longitude: f32,
}

/// Everything is `None` without a fix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rmc {
    pub position: Option<Position>,
    pub speed_kilometers_per_hour: Option<f32>,
    pub unix_time: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gga {
    pub satellites: u8,
    /// Above mean sea level; `None` without a fix.
    pub altitude_meters: Option<f32>,
}

/// `None` for other sentences, a bad checksum or a malformed field.
pub fn parse(line: &str) -> Option<Sentence> {
    let (body, checksum) = line.trim_end().strip_prefix('$')?.split_once('*')?;
    let expected = u8::from_str_radix(checksum, 16).ok()?;
    if body.bytes().fold(0, |sum, byte| sum ^ byte) != expected {
        return None;
    }
    let mut fields = body.split(',');
    let kind = fields.next()?;
    let fields: Vec<&str> = fields.collect();
    match kind.get(2..)? {
        "RMC" => rmc(&fields).map(Sentence::Rmc),
        "GGA" => gga(&fields).map(Sentence::Gga),
        _ => None,
    }
}

fn rmc(fields: &[&str]) -> Option<Rmc> {
    if *fields.get(1)? != "A" {
        return Some(Rmc {
            position: None,
            speed_kilometers_per_hour: None,
            unix_time: None,
        });
    }
    Some(Rmc {
        position: Some(position(fields.get(2..6)?)?),
        speed_kilometers_per_hour: decimal(fields.get(6)?)
            .map(|knots| knots * KNOTS_TO_KILOMETERS_PER_HOUR),
        unix_time: unix_time(fields.get(8)?, fields.first()?),
    })
}

fn gga(fields: &[&str]) -> Option<Gga> {
    let fix = fields.get(5)?.parse::<u8>().unwrap_or(0) > 0;
    Some(Gga {
        satellites: fields.get(6)?.parse().unwrap_or(0),
        altitude_meters: if fix { decimal(fields.get(8)?) } else { None },
    })
}

/// From `ddmm.mmmm,N,dddmm.mmmm,W`.
fn position(fields: &[&str]) -> Option<Position> {
    let [latitude, north_south, longitude, east_west] = fields else {
        return None;
    };
    Some(Position {
        latitude: degrees(latitude, 2)? * sign(north_south)?,
        longitude: degrees(longitude, 3)? * sign(east_west)?,
    })
}

fn degrees(field: &str, degree_digits: usize) -> Option<f32> {
    let degrees: u8 = field.get(..degree_digits)?.parse().ok()?;
    let minutes = decimal(field.get(degree_digits..)?)?;
    Some(f32::from(degrees) + minutes / 60.0)
}

/// `-12.345`; `str::parse::<f32>` would bring about 14 KB of float parsing into the image.
fn decimal(text: &str) -> Option<f32> {
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let mut mantissa: i64 = 0;
    for byte in whole.bytes().chain(fraction.bytes()) {
        if !byte.is_ascii_digit() {
            return None;
        }
        mantissa = mantissa
            .checked_mul(10)?
            .checked_add(i64::from(byte - b'0'))?;
    }
    let value = mantissa as f32 / 10_f32.powi(i32::try_from(fraction.len()).ok()?);
    Some(if negative { -value } else { value })
}

fn sign(hemisphere: &str) -> Option<f32> {
    match hemisphere {
        "N" | "E" => Some(1.0),
        "S" | "W" => Some(-1.0),
        _ => None,
    }
}

/// From the `ddmmyy` date and the `hhmmss.ss` time, both UTC.
fn unix_time(date: &str, time: &str) -> Option<i64> {
    let number = |text: &str, start: usize| text.get(start..start + 2)?.parse::<i64>().ok();
    let (day, month, year) = (number(date, 0)?, number(date, 2)?, 2000 + number(date, 4)?);
    let (hour, minute, second) = (number(time, 0)?, number(time, 2)?, number(time, 4)?);
    Some(days_since_epoch(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Howard Hinnant's `days_from_civil`.
fn days_since_epoch(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
