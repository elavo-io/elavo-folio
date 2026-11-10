//! Lens's unit conversions: "5 km in miles", "72 f to c", "10 lb kg",
//! "60 mph to km/h". Length, mass, temperature, volume, area, speed, data,
//! and time, all worked out offline.
//!
//! Currency is not here. Rates come from the network, and Lens does not
//! make network requests until there is a privacy decision about it (see
//! lens.md), so "100 usd in eur" is not a conversion.

use Family::*;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Family {
    Length,
    Mass,
    Temperature,
    Volume,
    Area,
    Speed,
    Data,
    Time,
}

struct Unit {
    /// What a person may type, lowercase, without spaces.
    names: &'static [&'static str],
    /// How the answer writes it.
    symbol: &'static str,
    family: Family,
    /// Size in the family's base unit: metres, kilograms, litres, square
    /// metres, metres per second, bytes, seconds. Unused for temperature.
    factor: f64,
}

const fn unit(
    names: &'static [&'static str],
    symbol: &'static str,
    family: Family,
    factor: f64,
) -> Unit {
    Unit {
        names,
        symbol,
        family,
        factor,
    }
}

/// Data units are explicit: KB, MB, GB, and TB are powers of 1000, and
/// KiB, MiB, GiB, and TiB are powers of 1024.
const UNITS: &[Unit] = &[
    unit(
        &["um", "micrometer", "micrometre", "micron"],
        "µm",
        Length,
        1e-6,
    ),
    unit(&["mm", "millimeter", "millimetre"], "mm", Length, 0.001),
    unit(&["cm", "centimeter", "centimetre"], "cm", Length, 0.01),
    unit(&["m", "meter", "metre"], "m", Length, 1.0),
    unit(&["km", "kilometer", "kilometre"], "km", Length, 1000.0),
    unit(&["in", "inch", "inches"], "in", Length, 0.0254),
    unit(&["ft", "foot", "feet"], "ft", Length, 0.3048),
    unit(&["yd", "yard"], "yd", Length, 0.9144),
    unit(&["mi", "mile"], "mi", Length, 1609.344),
    unit(&["nmi", "nauticalmile"], "nmi", Length, 1852.0),
    unit(&["mg", "milligram"], "mg", Mass, 1e-6),
    unit(&["g", "gram"], "g", Mass, 0.001),
    unit(&["kg", "kilogram", "kilo"], "kg", Mass, 1.0),
    unit(&["t", "tonne", "metricton"], "t", Mass, 1000.0),
    unit(&["oz", "ounce"], "oz", Mass, 0.028_349_523_125),
    unit(&["lb", "lbs", "pound"], "lb", Mass, 0.453_592_37),
    unit(&["st", "stone"], "st", Mass, 6.350_293_18),
    unit(&["c", "celsius", "degc"], "°C", Temperature, 0.0),
    unit(&["f", "fahrenheit", "degf"], "°F", Temperature, 0.0),
    unit(&["k", "kelvin"], "K", Temperature, 0.0),
    unit(
        &["ml", "milliliter", "millilitre", "cm3", "cc"],
        "mL",
        Volume,
        0.001,
    ),
    unit(&["cl", "centiliter", "centilitre"], "cL", Volume, 0.01),
    unit(&["dl", "deciliter", "decilitre"], "dL", Volume, 0.1),
    unit(&["l", "liter", "litre"], "L", Volume, 1.0),
    unit(&["m3", "cubicmeter", "cubicmetre"], "m³", Volume, 1000.0),
    unit(&["tsp", "teaspoon"], "tsp", Volume, 0.004_928_921_593_75),
    unit(
        &["tbsp", "tablespoon"],
        "tbsp",
        Volume,
        0.014_786_764_781_25,
    ),
    unit(
        &["floz", "fluidounce"],
        "fl oz",
        Volume,
        0.029_573_529_562_5,
    ),
    unit(&["cup"], "cups", Volume, 0.236_588_236_5),
    unit(&["pt", "pint"], "pt", Volume, 0.473_176_473),
    unit(&["qt", "quart"], "qt", Volume, 0.946_352_946),
    unit(&["gal", "gallon"], "gal", Volume, 3.785_411_784),
    unit(
        &["mm2", "sqmm", "squaremillimeter", "squaremillimetre"],
        "mm²",
        Area,
        1e-6,
    ),
    unit(
        &["cm2", "sqcm", "squarecentimeter", "squarecentimetre"],
        "cm²",
        Area,
        1e-4,
    ),
    unit(
        &["m2", "sqm", "squaremeter", "squaremetre"],
        "m²",
        Area,
        1.0,
    ),
    unit(
        &["km2", "sqkm", "squarekilometer", "squarekilometre"],
        "km²",
        Area,
        1e6,
    ),
    unit(&["ha", "hectare"], "ha", Area, 1e4),
    unit(&["ac", "acre"], "acres", Area, 4_046.856_422_4),
    unit(
        &["in2", "sqin", "squareinch", "squareinches"],
        "in²",
        Area,
        0.000_645_16,
    ),
    unit(
        &["ft2", "sqft", "squarefoot", "squarefeet"],
        "ft²",
        Area,
        0.092_903_04,
    ),
    unit(&["yd2", "sqyd", "squareyard"], "yd²", Area, 0.836_127_36),
    unit(
        &["mi2", "sqmi", "squaremile"],
        "mi²",
        Area,
        2_589_988.110_336,
    ),
    unit(
        &[
            "m/s",
            "mps",
            "meterpersecond",
            "meterspersecond",
            "metrepersecond",
            "metrespersecond",
        ],
        "m/s",
        Speed,
        1.0,
    ),
    unit(
        &[
            "km/h",
            "kmh",
            "kph",
            "kmph",
            "kilometerperhour",
            "kilometersperhour",
            "kilometreperhour",
            "kilometresperhour",
        ],
        "km/h",
        Speed,
        1.0 / 3.6,
    ),
    unit(
        &["mph", "mi/h", "mileperhour", "milesperhour"],
        "mph",
        Speed,
        0.447_04,
    ),
    unit(
        &["ft/s", "fps", "footpersecond", "feetpersecond"],
        "ft/s",
        Speed,
        0.3048,
    ),
    unit(&["kn", "kt", "knot"], "kn", Speed, 1852.0 / 3600.0),
    unit(&["bit"], "bits", Data, 0.125),
    unit(&["b", "byte"], "B", Data, 1.0),
    unit(&["kbit", "kilobit"], "kbit", Data, 125.0),
    unit(&["mbit", "megabit"], "Mbit", Data, 125e3),
    unit(&["gbit", "gigabit"], "Gbit", Data, 125e6),
    unit(&["kb", "kilobyte"], "KB", Data, 1e3),
    unit(&["mb", "megabyte"], "MB", Data, 1e6),
    unit(&["gb", "gigabyte"], "GB", Data, 1e9),
    unit(&["tb", "terabyte"], "TB", Data, 1e12),
    unit(&["pb", "petabyte"], "PB", Data, 1e15),
    unit(&["kib", "kibibyte"], "KiB", Data, 1024.0),
    unit(&["mib", "mebibyte"], "MiB", Data, 1_048_576.0),
    unit(&["gib", "gibibyte"], "GiB", Data, 1_073_741_824.0),
    unit(&["tib", "tebibyte"], "TiB", Data, 1_099_511_627_776.0),
    unit(&["pib", "pebibyte"], "PiB", Data, 1_125_899_906_842_624.0),
    unit(&["us", "microsecond"], "µs", Time, 1e-6),
    unit(&["ms", "millisecond"], "ms", Time, 0.001),
    unit(&["s", "sec", "secs", "second"], "s", Time, 1.0),
    unit(&["min", "mins", "minute"], "min", Time, 60.0),
    unit(&["h", "hr", "hrs", "hour"], "h", Time, 3600.0),
    unit(&["day"], "days", Time, 86_400.0),
    unit(&["wk", "week"], "weeks", Time, 604_800.0),
    // A month is a twelfth of a year of 365.25 days.
    unit(&["mo", "month"], "months", Time, 2_629_800.0),
    unit(&["yr", "year"], "years", Time, 31_557_600.0),
];

