// SPDX-License-Identifier: CC0-1.0

//! What Config answers for a resource key it holds no configuration item for, end to
//! end over the wire.
//!
//! Two rules are pinned here, each over a TABLE of inputs rather than one recorded call:
//!
//! 1. `BatchGetResourceConfig` answers a requested key with the current configuration
//!    item the discovered-resource inventory holds for it, and a key it holds none for
//!    is absent from BOTH `baseConfigurationItems` and `unprocessedResourceKeys` —
//!    the unprocessed list carries the keys the read could not process, which a client
//!    asks for again, so a key with nothing to report never enters it;
//! 2. a `PutResourceConfig` the registration precondition refused ("The custom resource
//!    type must be registered with CloudFormation") records nothing, so the resource id
//!    it named is reported by no configuration-item read: neither by a batched read nor
//!    by the discovered-resource list or counts.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — reusing the
//! `Emulator` harness `config_setup_and_rule_loop` defines, the write-path helpers
//! `config_write_path_validation` defines and the response readers
//! `batch_get_resource_config` defines, and every write is read back through the
//! operations that report it.

use serde_json::{json, Value};

use crate::batch_get_resource_config::{
    batch, key, pairs, reported_items, reported_unprocessed, Pair,
};
use crate::config_connectors::refusal;
use crate::config_discovered_resource_counts::counts_of;
use crate::config_setup_and_rule_loop::Emulator;
use crate::config_write_path_validation::{recording_emulator, resource_config};

/// The operation under test.
const BATCH: &str = "BatchGetResourceConfig";

/// Resource types the CloudFormation registry publishes, from `smithy_query(query="shape",
/// shape="ResourceType")`, picked across the catalogue rather than from the types the
/// recording used.
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`S3_BUCKET`].
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`S3_BUCKET`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";

