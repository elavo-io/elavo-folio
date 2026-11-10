//! Lens's calculator: arithmetic. Unit conversions are in `units`.
//! `evaluate` returns `None` for anything that is not clearly a calculation,
//! so a plain word or a bare number never shows a calculator result.

pub fn evaluate(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    arithmetic(input)
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
}