/// Words between the amount and the unit wanted. The last one wins, so
/// "5 in to cm" converts inches.
const SEPARATORS: &[&str] = &[" in ", " to ", " as ", " into "];

/// Significant digits in an answer.
const SIGNIFICANT: i32 = 6;

/// A conversion's answer, and what was converted, for the row's subtitle.
#[derive(Debug, PartialEq)]
pub struct Conversion {
    /// "3.10686 mi"
    pub result: String,
    /// "5 km to mi"
    pub expression: String,
}

fn find_unit(name: &str) -> Option<&'static Unit> {
    let name: String = name
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '°')
        .map(|c| match c {
            '²' => '2',
            '³' => '3',
            'µ' | 'μ' => 'u',
            c => c,
        })
        .collect();
    let find = |n: &str| UNITS.iter().find(|u| u.names.contains(&n));
    find(&name).or_else(|| {
        // Plurals: "miles", "cups", "knots". Not for one- or two-letter
        // symbols, so "ms" stays milliseconds.
        name.strip_suffix('s')
            .filter(|n| n.len() > 2)
            .and_then(find)
    })
}

fn to_kelvin(value: f64, symbol: &str) -> f64 {
    match symbol {
        "°C" => value + 273.15,
        "°F" => (value - 32.0) * 5.0 / 9.0 + 273.15,
        _ => value,
    }
}

