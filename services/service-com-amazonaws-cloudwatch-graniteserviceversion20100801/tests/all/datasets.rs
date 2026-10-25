// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetDataset`: the default dataset every account has in every Region,
//! and the customer managed KMS key the store holds for it.
//!
//! Every table row is a distinct input along what the rule under test generalises over (the
//! form of the identifier, the Region the request is signed for, and the partition, Region
//! and account the identifier's ARN carries), so the recorded happy path is one row among
//! many rather than the specification.

use crate::harness::{build_router_with_engine, call_in_region, error_code};
use serde_json::{json, Value};

/// The account id every emulated ARN carries, which is the only account whose dataset this
/// emulator serves.
const ACCOUNT: &str = "000000000000";

/// A string member of a body.
fn text<'body>(value: &'body Value, member: &str) -> Option<&'body str> {
    value.get(member).and_then(Value::as_str)
}

/// The ARN of `region`'s default dataset, in the format `GetDatasetOutput$Arn` documents:
/// `arn:aws:cloudwatch:Region:account-id:dataset/dataset-id`.
fn dataset_arn(region: &str) -> String {
    format!("arn:aws:cloudwatch:{region}:{ACCOUNT}:dataset/default")
}

#[tokio::test]
async fn the_default_dataset_is_reported_for_every_accepted_identifier_in_every_region() {
    let (router, _engine) = build_router_with_engine();

    // The Regions a request can be signed for; the dataset is "implicit for every account in
    // every Region", so each one answers with its own Region's ARN.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "sa-east-1"] {
        // Both accepted forms of `GetDatasetInput$DatasetIdentifier`: "you can specify either
        // `default` or the full dataset Amazon Resource Name (ARN)".
        for identifier in ["default".to_string(), dataset_arn(region)] {
            let case = format!("{region} / {identifier}");
            let (status, body) = call_in_region(
                &router,
                region,
                "GetDataset",
                &json!({"DatasetIdentifier": identifier}),
            )
            .await;

            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                text(&body, "DatasetId"),
                Some("default"),
                "case {case:?}: expected DatasetId default, got {body}"
            );
            assert_eq!(
                text(&body, "Arn"),
                Some(dataset_arn(region).as_str()),
                "case {case:?}: expected Arn {}, got {body}",
                dataset_arn(region)
            );
            assert!(
                body.get("KmsKeyArn").is_none(),
                "case {case:?}: a dataset with no customer managed key must omit KmsKeyArn, \
                 got {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_outside_the_callers_own_dataset_is_not_found() {
    let (router, _engine) = build_router_with_engine();

    // Every row satisfies the modelled `DatasetIdentifier` pattern, so the boundary validator
    // passes it through and the handler is what decides: each names a dataset in a scope this
    // endpoint does not serve (another Region, another account, another partition, or no
    // Region at all), and the only error `GetDataset` models for that is
    // `ResourceNotFoundException`.
    let rows = [
        ("another Region", "arn:aws:cloudwatch:eu-west-1:000000000000:dataset/default"),
        ("another account", "arn:aws:cloudwatch:us-east-1:123456789012:dataset/default"),
        ("another partition", "arn:aws-cn:cloudwatch:us-east-1:000000000000:dataset/default"),
        ("no Region", "arn:aws:cloudwatch::000000000000:dataset/default"),
    ];

    for (case, identifier) in rows {
        let (status, body) = call_in_region(
            &router,
            "us-east-1",
            "GetDataset",
            &json!({"DatasetIdentifier": identifier}),
        )
        .await;

        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn the_associated_kms_key_is_reported_for_the_region_that_holds_it() {
    let (router, engine) = build_router_with_engine();

    // The association itself is `AssociateDatasetKmsKey`'s to write, so the key is seeded
    // straight into the store this reader reads — the same store that handler writes.
    let key = "arn:aws:kms:us-east-1:000000000000:key/1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
    let state =
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(
            &*engine,
        )
        .expect("the service store is reachable from the shared engine");
    state
        .lock()
        .dataset_kms_keys
        .insert("us-east-1".to_string(), key.to_string());

    // Each row is the same read in a different Region: the key belongs to the dataset of the
    // Region it was associated in, and nowhere else.
    let rows = [("us-east-1", Some(key)), ("eu-west-1", None)];
    for (region, expected) in rows {
        for identifier in ["default".to_string(), dataset_arn(region)] {
            let case = format!("{region} / {identifier}");
            let (status, body) = call_in_region(
                &router,
                region,
                "GetDataset",
                &json!({"DatasetIdentifier": identifier}),
            )
            .await;

            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                text(&body, "KmsKeyArn"),
                expected,
                "case {case:?}: expected KmsKeyArn {expected:?}, got {body}"
            );
        }
    }

    // A read reports the association; it never creates, moves or drops one, so the store holds
    // exactly the one entry that was seeded and a later caller sees the same answer.
    let after = state.lock().dataset_kms_keys.clone();
    assert_eq!(
        after.len(),
        1,
        "reading a dataset must not add or remove an association, got {after:?}"
    );
    assert_eq!(
        after.get("us-east-1").map(String::as_str),
        Some(key),
        "reading a dataset must not change the association it reports, got {after:?}"
    );
}
