//! Lens's calculator: arithmetic and "X unit in unit" conversions.
//! `evaluate` returns `None` for anything that is not clearly a calculation,
//! so a plain word or a bare number never shows a calculator result.

pub fn evaluate(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    convert(input).or_else(|| arithmetic(input))
}

// ---------------------------------------------------------------- arithmetic

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Num(f64),
    Ident(String),
    Op(char),
    Open,
    Close,
}

fn tokenize(input: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                tokens.push(Token::Num(text.parse().ok()?));
            }
            'a'..='z' | 'A'..='Z' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect::<String>().to_lowercase();
                // "x" between operands is multiplication: "3 x 4".
                if word == "x" {
                    tokens.push(Token::Op('*'));
                } else {
                    tokens.push(Token::Ident(word));
                }
            }
            '+' | '-' | '*' | '/' | '^' | '%' => {
                tokens.push(Token::Op(c));
                i += 1;
            }
            '×' => {
                tokens.push(Token::Op('*'));
                i += 1;
            }
            '÷' => {
                tokens.push(Token::Op('/'));
                i += 1;
            }
            '(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            _ => return None,
        }
    }
    Some(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        token
    }

    fn expr(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        while let Some(Token::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let rhs = self.term()?;
            value = if op == '+' { value + rhs } else { value - rhs };
        }
        Some(value)
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        while let Some(Token::Op(op @ ('*' | '/' | '%'))) = self.peek().cloned() {
            self.pos += 1;
            let rhs = self.unary()?;
            value = match op {
                '*' => value * rhs,
                '/' => value / rhs,
                _ => value % rhs,
            };
        }
        Some(value)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(Token::Op('-')) => {
                self.pos += 1;
                Some(-self.unary()?)
            }
            Some(Token::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;
        if let Some(Token::Op('^')) = self.peek() {
            self.pos += 1;
            let exponent = self.unary()?;
            return Some(base.powf(exponent));
        }
        Some(base)
    }

    fn atom(&mut self) -> Option<f64> {
        match self.next()? {
            Token::Num(n) => Some(n),
            Token::Open => {
                let value = self.expr()?;
                matches!(self.next()?, Token::Close).then_some(value)
            }
            Token::Ident(name) => match name.as_str() {
                "pi" => Some(std::f64::consts::PI),
                "e" => Some(std::f64::consts::E),
                func => {
                    let f: fn(f64) -> f64 = match func {
                        "sqrt" => f64::sqrt,
                        "sin" => f64::sin,
                        "cos" => f64::cos,
                        "tan" => f64::tan,
                        "ln" => f64::ln,
                        "log" => f64::log10,
                        "abs" => f64::abs,
                        _ => return None,
                    };
                    if self.next()? != Token::Open {
                        return None;
                    }
                    let arg = self.expr()?;
                    matches!(self.next()?, Token::Close).then(|| f(arg))
                }
            },
            _ => None,
        }
    }
}

fn arithmetic(input: &str) -> Option<String> {
    let tokens = tokenize(input)?;
    // A bare number or constant is not a calculation.
    let is_calculation = tokens
        .iter()
        .any(|t| matches!(t, Token::Op(_) | Token::Open));
    if !is_calculation {
        return None;
    }
    let mut parser = Parser { tokens, pos: 0 };
    let value = parser.expr()?;
    if parser.pos != parser.tokens.len() || !value.is_finite() {
        return None;
    }
    Some(format_number(value))
}

pub fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = value.abs();
    if !(1e-6..1e15).contains(&magnitude) {
        return format!("{value:e}");
    }
    let text = format!("{value:.10}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

// ----------------------------------------------------------------- units

#[derive(Clone, Copy, PartialEq, Debug)]
enum Family {
    Length,
    Mass,
    Volume,
    Data,
    Time,
    Temperature,
}

struct Unit {
    names: &'static [&'static str],
    symbol: &'static str,
    family: Family,
    /// Size in the family's base unit. Unused for temperature.
    factor: f64,
}

