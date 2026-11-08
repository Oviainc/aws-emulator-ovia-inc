// SPDX-License-Identifier: CC0-1.0

//! Whether a billing period range a request carries is ordered, modifiable and starts in
//! an open bill — the checks every operation that takes such a range makes.

use super::*;

/// Does a half-open billing-period range — an INCLUSIVE start bound and an EXCLUSIVE
/// end bound, either of which may be absent and so open — cover at least one billing
/// period?
///
/// The start is in the range and the end is not, so an end at or before the start covers
/// no billing period at all. The bounds are the STRINGS a request carried, parsed through
/// [`billing_period_key`] so two spellings of one month are one period, and ordered by
/// the crate's one half-open comparison, [`billing_period_precedes`].
pub fn half_open_billing_period_range_is_ordered(
    inclusive_start: Option<&str>,
    exclusive_end: Option<&str>,
) -> bool {
    billing_period_precedes(
        inclusive_start.and_then(billing_period_key),
        exclusive_end.and_then(billing_period_key),
    )
}

/// Does a half-open billing-period range cover a non-empty span of billing periods — or
/// is that the refusal it earns?
///
/// The rule is [`half_open_billing_period_range_is_ordered`], and
/// `exclusive_end_member` is the member path of the end bound in the shape that carried
/// it, so every spelling of the one range — a write's
/// `InclusiveStartBillingPeriod`/`ExclusiveEndBillingPeriod` and a filter's
/// inclusive-start/exclusive-end pair alike — earns the same refusal naming its own
/// member.
pub fn check_half_open_billing_period_range_is_ordered(
    inclusive_start: Option<&str>,
    exclusive_end: Option<&str>,
    exclusive_end_member: &'static str,
) -> Result<(), ValidationRefusal> {
    if half_open_billing_period_range_is_ordered(inclusive_start, exclusive_end) {
        return Ok(());
    }
    Err((
        "ILLEGAL_BILLING_PERIOD_RANGE",
        exclusive_end_member,
        "the exclusive end billing period must be later than the inclusive start billing \
         period"
            .to_string(),
    ))
}

/// Is a `CustomLineItemBillingPeriodRange` ordered, i.e. does it describe a non-empty
/// span of billing periods?
///
/// The member names carry the rule: `InclusiveStartBillingPeriod` is in the range and
/// `ExclusiveEndBillingPeriod` is not, so an end equal to or earlier than the start
/// would cover no billing period at all. A range with no end is open and always
/// ordered.
pub fn billing_period_range_is_ordered(
    range: &crate::ops::types::CustomLineItemBillingPeriodRange,
) -> bool {
    half_open_billing_period_range_is_ordered(
        Some(range.inclusive_start_billing_period.as_str()),
        range.exclusive_end_billing_period.as_deref(),
    )
}

/// Does `range` describe a non-empty span of billing periods — or is that the refusal
/// it earns?
///
/// The rule is [`billing_period_range_is_ordered`]: the end bound is exclusive, so a
/// range whose end is at or before its inclusive start covers no billing period, which
/// `ValidationExceptionReason` enumerates `ILLEGAL_BILLING_PERIOD_RANGE` for.
///
/// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
/// the calling operation refuses the request with, so every operation taking that shape
/// answers an empty range the same way.
pub fn check_billing_period_range_is_ordered(
    range: &crate::ops::types::CustomLineItemBillingPeriodRange,
) -> Result<(), ValidationRefusal> {
    check_half_open_billing_period_range_is_ordered(
        Some(range.inclusive_start_billing_period.as_str()),
        range.exclusive_end_billing_period.as_deref(),
        "BillingPeriodRange.ExclusiveEndBillingPeriod",
    )
}

/// How many billing periods back a request may still apply a change to, counted from
/// the current one: the PREVIOUS billing period is the oldest, which is what the
/// operations bounded to it document ("in the current, or previous billing period").
pub(super) const MODIFIABLE_BILLING_PERIODS_BACK: i64 = 1;

/// Is `billing_period` one of the periods such an operation may apply a change to —
/// the CURRENT billing period or the one before it?
///
/// A period older than that belongs to a bill that is already closed, and one later
/// than the current period to a bill that does not exist yet; neither is the current
/// or the previous billing period. A value that is not a billing period at all
/// ([`billing_period_key`] reads none) is not one of them either.
pub fn billing_period_is_modifiable(billing_period: &str) -> bool {
    let Some(wanted) = billing_period_key(billing_period) else {
        return false;
    };
    let Some(current) = billing_period_key(&current_billing_period()) else {
        return false;
    };
    (current - MODIFIABLE_BILLING_PERIODS_BACK..=current).contains(&wanted)
}

