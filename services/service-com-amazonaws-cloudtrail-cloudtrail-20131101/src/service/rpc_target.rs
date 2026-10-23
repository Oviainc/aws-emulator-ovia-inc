// SPDX-License-Identifier: CC0-1.0

//! The target rule in isolation: which operation an `X-Amz-Target` names, over the
//! prefix forms a client can write.
//!
//! An `awsJson` target is `<targetPrefix>.<OperationName>` and the prefix is OPAQUE — an
//! AWS SDK sends whatever its service model publishes, which for this service is the
//! fully qualified Coral name `com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101`,
//! so the prefix itself carries dots. A Smithy shape name is an identifier (the
//! `smithy.api#Identifier` production: ASCII letters, digits and `_`) and so can never
//! contain a dot, which is why the operation is the segment after the LAST dot and the
//! service is the segment before it.
//!
//! The rule itself is `service_common::rpc_target::operation_for_service`, stated once
//! there for every service whose published prefix carries dots (this one and
//! CodeConnections today) and read here under this service's own shape name. The wire
//! behaviour it produces is asserted end-to-end in
//! `tests/all/target_prefix_dispatch.rs`; what is pinned here is the reader itself, over
//! a table of prefix forms.

use service_common::rpc_target::operation_for_service;

/// The local name of this service's model shape, which the shared reader matches the
/// target's prefix against.
const SHAPE: &str = "CloudTrail_20131101";

#[test]
fn the_operation_is_the_segment_after_the_last_dot() {
    // (case, target, the operation it names for this service)
    let rows: [(&str, &str, Option<&str>); 12] = [
        (
            "the fully qualified Coral prefix an SDK sends",
            "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateTrail",
            Some("CreateTrail"),
        ),
        (
            "the bare service shape name",
            "CloudTrail_20131101.DescribeTrails",
            Some("DescribeTrails"),
        ),
        (
            "one namespace label",
            "cloudtrail.CloudTrail_20131101.ListTrails",
            Some("ListTrails"),
        ),
        (
            "a deeper namespace than the SDK's own",
            "a.b.c.d.e.CloudTrail_20131101.GetTrailStatus",
            Some("GetTrailStatus"),
        ),
        (
            "surrounding whitespace",
            " CloudTrail_20131101.GetTrail ",
            Some("GetTrail"),
        ),
        (
            "an operation this model does not bind is still read as an operation",
            "CloudTrail_20131101.NoSuchOperation",
            Some("NoSuchOperation"),
        ),
        (
            // An operation name may carry digits; the dot is the only separator.
            "an operation name carrying digits",
            "CloudTrail_20131101.Query2",
            Some("Query2"),
        ),
        (
            "another service's prefix",
            "DynamoDB_20120810.GetItem",
            None,
        ),
        (
            "a prefix that merely ends in this shape name's characters",
            "NotCloudTrail_20131101.CreateTrail",
            None,
        ),
        (
            // The shape name is matched as the model spells it: a differently cased
            // label names a shape this model does not declare.
            "the shape name lower-cased",
            "cloudtrail_20131101.CreateTrail",
            None,
        ),
        ("no dot at all", "CreateTrail", None),
        ("an empty operation", "CloudTrail_20131101.", None),
    ];

    for (case, target, expected) in rows {
        let actual = operation_for_service(target, SHAPE);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?} from target {target:?}, got {actual:?}"
        );
    }
}
