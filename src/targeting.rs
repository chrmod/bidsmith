//! Human-readable geo / language codes ↔ Google Ads constants.
//!
//! Campaigns can declare `languages = ["en"]` / `locations = ["US"]` instead of
//! the opaque `languageConstants/1000` / `geoTargetConstants/2840` strings. This
//! module is the lookup table that translates the two directions.
//!
//! Country geo target constants follow Google's stable convention
//! `geoTargetConstants/(2000 + ISO-3166-1-numeric)` — US (numeric 840) is
//! `geoTargetConstants/2840`, Singapore (702) is `geoTargetConstants/2702`. Only
//! country-level codes are shipped; cities and regions are passed through as raw
//! `geoTargetConstants/NNNN` strings in the same list.

const GEO_CONSTANT_PREFIX: &str = "geoTargetConstants/";
const LANGUAGE_CONSTANT_PREFIX: &str = "languageConstants/";

/// Resolve a `locations` entry to a `geoTargetConstants/NNNN` string.
///
/// Accepts an ISO 3166-1 alpha-2 country code (`"US"`, case-insensitive) or a
/// raw `geoTargetConstants/NNNN` string (for cities / regions). Returns `None`
/// for an unknown code or a malformed raw constant.
pub fn resolve_location(input: &str) -> Option<String> {
    if let Some(rest) = input.strip_prefix(GEO_CONSTANT_PREFIX) {
        return all_ascii_digits(rest).then(|| input.to_string());
    }
    let numeric = country_numeric(input)?;
    Some(format!("{GEO_CONSTANT_PREFIX}{}", 2000 + numeric as u32))
}

/// Resolve a `languages` entry to a `languageConstants/NNNN` string.
///
/// Accepts a short language code (`"en"`, case-insensitive) or a raw
/// `languageConstants/NNNN` string. Returns `None` for an unknown code or a
/// malformed raw constant.
pub fn resolve_language(input: &str) -> Option<String> {
    if let Some(rest) = input.strip_prefix(LANGUAGE_CONSTANT_PREFIX) {
        return all_ascii_digits(rest).then(|| input.to_string());
    }
    let id = language_id(input)?;
    Some(format!("{LANGUAGE_CONSTANT_PREFIX}{id}"))
}

/// Reverse of [`resolve_location`]: a country geo constant back to its alpha-2
/// code (`geoTargetConstants/2840` → `"US"`). Cities / regions return `None`,
/// so callers keep the raw constant.
pub fn location_code(constant: &str) -> Option<&'static str> {
    let id: u32 = constant.strip_prefix(GEO_CONSTANT_PREFIX)?.parse().ok()?;
    if (2001..3000).contains(&id) {
        country_alpha2((id - 2000) as u16)
    } else {
        None
    }
}

/// Reverse of [`resolve_language`]: a language constant back to its code
/// (`languageConstants/1000` → `"en"`). Unknown constants return `None`.
pub fn language_code(constant: &str) -> Option<&'static str> {
    let id: u32 = constant.strip_prefix(LANGUAGE_CONSTANT_PREFIX)?.parse().ok()?;
    language_code_from_id(id)
}

fn all_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn country_numeric(alpha2: &str) -> Option<u16> {
    let up = alpha2.to_ascii_uppercase();
    COUNTRIES.iter().find(|(c, _)| *c == up).map(|(_, n)| *n)
}

fn country_alpha2(numeric: u16) -> Option<&'static str> {
    COUNTRIES.iter().find(|(_, n)| *n == numeric).map(|(c, _)| *c)
}

fn language_id(code: &str) -> Option<u32> {
    Some(match code.to_ascii_lowercase().as_str() {
        "en" => 1000,
        "de" => 1001,
        "fr" => 1002,
        "es" => 1003,
        "it" => 1004,
        "ja" => 1005,
        "da" => 1009,
        "nl" => 1010,
        "fi" => 1011,
        "ko" => 1012,
        "no" | "nb" => 1013,
        "pt" => 1014,
        "sv" => 1015,
        "zh" | "zh-cn" | "zh_cn" | "zh-hans" => 1017,
        "zh-tw" | "zh_tw" | "zh-hant" => 1018,
        "ar" => 1019,
        "bg" => 1020,
        "cs" => 1021,
        "el" => 1022,
        "hi" => 1023,
        "hu" => 1024,
        "id" => 1025,
        "is" => 1026,
        "he" | "iw" => 1027,
        "lv" => 1028,
        "lt" => 1029,
        "pl" => 1030,
        "ru" => 1031,
        _ => return None,
    })
}

