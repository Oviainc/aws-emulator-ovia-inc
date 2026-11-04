// SPDX-License-Identifier: CC0-1.0

//! The WORDING of two refusals, end to end over the wire.
//!
//! A refusal's message is part of its contract: a client that reads it to tell one
//! failure of an error class from another only can when the sentence is the service's
//! own. The two rules under test, each asserted over a TABLE of inputs rather than one
//! request:
//!
//! 1. `CreateSnapshot` against a namespace with no compute answers a sentence that is
//!    entirely FIXED — it names no resource, because the resource travels in the modelled
//!    `ResourceNotFoundException$resourceName` member — so the same text comes back
//!    whatever the namespace is called and whichever Region the call is signed for.
//! 2. `CreateUsageLimit` over a `resourceArn` that resolves to nothing answers a sentence
//!    whose ONE variable slot is the id segment the submitted ARN ends in, never the whole
//!    ARN.
//!
//! The harness (emulator, signed call, error-code reader, create helpers) is shared with
//! `namespace_workgroup_lifecycle`, which owns it.

use serde_json::json;

use crate::namespace_workgroup_lifecycle::{
    call, call_in, create_namespace, create_workgroup, emulator, error_code, ACCOUNT,
};

/// The fixed sentence this service refuses a snapshot of a workgroupless namespace with.
const NO_WORKGROUP: &str =
    "You can't create a snapshot because there isn't a workgroup created for the namespace.";

// ---------------------------------------------------------------------------
// Rule 1 — the workgroupless-namespace refusal carries no name.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_snapshot_without_compute_refusal_is_one_fixed_sentence() {
    // The rows differ along what the sentence could have interpolated and does not: the
    // namespace name (short, long, digit-laden, hyphenated) and the Region the call is
    // signed for. The snapshot name and the retention period vary with them, so no row
    // repeats another's request.
    let cases: [(&str, &str, &str, Option<i32>); 4] = [
        ("us-east-1", "nsw", "snap-nsw", None),
        (
            "eu-west-2",
            "a-namespace-with-a-rather-long-name-0123456789",
            "snap-long",
            Some(1),
        ),
        ("ap-southeast-2", "ns-42-7", "snap-digits", Some(3653)),
        ("us-west-2", "x-y-z-ns", "snap-hyphens", Some(-1)),
    ];

    let router = emulator();
    for (region, namespace, snapshot, retention) in cases {
        let case = format!("region={region} namespace={namespace}");
        let (status, body) = call_in(
            &router,
            region,
            "CreateNamespace",
            json!({ "namespaceName": namespace }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateNamespace expected 200, got {status} with {body}"
        );

        let mut request = json!({ "namespaceName": namespace, "snapshotName": snapshot });
        if let Some(retention) = retention {
            request["retentionPeriod"] = json!(retention);
        }
        let (status, body) = call_in(&router, region, "CreateSnapshot", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: CreateSnapshot without compute got {status} with {body}"
        );
        assert_eq!(
            body["message"],
            json!(NO_WORKGROUP),
            "case {case}: the refusal is the fixed sentence, got {:?}",
            body["message"]
        );
        assert!(
            !body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(namespace),
            "case {case}: the sentence must not carry the namespace name, got {:?}",
            body["message"]
        );
        // The resource the refusal is about travels in the member modelled for it.
        assert_eq!(
            body["resourceName"],
            json!(namespace),
            "case {case}: resourceName names the namespace, got {:?}",
            body["resourceName"]
        );

        // Attaching compute makes the identical request succeed, so the refusal was about
        // the missing workgroup and nothing else.
        let (status, body) = call_in(
            &router,
            region,
            "CreateWorkgroup",
            json!({ "workgroupName": format!("{namespace}-wg"), "namespaceName": namespace }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateWorkgroup expected 200, got {status} with {body}"
        );
        let (status, body) = call_in(&router, region, "CreateSnapshot", request).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateSnapshot with compute expected 200, got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — the unresolvable-resourceArn refusal names the ARN's id segment.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_unresolvable_resource_arn_refusal_names_the_arn_id_segment() {
    let router = emulator();
    let namespace = create_namespace(&router, "wording-ns").await;
    let workgroup = create_workgroup(&router, "wording-wg", "wording-ns").await;
    let real_workgroup_arn = workgroup["workgroupArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let real_namespace_arn = namespace["namespaceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let real_workgroup_id = workgroup["workgroupId"].as_str().unwrap_or_default();

    // A UUID that names nothing: distinct from anything the emulator minted.
    let absent_id = "3f8c1a7e-5b2d-4c6e-9a10-7d4e2f6b8c90";
    // Every row is an ARN that resolves to no resource of this account in this Region, and
    // they differ along the part that makes them unresolvable: a fresh id, another
    // resource kind, another account, another Region, another partition, and — the rows
    // that carry no `/` at all — a value whose whole text is its own id segment.
    let cases: Vec<(String, String)> = vec![
        (
            format!("arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:workgroup/{absent_id}"),
            absent_id.to_string(),
        ),
        (
            format!("arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:namespace/{absent_id}"),
            absent_id.to_string(),
        ),
        (
            format!("arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:snapshot/{absent_id}"),
            absent_id.to_string(),
        ),
        (
            format!(
                "arn:aws:redshift-serverless:us-east-1:999999999999:workgroup/{real_workgroup_id}"
            ),
            real_workgroup_id.to_string(),
        ),
        (
            format!(
                "arn:aws:redshift-serverless:eu-central-1:{ACCOUNT}:workgroup/{real_workgroup_id}"
            ),
            real_workgroup_id.to_string(),
        ),
        (
            format!(
                "arn:aws-us-gov:redshift-serverless:us-gov-west-1:{ACCOUNT}:workgroup/\
                 {real_workgroup_id}"
            ),
            real_workgroup_id.to_string(),
        ),
        ("not-an-arn".to_string(), "not-an-arn".to_string()),
        (String::new(), String::new()),
    ];

    for (resource_arn, id_segment) in &cases {
        let (status, body) = call(
            &router,
            "CreateUsageLimit",
            json!({
                "resourceArn": resource_arn,
                "usageType": "serverless-compute",
                "amount": 60,
                "period": "daily",
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case resourceArn={resource_arn:?}: got {status} with {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!(
                "The Resource ARN is not valid as the workgroup ID {id_segment}."
            )),
            "case resourceArn={resource_arn:?}: got {:?}",
            body["message"]
        );
    }

    // And the ARNs that DO resolve are accepted, so the sentence above is the answer to an
    // ARN that resolves to nothing rather than to this request's shape.
    for (resource_arn, usage_type) in [
        (&real_workgroup_arn, "serverless-compute"),
        (&real_namespace_arn, "cross-region-datasharing"),
    ] {
        let (status, body) = call(
            &router,
            "CreateUsageLimit",
            json!({
                "resourceArn": resource_arn,
                "usageType": usage_type,
                "amount": 60,
                "period": "daily",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case resourceArn={resource_arn:?}: expected 200, got {status} with {body}"
        );
    }
}
