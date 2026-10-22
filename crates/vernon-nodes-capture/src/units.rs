//! File-unit names and the vehicle-scale size check (spec section 2.2).
//!
//! The check is a Vernon heuristic: for a whole vehicle the longest bounding
//! box edge is 3.0 to 6.0 m, so the file unit that maps the longest edge into
//! that window is a candidate. A candidate is only *confident* when the box
//! also has car-like proportions; part-scale files (a bumper of 450 units)
//! stay ambiguous and are never rescaled automatically.

use glam::DVec3;
use vernon_io::FileUnit;

/// Result of the size check.
#[derive(Debug, Clone, PartialEq)]
pub enum UnitVerdict {
    /// Exactly one unit gives a car-sized, car-shaped box.
    Confident(FileUnit),
    /// The longest edge fits a vehicle in these units but the shape is not
    /// clearly a car, so a part in another unit is just as plausible.
    Ambiguous(Vec<FileUnit>),
    /// Nothing fits the vehicle window (part scale or unusual size).
    NoMatch,
}

pub const ALL_UNITS: [FileUnit; 4] = [
    FileUnit::Millimetre,
    FileUnit::Centimetre,
    FileUnit::Metre,
    FileUnit::Inch,
];

/// Short name used in parameters and messages.
pub fn unit_name(u: FileUnit) -> &'static str {
    match u {
        FileUnit::Millimetre => "mm",
        FileUnit::Centimetre => "cm",
        FileUnit::Metre => "m",
        FileUnit::Inch => "in",
    }
}

/// Parse `mm`, `cm`, `m` or `in`.
pub fn parse_unit(s: &str) -> Option<FileUnit> {
    match s {
        "mm" => Some(FileUnit::Millimetre),
        "cm" => Some(FileUnit::Centimetre),
        "m" => Some(FileUnit::Metre),
        "in" => Some(FileUnit::Inch),
        _ => None,
    }
}

/// Vehicle length window in mm.
pub const VEHICLE_MIN_MM: f64 = 3000.0;
pub const VEHICLE_MAX_MM: f64 = 6000.0;

/// Does a box (edges in file units) have the proportions of a car?
fn car_shaped(size: DVec3) -> bool {
    let mut e = size.to_array();
    e.sort_by(|a, b| b.total_cmp(a));
    if e[0] <= 0.0 {
        return false;
    }
    let w = e[1] / e[0];
    let h = e[2] / e[0];
    (0.30..=0.60).contains(&w) && (0.20..=0.50).contains(&h)
}

/// Classify bounding-box edges given in unknown file units.
pub fn classify_size(size: DVec3) -> UnitVerdict {
    let longest = size.max_element();
    if !(longest.is_finite() && longest > 0.0) {
        return UnitVerdict::NoMatch;
    }
    let candidates: Vec<FileUnit> = ALL_UNITS
        .iter()
        .copied()
        .filter(|u| {
            let mm = longest * u.to_mm();
            (VEHICLE_MIN_MM..=VEHICLE_MAX_MM).contains(&mm)
        })
        .collect();
    match candidates.as_slice() {
        [] => UnitVerdict::NoMatch,
        [one] if car_shaped(size) => UnitVerdict::Confident(*one),
        _ => UnitVerdict::Ambiguous(candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_golden_19_unit_guess() {
        let car = DVec3::new(4480.0, 1810.0, 1290.0);
        assert_eq!(classify_size(car), UnitVerdict::Confident(FileUnit::Millimetre));
        assert_eq!(classify_size(car / 10.0), UnitVerdict::Confident(FileUnit::Centimetre));
        assert_eq!(classify_size(car / 1000.0), UnitVerdict::Confident(FileUnit::Metre));
        assert_eq!(
            classify_size(DVec3::new(176.4, 71.3, 50.8)),
            UnitVerdict::Confident(FileUnit::Inch)
        );
        // A 450-unit bumper: could be a car in cm or a part in mm.
        assert_eq!(
            classify_size(DVec3::new(450.0, 120.0, 80.0)),
            UnitVerdict::Ambiguous(vec![FileUnit::Centimetre])
        );
        assert_eq!(classify_size(DVec3::new(20.0, 10.0, 5.0)), UnitVerdict::NoMatch);
        assert_eq!(classify_size(DVec3::ZERO), UnitVerdict::NoMatch);
    }

    #[test]
    fn unit_names_round_trip() {
        for u in ALL_UNITS {
            assert_eq!(parse_unit(unit_name(u)), Some(u));
        }
        assert_eq!(parse_unit("furlong"), None);
    }
}
