//! What the launcher works out as it is typed: arithmetic, as `2^10 / 3` or `sqrt(2) * pi`,
//! a percentage of something, as `15% of 80`, and a quantity in other units, as `5 km to mi`
//! or `100 f in c`.

/// A unit: what kind of quantity, and how many of the kind's base unit one of it is. A
/// temperature's scale is instead an offset and a factor from kelvin.
struct Unit {
    kind: &'static str,
    names: &'static [&'static str],
    factor: f64,
    offset: f64,
}

const fn unit(kind: &'static str, names: &'static [&'static str], factor: f64) -> Unit {
    Unit { kind, names, factor, offset: 0.0 }
}

const UNITS: &[Unit] = &[
    unit("length", &["mm", "毫米", "公釐"], 0.001),
    unit("length", &["cm", "公分", "釐米"], 0.01),
    unit("length", &["m", "公尺", "米", "meter", "meters"], 1.0),
    unit("length", &["km", "公里", "kilometer", "kilometers"], 1000.0),
    unit("length", &["in", "inch", "inches", "吋", "英寸"], 0.0254),
    unit("length", &["ft", "foot", "feet", "呎", "英尺"], 0.3048),
    unit("length", &["yd", "yard", "yards", "碼"], 0.9144),
    unit("length", &["mi", "mile", "miles", "英里", "哩"], 1609.344),
    unit("length", &["nmi", "海里"], 1852.0),
    unit("mass", &["mg", "毫克"], 0.001),
    unit("mass", &["g", "公克", "克", "gram", "grams"], 1.0),
    unit("mass", &["kg", "公斤", "千克", "kilogram", "kilograms"], 1000.0),
    unit("mass", &["t", "公噸", "噸", "tonne", "tonnes"], 1_000_000.0),
    unit("mass", &["oz", "ounce", "ounces", "盎司"], 28.349_523_125),
    unit("mass", &["lb", "lbs", "pound", "pounds", "磅"], 453.592_37),
    unit("mass", &["斤", "台斤"], 600.0),
    unit("volume", &["ml", "毫升"], 0.001),
    unit("volume", &["l", "公升", "升", "liter", "liters", "litre"], 1.0),
    unit("volume", &["gal", "gallon", "gallons", "加侖"], 3.785_411_784),
    unit("volume", &["cup", "cups", "杯"], 0.24),
    unit("data", &["b", "byte", "bytes", "位元組"], 1.0),
    unit("data", &["kb"], 1e3),
    unit("data", &["mb"], 1e6),
    unit("data", &["gb"], 1e9),
    unit("data", &["tb"], 1e12),
    unit("data", &["kib"], 1024.0),
    unit("data", &["mib"], 1_048_576.0),
    unit("data", &["gib"], 1_073_741_824.0),
    unit("data", &["tib"], 1_099_511_627_776.0),
    unit("time", &["ms", "毫秒"], 0.001),
    unit("time", &["s", "sec", "secs", "second", "seconds", "秒"], 1.0),
    unit("time", &["min", "mins", "minute", "minutes", "分鐘", "分"], 60.0),
    unit("time", &["h", "hr", "hrs", "hour", "hours", "小時", "時"], 3600.0),
    unit("time", &["d", "day", "days", "天", "日"], 86_400.0),
    unit("time", &["wk", "week", "weeks", "週", "星期"], 604_800.0),
    unit("area", &["m2", "m²", "平方公尺"], 1.0),
    unit("area", &["km2", "km²", "平方公里"], 1e6),
    unit("area", &["ha", "公頃"], 1e4),
    unit("area", &["acre", "acres", "英畝"], 4046.856_422_4),
    unit("area", &["坪"], 3.305_785),
    unit("speed", &["km/h", "kmh", "kph"], 1.0 / 3.6),
    unit("speed", &["m/s"], 1.0),
    unit("speed", &["mph"], 0.447_04),
    unit("speed", &["kn", "knot", "knots", "節"], 0.514_444),
    Unit { kind: "temperature", names: &["c", "°c", "celsius", "攝氏"], factor: 1.0, offset: 273.15 },
    Unit {
        kind: "temperature",
        names: &["f", "°f", "fahrenheit", "華氏"],
        factor: 5.0 / 9.0,
        offset: 459.67 * 5.0 / 9.0,
    },
    Unit { kind: "temperature", names: &["k", "kelvin", "克氏"], factor: 1.0, offset: 0.0 },
];

fn find_unit(name: &str) -> Option<&'static Unit> {
    let name = name.trim().to_lowercase();
    UNITS.iter().find(|unit| unit.names.contains(&name.as_str()))
}

