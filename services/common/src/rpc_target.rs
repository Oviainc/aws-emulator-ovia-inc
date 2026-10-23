// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of an `awsJson` `X-Amz-Target`: which SERVICE it names, and which
//! OPERATION of that service.
//!
//! On the `awsJson1_0` / `awsJson1_1` protocols a request names its operation out of
//! band, in `x-amz-target: <target prefix>.<OperationName>`. The target prefix is OPAQUE
//! text an SDK copies out of its service model: for most services it is the service
//! shape's local name (`DynamoDB_20120810`, `Logs_20140328`), but a number of services
//! publish the FULLY QUALIFIED Coral service name, which carries dots of its own
//! (`com.amazonaws.codeconnections.CodeConnections_20231201`,
//! `com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101`). A Smithy shape name is an
//! identifier (the `smithy.api#Identifier` production: ASCII letters, digits and `_`) and
//! so can never contain a dot, which is why
//!
//! * the OPERATION a target names is the segment after its LAST dot, and
//! * the SERVICE is the last dot-separated segment of what precedes that,
//!
//! one rule that answers the same thing for a single-identifier prefix as for a dotted
//! one, and is therefore the rule for every `awsJson` service rather than for the dotted
//! few.
//!
//! The framework's own readers split at the FIRST dot instead —
//! `aws_smithy_schema_server::rpc::routing_key_from_x_amz_target` for the operation
//! routing key, `emulator_server::service_from_target` for the routing key of a request
//! that carries no SigV4 credential scope — which is correct only while the prefix is a
//! single identifier. For a dotted prefix they read a namespace label (`com`) as the
//! service and the whole `<namespace tail>.<Service>.<Operation>` as the operation, so no
//! service answers to the name and no modelled operation is wired under the key: every
//! call to such a service is refused before any handler runs. Those readers live under
//! `vendor/`, which is read-only here, so this module states the rule once on the
//! emulator side:
//!
//! * [`names_service`] is the SERVICE half, which [`crate::claims_request`] reads instead
//!   of the framework's first-dot `service_from_target`, so every service claiming
//!   through this crate claims its own dotted-prefix traffic;
//! * [`operation_for_service`] / [`requested_operation`] / [`target_operation_of`] are the
//!   OPERATION half, which a service whose published prefix is dotted resolves its own
//!   targets with through the `Service::override_operation` seam, and recognises its own
//!   unsigned traffic with through `Service::can_handle`.
//!
//! Both halves read the same SEGMENT of the target — that is the whole fix — but they
//! compare it against the service's declared name with deliberately different strictness,
//! and the asymmetry is worth stating because it decides what a loosely spelled prefix
//! gets:
//!
//! * the CLAIM half is the framework's own routing-key comparison (trailing `_<digits>`
//!   API-version suffix dropped, case-insensitive). It has to be: the framework resolves
//!   a routing key that way, so a service that claimed more strictly would leave requests
//!   the router already matched to its endpoint with no claimant at all.
//! * the DISPATCH half matches the shape name EXACTLY as the model spells it, because its
//!   answer indexes that service's generated routing table, whose keys are the model's own
//!   shape names.
//!
//! So a target whose service segment is spelled loosely — the version suffix dropped, or
//! cased differently — is CLAIMED by the service and then resolves no operation here. That
//! is not a request served by the wrong handler: it falls through to the framework's own
//! routing, which answers it from the operation name when the prefix is a single
//! identifier (`CodeConnections.GetHost` -> `GetHost`, unchanged by this module) and
//! otherwise renders the protocol's routing refusal. Claimed-then-refused is the honest
//! answer for a prefix no model declares, and `the_two_halves_agree_on_every_prefix_form`
//! pins the agreement for every prefix form spelled as a model spells it, while
//! `a_loosely_spelled_prefix_is_claimed_and_then_refused` pins the asymmetry itself.
//!
//! Delete the per-service seams (and keep this module for the claim half) if
//! `routing_key_from_x_amz_target` ever resolves the operation from the last dot itself.

