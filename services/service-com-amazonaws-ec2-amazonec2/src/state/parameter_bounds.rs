// SPDX-License-Identifier: CC0-1.0

//! The published parameter bounds this service has to enforce by hand, and the sentences
//! EC2 refuses a request that violates one with.
//!
//! EC2 models no error shapes and puts no `@range` on most of the members it publishes a
//! valid range for, so both the bound and its refusal live in the handlers. A refusal is
//! part of the contract a caller programs against — a client that reads the limit out of
//! the message, or branches on the code, has to see what the real service says — so the
//! sentence a handler answers is EC2's own for that violation CLASS, carrying the bound
//! that operation publishes, never a description of the limit composed here.
//!
//! One definition per class, so two operations cannot word the same refusal differently.

use super::*;

/// The smallest page an operation that publishes no larger minimum will serve: one item.
/// A page of NONE is not a page, so zero and below is refused whatever else an operation
/// publishes.
pub const SMALLEST_PAGE_SIZE: i32 = 1;

/// The name EC2 reports a `Filters` entry that names no filter at all by.
///
/// An entry carrying values but no `Name` is still an entry the operation cannot use, and
/// the refusal has to say WHICH entry: EC2 names the absent one rather than quoting an
/// empty name, so an operator can see what the request actually sent.
pub const UNNAMED_FILTER: &str = "null";

/// How EC2 refuses a page size below the smallest one an operation publishes: HTTP 400
/// `InvalidParameterValue`, in the sentence [`max_results_below_bound_message`] words —
/// the value the request sent, in spaced parentheses, and the minimum that operation's
/// `MaxResults` publishes.
///
/// The one constructor every hand-enforced page-size floor of this crate answers through,
/// so the code and the wording of that refusal cannot drift between operations. Both
/// variable parts come from the request and from the operation's own published minimum, so
/// an operation whose minimum differs is refused with its own bound rather than with a
/// number copied from another.
pub fn max_results_below_published_minimum<E>(
    value: i32,
    minimum: i32,
) -> crate::scaffolded::Scaffolded<E> {
    invalid_parameter_value(max_results_below_bound_message(
        &value.to_string(),
        &minimum.to_string(),
    ))
}

/// How EC2 refuses a page size outside the range an operation publishes: the two bounds of
/// that range and nothing of the request.
///
/// FIXED text apart from the bounds — the sentence states the whole rule rather than the
/// value at fault, so a caller reads the range it has to stay inside off any one refusal.
pub fn max_results_outside_published_range<E>(
    minimum: i32,
    maximum: i32,
) -> crate::scaffolded::Scaffolded<E> {
    invalid_parameter_value(format!(
        "Maximum results allowed are between {minimum} and {maximum}"
    ))
}

/// How EC2 refuses an instance count outside the range the action publishes: the absolute
/// range, stated as the rule, with nothing of the request in it.
///
/// What the account or a Capacity Reservation can currently SATISFY is a different refusal:
/// this one is about the parameter's own published range, so it is answered before any
/// capacity is looked at.
pub fn instance_count_outside_published_range<E>(
    range: &::std::ops::RangeInclusive<i32>,
) -> crate::scaffolded::Scaffolded<E> {
    invalid_parameter_value(format!(
        "The instance count isn't valid. Enter a value between {} and {} instances, \
         and try again",
        range.start(),
        range.end(),
    ))
}

/// How EC2 refuses a request whose `MaxCount` is below its `MinCount`: one fixed sentence,
/// naming neither count.
///
/// The rule is about the two members' ORDER, not about either value, so the sentence names
/// no value — a caller that sent the pair already knows both.
pub fn maximum_instance_count_below_minimum<E>() -> crate::scaffolded::Scaffolded<E> {
    invalid_parameter_value("Maximum instance count must not be smaller than minimum instance count")
}

/// How EC2 refuses a parameter whose value has to DIFFER from another parameter's: HTTP 400
/// `InvalidParameterValue`, naming the value the request sent, the parameter it sent it for
/// and the parameter whose value it may not repeat.
///
/// FIXED text apart from those three. Both names are the names the REQUEST uses for those
/// members, so an operation whose pair is spelled another way is refused with its own names
/// rather than with a pair written down here, and one definition serves every operation that
/// publishes such a pair — two of them cannot word this refusal differently. The rule is
/// about the request's own members, so a handler answers it before resolving either value
/// against the store. The value is echoed through [`echoed_resource_id`], so naming it cannot
/// amplify a request into an arbitrarily long response.
pub fn parameter_must_differ_from<E>(
    parameter: &str,
    value: &str,
    other_parameter: &str,
) -> crate::scaffolded::Scaffolded<E> {
    invalid_parameter_value(format!(
        "Invalid value {value} for {parameter}. The value of {parameter} needs to be \
         different to {other_parameter}",
        value = echoed_resource_id(value),
    ))
}

/// How EC2 refuses an `attribute` parameter naming a value the action cannot act on: the
/// value the request sent and the fixed sentence that says the service knows no such
/// attribute.
///
/// It does NOT enumerate the attributes the action does accept: a caller reads those from
/// the API reference, and an enumeration composed here would be this service's own
/// description of the rule rather than EC2's answer. One definition for every operation
/// that narrows an `*AttributeName` enum to the subset it serves. The value is echoed
/// through [`echoed_resource_id`], so naming it cannot amplify a request into an
/// arbitrarily long response.
pub fn unknown_attribute_parameter_value<E>(attribute: &str) -> crate::scaffolded::Scaffolded<E> {
    invalid_parameter_value(format!(
        "Value ({}) for parameter attribute is invalid. Unknown attribute.",
        echoed_resource_id(attribute)
    ))
}

/// How EC2 refuses a request that names more than one attribute type at once: the fixed
/// prefix and the attributes the REQUEST named, in the order the request's own members are
/// declared.
///
/// A request carrying two attributes is refused for naming two of them, whichever two they
/// are, so the names are taken from the request rather than from a pair written down here.
pub fn fields_for_multiple_attribute_types<E>(
    attributes: &[&str],
) -> crate::scaffolded::Scaffolded<E> {
    invalid_parameter_combination(format!(
        "Fields for multiple attribute types specified: {}",
        attributes.join(", ")
    ))
}
