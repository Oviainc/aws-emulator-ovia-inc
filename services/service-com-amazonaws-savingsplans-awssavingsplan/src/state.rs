// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-savingsplans-awssavingsplan service.
//!
//! Three things live here, plus the helpers more than one operation needs:
//!
//! * the seeded, read-only **offering catalog** `DescribeSavingsPlansOfferings`
//!   searches and `CreateSavingsPlan` buys from. It is built in `Default`, so
//!   every offering id is stable for the life of the store;
//! * the **Savings Plans** this emulator has sold, in purchase order so paging is
//!   stable, with an id and an ARN index over them;
//! * the **idempotency** record, so a replayed `clientToken` answers with the plan
//!   the first call created instead of creating a second one.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Wire vocabulary
// ---------------------------------------------------------------------------

/// `SavingsPlanState` members this slice produces. The full value set is
/// [`crate::ops::types::SAVINGS_PLAN_STATE_VALUES`].
pub const STATE_PAYMENT_PENDING: &str = "payment-pending";
/// See [`STATE_PAYMENT_PENDING`].
pub const STATE_ACTIVE: &str = "active";
/// See [`STATE_PAYMENT_PENDING`].
pub const STATE_QUEUED: &str = "queued";
/// See [`STATE_PAYMENT_PENDING`].
pub const STATE_QUEUED_DELETED: &str = "queued-deleted";
/// See [`STATE_PAYMENT_PENDING`].
pub const STATE_RETURNED: &str = "returned";
/// See [`STATE_PAYMENT_PENDING`]. The state a plan reaches when its term ends.
pub const STATE_RETIRED: &str = "retired";

/// `SavingsPlanPaymentOption` members (model enum `SavingsPlanPaymentOption`).
pub const PAYMENT_ALL_UPFRONT: &str = "All Upfront";
/// See [`PAYMENT_ALL_UPFRONT`].
pub const PAYMENT_PARTIAL_UPFRONT: &str = "Partial Upfront";
/// See [`PAYMENT_ALL_UPFRONT`].
pub const PAYMENT_NO_UPFRONT: &str = "No Upfront";

/// `SavingsPlanOfferingPropertyKey` members (model enum
/// `SavingsPlanOfferingPropertyKey`): the only two property names an offering
/// carries.
pub const PROPERTY_INSTANCE_FAMILY: &str = "instanceFamily";
/// See [`PROPERTY_INSTANCE_FAMILY`].
pub const PROPERTY_REGION: &str = "region";

/// The ARN namespace and sigv4 signing name of this service, as the model's
/// `aws.api#service` trait gives them.
pub const SERVICE_NAMESPACE: &str = "savingsplans";

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// How long after the purchase call a Savings Plan leaves `payment-pending` for
/// `active`.
///
/// A chosen span, not a documented one: the contract the emulator owes is only
/// that the transition is not instantaneous, so a caller that describes the plan
/// straight after purchasing it still observes `payment-pending`.
pub const ACTIVATION_DELAY_MILLIS: i64 = 500;

/// The return window a purchased Savings Plan has.
///
/// AWS documents it ("Returning a Savings Plan", AWS Savings Plans User Guide:
/// "you can return a Savings Plan that you purchased in the last 7 days (168
/// hours)"), so the value is the service's, not ours, and it is what
/// `SavingsPlan.returnableUntil` reports.
pub const RETURN_WINDOW_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Wall-clock now, in milliseconds since the Unix epoch.
///
/// Total by construction: a clock before 1970, or one too far past it to fit in
/// `i64` milliseconds, answers with the epoch rather than panicking on the
/// request path. Neither is reachable on a host whose clock is set at all, and
/// every instant derived from this value is computed with saturating
/// arithmetic, so the epoch fallback cannot propagate into an overflow.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or_default()
}

/// How many fractional digits a rendered time carries: milliseconds.
pub const TIME_FRACTION_DIGITS: usize = 3;

