//! The one list of vendor and product names that must never reach a
//! user-visible string (labels, help, menus, parameter choices, report text;
//! see `CLEAN_ROOM.md`). They may appear in documentation and in a node's
//! `emulates` field only.
//!
//! The registry-wide test in `vernon-cli` scans every node's visible strings,
//! parameter defaults and choice options, and the report text of the cooked
//! demo pipeline against these lists.

/// Names that identify a product on their own: matched anywhere in a text,
/// ignoring case. Lower case.
pub const DISTINCTIVE: &[&str] = &[
    "autodesk",
    "3ds max",
    "3dsmax",
    "maxscript",
    "quicksurface",
    "geomagic",
    "zeiss",
    "airshaper",
    "simscale",
    "ansys",
    "optimumlap",
    "polycam",
    "ladybug",
    "cloudcompare",
    "houdini",
    "sidefx",
    "viewcube",
    "mcneel",
    "solidworks",
    "onshape",
    "revopoint",
    "creality",
    "exactflat",
    "meshmixer",
];

/// Product names that are also ordinary words (a rhino, a fusion of data, a
/// slate, a blender, a squish, plastic deformation). Matched only when
/// written as the product, capitalised, as a whole word.
pub const COMMON_WORDS: &[&str] = &[
    "Rhino",
    "Fusion",
    "Slate",
    "Blender",
    "Squish",
    "Grasshopper",
    "Honeybee",
    "Plasticity",
    "Bambu",
];

/// The first protected name found in `text`, if any.
pub fn find_protected(text: &str) -> Option<&'static str> {
    let low = text.to_lowercase();
    if let Some(n) = DISTINCTIVE.iter().copied().find(|n| low.contains(n)) {
        return Some(n);
    }
    COMMON_WORDS.iter().copied().find(|n| contains_word(text, n))
}

fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_are_found_and_ordinary_words_are_not() {
        assert_eq!(find_protected("Imported from Houdini"), Some("houdini"));
        assert_eq!(find_protected("AUTODESK style"), Some("autodesk"));
        assert_eq!(find_protected("the Rhino wrap"), Some("Rhino"));
        assert_eq!(find_protected("Fusion 360"), Some("Fusion"));
        assert_eq!(find_protected("translate the part"), None);
        assert_eq!(find_protected("sensor fusion and plasticity of the ASA"), None);
        assert_eq!(find_protected("Rhinoceros-free text"), None);
        assert_eq!(find_protected("COLMAP, OpenFOAM, CalculiX"), None);
    }
}