use crate::{shape_local_name, strip_version_suffix};
use aws_smithy_runtime_api::http::Request as RuntimeApiRequest;

/// The header an `awsJson` client names its operation in.
pub const TARGET_HEADER: &str = "x-amz-target";

/// The OPERATION an `X-Amz-Target` names: the segment after its LAST dot.
///
/// `None` for a target that names no operation at all — one with no dot, or with nothing
/// after its last dot. Surrounding whitespace is ignored, as the framework's own routing
/// keys ignore it.
#[must_use]
pub fn operation_in_target(target: &str) -> Option<&str> {
    let (_prefix, operation) = target.trim().rsplit_once('.')?;
    (!operation.is_empty()).then_some(operation)
}

/// The SERVICE an `X-Amz-Target` names: the last dot-separated segment of its prefix.
///
/// For a dotted prefix that is the service shape's local name with its namespace written
/// out (`com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.GetTrail` ->
/// `CloudTrail_20131101`); for a single-identifier prefix it is the prefix itself
/// (`DynamoDB_20120810.GetItem` -> `DynamoDB_20120810`).
///
/// A target with NO dot names no operation, so there is no prefix to split: the whole
/// value is read as the service, which is what the framework's own
/// `emulator_server::service_from_target` reads out of it. `None` only when the segment
/// that would name the service is empty.
#[must_use]
pub fn service_in_target(target: &str) -> Option<&str> {
    let target = target.trim();
    let prefix = match target.rsplit_once('.') {
        Some((prefix, _operation)) => prefix,
        None => target,
    };
    let service = prefix.rsplit_once('.').map_or(prefix, |(_, last)| last);
    (!service.is_empty()).then_some(service)
}

/// Whether `target` names the service whose shape id (or shape-local name) is
/// `service_shape_id` — the CLAIM half.
///
/// The comparison is the one the framework makes on a routing key: the trailing
/// `_<digits>` API-version suffix is dropped and what is left is compared
/// case-insensitively, so a client sending the versioned or the unversioned form, in any
/// casing, names the same service — exactly as `emulator_server::service_from_target`
/// does for a single-identifier prefix. Only WHICH segment is read differs, and that is
/// the defect this module exists for.
///
/// It is deliberately LOOSER than [`operation_for_service`] (see this module's doc): the
/// router resolves its routing key with this comparison, so claiming more strictly would
/// leave a request the router matched to this endpoint with no claimant.
#[must_use]
pub fn names_service(target: &str, service_shape_id: &str) -> bool {
    let declared = strip_version_suffix(shape_local_name(service_shape_id).trim());
    service_in_target(target)
        .is_some_and(|named| strip_version_suffix(named).eq_ignore_ascii_case(declared))
}

/// The operation `target` names FOR the service whose shape id (or shape-local name) is
/// `service_shape_id` — the DISPATCH half; `None` when the target names another service,
/// or names no operation.
///
/// Unlike [`names_service`], the service name is matched EXACTLY as the model spells it:
/// this answer picks an entry out of that service's generated routing table, and the
/// names in that table are the model's own shape names. A loosely spelled service segment
/// (version suffix dropped, or differently cased) is therefore claimed by the service and
/// answered `None` here, which leaves the request to the framework's own routing rather
/// than to a handler this rule picked for a prefix no model declares — see this module's
/// doc for why the two strictnesses differ.
#[must_use]
pub fn operation_for_service<'target>(
    target: &'target str,
    service_shape_id: &str,
) -> Option<&'target str> {
    let declared = shape_local_name(service_shape_id).trim();
    if service_in_target(target)? != declared {
        return None;
    }
    operation_in_target(target)
}

/// [`operation_for_service`] over an `X-Amz-Target` read off `headers` — the
/// `Service::can_handle` side, which is handed the request's parts.
#[must_use]
pub fn target_operation_of<'headers>(
    headers: &'headers http::HeaderMap,
    service_shape_id: &str,
) -> Option<&'headers str> {
    let target = headers.get(TARGET_HEADER)?.to_str().ok()?;
    operation_for_service(target, service_shape_id)
}