const UNITS: &[Unit] = &[
    Unit {
        names: &["mm", "millimeter", "millimetre"],
        symbol: "mm",
        family: Family::Length,
        factor: 0.001,
    },
    Unit {
        names: &["cm", "centimeter", "centimetre"],
        symbol: "cm",
        family: Family::Length,
        factor: 0.01,
    },
    Unit {
        names: &["m", "meter", "metre"],
        symbol: "m",
        family: Family::Length,
        factor: 1.0,
    },
    Unit {
        names: &["km", "kilometer", "kilometre"],
        symbol: "km",
        family: Family::Length,
        factor: 1000.0,
    },
    Unit {
        names: &["in", "inch", "inches"],
        symbol: "in",
        family: Family::Length,
        factor: 0.0254,
    },
    Unit {
        names: &["ft", "foot", "feet"],
        symbol: "ft",
        family: Family::Length,
        factor: 0.3048,
    },
    Unit {
        names: &["yd", "yard"],
        symbol: "yd",
        family: Family::Length,
        factor: 0.9144,
    },
    Unit {
        names: &["mi", "mile"],
        symbol: "mi",
        family: Family::Length,
        factor: 1609.344,
    },
    Unit {
        names: &["nmi"],
        symbol: "nmi",
        family: Family::Length,
        factor: 1852.0,
    },
    Unit {
        names: &["mg", "milligram"],
        symbol: "mg",
        family: Family::Mass,
        factor: 1e-6,
    },
    Unit {
        names: &["g", "gram"],
        symbol: "g",
        family: Family::Mass,
        factor: 0.001,
    },
    Unit {
        names: &["kg", "kilogram", "kilo"],
        symbol: "kg",
        family: Family::Mass,
        factor: 1.0,
    },
    Unit {
        names: &["t", "tonne"],
        symbol: "t",
        family: Family::Mass,
        factor: 1000.0,
    },
    Unit {
        names: &["oz", "ounce"],
        symbol: "oz",
        family: Family::Mass,
        factor: 0.028_349_523_125,
    },
    Unit {
        names: &["lb", "lbs", "pound"],
        symbol: "lb",
        family: Family::Mass,
        factor: 0.453_592_37,
    },
    Unit {
        names: &["st", "stone"],
        symbol: "st",
        family: Family::Mass,
        factor: 6.350_293_18,
    },
    Unit {
        names: &["ml", "milliliter", "millilitre"],
        symbol: "mL",
        family: Family::Volume,
        factor: 0.001,
    },
    Unit {
        names: &["cl"],
        symbol: "cL",
        family: Family::Volume,
        factor: 0.01,
    },
    Unit {
        names: &["dl"],
        symbol: "dL",
        family: Family::Volume,
        factor: 0.1,
    },
    Unit {
        names: &["l", "liter", "litre"],
        symbol: "L",
        family: Family::Volume,
        factor: 1.0,
    },
    Unit {
        names: &["tsp", "teaspoon"],
        symbol: "tsp",
        family: Family::Volume,
        factor: 0.004_928_921_593_75,
    },
    Unit {
        names: &["tbsp", "tablespoon"],
        symbol: "tbsp",
        family: Family::Volume,
        factor: 0.014_786_764_781_25,
    },
    Unit {
        names: &["floz"],
        symbol: "fl oz",
        family: Family::Volume,
        factor: 0.029_573_529_562_5,
    },
    Unit {
        names: &["cup"],
        symbol: "cup",
        family: Family::Volume,
        factor: 0.236_588_236_5,
    },
    Unit {
        names: &["pt", "pint"],
        symbol: "pt",
        family: Family::Volume,
        factor: 0.473_176_473,
    },
    Unit {
        names: &["qt", "quart"],
        symbol: "qt",
        family: Family::Volume,
        factor: 0.946_352_946,
    },
    Unit {
        names: &["gal", "gallon"],
        symbol: "gal",
        family: Family::Volume,
        factor: 3.785_411_784,
    },
    Unit {
        names: &["bit"],
        symbol: "bit",
        family: Family::Data,
        factor: 0.125,
    },
    Unit {
        names: &["byte"],
        symbol: "B",
        family: Family::Data,
        factor: 1.0,
    },
    Unit {
        names: &["kb", "kilobyte"],
        symbol: "KB",
        family: Family::Data,
        factor: 1e3,
    },
    Unit {
        names: &["mb", "megabyte"],
        symbol: "MB",
        family: Family::Data,
        factor: 1e6,
    },
    Unit {
        names: &["gb", "gigabyte"],
        symbol: "GB",
        family: Family::Data,
        factor: 1e9,
    },
    Unit {
        names: &["tb", "terabyte"],
        symbol: "TB",
        family: Family::Data,
        factor: 1e12,
    },
    Unit {
        names: &["kib", "kibibyte"],
        symbol: "KiB",
        family: Family::Data,
        factor: 1024.0,
    },
    Unit {
        names: &["mib", "mebibyte"],
        symbol: "MiB",
        family: Family::Data,
        factor: 1_048_576.0,
    },
    Unit {
        names: &["gib", "gibibyte"],
        symbol: "GiB",
        family: Family::Data,
        factor: 1_073_741_824.0,
    },
    Unit {
        names: &["tib", "tebibyte"],
        symbol: "TiB",
        family: Family::Data,
        factor: 1_099_511_627_776.0,
    },
    Unit {
        names: &["ms", "millisecond"],
        symbol: "ms",
        family: Family::Time,
        factor: 0.001,
    },
    Unit {
        names: &["s", "sec", "second"],
        symbol: "s",
        family: Family::Time,
        factor: 1.0,
    },
    Unit {
        names: &["min", "minute"],
        symbol: "min",
        family: Family::Time,
        factor: 60.0,
    },
    Unit {
        names: &["h", "hr", "hour"],
        symbol: "h",
        family: Family::Time,
        factor: 3600.0,
    },
    Unit {
        names: &["day"],
        symbol: "days",
        family: Family::Time,
        factor: 86_400.0,
    },
    Unit {
        names: &["week", "wk"],
        symbol: "weeks",
        family: Family::Time,
        factor: 604_800.0,
    },
    Unit {
        names: &["year", "yr"],
        symbol: "years",
        family: Family::Time,
        factor: 31_557_600.0,
    },
    Unit {
        names: &["c", "celsius"],
        symbol: "°C",
        family: Family::Temperature,
        factor: 0.0,
    },
    Unit {
        names: &["f", "fahrenheit"],
        symbol: "°F",
        family: Family::Temperature,
        factor: 0.0,
    },
    Unit {
        names: &["k", "kelvin"],
        symbol: "K",
        family: Family::Temperature,
        factor: 0.0,
    },
];

