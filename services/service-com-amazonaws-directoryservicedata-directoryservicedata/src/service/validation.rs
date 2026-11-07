// SPDX-License-Identifier: CC0-1.0

//! How Directory Service Data WORDS a request the constraint layer refused — the parts of
//! that wording the layer itself cannot state.
//!
//! A constraint refusal is the framework's envelope over one clause per violation, each
//! naming the value that was rejected, the member path it sat at and the requirement it
//! failed. The envelope, the paths and the requirement sentences are the framework's and are
//! never restated here; three halves of that rendering are not the framework's to know, and
//! this module is where each is answered, every one delegated to the shared renderer that
//! owns it:
//!
//! * **The value half of a clause is the value the REQUEST carried, unless the model
//!   WITHHOLDS it.** The framework's walk has no rendering for a COLLECTION — `@length` on
//!   a list is checked on the member before the walk descends into its elements, so what it
//!   stores is a SUMMARY of the size (`<n> items`), text no caller ever sent — and it
//!   ELIDES a long value at a ceiling of its own, behind a marker naming the byte count.
//!   Both are replaced by the value as the request's own rendering of it reads: a list as
//!   `[<element>, …]`
//!   (`service_common::validation_message::quoting_the_submitted_collection`), a scalar as
//!   the bytes submitted (`…::quoting_the_whole_submitted_value`). A member the model marks
//!   `@sensitive` is the one exception and is restored NOWHERE: the walk withheld its value
//!   as the violation was recorded, and the clause this service states is the one the walk
//!   wrote — the member and the requirement, and no value at all.
//! * **A constraint broken by a list's ELEMENTS is stated at the LIST.** The framework
//!   reports one clause per offending element, at `<list>.<position>.member`, naming only the
//!   constraint that element broke; this service states ONE clause at `<list>`, quoting the
//!   whole list and listing every requirement the ELEMENT shape declares — the set a caller
//!   needs to build a value the member admits, which the violations alone cannot carry
//!   because they hold only the constraints that failed
//!   (`…::aws_validation_refusal_over_elements_quoting`, which reads the set off the
//!   operation input schemas the generator attached it to).
//! * **An enumerated type's value set is stated in the order this service PUBLISHES it in**,
//!   which is a property of the enum rather than of the request and is not the order the
//!   model declares its members in ([`PUBLISHED_ENUM_VALUE_SETS`]).
//! * **The clauses of ONE refusal are stated in this service's own fixed order**, by the
//!   member path each is stated at ([`in_stated_clause_order`]) — neither the order the
//!   request wrote its members in nor the order the model declares them.
//!
//! A `@pattern` requirement is quoted as the EXPRESSION a value must satisfy rather than as
//! the model's anchored literal, through the tree's one statement of that rendering
//! (`service_common::validation::unanchored`): the `^`/`$` pair is how Smithy writes a
//! whole-value match, which "must satisfy regular expression pattern" already says. The CHECK
//! always runs the model's pattern as written.
//!
//! Nothing is re-checked and no constraint is re-implemented: the violations come from the
//! framework's own machinery (the protocol's deserializer for an absent `@required` member,
//! `constraints::validate` over the built input) through the tree's shared collecting walk,
//! which also owns every bound on what one clause may quote.

use std::collections::HashMap;

use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_schema::Schema;
use service_common::validation::{refusing_violations_with_mapped_echoes, unanchored};
use service_common::validation_message::{
    aws_validation_refusal_over_elements_quoting, quoting_the_submitted_collection,
    quoting_the_whole_submitted_value,
};
use smithy_server_runtime::{Operation, Refusal, ServerHttpProtocol, Violation, ViolationKind};

/// The order this service states the `GroupType` value set in when it refuses a value
/// outside it.
///
/// The model declares that enum's members `Distribution, Security`
/// (`smithy_query(query="shape", shape="GroupType")`), and the refusal of a value outside it
/// lists the set in THIS order. It is FIXED text: no member of the request and nothing in the
/// store varies it. It holds exactly the values the model declares for that enum, reordered
/// and nothing else, which the `validation_message_rendering` suite pins against
/// `crate::ops::types::GROUP_TYPE_VALUES`.
pub const GROUP_TYPE_VALUE_SET: [&str; 2] = ["Security", "Distribution"];

/// Every enum value set this service publishes an order for.
///
/// One row per enum whose stated order is known to differ from its declaration order; a value
/// set absent from this table is stated as the model declares it, because this service has
/// shown no other order for it.
const PUBLISHED_ENUM_VALUE_SETS: &[&[&str]] = &[&GROUP_TYPE_VALUE_SET];

