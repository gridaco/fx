//! Money in whole micro-dollars (identity.md §12).
//!
//! Prices and costs are US dollars with at most 6 decimal places; a route price with more is
//! refused when the table is read. Every sum and product is exact integer arithmetic; division
//! (per-thousand-characters prices, fractional units) rounds half to even. Money is written as a
//! JSON number (`micros / 1e6` through [`crate::value::number`]), so `0` and `0.0` are one value.
//! Text shows `$x.xx` (`{:.2}` of the exact decimal, ties to even, as Python's `f"{x:.2f}"`
//! does on gnode's floats for every amount with at most 6 decimals).

use crate::value::{as_f64, format_number};
use serde_json::Value;
use std::cmp::Ordering;
use std::fmt;

/// The most decimal places an amount may have.
const PLACES: i32 = 6;

/// An amount in micro-dollars. Never negative in a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Usd(pub i64);

impl Usd {
    pub const ZERO: Usd = Usd(0);

    /// Reads an amount from a JSON number: refused (with a sentence) when negative, not a number,
    /// or with more than 6 decimal places (decided on the number's JCS text: `0.0000001` is
    /// refused, `0.000001` accepted).
    pub fn from_value(value: &Value) -> Result<Usd, String> {
        match value {
            Value::Number(n) => {
                let x = as_f64(n);
                Usd::from_f64(x, &format_number(x))
            }
            other => Err(format!(
                "{} is not an amount of US dollars",
                crate::text::py_repr(other)
            )),
        }
    }

    /// Reads an amount from a command-line text such as `--max-usd 1.5`: a finite, non-negative
    /// decimal with at most 6 places; `nan`, `inf` and exponents of more places are refused.
    pub fn parse(text: &str) -> Result<Usd, String> {
        let refused = || {
            format!(
                "{} is not an amount of US dollars",
                crate::text::py_repr_str(text)
            )
        };
        if !is_decimal_text(text) {
            return Err(refused());
        }
        let x: f64 = text.parse().map_err(|_| refused())?;
        if !x.is_finite() {
            return Err(refused());
        }
        Usd::from_f64(x, text)
    }

    /// An amount from a double, judged on its JCS text; `shown` is how messages name it.
    fn from_f64(x: f64, shown: &str) -> Result<Usd, String> {
        if !x.is_finite() {
            return Err(format!("{shown} is not an amount of US dollars"));
        }
        if x < 0.0 {
            return Err(format!("{shown} is negative"));
        }
        let text = format_number(x);
        let (mantissa, exponent) =
            decimal_of(&text).ok_or_else(|| format!("{shown} is not an amount of US dollars"))?;
        if exponent < -PLACES {
            return Err(format!("{shown} has more than 6 decimal places"));
        }
        let micros = pow10(exponent + PLACES)
            .and_then(|scale| mantissa.checked_mul(scale))
            .and_then(|micros| i64::try_from(micros).ok())
            .ok_or_else(|| format!("{shown} is too large an amount of US dollars"))?;
        Ok(Usd(micros))
    }

    /// The amount as an FX JSON number.
    pub fn to_value(self) -> Value {
        // A whole number of micro-dollars divided by 1e6 is always finite.
        crate::value::number(self.0 as f64 / 1_000_000.0).unwrap_or_else(|_| Value::from(0))
    }

    /// `$x.xx`, as gnode's `f"${x:.2f}"`.
    pub fn dollars_2(self) -> String {
        format!("${:.2}", self.0 as f64 / 1_000_000.0)
    }

    /// The amount times a whole count, saturating.
    pub fn times(self, count: u64) -> Usd {
        let count = i64::try_from(count).unwrap_or(i64::MAX);
        Usd(self.0.saturating_mul(count))
    }

    /// `self × units / divisor`, rounded half to even (identity.md §12): per-second and
    /// per-thousand-characters prices. A product beyond `i64` saturates.
    pub fn times_units(self, units: Units, divisor: u32) -> Usd {
        if self.0 == 0 || units.mantissa == 0 {
            return Usd::ZERO;
        }
        let Some(numerator) = (self.0 as i128).checked_mul(units.mantissa) else {
            // Only a whole count beyond any real price overflows: the most there is.
            let negative = (self.0 < 0) != (units.mantissa < 0);
            return Usd(if negative { i64::MIN } else { i64::MAX });
        };
        let denominator = 10i128
            .checked_pow(units.scale)
            .and_then(|scale| scale.checked_mul(i128::from(divisor.max(1))));
        // A denominator beyond i128 is more than twice any numerator, so the quotient rounds to 0.
        let quotient = denominator.map_or(0, |d| div_half_even(numerator, d));
        Usd(quotient.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
    }
}

impl std::ops::Add for Usd {
    type Output = Usd;
    /// Saturating: a worst case beyond any real amount stays the largest one.
    fn add(self, other: Usd) -> Usd {
        Usd(self.0.saturating_add(other.0))
    }
}

impl std::iter::Sum for Usd {
    fn sum<I: Iterator<Item = Usd>>(iter: I) -> Usd {
        iter.fold(Usd::ZERO, |a, b| a + b)
    }
}

impl fmt::Display for Usd {
    /// The amount in dollars, in its JCS number form (`0.04`, `6.8625`, `0`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::value::format_number(self.0 as f64 / 1_000_000.0))
    }
}