/// [`operation_for_service`] over an `X-Amz-Target` read off `request` — the
/// `Service::override_operation` side, which is handed the whole runtime-API request.
///
/// The returned name is looked up in the service's OWN generated routing table by the
/// caller (`self.entry.routing_table().get(operation)`), so a target naming an operation
/// the model does not bind still falls through to the framework's routing refusal rather
/// than reaching a handler it was not meant to.
#[must_use]
pub fn requested_operation<'request>(
    request: &'request RuntimeApiRequest,
    service_shape_id: &str,
) -> Option<&'request str> {
    operation_for_service(request.headers().get(TARGET_HEADER)?, service_shape_id)
}

#[cfg(test)]
mod tests {
    //! The target rule over the prefix forms a client can write, for two services whose
    //! published prefixes differ in shape: one whose SDK prefix is the fully qualified
    //! Coral name with a version label in it, and one whose prefix is its absolute shape
    //! id. Every row holds for both halves of the rule, so the service a target is
    //! claimed by is the service its operation is resolved against.

    use super::*;

    /// A service whose published prefix is the fully qualified Coral name
    /// (`com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101`), shape id from
    /// `smithy_query(query="service_info")` on its model.
    const CLOUDTRAIL: &str = "com.amazonaws.cloudtrail#CloudTrail_20131101";

    /// A service whose published prefix is its absolute shape id
    /// (`com.amazonaws.codeconnections.CodeConnections_20231201`), shape id from
    /// `smithy_query(query="service_info")` on its model.
    const CODECONNECTIONS: &str = "com.amazonaws.codeconnections#CodeConnections_20231201";

    /// A service whose published prefix carries no dot at all, the form the framework's
    /// own first-dot reader was written for.
    const DYNAMODB: &str = "com.amazonaws.dynamodb#DynamoDB_20120810";

    #[test]
    fn the_operation_is_the_segment_after_the_last_dot() {
        // (case, target, the operation it names at all)
        let rows: [(&str, &str, Option<&str>); 10] = [
            (
                "the fully qualified Coral prefix an SDK sends",
                "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateTrail",
                Some("CreateTrail"),
            ),
            (
                "an absolute shape id as the prefix",
                "com.amazonaws.codeconnections.CodeConnections_20231201.DeleteRepositoryLink",
                Some("DeleteRepositoryLink"),
            ),
            ("a single-identifier prefix", "DynamoDB_20120810.GetItem", Some("GetItem")),
            ("one namespace label", "cloudtrail.CloudTrail_20131101.ListTrails", Some("ListTrails")),
            (
                "a deeper namespace than any SDK sends",
                "a.b.c.d.e.CloudTrail_20131101.GetTrailStatus",
                Some("GetTrailStatus"),
            ),
            ("surrounding whitespace", " DynamoDB_20120810.Query ", Some("Query")),
            // An operation name is a Smithy identifier: letters, digits and `_`.
            ("an operation name carrying digits", "DynamoDB_20120810.Query2", Some("Query2")),
            (
                "an operation name carrying an underscore",
                "DynamoDB_20120810.Get_Item",
                Some("Get_Item"),
            ),
            ("no dot at all", "GetItem", None),
            ("an empty operation", "DynamoDB_20120810.", None),
        ];

        for (case, target, expected) in rows {
            let actual = operation_in_target(target);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?} from target {target:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn the_service_is_the_last_segment_of_the_prefix() {
        // (case, target, the service it names)
        let rows: [(&str, &str, Option<&str>); 8] = [
            (
                "the fully qualified Coral prefix an SDK sends",
                "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateTrail",
                Some("CloudTrail_20131101"),
            ),
            (
                "an absolute shape id as the prefix",
                "com.amazonaws.codeconnections.CodeConnections_20231201.GetHost",
                Some("CodeConnections_20231201"),
            ),
            ("a single-identifier prefix", "DynamoDB_20120810.GetItem", Some("DynamoDB_20120810")),
            ("one namespace label", "cloudtrail.CloudTrail_20131101.ListTrails", Some("CloudTrail_20131101")),
            // No operation segment to strip: the value itself is all there is to read,
            // which is what the framework's own reader takes out of it.
            ("no dot at all", "DynamoDB_20120810", Some("DynamoDB_20120810")),
            ("a prefix with nothing after its last dot", "CloudTrail_20131101.", Some("CloudTrail_20131101")),
            ("an empty prefix", ".GetItem", None),
            ("an empty target", "", None),
        ];

        for (case, target, expected) in rows {
            let actual = service_in_target(target);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?} from target {target:?}, got {actual:?}"
            );
        }
    }