/// The order this service states `values` in, when `values` is the value set of an enum whose
/// order it publishes ([`PUBLISHED_ENUM_VALUE_SETS`]); `None` for any other value set.
///
/// An enum is recognised by its value SET rather than by the member that carried the rejected
/// value, so every member targeting one enum — of any operation, at any depth — states that
/// enum's set in the one order, and a set this service publishes no order for keeps the order
/// it was presented in. The match is on membership and size, never on position, so a model
/// revision that reordered its own declaration of the same enum would still be stated in the
/// published order.
#[must_use]
pub fn published_enum_value_set(values: &[String]) -> Option<&'static [&'static str]> {
    PUBLISHED_ENUM_VALUE_SETS.iter().copied().find(|published| {
        published.len() == values.len()
            && published
                .iter()
                .all(|value| values.iter().any(|presented| presented == value))
    })
}

/// `violation` with an enumerated type's value set stated in the order this service publishes
/// it in, and any other violation returned unchanged.
///
/// Only the ORDER of the set changes: the clause, the member path, the echoed value and the
/// requirement's wording keep coming from the framework, which is the one place they are
/// written.
fn in_published_enum_order(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return violation.clone();
    };
    let Some(published) = published_enum_value_set(declared) else {
        return violation.clone();
    };
    Violation {
        kind: ViolationKind::EnumValue(
            published.iter().map(|value| (*value).to_owned()).collect(),
        ),
        ..violation.clone()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs — a `@required` member
/// absent, or a value outside a declared constraint — with each clause's value half stating
/// what the caller submitted (except for a member the model WITHHOLDS the value of), and
/// with every list of scalars the request carried, keyed by the member path a clause names
/// that list at.
///
/// Both halves come out of ONE read of the request, the tree's shared collecting walk
/// (`service_common::validation::refusing_violations_with_mapped_echoes`): the violations are
/// the framework's own, and the lists are the value a clause stated AT a list member quotes,
/// which no violation carries. Empty when the request satisfies the model, and empty for a
/// failure that is NOT a refusal — a body the codec cannot read is a serialization failure,
/// which this service's request layer answers before this walk is reached.
#[must_use]
pub fn refusing_request<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> (Vec<Violation>, HashMap<String, String>) {
    let (refused, lists) = refusing_violations_with_mapped_echoes::<O>(protocol, req);
    let violations = refused
        .into_iter()
        .map(|(violation, echo)| {
            // A member the model marks `@sensitive` keeps the clause the walk wrote, which
            // names the member and the requirement and states NO value: the value is the
            // one thing such a clause must not disclose, so neither restoring rule below
            // is asked about it and the request's bytes never leave the walk.
            if violation.sensitive {
                return violation;
            }
            // A collection's clause states the collection, a scalar's the bytes submitted;
            // each rule touches only its own clause, so the two compose rather than overlap.
            let quoted = quoting_the_submitted_collection(violation, echo.clone());
            quoting_the_whole_submitted_value(quoted, &echo)
        })
        .collect();
    (violations, lists)
}

/// The order this service states the clauses of ONE refusal in: by the member path each
/// clause is stated at, in lexicographic order.
///
/// A request that breaks the constraints of two members is refused with both clauses in one
/// message, and which clause comes first is a property of the SERVICE: it is neither the
/// order the request wrote its members in nor the order the model declares them, so one
/// request always reads back one message however a client happened to serialize it.
///
/// Only the ORDER changes: each clause, its member path, its echoed value and its
/// requirement keep coming from the framework. The sort is STABLE, so two clauses about ONE
/// member keep the order the framework's walk collected them in — which of a member's
/// requirements is reported first is the walk's to say and not this module's — and the walk
/// hands over at most its own ceiling of violations, so the work is bounded by the tree's
/// shared walk rather than by one request.
fn in_stated_clause_order(violations: &[Violation]) -> Vec<Violation> {
    let mut ordered = violations.to_vec();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    ordered
}

/// The prose this service states for `violations`: how many of the request's values failed,
/// then one clause per failure, in the renderings this module's docs list. `None` when there
/// is nothing to refuse.
///
/// `input_schemas` are the operation input shapes the ELEMENT constraint sets are read off —
/// the schemas of the operation that was called, or every one this service serves where the
/// refusal does not say which. `quoted_collection` answers, for the member path a clause is
/// stated at, the collection that member held; `None` from it states the clause with no value,
/// which is the fail-safe direction.
#[must_use]
pub fn refusal_message(
    violations: &[Violation],
    input_schemas: &[&'static Schema],
    quoted_collection: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let stated: Vec<Violation> = in_stated_clause_order(violations)
        .iter()
        .map(in_published_enum_order)
        .collect();
    aws_validation_refusal_over_elements_quoting(
        &Refusal::Invalid(stated),
        input_schemas,
        unanchored,
        quoted_collection,
    )
    .map(|refused| refused.message)
}
