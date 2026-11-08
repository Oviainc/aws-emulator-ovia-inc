// SPDX-License-Identifier: CC0-1.0

//! What a message attribute a send supplies has to look like: the name's form, the value's
//! data type and the typed value itself.
//!
//! One place for the whole set of rules, so `SendMessage` and `SendMessageBatch` refuse the
//! same attribute the same way and a batch entry's failure carries the wording a single
//! send's does.

use super::*;

/// Why Amazon SQS will not take `name` as a message attribute name, or `None` when it
/// will.
///
/// The documented rules (Amazon SQS Developer Guide, message attribute components): the
/// name "can contain alphanumeric characters and the underscore (_), hyphen (-), and
/// period (.)", "can contain up to 256 characters", "must not start or end with a
/// period" and "must not have periods in succession", and the `AWS.` and `Amazon.`
/// prefixes are reserved whatever their casing. A name breaking any of them is refused
/// and nothing is enqueued for it.
pub fn message_attribute_name_refusal(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("The request must contain non-empty message attribute names.".to_string());
    }
    if name.len() > MAX_MESSAGE_ATTRIBUTE_NAME_LEN {
        return Some(format!(
            "Message (user) attribute name must be shorter than \
             {MAX_MESSAGE_ATTRIBUTE_NAME_LEN} bytes."
        ));
    }
    let lowered = name.to_lowercase();
    if RESERVED_MESSAGE_ATTRIBUTE_PREFIXES
        .iter()
        .any(|reserved| lowered.starts_with(reserved))
    {
        return Some(
            "You can't use message attribute names beginning with 'AWS.' or 'Amazon'. \
             These strings are reserved for internal use."
                .to_string(),
        );
    }
    if let Some(character) = name
        .chars()
        .find(|character| !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')))
    {
        return Some(format!(
            "Message (user) attribute name contains invalid non-alphanumeric character \
             '#x{:X}'. You can use only alphanumeric characters, hyphens (-), underscores (_), \
             or periods (.).",
            character as u32
        ));
    }
    if name.starts_with('.') {
        return Some(
            "Message (user) attribute name must not begin with character '.'.".to_string(),
        );
    }
    if name.ends_with('.') {
        return Some(
            "Message (user) attribute name must not end with character '.'.".to_string(),
        );
    }
    if name.contains("..") {
        return Some(
            "Message (user) attribute name must not have successive '.' characters.".to_string(),
        );
    }
    None
}

/// Why this message attribute cannot be accepted, or `None` when it can: the name rules
/// above, then the type and value rules below.
pub fn message_attribute_refusal(name: &str, value: &MessageAttributeValue) -> Option<String> {
    message_attribute_name_refusal(name)
        .or_else(|| attribute_value_refusal(name, AttributeParts::from(value)))
}

/// Why Amazon SQS will not take this attribute's `DataType` and value, or `None`.
///
/// The documented rules: the type is one of the logical types in
/// [`MESSAGE_ATTRIBUTE_DATA_TYPES`], optionally with a non-empty custom label; a
/// `String` or `Number` carries its value in `StringValue` and a `Binary` in
/// `BinaryValue` ("for the Number data type, you must use StringValue"), so a value in
/// the other field is not a value of the declared type at all;
/// "Name, type, value and the message body must not be empty or null", so a value
/// carrying no byte is no value either; and a `Number`'s value is a number.
pub(super) fn attribute_value_refusal(name: &str, parts: AttributeParts<'_>) -> Option<String> {
    if let Some(member) = parts.list_member {
        return Some(format!(
            "Message (user) attribute '{name}' carries {member}, which is not implemented \
             and is reserved for future use."
        ));
    }
    let Some(logical_type) = attribute_logical_type(parts.data_type) else {
        return Some(format!(
            "The type of message (user) attribute '{name}' is invalid. You must use only the \
             following supported type prefixes: Binary, Number, String."
        ));
    };
    let binary = logical_type == "Binary";
    // Which FIELD carries the value of that type: "for the Number data type, you must
    // use StringValue", so only `Binary` is carried in `BinaryValue`. A value supplied
    // in the other field is not a value of the type the attribute declares, and one
    // absent from its field or empty in it is no value at all.
    let (field, carried, in_the_other_field) = if binary {
        (
            "Binary",
            parts.binary_value.map(<[u8]>::len),
            parts.string_value.is_some(),
        )
    } else {
        (
            "String",
            parts.string_value.map(str::len),
            parts.binary_value.is_some(),
        )
    };
    if in_the_other_field {
        return Some(format!(
            "Message (user) attribute '{name}' with type '{logical_type}' must use field \
             '{field}'."
        ));
    }
    if carried.is_none_or(|length| length == 0) {
        return Some(format!(
            "Message (user) attribute '{name}' must contain a non-empty value of type \
             '{field}'."
        ));
    }
    if logical_type == "Number" {
        if let Some(value) = parts.string_value {
            return number_value_refusal(name, value);
        }
    }
    None
}