/// What a query works out to, as the answer to show: the result, and how it was read.
/// None when the query is not a calculation, or is only a number.
pub fn answer(query: &str) -> Option<(String, String)> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    if let Some(converted) = convert(query) {
        return Some(converted);
    }
    // `15% of 80`.
    let lower = query.to_lowercase();
    if let Some((percent, of)) = lower.split_once("% of ").or_else(|| lower.split_once("%的")) {
        let value = evaluate(percent)? / 100.0 * evaluate(of)?;
        return value.is_finite().then(|| (format(value), query.to_string()));
    }
    // A number alone is no calculation, nor is a word that happens to be a constant.
    if query.parse::<f64>().is_ok() || tokens(query).is_none_or(|tokens| tokens.len() < 2) {
        return None;
    }
    let value = evaluate(query)?;
    value.is_finite().then(|| (format(value), query.to_string()))
}

/// `5 km to mi`, `5km in mi`, `100 f > c`.
fn convert(query: &str) -> Option<(String, String)> {
    let lower = query.to_lowercase();
    let (from, to) =
        [" to ", " in ", " > ", " 換成 ", "換成", "→"].iter().find_map(|word| lower.split_once(word))?;
    let to = find_unit(to)?;
    let from = from.trim();
    let split = from
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_ascii_digit() || *c == ')' || *c == '.')
        .map(|(index, c)| index + c.len_utf8())?;
    let (amount, unit) = from.split_at(split);
    let unit = find_unit(unit)?;
    if unit.kind != to.kind {
        return None;
    }
    let amount = evaluate(amount)?;
    let base = amount * unit.factor + unit.offset;
    let value = (base - to.offset) / to.factor;
    value.is_finite().then(|| (format!("{} {}", format(value), to.names[0]), query.to_string()))
}

/// A number as people write it: commas between thousands, at most ten significant digits.
pub fn format(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if value.abs() >= 1e15 || value.abs() < 1e-6 {
        return format!("{value:.6e}");
    }
    let digits = (10 - value.abs().log10().floor() as i32 - 1).clamp(0, 10) as usize;
    let text = format!("{value:.digits$}");
    let text =
        if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text };
    let (sign, text) = text.strip_prefix('-').map_or(("", text.as_str()), |rest| ("-", rest));
    let (whole, fraction) =
        text.split_once('.').map_or((text, None), |(whole, fraction)| (whole, Some(fraction)));
    let mut grouped = String::new();
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

/// Arithmetic by precedence: + and - under * / and %, under ^, which binds to the right,
/// with functions, constants, and a number's % as a hundredth of it.
pub fn evaluate(text: &str) -> Option<f64> {
    let tokens = tokens(text)?;
    let mut parser = Parser { tokens, at: 0 };
    let value = parser.sum()?;
    (parser.at == parser.tokens.len()).then_some(value)
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Word(String),
    Op(char),
}