    /// The operation half is resolved only for the service the target names, and the
    /// shape name is matched as the model spells it: that answer indexes a routing table
    /// of model-spelled operation names.
    #[test]
    fn an_operation_is_resolved_only_for_the_service_the_target_names() {
        // (case, target, shape id, the operation it names for that service)
        let rows: [(&str, &str, &str, Option<&str>); 9] = [
            (
                "its own SDK prefix",
                "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateTrail",
                CLOUDTRAIL,
                Some("CreateTrail"),
            ),
            (
                "its own bare shape name",
                "CloudTrail_20131101.CreateTrail",
                CLOUDTRAIL,
                Some("CreateTrail"),
            ),
            (
                "its own absolute shape id as the prefix",
                "com.amazonaws.codeconnections.CodeConnections_20231201.DeleteRepositoryLink",
                CODECONNECTIONS,
                Some("DeleteRepositoryLink"),
            ),
            (
                "a single-identifier prefix, the form the framework already routed",
                "DynamoDB_20120810.GetItem",
                DYNAMODB,
                Some("GetItem"),
            ),
            (
                "an operation the model does not bind is still read as an operation",
                "CloudTrail_20131101.NoSuchOperation",
                CLOUDTRAIL,
                Some("NoSuchOperation"),
            ),
            (
                "another service's prefix",
                "DynamoDB_20120810.GetItem",
                CLOUDTRAIL,
                None,
            ),
            (
                "a prefix that merely ends in this shape name's characters",
                "NotCloudTrail_20131101.CreateTrail",
                CLOUDTRAIL,
                None,
            ),
            (
                // The shape name is matched as the model spells it: a differently cased
                // label names a shape no model declares.
                "the shape name lower-cased",
                "cloudtrail_20131101.CreateTrail",
                CLOUDTRAIL,
                None,
            ),
            ("no operation segment", "CloudTrail_20131101.", CLOUDTRAIL, None),
        ];

        for (case, target, shape_id, expected) in rows {
            let actual = operation_for_service(target, shape_id);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?} from target {target:?} for {shape_id}, \
                 got {actual:?}"
            );
        }
    }

    /// The service half is the framework's own routing-key comparison — version suffix
    /// dropped, case-insensitive — asked of the RIGHT segment.
    #[test]
    fn a_target_names_its_service_however_the_prefix_is_written() {
        // (case, target, shape id, expected)
        let rows: [(&str, &str, &str, bool); 9] = [
            (
                "the dotted prefix an SDK sends",
                "com.amazonaws.codeconnections.CodeConnections_20231201.GetHost",
                CODECONNECTIONS,
                true,
            ),
            (
                "the bare shape name",
                "CodeConnections_20231201.GetHost",
                CODECONNECTIONS,
                true,
            ),
            (
                "the version suffix dropped by the client",
                "CodeConnections.GetHost",
                CODECONNECTIONS,
                true,
            ),
            (
                "a lower-cased prefix",
                "com.amazonaws.codeconnections.codeconnections_20231201.GetHost",
                CODECONNECTIONS,
                true,
            ),
            (
                "a single-identifier prefix, unchanged by this rule",
                "DynamoDB_20120810.GetItem",
                DYNAMODB,
                true,
            ),
            ("another service's prefix", "AWSKMS.Encrypt", DYNAMODB, false),
            (
                "another service's dotted prefix",
                "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateTrail",
                CODECONNECTIONS,
                false,
            ),
            (
                "a prefix that merely ends in this shape name's characters",
                "NotCodeConnections_20231201.GetHost",
                CODECONNECTIONS,
                false,
            ),
            ("an empty target", "", CODECONNECTIONS, false),
        ];

        for (case, target, shape_id, expected) in rows {
            let actual = names_service(target, shape_id);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected names_service({target:?}, {shape_id}) == {expected}, \
                 got {actual}"
            );
        }
    }

    /// Both halves read the same target: for a prefix spelled as a model spells its shape
    /// name, whatever service a target is claimed by is the service the operation after
    /// the last dot is resolved for.
    #[test]
    fn the_two_halves_agree_on_every_prefix_form() {
        const SHAPES: [&str; 3] = [CLOUDTRAIL, CODECONNECTIONS, DYNAMODB];
        // Every prefix form, each paired with the operation written after it.
        let targets = [
            "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateTrail",
            "CloudTrail_20131101.CreateTrail",
            "com.amazonaws.codeconnections.CodeConnections_20231201.GetHost",
            "CodeConnections_20231201.GetHost",
            "DynamoDB_20120810.GetItem",
            "a.b.c.DynamoDB_20120810.GetItem",
        ];

        for target in targets {
            let operation = operation_in_target(target);
            let claimants: Vec<&str> = SHAPES
                .iter()
                .copied()
                .filter(|shape| names_service(target, shape))
                .collect();
            assert_eq!(
                claimants.len(),
                1,
                "case {target:?}: exactly one service may claim a target, got {claimants:?}"
            );
            let resolved = operation_for_service(target, claimants[0]);
            assert_eq!(
                resolved, operation,
                "case {target:?}: the claimant {} must resolve the operation the target \
                 names ({operation:?}), got {resolved:?}",
                claimants[0]
            );
        }
    }

    /// The ONE asymmetry between the halves, pinned rather than left to be discovered: a
    /// service segment spelled loosely — the API-version suffix dropped, or cased
    /// differently from the model — is CLAIMED (the framework resolves its routing key
    /// that way, so the endpoint must answer for it) and resolves NO operation (the
    /// answer would index a routing table keyed by the model's own names).
    ///
    /// Such a request is then left to the framework's own routing, which answers it from
    /// the operation name alone for a single-identifier prefix and renders the protocol's
    /// routing refusal for a dotted one — never a handler chosen for a prefix no model
    /// declares. `tests/all/target_prefix_forms.rs` in the CodeConnections crate asserts
    /// both of those answers on the wire.
    #[test]
    fn a_loosely_spelled_prefix_is_claimed_and_then_refused() {
        // (case, target, shape id)
        let rows: [(&str, &str, &str); 4] = [
            (
                "the version suffix dropped, single-identifier prefix",
                "CodeConnections.GetHost",
                CODECONNECTIONS,
            ),
            (
                "the version suffix dropped, dotted prefix",
                "com.amazonaws.codeconnections.CodeConnections.GetHost",
                CODECONNECTIONS,
            ),
            (
                "the shape name lower-cased, single-identifier prefix",
                "cloudtrail_20131101.CreateTrail",
                CLOUDTRAIL,
            ),
            (
                "the shape name lower-cased, dotted prefix",
                "com.amazonaws.cloudtrail.v20131101.cloudtrail_20131101.CreateTrail",
                CLOUDTRAIL,
            ),
        ];

        for (case, target, shape_id) in rows {
            assert!(
                names_service(target, shape_id),
                "case {case:?}: {target:?} must still be claimed by {shape_id}, as the \
                 framework's own routing key is resolved that way"
            );
            let resolved = operation_for_service(target, shape_id);
            assert_eq!(
                resolved, None,
                "case {case:?}: {target:?} names no shape {shape_id} declares, so no \
                 operation may be resolved for it, got {resolved:?}"
            );
        }
    }
}