/// The resource ids `ListDiscoveredResources` reports of `resource_type` among
/// `resource_ids`.
async fn discovered_ids(
    case: &str,
    emulator: &Emulator,
    resource_type: &str,
    resource_ids: &[&str],
) -> Vec<String> {
    let body = emulator
        .ok(
            "ListDiscoveredResources",
            &json!({ "resourceType": resource_type, "resourceIds": resource_ids }),
        )
        .await;
    body["resourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected resourceIdentifiers, got {body}"))
        .iter()
        .map(|identifier| {
            identifier["resourceId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// An emulator recording two resources, holding a third whose deletion it recorded.
async fn emulator_with_an_inventory() -> Emulator {
    let emulator = recording_emulator().await;
    for (resource_type, resource_id) in [(EC2_VOLUME, "vol-0001"), (DYNAMODB_TABLE, "orders")] {
        emulator
            .ok(
                "PutResourceConfig",
                &resource_config(resource_type, resource_id, r#"{"state":"available"}"#),
            )
            .await;
    }
    emulator
        .ok(
            "PutResourceConfig",
            &resource_config(S3_BUCKET, "bucket-gone", r#"{"state":"available"}"#),
        )
        .await;
    emulator
        .ok(
            "DeleteResourceConfig",
            &json!({ "ResourceType": S3_BUCKET, "ResourceId": "bucket-gone" }),
        )
        .await;
    emulator
}

#[tokio::test]
async fn a_key_with_no_configuration_item_is_reported_in_neither_list() {
    // Rows differ along what the rule generalises over: why the inventory holds no
    // current configuration item for the key (a type nothing was recorded for, an id
    // nothing was recorded under, an id recorded under another type, a resource whose
    // deletion was recorded), how many such keys one batch carries, and whether the
    // batch also names resources the inventory does hold.
    let cases: [(&str, Vec<Value>, &[Pair]); 7] = [
        (
            "a registered type nothing at all was recorded for",
            vec![key(S3_BUCKET, "no-such-bucket")],
            &[],
        ),
        (
            "an id the inventory does not hold",
            vec![key(EC2_VOLUME, "vol-0404")],
            &[],
        ),
        (
            "an id recorded under another type",
            vec![key(S3_BUCKET, "vol-0001")],
            &[],
        ),
        (
            "a resource whose deletion was recorded",
            vec![key(S3_BUCKET, "bucket-gone")],
            &[],
        ),
        (
            "several keys the inventory holds nothing for",
            vec![
                key(S3_BUCKET, "no-such-bucket"),
                key(EC2_VOLUME, "vol-0404"),
                key(DYNAMODB_TABLE, "no-such-table"),
            ],
            &[],
        ),
        (
            "one such key beside the resources the inventory holds",
            vec![
                key(EC2_VOLUME, "vol-0404"),
                key(DYNAMODB_TABLE, "orders"),
                key(S3_BUCKET, "bucket-gone"),
                key(EC2_VOLUME, "vol-0001"),
            ],
            &[(DYNAMODB_TABLE, "orders"), (EC2_VOLUME, "vol-0001")],
        ),
        (
            "only resources the inventory holds",
            vec![key(EC2_VOLUME, "vol-0001"), key(DYNAMODB_TABLE, "orders")],
            &[(EC2_VOLUME, "vol-0001"), (DYNAMODB_TABLE, "orders")],
        ),
    ];

    for (case, keys, expected_items) in cases {
        let emulator = emulator_with_an_inventory().await;
        let request = batch(&keys);

        let body = emulator.ok(BATCH, &request).await;
        assert_eq!(
            reported_items(case, &body),
            pairs(expected_items),
            "case {case:?}: expected the configuration items {expected_items:?}, got {body}"
        );
        assert_eq!(
            reported_unprocessed(case, &body),
            Vec::<(String, String)>::new(),
            "case {case:?}: a key with nothing to report is no key left unprocessed, got {body}"
        );

        // The read is a report: asking again answers the same thing, so a client that
        // loops on the unprocessed list never has anything to retry.
        let again = emulator.ok(BATCH, &request).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second batch read must answer the same, got {again}"
        );
    }
}

#[tokio::test]
async fn a_write_the_registration_precondition_refused_is_reported_by_no_read() {
    // Rows differ along why the CloudFormation registry holds no registration for the
    // type the write named: an organization nothing registered a type under, an
    // organization reserved against custom resource types, a misspelled registered type,
    // and a registered type spelled in another case.
    let cases: [(&str, &str, &str); 4] = [
        (
            "an organization nothing registered",
            "Acme::Testing::Widget",
            "widget-1",
        ),
        (
            "a reserved organization",
            "Amazon::Testing::Widget",
            "widget-2",
        ),
        (
            "a misspelled registered type",
            "AWS::S3::Buckets",
            "widget-3",
        ),
        (
            "a registered type in another case",
            "aws::s3::bucket",
            "widget-4",
        ),
    ];

    for (case, resource_type, resource_id) in cases {
        let emulator = emulator_with_an_inventory().await;

        let (status, code, message) = refusal(
            &emulator,
            "PutResourceConfig",
            &resource_config(resource_type, resource_id, r#"{"state":"new"}"#),
        )
        .await;
        let expected = format!(
            "Cannot find registered Resource Type with name {resource_type}. Please register \
             the Resource Type with Cloudformation and try again."
        );
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ValidationException", expected.as_str()),
            "case {case:?}: expected the registration precondition for {resource_type:?}, got \
             {status} with {code} and {message:?}"
        );

        // Nothing was recorded, so the id the refused write named is reported by no
        // configuration-item read — and a batched read for it reports it in neither
        // list, rather than handing a client a key to retry.
        let body = emulator
            .ok(BATCH, &batch(&[key(S3_BUCKET, resource_id)]))
            .await;
        assert_eq!(
            (
                reported_items(case, &body),
                reported_unprocessed(case, &body)
            ),
            (Vec::<(String, String)>::new(), Vec::<(String, String)>::new()),
            "case {case:?}: the refused resource id is reported in neither list, got {body}"
        );

        let listed = discovered_ids(case, &emulator, S3_BUCKET, &[resource_id]).await;
        assert_eq!(
            listed,
            Vec::<String>::new(),
            "case {case:?}: the refused resource id is no discovered resource, got {listed:?}"
        );

        // The inventory the accepted writes built is untouched by the refusal.
        let counts = emulator.ok("GetDiscoveredResourceCounts", &json!({})).await;
        let counted = counts_of(case, &counts);
        assert_eq!(
            counted,
            vec![
                (DYNAMODB_TABLE.to_string(), 1),
                (EC2_VOLUME.to_string(), 1),
            ],
            "case {case:?}: a refused write records nothing, so only the two accepted \
             resources are discovered, got {counted:?}"
        );
    }
}