/// Is the bill of `billing_period` still OPEN — has the period not been billed yet, or
/// is it one of the two the bill is still being assembled for?
///
/// The bills of the periods before the PREVIOUS billing period are issued and closed,
/// and a closed bill does not change: what a custom line item charged a period whose
/// invoice went out is what it keeps charging it. The periods from the previous one on,
/// the current one and every period that has not been billed at all included, have no
/// issued bill, so a change may still take effect in them. A value that is not a billing
/// period at all ([`billing_period_key`] reads none) names no open bill either.
pub fn billing_period_bill_is_open(billing_period: &str) -> bool {
    let Some(wanted) = billing_period_key(billing_period) else {
        return false;
    };
    let Some(current) = billing_period_key(&current_billing_period()) else {
        return false;
    };
    wanted >= current - MODIFIABLE_BILLING_PERIODS_BACK
}

/// The refusal a range earns whose `InclusiveStartBillingPeriod` — the period from which
/// the request takes effect — names a billing period the calling operation may not act
/// in.
///
/// `ValidationExceptionReason` enumerates both the reason for a value that is no billing
/// period at all (`ILLEGAL_BILLING_PERIOD`) and the reason for a real period the
/// operation may not act in (`INVALID_BILLING_PERIOD_FOR_OPERATION`), so the class of the
/// value decides which of the two the request is refused with. ONE wording, built here,
/// so the operations that bound the start of this shape answer a start they may not act
/// in the same way whichever bound each of them applies.
pub(super) fn billing_period_range_start_refusal(start: &str) -> ValidationRefusal {
    let member = "BillingPeriodRange.InclusiveStartBillingPeriod";
    if billing_period_key(start).is_none() {
        return (
            "ILLEGAL_BILLING_PERIOD",
            member,
            format!("{start} is not a billing period"),
        );
    }
    let current = current_billing_period();
    (
        "INVALID_BILLING_PERIOD_FOR_OPERATION",
        member,
        format!(
            "billing period {start} is neither the current billing period ({current}) nor the \
             previous one"
        ),
    )
}

/// May a request carrying `range` be applied by an operation the model documents as
/// acting in the current, or previous, billing period — or is that the refusal it
/// earns?
///
/// `InclusiveStartBillingPeriod` is the period from which the request takes effect, so
/// it is the bound this window judges: a start before the previous billing period
/// restates a closed bill and a start after the current one a bill that does not exist
/// yet, while the exclusive end only says how far forward the change reaches, exactly
/// as an absent end (the open future) does.
///
/// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
/// the calling operation refuses the request with
/// ([`billing_period_range_start_refusal`]), so every operation bounded to that window
/// answers an out-of-window range the same way.
pub fn check_billing_period_range_is_modifiable(
    range: &crate::ops::types::CustomLineItemBillingPeriodRange,
) -> Result<(), ValidationRefusal> {
    let start = &range.inclusive_start_billing_period;
    if billing_period_is_modifiable(start) {
        return Ok(());
    }
    Err(billing_period_range_start_refusal(start))
}

/// May a request carrying `range` be applied by an operation that writes the billing
/// periods the range names without the model bounding it to one window of them — or is
/// that the refusal it earns?
///
/// `InclusiveStartBillingPeriod` is the period from which the request takes effect, so a
/// start whose bill is already closed would restate an issued invoice, which no write of
/// this store does ([`billing_period_bill_is_open`]). The periods from the previous one
/// on are open, so how far FORWARD the range reaches is the request's to choose, exactly
/// as an absent end (the open future) is — the bound an operation the model documents as
/// acting "in the current, or previous billing period" applies to its own start on top of
/// this one is [`check_billing_period_range_is_modifiable`].
///
/// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
/// the calling operation refuses the request with
/// ([`billing_period_range_start_refusal`]), so a start in a closed bill is answered the
/// same way whichever operation took the range.
pub fn check_billing_period_range_starts_in_an_open_bill(
    range: &crate::ops::types::CustomLineItemBillingPeriodRange,
) -> Result<(), ValidationRefusal> {
    let start = &range.inclusive_start_billing_period;
    if billing_period_bill_is_open(start) {
        return Ok(());
    }
    Err(billing_period_range_start_refusal(start))
}
