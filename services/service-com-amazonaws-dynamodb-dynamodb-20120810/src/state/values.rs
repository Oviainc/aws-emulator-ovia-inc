// SPDX-License-Identifier: CC0-1.0

//! The documented rules an `AttributeValue` obeys whatever carried it: its type code,
//! its accounted size, its nesting depth, and the canonical form a number is stored in.

use super::*;

/// The `AttributeValue` union member name of a value, for a type-mismatch message.
pub fn attribute_value_type(value: &AttributeValue) -> &'static str {
    match value {
        AttributeValue::S(_) => "S",
        AttributeValue::N(_) => "N",
        AttributeValue::B(_) => "B",
        AttributeValue::Ss(_) => "SS",
        AttributeValue::Ns(_) => "NS",
        AttributeValue::Bs(_) => "BS",
        AttributeValue::M(_) => "M",
        AttributeValue::L(_) => "L",
        AttributeValue::Null(_) => "NULL",
        AttributeValue::Bool(_) => "BOOL",
    }
}

/// The stored size of one attribute value, by the rule the DynamoDB Developer Guide
/// gives under "Item sizes and formats": a string or binary value is its raw byte
/// length, a number is "approximately 1 byte per two significant digits + 1 byte", a
/// boolean or null is 1 byte, and a list or map is "3 bytes of overhead, plus the
/// size of each element or key-value pair" and 1 byte per element.
/// The walk is ITERATIVE, over an explicit stack: nesting in a request is chosen by
/// the caller, so recursing on it would make stack use a function of the request.
/// Sizes accumulate saturatingly, so the arithmetic here cannot overflow either.
pub fn attribute_value_size(value: &AttributeValue) -> i64 {
    let mut total: i64 = 0;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        let own = match value {
            AttributeValue::S(text) => byte_len(text.len()),
            AttributeValue::B(bytes) => byte_len(bytes.len()),
            AttributeValue::N(text) => number_size(text),
            AttributeValue::Bool(_) | AttributeValue::Null(_) => 1,
            AttributeValue::Ss(values) => sum_of(values.iter().map(|text| byte_len(text.len()))),
            AttributeValue::Ns(values) => sum_of(values.iter().map(|text| number_size(text))),
            AttributeValue::Bs(values) => sum_of(values.iter().map(|bytes| byte_len(bytes.len()))),
            AttributeValue::L(values) => {
                // One byte per element, plus the elements themselves, which the
                // stack picks up rather than a recursive call.
                pending.extend(values.iter());
                3i64.saturating_add(byte_len(values.len()))
            }
            AttributeValue::M(entries) => {
                let names = sum_of(
                    entries
                        .keys()
                        .map(|name| byte_len(name.len()).saturating_add(1)),
                );
                pending.extend(entries.values());
                3i64.saturating_add(names)
            }
        };
        total = total.saturating_add(own);
    }
    total
}