/// A count of price units (seconds, characters) as an exact decimal `mantissa / 10^scale`: the
/// decimal a person wrote, read from the number's JCS text, so `2.5` seconds is exactly 25/10.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Units {
    pub mantissa: i128,
    pub scale: u32,
}

impl Units {
    /// The units of a finite, non-negative number. A number too large for the exact form
    /// saturates; NaN and the infinities count as none.
    pub fn from_f64(x: f64) -> Units {
        if !x.is_finite() {
            return Units::whole(0);
        }
        let Some((mantissa, exponent)) = decimal_of(&format_number(x)) else {
            return Units::whole(0);
        };
        if exponent >= 0 {
            let mantissa = pow10(exponent)
                .and_then(|scale| mantissa.checked_mul(scale))
                .unwrap_or(if mantissa < 0 { i128::MIN } else { i128::MAX });
            Units { mantissa, scale: 0 }
        } else {
            Units {
                mantissa,
                scale: exponent.unsigned_abs(),
            }
        }
    }

    /// A whole number of units.
    pub fn whole(n: u64) -> Units {
        Units {
            mantissa: n as i128,
            scale: 0,
        }
    }

    /// The smaller of two counts.
    pub fn min(self, other: Units) -> Units {
        if compare(&other, &self) == Ordering::Less {
            other
        } else {
            self
        }
    }
}

/// Compares two exact decimals.
fn compare(a: &Units, b: &Units) -> Ordering {
    match a.scale.cmp(&b.scale) {
        Ordering::Equal => a.mantissa.cmp(&b.mantissa),
        Ordering::Greater => match scale_up(b.mantissa, a.scale - b.scale) {
            Some(scaled) => a.mantissa.cmp(&scaled),
            // `b` scaled overflows, so it is larger in magnitude than anything `a` holds.
            None if b.mantissa > 0 => Ordering::Less,
            None => Ordering::Greater,
        },
        Ordering::Less => compare(b, a).reverse(),
    }
}

/// `m × 10^k`, or `None` when it does not fit.
fn scale_up(m: i128, k: u32) -> Option<i128> {
    10i128.checked_pow(k).and_then(|p| m.checked_mul(p))
}

/// `10^k` for `k ≥ 0`.
fn pow10(k: i32) -> Option<i128> {
    u32::try_from(k).ok().and_then(|k| 10i128.checked_pow(k))
}

/// `n / d` for `d > 0`, rounded half to even.
fn div_half_even(n: i128, d: i128) -> i128 {
    let (quotient, remainder) = (n.div_euclid(d), n.rem_euclid(d));
    // `remainder` is in [0, d); compare it with half of `d` without overflowing.
    let half = d - remainder;
    match remainder.cmp(&half) {
        Ordering::Less => quotient,
        Ordering::Greater => quotient + 1,
        Ordering::Equal if quotient % 2 == 0 => quotient,
        Ordering::Equal => quotient + 1,
    }
}