fn from_kelvin(value: f64, symbol: &str) -> f64 {
    match symbol {
        "°C" => value - 273.15,
        "°F" => (value - 273.15) * 9.0 / 5.0 + 32.0,
        _ => value,
    }
}

/// Split "5.5km", "5.5 km", or "-40 f" into the number and the unit text.
fn amount(text: &str) -> Option<(f64, &str)> {
    let text = text.trim();
    let split = text
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+'))))
        .map(|(i, _)| i)?;
    let number: f64 = text[..split].trim().parse().ok()?;
    Some((number, text[split..].trim()))
}

/// The amount with its unit, and the unit wanted: "5 km in mi", or with no
/// word between them, "10 lb kg".
fn split(input: &str) -> Option<(&str, &str)> {
    let by_word = SEPARATORS
        .iter()
        .filter_map(|sep| input.rfind(sep).map(|i| (i, sep.len())))
        .max_by_key(|(i, _)| *i)
        .map(|(i, len)| (&input[..i], &input[i + len..]));
    by_word.or_else(|| {
        let i = input.rfind(char::is_whitespace)?;
        Some((&input[..i], &input[i..]))
    })
}

pub fn convert(input: &str) -> Option<Conversion> {
    let lower = input.trim().to_lowercase();
    let (left, right) = split(&lower)?;
    let (value, from_name) = amount(left)?;
    let from = find_unit(from_name)?;
    let to = find_unit(right)?;
    if from.family != to.family {
        return None;
    }
    let result = if from.family == Temperature {
        from_kelvin(to_kelvin(value, from.symbol), to.symbol)
    } else {
        value * from.factor / to.factor
    };
    if !result.is_finite() {
        return None;
    }
    Some(Conversion {
        result: format!("{} {}", format_significant(result), to.symbol),
        expression: format!(
            "{} {} to {}",
            format_significant(value),
            from.symbol,
            to.symbol
        ),
    })
}