/// Render epoch milliseconds the way this service renders a time in a `String`
/// member (`start`, `end`, `returnableUntil`): RFC 3339 / ISO 8601 in UTC, at
/// FIXED WIDTH with exactly [`TIME_FRACTION_DIGITS`] fractional digits.
///
/// The width is part of the value. `SavingsPlan.start`/`end`/`returnableUntil`
/// target `String`, not `timestamp`, so the service — not the client's
/// serializer — chooses the spelling, and `aws_smithy_types`' RFC-3339 writer
/// omits the fractional part entirely on a whole second. A variable-width
/// rendering makes a parser configured for millisecond precision fail, and
/// makes two instants of one record sort differently as strings than in time
/// (`...:52Z` sorts AFTER `...:52.100Z`). Fixed width keeps lexicographic and
/// chronological order the same.
pub fn format_millis(millis: i64) -> String {
    let whole_seconds = millis.div_euclid(1000);
    let fraction = millis.rem_euclid(1000);
    let rendered = ::aws_smithy_types::DateTime::from_secs(whole_seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        // A whole-second instant always renders; the fallback keeps this
        // total rather than turning a clock oddity into a 500.
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    match rendered.strip_suffix('Z') {
        Some(seconds) => format!(
            "{seconds}.{fraction:0width$}Z",
            width = TIME_FRACTION_DIGITS
        ),
        None => rendered,
    }
}

/// Read a time a request carries back to epoch milliseconds, so a filter on an
/// instant compares INSTANTS rather than two spellings of one.
///
/// Both ISO-8601 spellings are accepted: the `Z` form this service renders and
/// the explicit-offset form a client may send.
pub fn parse_millis(value: &str) -> Option<i64> {
    use ::aws_smithy_types::date_time::Format;

    [Format::DateTime, Format::DateTimeWithOffset]
        .into_iter()
        .find_map(|format| ::aws_smithy_types::DateTime::from_str(value, format).ok())
        .and_then(|time| time.to_millis().ok())
}

/// Truncate epoch milliseconds to whole seconds: a purchase time is
/// second-granular (`DateTime` is documented as `YYYY-MM-DDTHH:MM:SSZ`).
///
/// Saturating, not plain arithmetic: the value can come from the request, and a
/// total function here is what keeps an extreme one from overflowing rather than
/// being answered.
fn truncate_to_second(millis: i64) -> i64 {
    millis.saturating_sub(millis.rem_euclid(1000))
}

/// How far either side of now a caller-supplied `purchaseTime` may fall.
///
/// A chosen bound, not a documented one: the longest term the catalog sells is
/// three years, so a purchase dated a decade out is not a purchase. Refusing it
/// at the edge keeps a request-derived instant away from the term and
/// return-window arithmetic entirely.
pub const MAX_PURCHASE_TIME_SKEW_MILLIS: i64 = 10 * 365 * 24 * 60 * 60 * 1000;

/// Refuse a `purchaseTime` further than [`MAX_PURCHASE_TIME_SKEW_MILLIS`] from
/// now, in either direction.
pub fn check_purchase_time(purchase_millis: i64, now_millis: i64) -> Result<(), String> {
    let skew = purchase_millis.saturating_sub(now_millis).saturating_abs();
    if skew > MAX_PURCHASE_TIME_SKEW_MILLIS {
        return Err(format!(
            "purchaseTime must be within {MAX_PURCHASE_TIME_SKEW_MILLIS} ms of now, got '{}'",
            format_millis(purchase_millis)
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Amounts
// ---------------------------------------------------------------------------

/// How many fractional digits a rendered `Amount` carries.
///
/// `Amount` targets `string` in the model, so the rendering IS the wire value:
/// a client that compares, displays or hashes it sees exactly these digits,
/// rather than a number it is free to reformat. This service renders every
/// `Amount` at this one fixed scale — eight fractional digits, the scale AWS
/// bills Savings Plans commitments and payments at — so no amount is ever
/// reported in two spellings.
pub const AMOUNT_WIRE_DIGITS: usize = 8;

/// The scale an `Amount` is held at: the wire scale, so every value the service
/// accepts is an exact integer, no rounding creeps into a stored amount, and
/// the stored value and the rendered one are the same number.
pub const AMOUNT_SCALE: i128 = 100_000_000;

/// The most digits an `Amount` a REQUEST carries may have after the decimal
/// point: "You cannot specify more than five digits after the decimal point".
/// The service's own rendering is wider (see [`AMOUNT_WIRE_DIGITS`]).
pub const MAX_AMOUNT_DIGITS: usize = 5;

/// The smallest hourly commitment `CreateSavingsPlan` accepts, from the
/// documented rule on `commitment`: "a value between 0.001 and 1 million".
pub const MIN_COMMITMENT_UNITS: i128 = AMOUNT_SCALE / 1000;
/// The largest hourly commitment, from the same sentence.
pub const MAX_COMMITMENT_UNITS: i128 = 1_000_000 * AMOUNT_SCALE;

/// Parse an `Amount` into [`AMOUNT_SCALE`] units, refusing anything that is not
/// a plain non-negative decimal number with at most [`MAX_AMOUNT_DIGITS`]
/// decimal places.
///
/// `Amount` is an unconstrained string in the model, so the shape of the value
/// is the handler's to enforce.
pub fn parse_amount(member: &str, value: &str) -> Result<i128, String> {
    parse_amount_with_digits(member, value, MAX_AMOUNT_DIGITS)
}

/// Read an amount the way a FILTER value is read: at the full wire scale, since
/// the value a client filters with is the one this service rendered for it
/// ([`AMOUNT_WIRE_DIGITS`] fractional digits), which is wider than the
/// [`MAX_AMOUNT_DIGITS`] a request may SPECIFY.
pub fn parse_amount_at_wire_scale(value: &str) -> Option<i128> {
    parse_amount_with_digits("amount", value, AMOUNT_WIRE_DIGITS).ok()
}

/// How a leading `-` is read by [`parse_decimal_amount`].
///
/// A member whose documented rule has a RANGE reads a negative amount as the
/// number it is, so the clause a caller gets names the bound the value missed;
/// a member whose rule is simply "an amount" refuses the sign as a malformed
/// value, which is all there is to say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Negative {
    /// Refused as a malformed amount.
    Refused,
    /// Read as the negative number it is.
    Read,
}

/// [`parse_amount`], with the number of fractional digits the caller admits.
fn parse_amount_with_digits(member: &str, value: &str, max_digits: usize) -> Result<i128, String> {
    parse_decimal_amount(member, value, max_digits, Negative::Refused)
}

/// Parse a plain decimal amount into [`AMOUNT_SCALE`] units, admitting at most
/// `max_digits` fractional digits and reading a leading `-` as `negative` says.
fn parse_decimal_amount(
    member: &str,
    value: &str,
    max_digits: usize,
    negative: Negative,
) -> Result<i128, String> {
    let trimmed = value.trim();
    let form = match negative {
        Negative::Refused => "non-negative decimal amount",
        Negative::Read => "decimal amount",
    };
    let invalid = || format!("{member} must be a {form}, got '{value}'");
    // The sign is taken off the front before the digit rules are applied, so a
    // negative amount is read by exactly the rules a positive one is.
    let (sign, magnitude) = match trimmed.strip_prefix('-') {
        Some(magnitude) if negative == Negative::Read => (-1, magnitude),
        Some(_) => return Err(invalid()),
        None => (1, trimmed),
    };
    let (whole, fraction) = match magnitude.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (magnitude, ""),
    };
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (magnitude.contains('.') && !digits(fraction)) {
        return Err(invalid());
    }
    if fraction.len() > max_digits {
        return Err(format!(
            "{member} cannot specify more than {max_digits} digits after the decimal point, got '{value}'"
        ));
    }
    let whole: i128 = whole.parse().map_err(|_| invalid())?;
    let mut units = whole.checked_mul(AMOUNT_SCALE).ok_or_else(invalid)?;
    if !fraction.is_empty() {
        // Right-pad to the held scale, so ".1" and ".10000000" are one value.
        let padded = format!("{fraction:0<width$}", width = AMOUNT_WIRE_DIGITS);
        units = units
            .checked_add(padded.parse::<i128>().map_err(|_| invalid())?)
            .ok_or_else(invalid)?;
    }
    Ok(sign * units)
}

/// Parse and validate `CreateSavingsPlan`'s `commitment` against the rule the
/// model documents for it: a value between 0.001 and 1 million, with at most
/// five digits after the decimal point.
///
/// A value OUTSIDE that interval is a well-formed amount the member's range
/// refuses, so the clause names the member and the bound it missed — the floor
/// for anything below it, including zero and a negative amount, and the ceiling
/// for anything above. Only a value that is not a decimal number at all, or one
/// spelled at more digits than a request may carry, is refused on its form.
pub fn parse_commitment(value: &str) -> Result<i128, String> {
    let units = parse_decimal_amount("commitment", value, MAX_AMOUNT_DIGITS, Negative::Read)?;
    if units < MIN_COMMITMENT_UNITS {
        return Err(format!(
            "'commitment' cannot be less than {}",
            format_bound(MIN_COMMITMENT_UNITS)
        ));
    }
    if units > MAX_COMMITMENT_UNITS {
        return Err(format!(
            "'commitment' cannot be greater than {}",
            format_bound(MAX_COMMITMENT_UNITS)
        ));
    }
    Ok(units)
}

/// A BOUND as a message quotes it: the amount in its shortest exact decimal
/// spelling — trailing fractional zeros dropped, and the decimal point with them
/// when the amount is whole.
///
/// A bound is a number the service publishes rather than a value a caller sent,
/// and the documentation writes it this way ("a value between 0.001 and 1
/// million"), so it is quoted this way too. Every amount the service REPORTS
/// still goes through [`format_amount`]'s one fixed scale.
pub fn format_bound(units: i128) -> String {
    let rendered = format_amount(units);
    match rendered.split_once('.') {
        Some((whole, fraction)) => match fraction.trim_end_matches('0') {
            "" => whole.to_string(),
            trimmed => format!("{whole}.{trimmed}"),
        },
        None => rendered,
    }
}

/// Render [`AMOUNT_SCALE`] units as an `Amount`: FIXED WIDTH, always with a
/// decimal point and exactly [`AMOUNT_WIRE_DIGITS`] fractional digits.
///
/// Trailing zeros are significant here, not noise: see [`AMOUNT_WIRE_DIGITS`].
pub fn format_amount(units: i128) -> String {
    let sign = if units < 0 { "-" } else { "" };
    let units = units.abs();
    format!(
        "{sign}{}.{:0width$}",
        units / AMOUNT_SCALE,
        units % AMOUNT_SCALE,
        width = AMOUNT_WIRE_DIGITS
    )
}

/// Round `units` up to a whole amount (no fractional part).
pub fn ceil_to_whole(units: i128) -> i128 {
    units
        .saturating_add(AMOUNT_SCALE - 1)
        .saturating_div(AMOUNT_SCALE)
        .saturating_mul(AMOUNT_SCALE)
}

/// Round `units` down to a whole amount (no fractional part).
pub fn floor_to_whole(units: i128) -> i128 {
    units.saturating_div(AMOUNT_SCALE).saturating_mul(AMOUNT_SCALE)
}

/// Whether `units` is a whole amount — the form `upfrontPaymentAmount` must
/// take ("a whole number between 50 and 99 percent of the total value").
pub fn is_whole(units: i128) -> bool {
    units % AMOUNT_SCALE == 0
}

/// Divide `units` by `divisor`, rounding to the nearest unit: the recurring
/// payment is the balance spread over the term's hours, and the service holds
/// it at [`AMOUNT_SCALE`].
pub fn divide_rounded(units: i128, divisor: i128) -> i128 {
    if divisor == 0 {
        return 0;
    }
    (units + divisor / 2) / divisor
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Encode a page offset into an opaque `PaginationToken`.
///
/// Hex keeps the token inside the model's `^[A-Za-z0-9/=\+]+$` pattern, and
/// `kind` keeps a token minted by one paginator from resuming another's page.
pub fn encode_page_token(kind: &str, offset: usize) -> String {
    hex::encode(format!("{kind}:{offset}"))
}

/// Decode a token [`encode_page_token`] produced, or `None` when it was not
/// produced by this paginator.
pub fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let raw = hex::decode(token).ok()?;
    let text = String::from_utf8(raw).ok()?;
    text.strip_prefix(kind)?.strip_prefix(':')?.parse().ok()
}

/// One page of `items` starting at `offset`, plus the offset of the next page
/// when one remains. `limit` of `None` means "every remaining item".
pub fn paginate<T: Clone>(
    items: &[T],
    offset: usize,
    limit: Option<usize>,
) -> (Vec<T>, Option<usize>) {
    let start = offset.min(items.len());
    let end = match limit {
        Some(limit) => start.saturating_add(limit).min(items.len()),
        None => items.len(),
    };
    let page = items[start..end].to_vec();
    let next = (end < items.len()).then_some(end);
    (page, next)
}

/// The page the offerings search serves when the caller names no size.
///
/// `DescribeSavingsPlansOfferings$maxResults` is `@required` with `@default: 0`
/// and a `PageSize` range of 0..1000, so 0 is "no size asked for" and the
/// service picks one: a search of a catalog the service owns comes back as one
/// page of it, resumable through `nextToken`, never as the whole catalog.
///
/// A chosen bound, not a documented one: AWS publishes no default page size for
/// this search. What the recorded answers show is only that a search WAS
/// answered with one page and a cursor the next call resumed, which is the part
/// this constant exists to keep true.
///
/// The seeded catalog is deliberately larger than this, so the page is
/// observable: `a_search_that_names_no_page_size_gets_the_services_own_page`
/// asserts a search that names no size answers exactly one page of this many
/// offerings, and fails loudly if the catalog ever shrinks below it.
pub const DEFAULT_OFFERINGS_PAGE_SIZE: usize = 20;

/// Whether a list-shaped request filter admits `value`: an absent or empty list
/// constrains nothing, a populated one admits only the values it names.
pub fn filter_admits(filter: Option<&Vec<String>>, value: &str) -> bool {
    match filter {
        Some(values) if !values.is_empty() => values.iter().any(|v| v == value),
        _ => true,
    }
}

/// The identity of an idempotent request: everything about it that the stored
/// resource depends on.
///
/// A replayed `clientToken` is only idempotent for the SAME request; a token
/// reused for a different one is refused rather than answered with the first
/// call's resource, which would report success for a write that never happened.
pub fn fingerprint(parts: &[(&str, &str)]) -> String {
    // Every component is length-prefixed, so no two different requests can
    // encode to the same string however their values are spelled: a tag value
    // carrying the separator, or a key carrying the `=`, cannot be read as a
    // different part.
    parts
        .iter()
        .map(|(name, value)| format!("{}:{name}:{}:{value}", name.len(), value.len()))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// The most tags one resource may carry.
///
/// AWS's documented tag limit ("Tag naming limits and requirements", AWS
/// General Reference: a maximum of 50 tags per resource). The model declares no
/// constraint on `TagMap`, so the handler enforces it.
pub const MAX_TAGS_PER_PLAN: usize = 50;

/// The longest tag key AWS accepts (128 Unicode characters, same source).
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag value AWS accepts (256 Unicode characters, same source).
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Refuse a tag set that is larger than the service accepts, or that carries an
/// over-long key or value. Checked before anything is stored, so a refused
/// write changes nothing.
pub fn check_tags(tags: &BTreeMap<String, String>) -> Result<(), String> {
    if tags.len() > MAX_TAGS_PER_PLAN {
        return Err(format!(
            "a Savings Plan cannot carry more than {MAX_TAGS_PER_PLAN} tags, got {}",
            tags.len()
        ));
    }
    for (key, value) in tags {
        if key.chars().count() > MAX_TAG_KEY_CHARS {
            return Err(format!(
                "a tag key cannot be longer than {MAX_TAG_KEY_CHARS} characters, got {}",
                key.chars().count()
            ));
        }
        if value.chars().count() > MAX_TAG_VALUE_CHARS {
            return Err(format!(
                "the value of tag '{key}' cannot be longer than {MAX_TAG_VALUE_CHARS} characters, got {}",
                value.chars().count()
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// Build the ARN of a Savings Plan.
///
/// Savings Plans are account-wide rather than regional, so the ARN's region
/// segment is empty — the form the model's `SavingsPlanArn` pattern admits
/// through its `([a-z]{2}-[a-z]+-\d{1}|)` alternative.
pub fn savings_plan_arn(savings_plan_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        "",
        &format!("savingsplan/{savings_plan_id}"),
    )
}

/// Whether an ARN's partition field names an AWS partition.
///
/// The partition group of the `SavingsPlanArn` pattern is `aws([-a-z]*)?`, so
/// `aws` and every suffixed partition — `aws-cn`, `aws-us-gov`, `aws-iso-b` —
/// are all admitted, and this mirrors that group rather than naming the
/// partitions one by one: a new partition is a new suffix, not a new rule.
///
/// The checked-in model used to carry a narrower `^arn:aws:` for that group,
/// which refused a China or GovCloud plan ARN at the boundary validator. The
/// live service accepts those and QUOTES the partition-tolerant expression in
/// its own `ValidationException` (fixture
/// `savingsplans-tagging-error-paths-unknown-resource`), so the model's pattern
/// was corrected to the one AWS enforces and states.
fn is_aws_partition(field: &str) -> bool {
    match field.strip_prefix("aws") {
        Some(suffix) => suffix.chars().all(|ch| ch == '-' || ch.is_ascii_lowercase()),
        None => false,
    }
}

/// The Savings Plan id inside a `SavingsPlanArn`, or `None` when the ARN names
/// some other service or some other resource type.
///
/// The model's `@pattern` already refused anything that is not ARN-shaped
/// before the handler ran, so this only has to reject a well-formed ARN that is
/// not one of ours — which the partition never makes it: an ARN in any AWS
/// partition is a Savings Plan ARN, and whether a plan with it exists is a
/// question for the store (a 404), not for the shape of the ARN (a 400).
pub fn savings_plan_id_from_arn(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(SERVICE_NAMESPACE),
        Some(_region),
        Some(_account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if !is_aws_partition(partition) {
        return None;
    }
    resource.strip_prefix("savingsplan/")
}

/// How this service states that the resource an operation was pointed at does
/// not exist.
///
/// AWS builds this message FROM THE REQUEST, naming the `resourceArn` the
/// caller sent — `Resource with resourceArn: <arn> doesn't exist` — so the
/// template is part of the contract and lives in ONE place, shared by every
/// operation that takes a `resourceArn`.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource with resourceArn: {resource_arn} doesn't exist")
}

// ---------------------------------------------------------------------------
// The offering catalog
// ---------------------------------------------------------------------------

/// A Savings Plan offering: one purchasable combination of plan type, payment
/// option, term and (for the plan types that are scoped to one) region and
/// instance family. Projected onto the wire as `SavingsPlanOffering`.
#[derive(Debug, Clone)]
pub struct Offering {
    pub offering_id: String,
    pub product_types: Vec<String>,
    pub plan_type: String,
    pub description: String,
    pub payment_option: String,
    pub duration_seconds: i64,
    pub currency: String,
    pub service_code: String,
    pub usage_type: String,
    pub operation: String,
    /// `(name, value)` pairs, where `name` is a
    /// `SavingsPlanOfferingPropertyKey`.
    pub properties: Vec<(String, String)>,
}

impl Offering {
    /// The value of the named property, or `None` when this offering does not
    /// carry it (a Compute Savings Plan is scoped to no region).
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The term, in hours — what an hourly commitment is multiplied by to get
    /// the total value of the plan. Both terms the catalog sells are whole
    /// numbers of hours.
    pub fn term_hours(&self) -> i128 {
        i128::from(self.duration_seconds / 3600)
    }
}

/// The terms AWS sells Savings Plans in: a one-year and a three-year term, each
/// as `(durationSeconds, description label, usage-type token)`.
///
/// `durationSeconds` is the term the wire reports
/// (`SavingsPlanOffering$durationSeconds`); the description label is how the
/// term is spelled in `description` and the usage-type token how it is spelled
/// in the billing `usageType` (see [`offering_description`] and
/// [`offering_usage_type`]).
const TERMS: [(i64, &str, &str); 2] = [
    (31_536_000, "1 year", "1yr"),
    (94_608_000, "3 year", "3yr"),
];

/// Every `SavingsPlanPaymentOption` the model declares, in the order the
/// catalog enumerates them.
///
/// The catalog is walked in a stable total order over the attributes that
/// distinguish one offering from another — ascending term, then payment option
/// in LEXICOGRAPHIC order, then (where the plan type is scoped to them) region
/// and instance family — so page N of a search is the same rows on every call
/// and two searches agree on which offering comes first. Lexicographic order is
/// not the model's declaration order (`All Upfront`, `Partial Upfront`,
/// `No Upfront`), which is why this list is not in the enum's order.
const PAYMENT_OPTIONS: [&str; 3] = [
    PAYMENT_ALL_UPFRONT,
    PAYMENT_NO_UPFRONT,
    PAYMENT_PARTIAL_UPFRONT,
];

/// How an offering's `description` names it: the term, the payment option and
/// the plan type, in that order (`SavingsPlanDescription`, pattern
/// `^[a-zA-Z0-9_\- ]+$`, so no punctuation beyond `-` and `_`).
fn offering_description(term_label: &str, payment_option: &str, plan_label: &str) -> String {
    format!("{term_label} {payment_option} {plan_label} Savings Plan")
}

/// The billing `usageType` of an offering's line item: the usage-type family of
/// the plan type (prefixed by the region it is scoped to, where it is scoped to
/// one) followed by the term and payment option that line item is billed
/// under — so every offering reports the usage type a billing report would show
/// for it, and no two offerings of one plan type share a usage type.
///
/// `family` carries the region prefix and any further dimension the plan type is
/// scoped to (an EC2 instance family). `SavingsPlanUsageType` admits `:`, `.`,
/// `/` and `-` (pattern `^[a-zA-Z0-9_ \/.:-]+$`), so the `:`-separated form is
/// inside the model's constraint.
fn offering_usage_type(family: &str, term_token: &str, payment_option: &str) -> String {
    let payment_token: String = payment_option
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    format!("{family}:{term_token}{payment_token}")
}

/// The regions the catalog is seeded for, each with the `CurrencyCode` the
/// region bills in.
///
/// Savings Plans are sold in every commercial region, so the catalog carries a
/// region-scoped offering for each one rather than for a handful: a client that
/// searches for offerings in the region it runs in finds them, and the catalog
/// is the size a real one is — bigger than any one page a caller may ask for
/// (`PageSize` tops out at 1000), so a search of it is genuinely paginated.
///
/// The currency is the one the model's `CurrencyCode` enum offers for that
/// region's billing: `CNY` in the China partition, `EUR` in Europe, `USD`
/// elsewhere.
const REGIONS: [(&str, &str); 22] = [
    ("us-east-1", "USD"),
    ("us-east-2", "USD"),
    ("us-west-1", "USD"),
    ("us-west-2", "USD"),
    ("ca-central-1", "USD"),
    ("sa-east-1", "USD"),
    ("eu-west-1", "EUR"),
    ("eu-west-2", "EUR"),
    ("eu-west-3", "EUR"),
    ("eu-central-1", "EUR"),
    ("eu-north-1", "EUR"),
    ("eu-south-1", "EUR"),
    ("ap-south-1", "USD"),
    ("ap-east-1", "USD"),
    ("ap-northeast-1", "USD"),
    ("ap-northeast-2", "USD"),
    ("ap-southeast-1", "USD"),
    ("ap-southeast-2", "USD"),
    ("me-south-1", "USD"),
    ("af-south-1", "USD"),
    ("cn-north-1", "CNY"),
    ("cn-northwest-1", "CNY"),
];

/// The EC2 instance families the catalog carries EC2 Instance offerings for
/// (`SavingsPlanOffering$properties` -> `instanceFamily`): the general purpose,
/// compute, memory and storage optimised families of several generations, so a
/// search by instance family finds the family a client is actually running.
const EC2_INSTANCE_FAMILIES: [&str; 18] = [
    "m5", "m6a", "m6i", "m7g", "c5", "c6a", "c6i", "c7g", "r5", "r6a", "r6i", "r7g", "t3",
    "t3a", "t4g", "i3", "i4i", "x2idn",
];

/// Build the catalog: one offering per plan type x term x payment option, times
/// the region (and, for EC2 Instance plans, the instance family) each plan type
/// is scoped to.
///
/// The catalog is enumerated BY PLAN TYPE — the order the model's
/// `SavingsPlanType` enum declares (Compute, EC2Instance, SageMaker, Database) —
/// then by ascending term, then by payment option, then by the region and
/// instance family the plan type is scoped to. That is the order the recorded
/// catalog answers in: a search narrowed to Compute plans answers with that plan
/// type's six term/payment-option rows as the first rows of the catalog, in
/// payment-option order. Plan types that are not scoped to a region (Compute)
/// carry one row per term and payment option; the scoped ones carry one per
/// region, and EC2 Instance plans one per instance family within the region.
///
/// The product types of each plan type are the ones AWS covers with it:
/// Compute Savings Plans apply to EC2, Fargate and Lambda usage in any region;
/// EC2 Instance Savings Plans to one instance family in one region; SageMaker
/// and Database Savings Plans to their own products.
fn seed_offerings(id_seed: u128) -> Vec<Offering> {
    let mut offerings = Vec::new();
    for (duration_seconds, term_label, term_token) in TERMS {
        for payment_option in PAYMENT_OPTIONS {
            offerings.push(offering(
                "Compute",
                // The order a Compute Savings Plan's coverage is reported in,
                // as the recorded AWS catalog row lists it. The order of a
                // coverage list carries no meaning on the wire; the catalog
                // mirrors the recorded one so a client diffing the two sees the
                // same row.
                &["Fargate", "EC2", "Lambda"],
                &offering_description(term_label, payment_option, "Compute"),
                payment_option,
                duration_seconds,
                "USD",
                "ComputeSavingsPlans",
                &offering_usage_type("ComputeSP", term_token, payment_option),
                "",
                Vec::new(),
            ));
        }
    }
    for (duration_seconds, term_label, term_token) in TERMS {
        for payment_option in PAYMENT_OPTIONS {
            for (region, currency) in REGIONS {
                for family in EC2_INSTANCE_FAMILIES {
                    offerings.push(offering(
                        "EC2Instance",
                        &["EC2"],
                        &offering_description(term_label, payment_option, "EC2 Instance"),
                        payment_option,
                        duration_seconds,
                        currency,
                        "AmazonEC2",
                        &offering_usage_type(
                            &format!("{region}-EC2SP:{family}"),
                            term_token,
                            payment_option,
                        ),
                        "RunInstances",
                        vec![
                            (PROPERTY_INSTANCE_FAMILY.to_string(), family.to_string()),
                            (PROPERTY_REGION.to_string(), region.to_string()),
                        ],
                    ));
                }
            }
        }
    }
    for (duration_seconds, term_label, term_token) in TERMS {
        for payment_option in PAYMENT_OPTIONS {
            for (region, currency) in REGIONS {
                offerings.push(offering(
                    "SageMaker",
                    &["SageMaker"],
                    &offering_description(term_label, payment_option, "SageMaker"),
                    payment_option,
                    duration_seconds,
                    currency,
                    "AmazonSageMaker",
                    &offering_usage_type(
                        &format!("{region}-SageMakerSP"),
                        term_token,
                        payment_option,
                    ),
                    "RunInstance",
                    vec![(PROPERTY_REGION.to_string(), region.to_string())],
                ));
            }
        }
    }
    for (duration_seconds, term_label, term_token) in TERMS {
        for payment_option in PAYMENT_OPTIONS {
            for (region, currency) in REGIONS {
                offerings.push(offering(
                    "Database",
                    &["RDS", "DocDB", "Neptune", "ElastiCache"],
                    &offering_description(term_label, payment_option, "Database"),
                    payment_option,
                    duration_seconds,
                    currency,
                    "AmazonRDS",
                    &offering_usage_type(
                        &format!("{region}-DatabaseSP"),
                        term_token,
                        payment_option,
                    ),
                    "CreateDBInstance",
                    vec![(PROPERTY_REGION.to_string(), region.to_string())],
                ));
            }
        }
    }
    for (index, entry) in offerings.iter_mut().enumerate() {
        entry.offering_id = mint_id(id_seed, index as u64);
    }
    offerings
}

/// Mint an id in the v4 UUID form: `counter` makes it unique within the store
/// and `seed` makes it unguessable from one store to the next, and the two are
/// run through SHA-256 so no request value can be read back out of an id.
///
/// The form is what matters on the wire: the model's `UUID` pattern (and the
/// `SavingsPlanArn` pattern's `[1-5]`/`[89ab]` nibbles) require hyphenated
/// lowercase hex with the RFC 4122 version and variant bits set, which is what
/// this writes.
fn mint_id(seed: u128, counter: u64) -> String {
    let digest = Sha256::digest(format!("{seed}/{counter}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A per-store seed for [`mint_id`].
fn id_seed() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default()
}

/// One catalog entry. Its `offering_id` is assigned by [`seed_offerings`] once
/// the catalog is built, so an offering keeps one id for the life of the store,
/// the way a real offering id outlives any one request.
#[allow(clippy::too_many_arguments)]
fn offering(
    plan_type: &str,
    product_types: &[&str],
    description: &str,
    payment_option: &str,
    duration_seconds: i64,
    currency: &str,
    service_code: &str,
    usage_type: &str,
    operation: &str,
    properties: Vec<(String, String)>,
) -> Offering {
    Offering {
        offering_id: String::new(),
        product_types: product_types.iter().map(|p| (*p).to_string()).collect(),
        plan_type: plan_type.to_string(),
        description: description.to_string(),
        payment_option: payment_option.to_string(),
        duration_seconds,
        currency: currency.to_string(),
        service_code: service_code.to_string(),
        usage_type: usage_type.to_string(),
        operation: operation.to_string(),
        properties,
    }
}

// ---------------------------------------------------------------------------
// Purchased plans
// ---------------------------------------------------------------------------

/// A purchased Savings Plan, as this service stores it. Projected onto the wire
/// as `SavingsPlan` by [`SavingsPlanRecord::project`].
#[derive(Debug, Clone)]
pub struct SavingsPlanRecord {
    pub savings_plan_id: String,
    pub savings_plan_arn: String,
    pub offering_id: String,
    pub description: String,
    /// The plan's term start, epoch millis: the purchase time.
    pub start_millis: i64,
    /// The plan's term end, epoch millis: `start` plus the term duration.
    pub end_millis: i64,
    pub state: String,
    /// When the clock owes this plan its move out of `queued`.
    pub pending_due_millis: i64,
    /// When the clock owes this plan its move out of `payment-pending`.
    pub active_due_millis: i64,
    pub region: Option<String>,
    pub ec2_instance_family: Option<String>,
    pub savings_plan_type: String,
    pub payment_option: String,
    pub product_types: Vec<String>,
    pub currency: String,
    /// The hourly commitment, in [`AMOUNT_SCALE`] units. Held as a NUMBER, not
    /// as the string it is rendered to, so one amount has exactly one spelling
    /// on the wire ([`format_amount`]) and a filter on it compares numbers.
    pub commitment_units: i128,
    /// The up-front payment, in [`AMOUNT_SCALE`] units.
    pub upfront_payment_units: i128,
    /// The recurring hourly payment, in [`AMOUNT_SCALE`] units.
    pub recurring_payment_units: i128,
    pub term_duration_in_seconds: i64,
    pub tags: BTreeMap<String, String>,
}

impl SavingsPlanRecord {
    /// Whether a return can be requested for this plan RIGHT NOW: it must have
    /// started and been paid for, and still be inside its return window.
    ///
    /// A plan that has not started yet (`queued`) is not returned but cancelled,
    /// through `DeleteQueuedSavingsPlan`, so it is not returnable here however
    /// open its window is.
    pub fn is_returnable(&self, now_millis: i64) -> bool {
        matches!(
            self.state.as_str(),
            STATE_PAYMENT_PENDING | STATE_ACTIVE
        ) && now_millis <= self.return_window_closes_millis()
    }

    /// Whether this plan still HAS a return window: its state has not foreclosed
    /// a return (it has not been returned, cancelled or retired) and the window
    /// has not run out.
    ///
    /// Wider than [`Self::is_returnable`] by one state: a plan that has not
    /// started yet still has the window its start opens, which is what
    /// `returnableUntil` reports even though a return cannot be requested until
    /// the plan starts.
    pub fn has_return_window(&self, now_millis: i64) -> bool {
        matches!(
            self.state.as_str(),
            STATE_QUEUED | STATE_PAYMENT_PENDING | STATE_ACTIVE
        ) && now_millis <= self.return_window_closes_millis()
    }

    /// The instant this plan's return window closes. Saturating: `start_millis`
    /// carries a caller-supplied purchase time.
    pub fn return_window_closes_millis(&self) -> i64 {
        self.start_millis.saturating_add(RETURN_WINDOW_MILLIS)
    }

    /// `returnableUntil`: "the time until when a return for the Savings Plan can
    /// be requested. If the Savings Plan is not returnable, the field reflects
    /// the Savings Plans start time."
    ///
    /// Reported from [`Self::has_return_window`], not from
    /// [`Self::is_returnable`]: the member is the instant the window CLOSES,
    /// which a plan whose term has not begun still has.
    pub fn returnable_until_millis(&self, now_millis: i64) -> i64 {
        if self.has_return_window(now_millis) {
            self.return_window_closes_millis()
        } else {
            self.start_millis
        }
    }

    /// Merge `tags` into this plan's tags, refusing the WHOLE write when the
    /// result would break [`check_tags`] — so a refused `TagResource` leaves the
    /// stored tag set exactly as it was, and no caller can grow it past the
    /// service's tag limit.
    pub fn merge_tags(
        &mut self,
        tags: impl IntoIterator<Item = (String, String)>,
    ) -> Result<(), String> {
        let mut merged = self.tags.clone();
        merged.extend(tags);
        check_tags(&merged)?;
        self.tags = merged;
        Ok(())
    }

    /// Remove the named tags from this plan, ignoring a key it does not carry.
    ///
    /// `UntagResource` states the keys the resource must END UP without, so a key
    /// that was never there is already in the state the caller asked for: an
    /// absent key is not an error, which is what makes a retried untag safe.
    /// Nothing here can break [`check_tags`] — removal only shrinks the set.
    pub fn remove_tags<'a>(&mut self, tag_keys: impl IntoIterator<Item = &'a str>) {
        for key in tag_keys {
            self.tags.remove(key);
        }
    }

    /// Whether a `SavingsPlanFilter` admits this plan. `name` is a
    /// `SavingsPlansFilterName`; a filter with no values constrains nothing.
    pub fn matches_filter(&self, name: &str, values: Option<&Vec<String>>) -> bool {
        // An amount or term filter compares NUMBERS and an instant filter
        // compares INSTANTS, so a filter spelled "0.100" matches a commitment
        // held as "0.1" and one spelled with an offset matches the same moment.
        let (field, kind): (Option<String>, FilterKind) = match name {
            "region" => (self.region.clone(), FilterKind::Text),
            "ec2-instance-family" | "instance-family" => {
                (self.ec2_instance_family.clone(), FilterKind::Text)
            }
            "commitment" => (
                Some(format_amount(self.commitment_units)),
                FilterKind::Number(self.commitment_units),
            ),
            "upfront" => (
                Some(format_amount(self.upfront_payment_units)),
                FilterKind::Number(self.upfront_payment_units),
            ),
            "term" => (
                Some(self.term_duration_in_seconds.to_string()),
                FilterKind::Number(
                    i128::from(self.term_duration_in_seconds).saturating_mul(AMOUNT_SCALE),
                ),
            ),
            "savings-plan-type" => (Some(self.savings_plan_type.clone()), FilterKind::Text),
            "payment-option" => (Some(self.payment_option.clone()), FilterKind::Text),
            "start" => (
                Some(format_millis(self.start_millis)),
                FilterKind::Instant(self.start_millis),
            ),
            "end" => (
                Some(format_millis(self.end_millis)),
                FilterKind::Instant(self.end_millis),
            ),
            _ => (None, FilterKind::Text),
        };
        match (values, field) {
            (Some(values), _) if values.is_empty() => true,
            (None, _) => true,
            (Some(_), None) => false,
            (Some(values), Some(field)) => values.iter().any(|value| {
                if *value == field {
                    return true;
                }
                match kind {
                    FilterKind::Text => false,
                    FilterKind::Number(units) => {
                        parse_amount_at_wire_scale(value) == Some(units)
                    }
                    FilterKind::Instant(millis) => parse_millis(value) == Some(millis),
                }
            }),
        }
    }
}

/// How a `SavingsPlanFilter`'s value is compared with the member it names.
#[derive(Debug, Clone, Copy)]
enum FilterKind {
    /// Compared as text, the way the model's enum-valued members are.
    Text,
    /// Compared as a number (the member's value in [`AMOUNT_SCALE`] units), so
    /// two spellings of one amount are one value.
    Number(i128),
    /// Compared as an instant (epoch millis), for the same reason.
    Instant(i64),
}

/// The most Savings Plans this account may hold.
///
/// A quota, answered with the service's own modelled
/// `ServiceQuotaExceededException`: the store keeps every plan ever purchased
/// (a Savings Plan is never deleted — it is returned, cancelled or retired),
/// so without a ceiling one caller could grow it for the life of the process.
pub const MAX_PLANS_PER_ACCOUNT: usize = 1000;

/// Per-service state for com-amazonaws-savingsplans-awssavingsplan. A handler
/// reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSavingsplansAwssavingsplanState {
    /// The seeded, read-only catalog.
    offerings: Vec<Offering>,
    /// Every plan sold, in purchase order.
    plans: Vec<SavingsPlanRecord>,
    /// `savingsPlanId` -> index into `plans`.
    plan_index: HashMap<String, usize>,
    /// `savingsPlanArn` -> index into `plans`.
    arn_index: HashMap<String, usize>,
    /// `<operation>/<clientToken>` -> the request that token's first call
    /// carried (its [`fingerprint`]) and the `savingsPlanId` it resolved to.
    idempotency: HashMap<String, (String, String)>,
    /// This store's id seed, and how many ids it has minted: see [`mint_id`].
    id_seed: u128,
    minted_ids: u64,
}

impl Default for ComAmazonawsSavingsplansAwssavingsplanState {
    fn default() -> Self {
        let seed = id_seed();
        let offerings = seed_offerings(seed);
        Self {
            minted_ids: offerings.len() as u64,
            offerings,
            plans: Vec::new(),
            plan_index: HashMap::new(),
            arn_index: HashMap::new(),
            idempotency: HashMap::new(),
            id_seed: seed,
        }
    }
}

impl ComAmazonawsSavingsplansAwssavingsplanState {
    /// Mint a fresh id for a resource this store is creating. See [`mint_id`].
    pub fn next_id(&mut self) -> String {
        self.minted_ids = self.minted_ids.saturating_add(1);
        mint_id(self.id_seed, self.minted_ids)
    }

    /// The whole catalog, in catalog order.
    pub fn offerings(&self) -> &[Offering] {
        &self.offerings
    }

    /// The catalog entry with this offering id.
    pub fn offering(&self, offering_id: &str) -> Option<&Offering> {
        self.offerings
            .iter()
            .find(|offering| offering.offering_id == offering_id)
    }

    /// Every plan sold, in purchase order.
    pub fn plans(&self) -> &[SavingsPlanRecord] {
        &self.plans
    }

    /// The plan with this id.
    pub fn plan(&self, savings_plan_id: &str) -> Option<&SavingsPlanRecord> {
        self.plan_index
            .get(savings_plan_id)
            .and_then(|index| self.plans.get(*index))
    }

    /// The plan with this id, mutably.
    pub fn plan_mut(&mut self, savings_plan_id: &str) -> Option<&mut SavingsPlanRecord> {
        let index = *self.plan_index.get(savings_plan_id)?;
        self.plans.get_mut(index)
    }

    /// The plan with this ARN.
    pub fn plan_by_arn(&self, arn: &str) -> Option<&SavingsPlanRecord> {
        self.arn_index
            .get(arn)
            .and_then(|index| self.plans.get(*index))
    }

    /// The plan with this ARN, mutably.
    pub fn plan_by_arn_mut(&mut self, arn: &str) -> Option<&mut SavingsPlanRecord> {
        let index = *self.arn_index.get(arn)?;
        self.plans.get_mut(index)
    }

    /// Whether another purchase would exceed [`MAX_PLANS_PER_ACCOUNT`].
    pub fn at_plan_quota(&self) -> bool {
        self.plans.len() >= MAX_PLANS_PER_ACCOUNT
    }

    /// Record a newly purchased plan, keeping both indexes in step.
    pub fn insert_plan(&mut self, record: SavingsPlanRecord) {
        let index = self.plans.len();
        self.plan_index
            .insert(record.savings_plan_id.clone(), index);
        self.arn_index.insert(record.savings_plan_arn.clone(), index);
        self.plans.push(record);
    }

    /// What an earlier call with this `clientToken` carried and resolved to:
    /// `(fingerprint, savingsPlanId)`.
    pub fn idempotent(&self, operation: &str, client_token: &str) -> Option<(&str, &str)> {
        self.idempotency
            .get(&format!("{operation}/{client_token}"))
            .map(|(fingerprint, id)| (fingerprint.as_str(), id.as_str()))
    }

    /// Remember the request this `clientToken` carried and the plan it resolved
    /// to, so a replay of the SAME request answers with that plan and a replay
    /// of a different one can be refused.
    pub fn remember_idempotent(
        &mut self,
        operation: &str,
        client_token: &str,
        fingerprint: &str,
        savings_plan_id: &str,
    ) {
        self.idempotency.insert(
            format!("{operation}/{client_token}"),
            (fingerprint.to_string(), savings_plan_id.to_string()),
        );
    }

    /// Apply every state transition the clock already owes at `now_millis`.
    ///
    /// A queued purchase becomes payable at its purchase time, a payable plan
    /// becomes active once its payment has settled, and an active plan retires
    /// when its term ends. Each was owed at that instant whether or not anybody
    /// called, so a reporting operation materialises them before it answers
    /// rather than inventing them.
    pub fn materialize_due(&mut self, now_millis: i64) {
        for plan in &mut self.plans {
            if plan.state == STATE_QUEUED && now_millis >= plan.pending_due_millis {
                plan.state = STATE_PAYMENT_PENDING.to_string();
            }
            if plan.state == STATE_PAYMENT_PENDING && now_millis >= plan.active_due_millis {
                plan.state = STATE_ACTIVE.to_string();
            }
            if matches!(plan.state.as_str(), STATE_PAYMENT_PENDING | STATE_ACTIVE)
                && now_millis >= plan.end_millis
            {
                plan.state = STATE_RETIRED.to_string();
            }
        }
    }
}

/// Build the record for a purchase of `offering`.
///
/// `start_millis` is the purchase time: a purchase dated in the future is
/// *queued* (it has not been paid for yet), one dated now or earlier is
/// `payment-pending` until its payment settles.
#[allow(clippy::too_many_arguments)]
pub fn purchase(
    offering: &Offering,
    savings_plan_id: String,
    commitment_units: i128,
    upfront_payment_units: i128,
    recurring_payment_units: i128,
    purchase_millis: i64,
    now_millis: i64,
    tags: BTreeMap<String, String>,
) -> SavingsPlanRecord {
    let start_millis = truncate_to_second(purchase_millis);
    let queued = start_millis > now_millis;
    SavingsPlanRecord {
        savings_plan_arn: savings_plan_arn(&savings_plan_id),
        savings_plan_id,
        offering_id: offering.offering_id.clone(),
        description: offering.description.clone(),
        start_millis,
        // Saturating throughout: `start_millis` carries a caller-supplied
        // purchase time, so none of this arithmetic may be partial.
        end_millis: start_millis.saturating_add(offering.duration_seconds.saturating_mul(1000)),
        state: if queued {
            STATE_QUEUED.to_string()
        } else {
            STATE_PAYMENT_PENDING.to_string()
        },
        pending_due_millis: start_millis,
        active_due_millis: if queued {
            start_millis.saturating_add(ACTIVATION_DELAY_MILLIS)
        } else {
            now_millis.saturating_add(ACTIVATION_DELAY_MILLIS)
        },
        region: offering.property(PROPERTY_REGION).map(str::to_string),
        ec2_instance_family: offering
            .property(PROPERTY_INSTANCE_FAMILY)
            .map(str::to_string),
        savings_plan_type: offering.plan_type.clone(),
        payment_option: offering.payment_option.clone(),
        product_types: offering.product_types.clone(),
        currency: offering.currency.clone(),
        commitment_units,
        upfront_payment_units,
        recurring_payment_units,
        term_duration_in_seconds: offering.duration_seconds,
        tags,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSavingsplansAwssavingsplanState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSavingsplansAwssavingsplanState>)
}
