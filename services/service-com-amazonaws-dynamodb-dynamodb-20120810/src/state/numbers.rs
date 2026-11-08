// SPDX-License-Identifier: CC0-1.0

//! The order-preserving encoding of an `N` attribute value: how the textual form one
//! request carries becomes the key the store compares, and the rules a text has to
//! satisfy to be a number this service stores at all.
//!
//! A file of its own beside the item store rather than inside it: this is about ONE
//! value's numeric domain, not about what the store holds. The type itself stays in
//! `state.rs` beside the other key values; only what it DOES lives here.

use super::*;

impl NumberKey {
    /// Parse the textual form of an `N` attribute value, or `None` when it is not a
    /// value DynamoDB stores. Use [`Self::try_parse`] where the reason matters.
    pub fn parse(text: &str) -> Option<Self> {
        Self::try_parse(text).ok()
    }

    /// Parse the textual form of an `N` attribute value, naming the rule a refused
    /// text breaks.
    pub fn try_parse(text: &str) -> Result<Self, NumberFault> {
        let (negative, unsigned) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
            Some(at) => {
                let text = &unsigned[at + 1..];
                // An exponent spelled wider than the integer it is read into denotes a
                // magnitude far outside the documented range whatever the mantissa
                // shifts it by — so the TEXT is no number this service can convert,
                // which is the same fault `parse` reports for an exponent that is not
                // digits at all. Bounded before it is parsed, because `parse::<i32>`
                // reads through any number of leading zeros.
                if text.len() > MAX_EXPONENT_TEXT_CHARS {
                    return Err(NumberFault::NotNumeric);
                }
                (
                    &unsigned[..at],
                    text.parse::<i32>().map_err(|_| NumberFault::NotNumeric)?,
                )
            }
            None => (unsigned, 0),
        };
        let (integral, fractional) = match mantissa.find('.') {
            Some(at) => (&mantissa[..at], &mantissa[at + 1..]),
            None => (mantissa, ""),
        };
        if integral.is_empty() && fractional.is_empty() {
            return Err(NumberFault::NotNumeric);
        }
        if !integral.bytes().chain(fractional.bytes()).all(|b| b.is_ascii_digit()) {
            return Err(NumberFault::NotNumeric);
        }
        // The TEXT is bounded before anything is normalised: the digits a longer one
        // carries cannot change which documented value it denotes, and the parsed
        // value's accounted size is computed from its significant digits, not its
        // text. Both terms come from the request.
        if integral.len().saturating_add(fractional.len()) > MAX_NUMBER_TEXT_DIGITS {
            return Err(NumberFault::TextTooLong);
        }
        let mut digits: Vec<u8> = integral
            .bytes()
            .chain(fractional.bytes())
            .map(|b| b - b'0')
            .collect();
        // The value is 0.<digits> * 10^point before normalisation: the decimal point
        // sits after the integral digits, shifted by the exponent. Both terms come
        // from the request, so every step is checked: an exponent that leaves the
        // documented range is refused as an over- or underflow, whichever direction
        // it left it in.
        let shifted = i32::try_from(integral.len())
            .ok()
            .and_then(|len| len.checked_add(exponent));
        let mut point = match shifted {
            Some(point) => point,
            None if exponent > 0 => return Err(NumberFault::Overflow),
            None => return Err(NumberFault::Underflow),
        };
        let leading_zeros = digits.iter().take_while(|digit| **digit == 0).count();
        digits.drain(..leading_zeros);
        point = match i32::try_from(leading_zeros)
            .ok()
            .and_then(|zeros| point.checked_sub(zeros))
        {
            Some(point) => point,
            None => return Err(NumberFault::Underflow),
        };
        while digits.last() == Some(&0) {
            digits.pop();
        }
        if digits.is_empty() {
            // Every spelling of zero normalises to the same value, sign included.
            return Ok(Self {
                negative: false,
                point: 0,
                digits,
            });
        }
        if digits.len() > MAX_SIGNIFICANT_DIGITS {
            return Err(NumberFault::Precision);
        }
        if point > MAX_POINT {
            return Err(NumberFault::Overflow);
        }
        if point < MIN_POINT {
            return Err(NumberFault::Underflow);
        }
        Ok(Self {
            negative,
            point,
            digits,
        })
    }

    /// Compare magnitudes (both treated as non-negative).
    pub(super) fn magnitude_cmp(&self, other: &Self) -> Ordering {
        match (self.digits.is_empty(), other.digits.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            // A larger decimal exponent means a larger magnitude once both are
            // normalised to 0.<digits>; equal exponents fall back to the digits,
            // whose lexicographic order IS their numeric order.
            (false, false) => self
                .point
                .cmp(&other.point)
                .then_with(|| self.digits.cmp(&other.digits)),
        }
    }

    /// The number of significant digits, which is what an `N` value's stored size is
    /// computed from.
    pub(super) fn significant_digits(&self) -> i64 {
        i64::try_from(self.digits.len()).unwrap_or(i64::MAX)
    }

    /// This number's CANONICAL text — the one form every read of it reports.
    ///
    /// An `N` value travels as a string ("Numbers are sent across the network to
    /// DynamoDB as strings, to maximize compatibility across languages and libraries.
    /// However, DynamoDB treats them as number type attributes" — `AttributeValue$N`),
    /// so what is stored is the NUMBER and what a read reports is that number rendered
    /// once, not the lexeme a writer happened to send. It is also the form a value
    /// COMPUTED by the service carries (an `ADD`ed or `SET`-arithmetic number), so one
    /// value has one spelling however the operands were written. The rendering is plain
    /// decimal over the normalised digits — never an exponent, no sign on zero, and a
    /// leading or trailing zero only where the decimal point needs one — so every
    /// spelling of one value reads back identically.
    pub fn canonical_text(&self) -> String {
        if self.digits.is_empty() {
            return "0".to_string();
        }
        let digits: String = self
            .digits
            .iter()
            .map(|digit| char::from(b'0' + digit))
            .collect();
        // The value is 0.<digits> * 10^point, so `point` is where the decimal point
        // falls among the digits: left of them all (a leading "0." and the zeros the
        // exponent asks for), right of them all (trailing zeros), or between two.
        let magnitude = match usize::try_from(self.point) {
            Err(_) | Ok(0) => {
                let zeros = usize::try_from(self.point.unsigned_abs()).unwrap_or(0);
                format!("0.{}{digits}", "0".repeat(zeros))
            }
            Ok(at) if at >= self.digits.len() => {
                format!("{digits}{}", "0".repeat(at - self.digits.len()))
            }
            Ok(at) => format!("{}.{}", &digits[..at], &digits[at..]),
        };
        if self.negative {
            format!("-{magnitude}")
        } else {
            magnitude
        }
    }
}