fn language_code_from_id(id: u32) -> Option<&'static str> {
    Some(match id {
        1000 => "en",
        1001 => "de",
        1002 => "fr",
        1003 => "es",
        1004 => "it",
        1005 => "ja",
        1009 => "da",
        1010 => "nl",
        1011 => "fi",
        1012 => "ko",
        1013 => "no",
        1014 => "pt",
        1015 => "sv",
        1017 => "zh-CN",
        1018 => "zh-TW",
        1019 => "ar",
        1020 => "bg",
        1021 => "cs",
        1022 => "el",
        1023 => "hi",
        1024 => "hu",
        1025 => "id",
        1026 => "is",
        1027 => "he",
        1028 => "lv",
        1029 => "lt",
        1030 => "pl",
        1031 => "ru",
        _ => return None,
    })
}

/// ISO 3166-1 alpha-2 → numeric. The geo target constant is `2000 + numeric`.
const COUNTRIES: &[(&str, u16)] = &[
    ("AD", 20), ("AE", 784), ("AF", 4), ("AG", 28), ("AI", 660), ("AL", 8),
    ("AM", 51), ("AO", 24), ("AQ", 10), ("AR", 32), ("AS", 16), ("AT", 40),
    ("AU", 36), ("AW", 533), ("AX", 248), ("AZ", 31), ("BA", 70), ("BB", 52),
    ("BD", 50), ("BE", 56), ("BF", 854), ("BG", 100), ("BH", 48), ("BI", 108),
    ("BJ", 204), ("BL", 652), ("BM", 60), ("BN", 96), ("BO", 68), ("BQ", 535),
    ("BR", 76), ("BS", 44), ("BT", 64), ("BV", 74), ("BW", 72), ("BY", 112),
    ("BZ", 84), ("CA", 124), ("CC", 166), ("CD", 180), ("CF", 140), ("CG", 178),
    ("CH", 756), ("CI", 384), ("CK", 184), ("CL", 152), ("CM", 120), ("CN", 156),
    ("CO", 170), ("CR", 188), ("CU", 192), ("CV", 132), ("CW", 531), ("CX", 162),
    ("CY", 196), ("CZ", 203), ("DE", 276), ("DJ", 262), ("DK", 208), ("DM", 212),
    ("DO", 214), ("DZ", 12), ("EC", 218), ("EE", 233), ("EG", 818), ("EH", 732),
    ("ER", 232), ("ES", 724), ("ET", 231), ("FI", 246), ("FJ", 242), ("FK", 238),
    ("FM", 583), ("FO", 234), ("FR", 250), ("GA", 266), ("GB", 826), ("GD", 308),
    ("GE", 268), ("GF", 254), ("GG", 831), ("GH", 288), ("GI", 292), ("GL", 304),
    ("GM", 270), ("GN", 324), ("GP", 312), ("GQ", 226), ("GR", 300), ("GS", 239),
    ("GT", 320), ("GU", 316), ("GW", 624), ("GY", 328), ("HK", 344), ("HM", 334),
    ("HN", 340), ("HR", 191), ("HT", 332), ("HU", 348), ("ID", 360), ("IE", 372),
    ("IL", 376), ("IM", 833), ("IN", 356), ("IO", 86), ("IQ", 368), ("IR", 364),
    ("IS", 352), ("IT", 380), ("JE", 832), ("JM", 388), ("JO", 400), ("JP", 392),
    ("KE", 404), ("KG", 417), ("KH", 116), ("KI", 296), ("KM", 174), ("KN", 659),
    ("KP", 408), ("KR", 410), ("KW", 414), ("KY", 136), ("KZ", 398), ("LA", 418),
    ("LB", 422), ("LC", 662), ("LI", 438), ("LK", 144), ("LR", 430), ("LS", 426),
    ("LT", 440), ("LU", 442), ("LV", 428), ("LY", 434), ("MA", 504), ("MC", 492),
    ("MD", 498), ("ME", 499), ("MF", 663), ("MG", 450), ("MH", 584), ("MK", 807),
    ("ML", 466), ("MM", 104), ("MN", 496), ("MO", 446), ("MP", 580), ("MQ", 474),
    ("MR", 478), ("MS", 500), ("MT", 470), ("MU", 480), ("MV", 462), ("MW", 454),
    ("MX", 484), ("MY", 458), ("MZ", 508), ("NA", 516), ("NC", 540), ("NE", 562),
    ("NF", 574), ("NG", 566), ("NI", 558), ("NL", 528), ("NO", 578), ("NP", 524),
    ("NR", 520), ("NU", 570), ("NZ", 554), ("OM", 512), ("PA", 591), ("PE", 604),
    ("PF", 258), ("PG", 598), ("PH", 608), ("PK", 586), ("PL", 616), ("PM", 666),
    ("PN", 612), ("PR", 630), ("PS", 275), ("PT", 620), ("PW", 585), ("PY", 600),
    ("QA", 634), ("RE", 638), ("RO", 642), ("RS", 688), ("RU", 643), ("RW", 646),
    ("SA", 682), ("SB", 90), ("SC", 690), ("SD", 729), ("SE", 752), ("SG", 702),
    ("SH", 654), ("SI", 705), ("SJ", 744), ("SK", 703), ("SL", 694), ("SM", 674),
    ("SN", 686), ("SO", 706), ("SR", 740), ("SS", 728), ("ST", 678), ("SV", 222),
    ("SX", 534), ("SY", 760), ("SZ", 748), ("TC", 796), ("TD", 148), ("TF", 260),
    ("TG", 768), ("TH", 764), ("TJ", 762), ("TK", 772), ("TL", 626), ("TM", 795),
    ("TN", 788), ("TO", 776), ("TR", 792), ("TT", 780), ("TV", 798), ("TW", 158),
    ("TZ", 834), ("UA", 804), ("UG", 800), ("UM", 581), ("US", 840), ("UY", 858),
    ("UZ", 860), ("VA", 336), ("VC", 670), ("VE", 862), ("VG", 92), ("VI", 850),
    ("VN", 704), ("VU", 548), ("WF", 876), ("WS", 882), ("YE", 887), ("YT", 175),
    ("ZA", 710), ("ZM", 894), ("ZW", 716),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_round_trips_country_code() {
        assert_eq!(resolve_location("US").as_deref(), Some("geoTargetConstants/2840"));
        assert_eq!(resolve_location("us").as_deref(), Some("geoTargetConstants/2840"));
        assert_eq!(resolve_location("SG").as_deref(), Some("geoTargetConstants/2702"));
        assert_eq!(location_code("geoTargetConstants/2840"), Some("US"));
        assert_eq!(location_code("geoTargetConstants/2702"), Some("SG"));
    }

    #[test]
    fn location_passes_through_raw_constant() {
        assert_eq!(
            resolve_location("geoTargetConstants/1023191").as_deref(),
            Some("geoTargetConstants/1023191"),
        );
        // A city constant has no country code, so export keeps it raw.
        assert_eq!(location_code("geoTargetConstants/1023191"), None);
    }

    #[test]
    fn location_rejects_unknown_and_malformed() {
        assert_eq!(resolve_location("XX"), None);
        assert_eq!(resolve_location("geoTargetConstants/abc"), None);
        assert_eq!(resolve_location("geoTargetConstants/"), None);
    }

    #[test]
    fn language_round_trips_code() {
        assert_eq!(resolve_language("en").as_deref(), Some("languageConstants/1000"));
        assert_eq!(resolve_language("PL").as_deref(), Some("languageConstants/1030"));
        assert_eq!(resolve_language("zh-TW").as_deref(), Some("languageConstants/1018"));
        assert_eq!(language_code("languageConstants/1000"), Some("en"));
        assert_eq!(language_code("languageConstants/1030"), Some("pl"));
    }

    #[test]
    fn language_passes_through_raw_constant() {
        assert_eq!(
            resolve_language("languageConstants/1045").as_deref(),
            Some("languageConstants/1045"),
        );
        assert_eq!(language_code("languageConstants/1045"), None);
        assert_eq!(resolve_language("xx"), None);
    }
}