/// `value` to [`SIGNIFICANT`] digits, without trailing zeros. Whole digits
/// are never dropped, and very large or small values use an exponent.
pub fn format_significant(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs();
    if !(1e-6..1e15).contains(&magnitude) {
        let rounded: f64 = format!("{value:.*e}", (SIGNIFICANT - 1) as usize)
            .parse()
            .unwrap_or(value);
        return format!("{rounded:e}");
    }
    let whole_digits = magnitude.log10().floor() as i32 + 1;
    let decimals = (SIGNIFICANT - whole_digits).max(0) as usize;
    let text = format!("{value:.decimals$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        &text
    };
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(input: &str) -> Option<String> {
        convert(input).map(|c| c.result)
    }

    #[test]
    fn length() {
        assert_eq!(result("5 km in miles").as_deref(), Some("3.10686 mi"));
        assert_eq!(result("5 km in mi").as_deref(), Some("3.10686 mi"));
        assert_eq!(result("6ft in cm").as_deref(), Some("182.88 cm"));
        assert_eq!(result("12 inches to feet").as_deref(), Some("1 ft"));
        assert_eq!(result("5 in to cm").as_deref(), Some("12.7 cm"));
        assert_eq!(result("1 nmi in km").as_deref(), Some("1.852 km"));
    }

    #[test]
    fn mass() {
        assert_eq!(result("10 lb kg").as_deref(), Some("4.53592 kg"));
        assert_eq!(result("2 lb to kg").as_deref(), Some("0.907185 kg"));
        assert_eq!(result("1 kg in oz").as_deref(), Some("35.274 oz"));
        assert_eq!(result("1 stone in pounds").as_deref(), Some("14 lb"));
    }

    #[test]
    fn temperature() {
        assert_eq!(result("72 f to c").as_deref(), Some("22.2222 °C"));
        assert_eq!(result("100 f to c").as_deref(), Some("37.7778 °C"));
        assert_eq!(result("-40 c in f").as_deref(), Some("-40 °F"));
        assert_eq!(result("0 c in k").as_deref(), Some("273.15 K"));
        assert_eq!(result("32 °F to °C").as_deref(), Some("0 °C"));
    }

    #[test]
    fn volume() {
        assert_eq!(result("3 cups to ml").as_deref(), Some("709.765 mL"));
        assert_eq!(result("1 gal in liters").as_deref(), Some("3.78541 L"));
        assert_eq!(result("2 tbsp in tsp").as_deref(), Some("6 tsp"));
        assert_eq!(result("8 fl oz in ml").as_deref(), Some("236.588 mL"));
    }

    #[test]
    fn area() {
        assert_eq!(result("1 acre in m2").as_deref(), Some("4046.86 m²"));
        assert_eq!(result("2 ha in acres").as_deref(), Some("4.94211 acres"));
        assert_eq!(result("1 sq mi to km²").as_deref(), Some("2.58999 km²"));
        assert_eq!(
            result("100 sq ft in square meters").as_deref(),
            Some("9.2903 m²")
        );
    }

    #[test]
    fn speed() {
        assert_eq!(result("60 mph to km/h").as_deref(), Some("96.5606 km/h"));
        assert_eq!(result("100 km/h in mph").as_deref(), Some("62.1371 mph"));
        assert_eq!(result("10 m/s in kph").as_deref(), Some("36 km/h"));
        assert_eq!(result("20 knots in mph").as_deref(), Some("23.0156 mph"));
    }

    #[test]
    fn data_keeps_decimal_and_binary_apart() {
        assert_eq!(result("1 gb in mb").as_deref(), Some("1000 MB"));
        assert_eq!(result("1 gib in mib").as_deref(), Some("1024 MiB"));
        assert_eq!(result("1 gib in gb").as_deref(), Some("1.07374 GB"));
        assert_eq!(result("1 gb in gib").as_deref(), Some("0.931323 GiB"));
        assert_eq!(result("1 tb in bytes").as_deref(), Some("1000000000000 B"));
        assert_eq!(result("100 mbit in mb").as_deref(), Some("12.5 MB"));
        assert_eq!(result("8 bits in bytes").as_deref(), Some("1 B"));
    }

    #[test]
    fn time() {
        assert_eq!(result("90 min in hours").as_deref(), Some("1.5 h"));
        assert_eq!(result("1 day in hours").as_deref(), Some("24 h"));
        assert_eq!(result("2 weeks to days").as_deref(), Some("14 days"));
        assert_eq!(result("1500 ms in s").as_deref(), Some("1.5 s"));
        assert_eq!(result("1 year in days").as_deref(), Some("365.25 days"));
    }

    #[test]
    fn the_subtitle_says_what_was_converted() {
        assert_eq!(
            convert("5 km in miles"),
            Some(Conversion {
                result: "3.10686 mi".into(),
                expression: "5 km to mi".into(),
            })
        );
        assert_eq!(
            convert("72 f to c").map(|c| c.expression).as_deref(),
            Some("72 °F to °C")
        );
    }

    #[test]
    fn not_a_conversion() {
        assert_eq!(result("5 km in kg"), None);
        assert_eq!(result("files in folder"), None);
        assert_eq!(result("5 parsecs in m"), None);
        assert_eq!(result("5 km"), None);
        assert_eq!(result("km in mi"), None);
        assert_eq!(result("report 2024"), None);
        assert_eq!(result(""), None);
    }

    #[test]
    fn currency_needs_the_network_so_is_not_converted() {
        assert_eq!(result("100 usd in eur"), None);
        assert_eq!(result("5 dollars to euros"), None);
        assert_eq!(result("$5 to €"), None);
        assert_eq!(result("20 gbp jpy"), None);
    }

    #[test]
    fn significant_digits() {
        assert_eq!(format_significant(3.106_855_961), "3.10686");
        assert_eq!(format_significant(1_073.741_824), "1073.74");
        assert_eq!(format_significant(1_234_567.89), "1234568");
        assert_eq!(format_significant(0.000_123_456_7), "0.000123457");
        assert_eq!(format_significant(2.0), "2");
        assert_eq!(format_significant(-40.0), "-40");
        assert_eq!(format_significant(-0.000_000_4), "-4e-7");
        assert_eq!(format_significant(1e20), "1e20");
        assert_eq!(format_significant(0.0), "0");
    }
}