/// Why Amazon SQS will not take `value` as a `Number` attribute's value, or `None`.
///
/// The documented rules for the type (Amazon SQS Developer Guide, message attribute data
/// types): "numbers are positive or negative integers or floating-point numbers", "a
/// number can have up to 38 digits of precision, and it must be within the range
/// -10^128 to 10^126". The value is reported back as the request wrote it, never as the
/// number it names, because that is the text the caller has to fix.
pub(super) fn number_value_refusal(name: &str, value: &str) -> Option<String> {
    let Some(number) = DecimalNumber::parse(value) else {
        return Some(format!(
            "Can't cast the value of message (user) attribute '{name}' to a number."
        ));
    };
    if number.precision_digits() > MAX_NUMBER_PRECISION_DIGITS {
        return Some(format!(
            "Message (user) attribute '{name}' number value [{value}] must contain fewer \
             than {MAX_NUMBER_PRECISION_DIGITS} precision digits."
        ));
    }
    let (low, high) = NUMBER_DECIMAL_POINT_RANGE;
    if number.point < low || number.point > high {
        return Some(format!(
            "Message (user) attribute '{name}' number value [{value}] must be within the \
             range -10^128 to 10^126."
        ));
    }
    None
}

/// Store every `Number` attribute's value in the plain decimal form Amazon SQS keeps it
/// in, so a value written with an exponent, with leading zeros or with insignificant
/// trailing ones comes back as the number it names and both digests cover what the queue
/// actually holds.
///
/// A `Number` attribute carries a NUMBER, not the text of one, so the value a receive
/// reports is that number written out plainly — `1e3` is the number one thousand and
/// `0042.50` is forty-two and a half, and that is what comes back. Every other data type
/// is text or bytes and is kept exactly as sent. Applied after validation, which reports
/// the value as the request wrote it.
pub fn store_message_attributes_canonically(
    attributes: &mut BTreeMap<String, MessageAttributeValue>,
) {
    for value in attributes.values_mut() {
        if attribute_logical_type(&value.data_type) != Some("Number") {
            continue;
        }
        let plain = value
            .string_value
            .as_deref()
            .and_then(DecimalNumber::parse)
            .filter(|number| {
                let (low, high) = NUMBER_DECIMAL_POINT_RANGE;
                number.point >= low && number.point <= high
            })
            .map(DecimalNumber::plain);
        if let Some(plain) = plain {
            value.string_value = Some(plain);
        }
    }
}

/// One `Number` attribute value, taken apart: its sign, its digits in order, and where
/// its decimal point falls among them once the exponent has moved it.
///
/// The digits are MOVED, never parsed into a float, so a value carrying more precision
/// than any float holds keeps every digit it was sent with.
#[derive(Debug, Clone)]
pub(super) struct DecimalNumber {
    /// Whether the value carried a `-` sign.
    pub(super) negative: bool,
    /// Every digit of the significand, in order, leading zeros included.
    pub(super) digits: String,
    /// How many of those digits stand before the decimal point. Zero or negative puts
    /// the point before the first digit.
    pub(super) point: i64,
}

impl DecimalNumber {
    /// `value` taken apart, or `None` when it is not a number of the kind the `Number`
    /// data type carries: "numbers are positive or negative integers or floating-point
    /// numbers", written plainly or with a decimal exponent.
    pub(super) fn parse(value: &str) -> Option<Self> {
        let all_digits =
            |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        let negative = value.starts_with('-');
        let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
        let (significand, exponent) = match unsigned.split_once(['e', 'E']) {
            Some((significand, exponent)) => {
                let unsigned_exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
                if !all_digits(unsigned_exponent) {
                    return None;
                }
                (significand, exponent.parse::<i64>().ok()?)
            }
            None => (unsigned, 0),
        };
        let (whole, fraction) = match significand.split_once('.') {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (significand, None),
        };
        if !all_digits(whole) || !fraction.is_none_or(all_digits) {
            return None;
        }
        Some(Self {
            negative,
            digits: format!("{whole}{}", fraction.unwrap_or_default()),
            point: i64::try_from(whole.len()).ok()?.checked_add(exponent)?,
        })
    }

    /// How many digits of precision the value carries: its digits without the leading
    /// zeros, which stand for no precision.
    pub(super) fn precision_digits(&self) -> usize {
        self.digits.trim_start_matches('0').len()
    }