/// Whether an attribute value nests deeper than `limit` levels, walked iteratively
/// for the same reason [`attribute_value_size`] is.
pub fn attribute_depth_exceeds(value: &AttributeValue, limit: usize) -> bool {
    let mut pending = vec![(value, 1usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > limit {
            return true;
        }
        match value {
            AttributeValue::L(values) => {
                pending.extend(values.iter().map(|value| (value, depth + 1)));
            }
            AttributeValue::M(entries) => {
                pending.extend(entries.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    false
}

/// The stored size of one item: every attribute's name plus its value.
pub fn item_size(item: &Item) -> i64 {
    sum_of(
        item.iter()
            .map(|(name, value)| byte_len(name.len()).saturating_add(attribute_value_size(value))),
    )
}

/// The `ValidationException` a bad `N` value earns, naming the rule its text breaks.
///
/// Every clause is DynamoDB's own for the documented rule it states — the unparsable
/// text, the precision ceiling and the two magnitude bounds — and each is stated
/// inside the validation envelope ([`validation_message`]), which is where this
/// service states a clause about a value a request carried. Only the numeric-text
/// limit is this emulator's, and its message says so.
pub fn number_rejection(text: &str, fault: NumberFault) -> RequestError {
    match fault {
        NumberFault::NotNumeric => refused(format!(
            "The parameter cannot be converted to a numeric value: {}",
            clipped(text)
        )),
        NumberFault::Precision => refused(format!(
            "Attempting to store more than {MAX_SIGNIFICANT_DIGITS} significant digits in a Number"
        )),
        NumberFault::Overflow => refused(
            "Number overflow. Attempting to store a number with magnitude larger than supported \
             range"
                .to_string(),
        ),
        NumberFault::Underflow => refused(
            "Number underflow. Attempting to store a number with magnitude smaller than \
             supported range"
                .to_string(),
        ),
        NumberFault::TextTooLong => RequestError::Validation(format!(
            "One or more parameter values were invalid: A Number may carry at most \
             {MAX_NUMBER_TEXT_DIGITS} digits of text, this emulator's input-size limit"
        )),
    }
}

/// A request value echoed back in a refusal, bounded.
///
/// The text is the caller's, so a message that embedded all of it would grow with the
/// request; the head of it is what identifies the offending value to a human.
pub(super) fn clipped(text: &str) -> String {
    const KEEP: usize = 48;
    if text.chars().count() <= KEEP {
        return text.to_string();
    }
    let head: String = text.chars().take(KEEP).collect();
    format!("{head}…")
}

// THE DOCUMENTED VALUE RULES, which the canonicalizing walk below
// ([`canonicalize_attribute_value`]) enforces as it goes. None of them is expressible
// in the Smithy model, so the boundary validator passes a request that breaks them:
//
//   * a number carries at most 38 significant digits inside the documented exponent
//     range ("Number: ... can have up to 38 digits of precision" — "Service, Account,
//     and Table Quotas"), wherever it appears: an `N`, a member of an `NS`, or either
//     nested in an `L` or `M`;
//   * a set is not empty ("Set type attributes cannot be empty. Invalid Requests with
//     empty values will be rejected with a ValidationException exception" — the
//     `PutItem` doc);
//   * a set's members are distinct ("the values within a set must be unique" and "No
//     two elements can be equal" — the `AttributeValue$SS`/`NS`/`BS` docs and the
//     Developer Guide's set rules), compared the way the type compares: `NS` members
//     numerically, so `1` and `1.0` are one member.

/// Which set type a value is, for the refusals whose wording differs between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SetType {
    String,
    Number,
    Binary,
}

impl SetType {
    /// The clause DynamoDB refuses an EMPTY set of this type with — including the
    /// double space where the service's template leaves the attribute path empty, and
    /// the different sentence it uses for binary sets.
    pub(super) fn empty_clause(self) -> String {
        match self {
            Self::String => "An string set  may not be empty".to_string(),
            Self::Number => "An number set  may not be empty".to_string(),
            Self::Binary => "Binary sets should not be empty".to_string(),
        }
    }
}

/// Refuse an empty set, inside the same validation envelope ([`validation_message`])
/// every other clause is stated in.
pub(super) fn check_set_not_empty(set_type: SetType, members: usize) -> Result<(), RequestError> {
    if members == 0 {
        return Err(refused(format!(
            "One or more parameter values were invalid: {}",
            set_type.empty_clause()
        )));
    }
    Ok(())
}

/// Refuse a set that lists one member twice.
///
/// The three set types have three wordings, each DynamoDB's own, so the rendered
/// `collection` is the caller's:
///
/// * `SS`: `Input collection ["a", "a"] contains duplicates.`
/// * `NS`: `Input collection contains duplicates.`
/// * `BS`: `Input collection ["YQ==","YQ=="]of type BS contains duplicates.`
///
/// — down to the comma spacing each uses and the missing space after the bracket in
/// the binary one.
pub(super) fn duplicate_set_member(collection: String) -> RequestError {
    refused(format!(
        "One or more parameter values were invalid: Input collection \
         {collection}contains duplicates."
    ))
}

/// The collection as the `SS` message echoes it: quoted members, `, `-separated.
pub(super) fn echoed_strings(values: &[String]) -> String {
    format!(
        "[{}] ",
        joined(
            values.iter().map(|member| format!("\"{}\"", clipped(member))),
            ", "
        )
    )
}

/// The collection as the `BS` message echoes it: base64 members, `,`-separated, and the
/// type named with no space after the bracket.
pub(super) fn echoed_blobs(values: &[Vec<u8>]) -> String {
    format!(
        "[{}]of type BS ",
        joined(
            values.iter().map(|member| format!(
                "\"{}\"",
                clipped(&::aws_smithy_types::base64::encode(member))
            )),
            ","
        )
    )
}

/// Join at most the first few rendered members, eliding the rest.
///
/// The set's length is the caller's choice, so a message that echoed every member
/// would grow with the request.
pub(super) fn joined(members: impl Iterator<Item = String>, separator: &str) -> String {
    const MAX_ECHOED_MEMBERS: usize = 8;
    let mut rendered: Vec<String> = members.take(MAX_ECHOED_MEMBERS + 1).collect();
    if rendered.len() > MAX_ECHOED_MEMBERS {
        rendered.truncate(MAX_ECHOED_MEMBERS);
        rendered.push("…".to_string());
    }
    rendered.join(separator)
}

/// Add up sizes without overflowing.
pub(super) fn sum_of(sizes: impl Iterator<Item = i64>) -> i64 {
    sizes.fold(0i64, i64::saturating_add)
}

pub(super) fn byte_len(len: usize) -> i64 {
    i64::try_from(len).unwrap_or(i64::MAX)
}

/// A number's stored size: 1 byte per two significant digits, plus 1 byte. An
/// unparseable value is sized by its text, which is all there is to store.
pub(super) fn number_size(text: &str) -> i64 {
    match NumberKey::parse(text) {
        Some(number) => number.significant_digits() / 2 + 1,
        None => byte_len(text.len()),
    }
}

/// One `N` value's canonical stored text, or the refusal for a text that denotes no
/// number this store can hold — which names the rule the text broke
/// ([`number_rejection`]), the way the service's own refusals do.
///
/// DynamoDB stores the number, so a write normalises the lexeme ONCE here and every
/// later read reports that form ([`NumberKey::canonical_text`]).
///
/// Crate-visible: the write path reaches it through [`canonicalize_item`], and a
/// handler that needs one value canonicalized (an `UpdateItem` attribute, say) is in
/// this crate too, so nothing outside it needs this.
pub(crate) fn canonical_number_text(text: &str) -> Result<String, RequestError> {
    NumberKey::try_parse(text)
        .map(|number| number.canonical_text())
        .map_err(|fault| number_rejection(text, fault))
}

/// Canonicalize every number inside one attribute value, and hold every set to the
/// documented set rules.
///
/// The rules are about the value TYPE, not about where a value sits, so this reaches
/// the `N` of a scalar, every member of an `NS`, and every number or set nested in an
/// `L` or an `M` at any depth: a number is refused unless it is one the documented
/// precision and exponent range can hold ([`canonical_number_text`]), and a set is
/// refused if it is empty ([`check_set_not_empty`]) or lists one value twice
/// ([`duplicate_set_member`]). Set distinctness is checked AFTER canonicalization, so
/// two spellings of one number are the duplicate member DynamoDB refuses rather than
/// two members of the stored set. The walk is ITERATIVE, over an explicit stack, for
/// the reason [`attribute_value_size`]'s is: how deeply a value nests is the caller's
/// choice, so recursing on it would make stack use a function of the request.
pub(crate) fn canonicalize_attribute_value(
    value: &mut AttributeValue,
) -> Result<(), RequestError> {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            AttributeValue::N(text) => *text = canonical_number_text(text)?,
            AttributeValue::Ns(members) => {
                check_set_not_empty(SetType::Number, members.len())?;
                for member in members.iter_mut() {
                    *member = canonical_number_text(member)?;
                }
                if has_duplicate(members) {
                    // The number-set wording names no members at all.
                    return Err(duplicate_set_member(String::new()));
                }
            }
            AttributeValue::Ss(members) => {
                check_set_not_empty(SetType::String, members.len())?;
                if has_duplicate(members) {
                    return Err(duplicate_set_member(echoed_strings(members)));
                }
            }
            AttributeValue::Bs(members) => {
                check_set_not_empty(SetType::Binary, members.len())?;
                if has_duplicate(members) {
                    return Err(duplicate_set_member(echoed_blobs(members)));
                }
            }
            AttributeValue::L(members) => pending.extend(members.iter_mut()),
            AttributeValue::M(entries) => pending.extend(entries.values_mut()),
            AttributeValue::S(_)
            | AttributeValue::B(_)
            | AttributeValue::Bool(_)
            | AttributeValue::Null(_) => {}
        }
    }
    Ok(())
}

/// Whether a set lists one value twice: a set holds each of its values once, so a
/// request naming one twice is a `ValidationException` rather than a stored duplicate.
///
/// Members are compared as the values they are — numerically for `NS`, because its
/// members are canonicalized first — and the check sorts BORROWED members, so it costs
/// O(n log n) and copies nothing.
pub(super) fn has_duplicate<T: Ord>(members: &[T]) -> bool {
    let mut sorted: Vec<&T> = members.iter().collect();
    sorted.sort_unstable();
    sorted.windows(2).any(|pair| pair[0] == pair[1])
}

/// Hold one attribute value to the documented value rules WITHOUT changing it: the
/// numeric domain (precision, exponent range, numeric text) and the set rules
/// (non-empty, distinct members), at every depth.
///
/// The rules belong to the value's TYPE, so they hold wherever a request carries a
/// value and not only where one is stored: a value an expression binds, a parameter a
/// statement binds and a key a read addresses are all held to them, which is what
/// [`crate::state::check_attribute_value`] does for every one of them. The check runs
/// over a copy because the rules are enforced by the canonicalizing walk
/// ([`canonicalize_attribute_value`]) — one walk states each rule once, and a value
/// this only reads must come out of it unchanged. The copy is bounded: the caller sizes
/// the value against the documented item ceiling first.
pub(crate) fn check_value_rules(value: &AttributeValue) -> Result<(), RequestError> {
    let mut probe = value.clone();
    canonicalize_attribute_value(&mut probe)
}

/// Canonicalize every number in one item and hold its values to the documented rules,
/// which is what a write does before the item lands.
///
/// The attributes are walked in NAME order rather than in the hash map's own order,
/// and the walk STOPS at the first refusal: two identical requests are therefore
/// refused with identical messages, and a refused item is left with the attributes
/// after the failing one untouched. The refusal is passed through as the value rule
/// worded it, because that is how the service words these: its messages name the rule
/// the value broke, not the attribute that carried it.
pub fn canonicalize_item(item: &mut Item) -> Result<(), RequestError> {
    let mut names: Vec<String> = item.keys().cloned().collect();
    names.sort_unstable();
    for name in names {
        let Some(value) = item.get_mut(&name) else {
            continue;
        };
        canonicalize_attribute_value(value)?;
    }
    Ok(())
}
