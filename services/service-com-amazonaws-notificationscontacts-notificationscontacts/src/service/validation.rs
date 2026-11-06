// SPDX-License-Identifier: CC0-1.0

//! How this service states the framework's boundary refusal of a request.
//!
//! The words are NOT written here. `service_common::validation_message` is the
//! cross-service home for them — the envelope, the member paths, the requirements, the
//! unanchored `@pattern` expression and the schema layer's property-count finding — and
//! this module only makes the two choices that belong to a service: WHICH of those
//! renderings this service's recordings show, and in which order they are asked.
//!
//! The dialect those recordings show:
//!
//! * a MAP carrying more entries than its modelled maximum is a refusal of the request's
//!   SHAPE against the schema, stated as that layer's property-count finding and asked
//!   FIRST, because the members of a request whose shape that layer already refuses are
//!   never reported;
//! * every clause of the per-member envelope states the member path and the requirement
//!   broken and NO submitted value — whichever member and whichever constraint class — so
//!   the decision is made about the violation rather than about a set of member paths. A
//!   member that was not there at all has no value to withhold and keeps the `null` the
//!   framework states an absent member with.
//!
//! Nothing here is keyed to an operation or to a submitted value, so one rule covers every
//! constrained member of every operation of this service; the `@pattern`s this service has
//! to apply ITSELF (`crate::state::pattern_validation_failure`) are worded through the
//! same renderings, so a constraint the walk applies and one a handler applies read alike.

use smithy_server_runtime::{Refusal, Refused};

/// How this service states `refusal`, or `None` to keep the framework's own wording.
///
/// `None` covers both what this service does not reword (an authentication refusal is not
/// an input-constraint failure, and its code and status are not this service's to change)
/// and a refusal with nothing to state.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    if let Some(refused) = service_common::validation_message::property_count_refusal(refusal) {
        return Some(refused);
    }
    service_common::validation_message::aws_validation_refusal_withholding_violations(
        refusal,
        |violation| violation.value.is_some() || violation.sensitive,
    )
}