    /// The value in plain decimal notation, carrying only the digits that are part of the
    /// number: no exponent, no leading `+`, no leading zero before another digit and no
    /// trailing zero after the last significant one in the fraction — `1e3` is `1000`,
    /// `007` is `7`, `1e-3` is `0.001`, `0042.50` is `42.5` and `0.00` is `0`.
    ///
    /// Call it only on a value inside [`NUMBER_DECIMAL_POINT_RANGE`], which is what
    /// bounds how many zeros writing it out plainly takes.
    pub(super) fn plain(self) -> String {
        let digits = &self.digits;
        let length = i64::try_from(digits.len()).unwrap_or(i64::MAX);
        let written = if self.point <= 0 {
            let zeros = "0".repeat(usize::try_from(-self.point).unwrap_or(0));
            format!("0.{zeros}{digits}")
        } else if self.point >= length {
            let zeros = "0".repeat(usize::try_from(self.point - length).unwrap_or(0));
            format!("{digits}{zeros}")
        } else {
            let at = usize::try_from(self.point).unwrap_or(0);
            format!("{}.{}", &digits[..at], &digits[at..])
        };
        // A leading zero only stands before the decimal point: `007` is `7`, `0.5` stays
        // `0.5`.
        let (integral, fraction) = match written.split_once('.') {
            Some((integral, fraction)) => (integral, Some(fraction)),
            None => (written.as_str(), None),
        };
        let trimmed = integral.trim_start_matches('0');
        let integral = if trimmed.is_empty() { "0" } else { trimmed };
        // And a zero in the fraction standing after every significant digit is no part of
        // the number either: it names the same number without it, which is why the
        // trailing zeros of a fraction go the way the leading zeros of the integer part
        // do. A zero in the INTEGER part stays: it carries the magnitude.
        let fraction = fraction
            .map(|fraction| fraction.trim_end_matches('0'))
            .filter(|fraction| !fraction.is_empty());
        // Zero is zero, however many digits and whichever sign it was written with.
        let zero = integral == "0" && fraction.is_none();
        let sign = if self.negative && !zero { "-" } else { "" };
        match fraction {
            Some(fraction) => format!("{sign}{integral}.{fraction}"),
            None => format!("{sign}{integral}"),
        }
    }
}

/// The logical type `data_type` names, or `None` when it names none.
///
/// "You can also append custom labels" — `Number.float` is a `Number` — so the logical
/// type is what stands before the first period, and a label that is present must hold
/// something. The whole type is bounded like an attribute name (Amazon SQS Developer
/// Guide, message attribute components: the type "can be up to 256 characters long"),
/// and holds the same characters a name may.
pub fn attribute_logical_type(data_type: &str) -> Option<&'static str> {
    if data_type.chars().count() > MAX_MESSAGE_ATTRIBUTE_NAME_LEN {
        return None;
    }
    if !data_type
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        return None;
    }
    let (logical, label) = match data_type.split_once('.') {
        Some((logical, label)) => (logical, Some(label)),
        None => (data_type, None),
    };
    if label.is_some_and(|label| label.split('.').any(str::is_empty)) {
        return None;
    }
    MESSAGE_ATTRIBUTE_DATA_TYPES
        .into_iter()
        .find(|supported| *supported == logical)
}

/// Why this message SYSTEM attribute cannot be accepted, or `None` when it can.
///
/// "Currently, the only supported message system attribute is AWSTraceHeader. Its type
/// must be String and its value must be a correctly formatted X-Ray trace header string"
/// (`SendMessageRequest$MessageSystemAttributes`), and the map's own key shape
/// ([`MESSAGE_SYSTEM_ATTRIBUTE_NAME_FOR_SENDS_VALUES`]) publishes that one name — so any
/// other name is refused, and the one name takes a non-empty `StringValue` and nothing
/// else.
pub fn message_system_attribute_refusal(
    name: &str,
    value: &MessageSystemAttributeValue,
) -> Option<String> {
    if !MESSAGE_SYSTEM_ATTRIBUTE_NAME_FOR_SENDS_VALUES.contains(&name) {
        return Some(format!("Message system attribute name '{name}' is invalid."));
    }
    let parts = AttributeParts::from(value);
    let carries_a_string = parts
        .string_value
        .is_some_and(|string_value| !string_value.is_empty());
    if parts.data_type != "String"
        || !carries_a_string
        || parts.binary_value.is_some()
        || parts.list_member.is_some()
    {
        return Some(format!(
            "Message system attribute '{name}' must contain a non-empty value of type 'String'."
        ));
    }
    None
}