/// Whether `text` is a plain decimal: an optional sign, digits with an optional point, and an
/// optional exponent. Rust's float parser alone would also take `inf` and `nan`.
fn is_decimal_text(text: &str) -> bool {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (number, exponent) = match body.find(['e', 'E']) {
        Some(at) => (&body[..at], Some(&body[at + 1..])),
        None => (body, None),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok =
        digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    mantissa_ok && exponent_ok
}

/// A decimal text (`12.5`, `-3`, `1e+21`, `1.5e-7`) as `mantissa × 10^exponent`, trailing
/// zeros of the mantissa removed. `None` for anything else or a mantissa beyond `i128`.
fn decimal_of(text: &str) -> Option<(i128, i32)> {
    let (negative, body) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (number, exponent) = match body.find(['e', 'E']) {
        Some(at) => (&body[..at], body[at + 1..].parse::<i32>().ok()?),
        None => (body, 0),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let mut mantissa: i128 = 0;
    for b in whole.bytes().chain(fraction.bytes()) {
        if !b.is_ascii_digit() {
            return None;
        }
        mantissa = mantissa
            .checked_mul(10)?
            .checked_add(i128::from(b - b'0'))?;
    }
    let mut exponent = exponent.checked_sub(i32::try_from(fraction.len()).ok()?)?;
    while mantissa != 0 && mantissa % 10 == 0 {
        mantissa /= 10;
        exponent = exponent.checked_add(1)?;
    }
    if mantissa == 0 {
        exponent = 0;
    }
    Some((if negative { -mantissa } else { mantissa }, exponent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usd(text: &str) -> Usd {
        Usd::parse(text).unwrap()
    }

    #[test]
    fn amounts_are_read_on_their_jcs_text() {
        assert_eq!(Usd::from_value(&json!(0.000001)), Ok(Usd(1)));
        assert_eq!(Usd::from_value(&json!(0.04)), Ok(Usd(40_000)));
        assert_eq!(Usd::from_value(&json!(6.8625)), Ok(Usd(6_862_500)));
        assert_eq!(Usd::from_value(&json!(0)), Ok(Usd(0)));
        assert_eq!(
            Usd::from_value(&json!(1e21)).unwrap_err(),
            "1e+21 is too large an amount of US dollars"
        );
        assert_eq!(Usd::from_value(&json!(12)), Ok(Usd(12_000_000)));
        assert_eq!(
            Usd::from_value(&json!(0.0000001)).unwrap_err(),
            "1e-7 has more than 6 decimal places"
        );
        assert_eq!(
            Usd::from_value(&json!(0.0123456)).unwrap_err(),
            "0.0123456 has more than 6 decimal places"
        );
        assert_eq!(
            Usd::from_value(&json!(0.1 + 0.2)).unwrap_err(),
            "0.30000000000000004 has more than 6 decimal places"
        );
        assert_eq!(Usd::from_value(&json!(-1)).unwrap_err(), "-1 is negative");
        assert!(Usd::from_value(&json!("1")).is_err());
        assert!(Usd::from_value(&json!(true)).is_err());
    }

    #[test]
    fn command_line_amounts() {
        assert_eq!(usd("1.5"), Usd(1_500_000));
        assert_eq!(usd("0"), Usd(0));
        assert_eq!(usd("-0"), Usd(0));
        assert_eq!(usd("+2"), Usd(2_000_000));
        assert_eq!(usd(".25"), Usd(250_000));
        assert_eq!(usd("3."), Usd(3_000_000));
        assert_eq!(usd("1e-3"), Usd(1_000));
        assert_eq!(usd("1.0000000"), Usd(1_000_000));
        assert_eq!(usd("0.000001"), Usd(1));
        for refused in [
            "nan", "NaN", "inf", "-inf", "infinity", "", ".", "1e", "e3", "1,5", " 1", "0x10",
        ] {
            assert!(Usd::parse(refused).is_err(), "{refused:?} was accepted");
        }
        assert_eq!(
            Usd::parse("1e-7").unwrap_err(),
            "1e-7 has more than 6 decimal places"
        );
        assert_eq!(
            Usd::parse("0.0000005").unwrap_err(),
            "0.0000005 has more than 6 decimal places"
        );
        assert_eq!(
            Usd::parse("0.12345650").unwrap_err(),
            "0.12345650 has more than 6 decimal places"
        );
        assert_eq!(Usd::parse("0.1234560"), Ok(Usd(123_456)));
        assert_eq!(Usd::parse("-1.5").unwrap_err(), "-1.5 is negative");
        assert_eq!(
            Usd::parse("nan").unwrap_err(),
            "'nan' is not an amount of US dollars"
        );
    }

    #[test]
    fn values_and_text() {
        assert_eq!(Usd(0).to_value(), json!(0));
        assert_eq!(Usd(1_000_000).to_value(), json!(1));
        assert_eq!(Usd(86_419).to_value(), json!(0.086419));
        assert_eq!(Usd(6_862_500).to_value(), json!(6.8625));
        assert_eq!(Usd(6_862_500).to_string(), "6.8625");
        assert_eq!(Usd(0).dollars_2(), "$0.00");
        assert_eq!(Usd(6_862_500).dollars_2(), "$6.86");
        assert_eq!(Usd(2_490_000).dollars_2(), "$2.49");
        // Ties go to even on the double, as Python's f"{x:.2f}".
        assert_eq!(Usd(125_000).dollars_2(), "$0.12");
        assert_eq!(Usd(375_000).dollars_2(), "$0.38");
        assert_eq!(Usd(5_000).dollars_2(), "$0.01");
        assert_eq!(Usd(15_000).dollars_2(), "$0.01");
        assert_eq!(Usd(2_675_000).dollars_2(), "$2.67");
        assert_eq!(Usd(1_234_567_000_000).dollars_2(), "$1234567.00");
    }

    #[test]
    fn units_are_exact_decimals() {
        assert_eq!(
            Units::from_f64(2.5),
            Units {
                mantissa: 25,
                scale: 1
            }
        );
        assert_eq!(Units::from_f64(3.0), Units::whole(3));
        assert_eq!(Units::from_f64(0.0), Units::whole(0));
        assert_eq!(
            Units::from_f64(1e21),
            Units {
                mantissa: 10i128.pow(21),
                scale: 0
            }
        );
        assert_eq!(
            Units::from_f64(1.5e-7),
            Units {
                mantissa: 15,
                scale: 8
            }
        );
        assert_eq!(
            Units::from_f64(1e300),
            Units {
                mantissa: i128::MAX,
                scale: 0
            }
        );
        assert_eq!(Units::from_f64(f64::NAN), Units::whole(0));
        assert_eq!(
            Units::from_f64(2.5).min(Units::whole(3)),
            Units::from_f64(2.5)
        );
        assert_eq!(
            Units::whole(3).min(Units::from_f64(2.5)),
            Units::from_f64(2.5)
        );
        assert_eq!(Units::whole(10).min(Units::whole(8)), Units::whole(8));
        assert_eq!(
            Units::from_f64(1e-300).min(Units::whole(1)),
            Units::from_f64(1e-300)
        );
        assert_eq!(
            Units::from_f64(1e300).min(Units::from_f64(1e-300)),
            Units::from_f64(1e-300)
        );
    }

    #[test]
    fn per_unit_products_round_half_to_even() {
        // 0.3 per thousand characters, 11 characters: 0.0033.
        assert_eq!(Usd(300_000).times_units(Units::whole(11), 1000), Usd(3_300));
        // 0.0375 per second for 3 seconds.
        assert_eq!(Usd(37_500).times_units(Units::whole(3), 1), Usd(112_500));
        // 2.5 seconds at 0.000001: 2.5 micro-dollars, a tie, rounds to 2.
        assert_eq!(Usd(1).times_units(Units::from_f64(2.5), 1), Usd(2));
        assert_eq!(Usd(1).times_units(Units::from_f64(3.5), 1), Usd(4));
        assert_eq!(Usd(1).times_units(Units::from_f64(3.4), 1), Usd(3));
        assert_eq!(Usd(1).times_units(Units::from_f64(3.6), 1), Usd(4));
        // 0.000001 per thousand: 500 characters are half a micro-dollar, 1500 one and a half.
        assert_eq!(Usd(1).times_units(Units::whole(500), 1000), Usd(0));
        assert_eq!(Usd(1).times_units(Units::whole(1500), 1000), Usd(2));
        assert_eq!(Usd(1).times_units(Units::whole(501), 1000), Usd(1));
        assert_eq!(Usd(7).times_units(Units::whole(0), 1), Usd(0));
        assert_eq!(Usd(7).times_units(Units::from_f64(1e-300), 1), Usd(0));
        assert_eq!(Usd(7).times_units(Units::from_f64(1e300), 1), Usd(i64::MAX));
        assert_eq!(Usd(-3).times_units(Units::from_f64(0.5), 1), Usd(-2));
    }

    #[test]
    fn sums_and_products_saturate() {
        assert_eq!(Usd(40_000).times(7), Usd(280_000));
        assert_eq!(Usd(2).times(u64::MAX), Usd(i64::MAX));
        assert_eq!(Usd(i64::MAX) + Usd(1), Usd(i64::MAX));
        assert_eq!([Usd(1), Usd(2), Usd(3)].into_iter().sum::<Usd>(), Usd(6));
    }

    #[test]
    fn half_even_division() {
        assert_eq!(div_half_even(5, 2), 2);
        assert_eq!(div_half_even(7, 2), 4);
        assert_eq!(div_half_even(-5, 2), -2);
        assert_eq!(div_half_even(-7, 2), -4);
        assert_eq!(div_half_even(-6, 4), -2);
        assert_eq!(div_half_even(i128::MAX, i128::MAX), 1);
        assert_eq!(div_half_even(1, 3), 0);
        assert_eq!(div_half_even(2, 3), 1);
    }

    #[test]
    fn decimal_texts() {
        assert_eq!(decimal_of("12.50"), Some((125, -1)));
        assert_eq!(decimal_of("-3"), Some((-3, 0)));
        assert_eq!(decimal_of("1e+21"), Some((1, 21)));
        assert_eq!(decimal_of("1.5e-7"), Some((15, -8)));
        assert_eq!(decimal_of("0"), Some((0, 0)));
        assert_eq!(decimal_of("0.000"), Some((0, 0)));
        assert_eq!(decimal_of("x"), None);
        assert_eq!(decimal_of("."), None);
    }
}