fn tokens(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c == ',' {
            chars.next();
        } else if c.is_ascii_digit() || c == '.' {
            let mut number = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() || c == '.' {
                    number.push(c);
                    chars.next();
                } else if c == ',' {
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(Token::Number(number.parse().ok()?));
        } else if c.is_alphabetic() || c == 'π' {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_alphanumeric() || c == 'π' {
                    word.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(Token::Word(word.to_lowercase()));
        } else {
            let op = match c {
                '×' | '*' => '*',
                '÷' | '/' => '/',
                '−' => '-',
                '+' | '-' | '^' | '%' | '(' | ')' => c,
                _ => return None,
            };
            tokens.push(Token::Op(op));
            chars.next();
        }
    }
    Some(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, op: char) -> bool {
        if self.peek() == Some(&Token::Op(op)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn sum(&mut self) -> Option<f64> {
        let mut value = self.product()?;
        loop {
            if self.eat('+') {
                value += self.product()?;
            } else if self.eat('-') {
                value -= self.product()?;
            } else {
                return Some(value);
            }
        }
    }

    fn product(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        loop {
            if self.eat('*') {
                value *= self.unary()?;
            } else if self.eat('/') {
                value /= self.unary()?;
            } else if self.peek() == Some(&Token::Op('%'))
                && self.tokens.get(self.at + 1).is_some_and(starts_value)
            {
                self.at += 1;
                value %= self.unary()?;
            } else if self.peek().is_some_and(starts_value) {
                // `2pi`, `3(4 + 1)`.
                value *= self.unary()?;
            } else {
                return Some(value);
            }
        }
    }

    /// A sign binds looser than a power: -2^2 is -4.
    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') {
            return Some(-self.unary()?);
        }
        if self.eat('+') {
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.percent()?;
        if self.eat('^') {
            return Some(base.powf(self.unary()?));
        }
        Some(base)
    }

    /// A percentage, when no value follows that it could be the remainder of.
    fn percent(&mut self) -> Option<f64> {
        let mut value = self.atom()?;
        while self.peek() == Some(&Token::Op('%')) && !self.tokens.get(self.at + 1).is_some_and(starts_value)
        {
            self.at += 1;
            value /= 100.0;
        }
        Some(value)
    }

    fn atom(&mut self) -> Option<f64> {
        match self.peek()?.clone() {
            Token::Number(number) => {
                self.at += 1;
                Some(number)
            }
            Token::Op('(') => {
                self.at += 1;
                let value = self.sum()?;
                self.eat(')');
                Some(value)
            }
            Token::Word(word) => {
                self.at += 1;
                match word.as_str() {
                    "pi" | "π" => return Some(std::f64::consts::PI),
                    "e" => return Some(std::f64::consts::E),
                    _ => {}
                }
                let argument = self.unary()?;
                Some(match word.as_str() {
                    "sqrt" => argument.sqrt(),
                    "cbrt" => argument.cbrt(),
                    "abs" => argument.abs(),
                    "round" => argument.round(),
                    "floor" => argument.floor(),
                    "ceil" => argument.ceil(),
                    "ln" => argument.ln(),
                    "log" => argument.log10(),
                    "log2" => argument.log2(),
                    "exp" => argument.exp(),
                    // Degrees, as a calculator on a desk takes them.
                    "sin" => argument.to_radians().sin(),
                    "cos" => argument.to_radians().cos(),
                    "tan" => argument.to_radians().tan(),
                    "asin" => argument.asin().to_degrees(),
                    "acos" => argument.acos().to_degrees(),
                    "atan" => argument.atan().to_degrees(),
                    _ => return None,
                })
            }
            Token::Op(_) => None,
        }
    }
}

fn starts_value(token: &Token) -> bool {
    matches!(token, Token::Number(_) | Token::Word(_) | Token::Op('('))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(query: &str) -> Option<String> {
        answer(query).map(|(result, _)| result)
    }

    #[test]
    fn arithmetic_by_precedence() {
        assert_eq!(result("1 + 2 * 3").as_deref(), Some("7"));
        assert_eq!(result("(1 + 2) * 3").as_deref(), Some("9"));
        assert_eq!(result("2^3^2").as_deref(), Some("512"));
        assert_eq!(result("-2^2").as_deref(), Some("-4"));
        assert_eq!(result("10 / 4").as_deref(), Some("2.5"));
        assert_eq!(result("1000 * 1000").as_deref(), Some("1,000,000"));
        assert_eq!(result("17 % 5").as_deref(), Some("2"));
        assert_eq!(result("50%").as_deref(), Some("0.5"));
        assert_eq!(result("2pi").as_deref(), Some("6.283185307"));
        assert_eq!(result("sqrt(16) + sin 90").as_deref(), Some("5"));
        assert_eq!(result("3 × 4 ÷ 2").as_deref(), Some("6"));
    }

    #[test]
    fn what_is_no_calculation() {
        assert_eq!(result("42"), None);
        assert_eq!(result("firefox"), None);
        assert_eq!(result("1 +"), None);
        assert_eq!(result("1 / 0"), None);
    }

    #[test]
    fn percentages_and_units() {
        assert_eq!(result("15% of 80").as_deref(), Some("12"));
        assert_eq!(result("5 km to mi").as_deref(), Some("3.106855961 mi"));
        assert_eq!(result("100f in c").as_deref(), Some("37.77777778 c"));
        assert_eq!(result("1 gib to mb").as_deref(), Some("1,073.741824 mb"));
        assert_eq!(result("2 h to min").as_deref(), Some("120 min"));
        assert_eq!(result("5 km to kg"), None);
    }
}