fn unit(name: &str) -> Option<&'static Unit> {
    let name = name
        .trim()
        .trim_start_matches('°')
        .to_lowercase()
        .replace(' ', "");
    let find = |n: &str| UNITS.iter().find(|u| u.names.contains(&n));
    find(&name).or_else(|| {
        // Plurals: "meters", "pounds". Not for one- or two-letter symbols.
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

/// Split "5.5km" or "5.5 km" or "-40 f" into (number, unit text).
fn amount(text: &str) -> Option<(f64, &str)> {
    let text = text.trim();
    let split = text
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+'))))
        .map(|(i, _)| i)?;
    let number: f64 = text[..split].trim().parse().ok()?;
    Some((number, text[split..].trim()))
}

fn convert(input: &str) -> Option<String> {
    let lower = input.to_lowercase();
    let (left, right) = [" in ", " to ", " as "]
        .iter()
        .filter_map(|sep| lower.rfind(sep).map(|i| (i, sep.len())))
        .max_by_key(|(i, _)| *i)
        .map(|(i, len)| (&lower[..i], &lower[i + len..]))?;
    let (value, from_name) = amount(left)?;
    let from = unit(from_name)?;
    let to = unit(right)?;
    if from.family != to.family {
        return None;
    }
    let result = if from.family == Family::Temperature {
        from_kelvin(to_kelvin(value, from.symbol), to.symbol)
    } else {
        value * from.factor / to.factor
    };
    if !result.is_finite() {
        return None;
    }
    let rounded = (result * 1e6).round() / 1e6;
    Some(format!("{} {}", format_number(rounded), to.symbol))
}

#[cfg(test)]
mod tests {
    use super::evaluate;

    fn eval(s: &str) -> Option<String> {
        evaluate(s)
    }

    #[test]
    fn precedence_and_parentheses() {
        assert_eq!(eval("2+3*4").as_deref(), Some("14"));
        assert_eq!(eval("12*(3+4)").as_deref(), Some("84"));
        assert_eq!(eval("2^3^2").as_deref(), Some("512"));
        assert_eq!(eval("-2^2").as_deref(), Some("-4"));
        assert_eq!(eval("7 % 3").as_deref(), Some("1"));
        assert_eq!(eval("3 x 4").as_deref(), Some("12"));
        assert_eq!(eval("10 ÷ 4").as_deref(), Some("2.5"));
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(eval("sqrt(16)").as_deref(), Some("4"));
        assert_eq!(eval("2*pi").as_deref(), Some("6.2831853072"));
        assert_eq!(eval("log(1000)").as_deref(), Some("3"));
    }

    #[test]
    fn not_a_calculation() {
        assert_eq!(eval("firefox"), None);
        assert_eq!(eval("42"), None);
        assert_eq!(eval("pi"), None);
        assert_eq!(eval("e"), None);
        assert_eq!(eval("1/0"), None);
        assert_eq!(eval("2+"), None);
        assert_eq!(eval("(2+3"), None);
        assert_eq!(eval("foo(2)"), None);
    }

    #[test]
    fn conversions() {
        assert_eq!(eval("5 km in mi").as_deref(), Some("3.106856 mi"));
        assert_eq!(eval("100 f to c").as_deref(), Some("37.777778 °C"));
        assert_eq!(eval("-40 c in f").as_deref(), Some("-40 °F"));
        assert_eq!(eval("2 lb to kg").as_deref(), Some("0.907185 kg"));
        assert_eq!(eval("1 gal in liters").as_deref(), Some("3.785412 L"));
        assert_eq!(eval("1 gib in mb").as_deref(), Some("1073.741824 MB"));
        assert_eq!(eval("90 min in hours").as_deref(), Some("1.5 h"));
        assert_eq!(eval("6ft in cm").as_deref(), Some("182.88 cm"));
    }

    #[test]
    fn conversion_rejects_mismatches() {
        assert_eq!(eval("5 km in kg"), None);
        assert_eq!(eval("files in folder"), None);
        assert_eq!(eval("5 parsecs in m"), None);
    }
}
