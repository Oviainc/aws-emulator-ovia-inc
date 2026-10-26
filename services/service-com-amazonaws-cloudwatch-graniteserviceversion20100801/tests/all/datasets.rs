// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetDataset` and `DisassociateDatasetKmsKey`: the default dataset
//! every account has in every Region, the customer managed KMS key the store holds for it,
//! and what removing that association leaves behind.
//!
//! Every table row is a distinct input along what the rule under test generalises over (the
//! form of the identifier, the Region the request is signed for, the partition, Region and
//! account the identifier's ARN carries, which key the Region holds, and how many callers
//! remove it at once), so the recorded happy path is one row among many rather than the
//! specification.

use std::collections::HashMap;
use std::sync::Arc;

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

/// A customer managed key ARN of the form `AssociateDatasetKmsKeyInput$KmsKeyArn` requires:
/// `arn:aws:kms:Region:account-id:key/key-id`.
fn key_arn(region: &str, key_id: &str) -> String {
    format!("arn:aws:kms:{region}:{ACCOUNT}:key/{key_id}")
}

/// The Regions whose default dataset this suite associates a key with, and the key id each
/// one's key carries — four distinct keys, so a removal in one Region is asserted to leave
/// every other Region's association exactly as it was.
const SEEDED_REGIONS: [(&str, &str); 4] = [
    ("us-east-1", "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"),
    ("eu-west-1", "f0e1d2c3-b4a5-4968-8778-69a5b4c3d2e1"),
    ("ap-southeast-2", "00000000-0000-4000-8000-000000000001"),
    ("sa-east-1", "deadbeef-dead-4ead-8ead-beefdeadbeef"),
];

/// The associations the store holds, by Region.
///
/// Writing the association is `AssociateDatasetKmsKey`'s to do, so this suite seeds and reads
/// the same store that handler writes and `GetDataset` reports from.
fn held_keys(engine: &engine::InMemoryEngine) -> HashMap<String, String> {
    let state =
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(
            engine,
        )
        .expect("the service store is reachable from the shared engine");
    // The guard is a named local, so it is dropped before the store handle it borrows.
    let store = state.lock();
    store.dataset_kms_keys.clone()
}

/// Associate every Region of [`SEEDED_REGIONS`] with its own key, and answer the associations
/// that leaves held.
fn seed_keys(engine: &engine::InMemoryEngine) -> HashMap<String, String> {
    let state =
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(
            engine,
        )
        .expect("the service store is reachable from the shared engine");
    {
        let mut store = state.lock();
        for (region, key_id) in SEEDED_REGIONS {
            store
                .dataset_kms_keys
                .insert(region.to_string(), key_arn(region, key_id));
        }
    }
    held_keys(engine)
}

/// The `KmsKeyArn` `GetDataset` reports for `region`'s default dataset, or `None` when it
/// omits the member.
async fn reported_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
) -> Option<String> {
    let (status, body) = call_in_region(
        router,
        region,
        "GetDataset",
        &json!({"DatasetIdentifier": "default"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region:?}: reading the dataset back must answer HTTP 200, got {status} with \
         body {body}"
    );
    text(&body, "KmsKeyArn").map(str::to_string)
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

#[tokio::test]
async fn disassociating_removes_only_the_key_of_the_dataset_the_request_names() {
    // Each row is a distinct call: which Region the request is signed for (so which dataset
    // it names), and which of the two accepted forms of `DatasetIdentifier` names it. The
    // store is seeded afresh per row, because removing an association is destructive.
    for (region, _) in SEEDED_REGIONS {
        for identifier in ["default".to_string(), dataset_arn(region)] {
            let case = format!("{region} / {identifier}");
            let (router, engine) = build_router_with_engine();
            let seeded = seed_keys(&engine);

            let (status, body) = call_in_region(
                &router,
                region,
                "DisassociateDatasetKmsKey",
                &json!({"DatasetIdentifier": identifier}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );

            // "After this operation completes, data that you publish to the dataset is
            // encrypted at rest using an Amazon Web Services owned key", which `GetDataset`
            // reports by omitting `KmsKeyArn`.
            let reported = reported_key(&router, region).await;
            assert_eq!(
                reported, None,
                "case {case:?}: the disassociated dataset must report no KmsKeyArn, got \
                 {reported:?}"
            );

            // Every other Region's dataset keeps the key it was associated with: the dataset
            // the call names is the one of the account and Region the request was signed for.
            let after = held_keys(&engine);
            let expected: HashMap<String, String> = seeded
                .iter()
                .filter(|(held, _)| held.as_str() != region)
                .map(|(held, key)| (held.clone(), key.clone()))
                .collect();
            assert_eq!(
                after, expected,
                "case {case:?}: expected the remaining associations {expected:?}, got {after:?}"
            );
            for (other, _) in SEEDED_REGIONS.iter().filter(|(other, _)| *other != region) {
                let reported = reported_key(&router, other).await;
                assert_eq!(
                    reported.as_deref(),
                    seeded.get(*other).map(String::as_str),
                    "case {case:?}: {other} must still report the key it was associated with, \
                     got {reported:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn disassociating_a_dataset_with_no_associated_key_is_not_found() {
    // (case, the Region the request is signed for, the identifier it names, whether every
    // Region's dataset was associated with a key first, whether the call is made twice).
    //
    // Rows differ along how the call finds no key to remove: the Region never had one, the
    // Region's own key was already removed by the call before, or the identifier names a
    // dataset in a scope this endpoint does not serve at all (another Region, another account,
    // another partition, or no Region), which is a dataset rather than a key that is not
    // there. Every identifier satisfies the modelled `DatasetIdentifier` pattern, so the
    // handler is what decides.
    let rows: [(&str, &str, String, bool, bool); 8] = [
        ("never associated, named plainly", "us-east-1", "default".to_string(), false, false),
        (
            "never associated, named by ARN",
            "eu-west-1",
            dataset_arn("eu-west-1"),
            false,
            false,
        ),
        (
            "already disassociated, named plainly",
            "us-east-1",
            "default".to_string(),
            true,
            true,
        ),
        (
            "already disassociated, named by ARN",
            "ap-southeast-2",
            dataset_arn("ap-southeast-2"),
            true,
            true,
        ),
        ("another Region's dataset", "us-east-1", dataset_arn("eu-west-1"), true, false),
        (
            "another account's dataset",
            "us-east-1",
            "arn:aws:cloudwatch:us-east-1:123456789012:dataset/default".to_string(),
            true,
            false,
        ),
        (
            "another partition's dataset",
            "us-east-1",
            "arn:aws-cn:cloudwatch:us-east-1:000000000000:dataset/default".to_string(),
            true,
            false,
        ),
        (
            "a dataset ARN with no Region",
            "us-east-1",
            format!("arn:aws:cloudwatch::{ACCOUNT}:dataset/default"),
            true,
            false,
        ),
    ];

    for (case, region, identifier, associate_first, call_twice) in rows {
        let (router, engine) = build_router_with_engine();
        let before = if associate_first {
            seed_keys(&engine)
        } else {
            held_keys(&engine)
        };
        // The rows that call twice are the ones whose first call removes the association,
        // leaving nothing for the second one to remove.
        if call_twice {
            let (status, body) = call_in_region(
                &router,
                region,
                "DisassociateDatasetKmsKey",
                &json!({"DatasetIdentifier": identifier}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first disassociation must answer HTTP 200, got {status} \
                 with body {body}"
            );
        }

        let (status, body) = call_in_region(
            &router,
            region,
            "DisassociateDatasetKmsKey",
            &json!({"DatasetIdentifier": identifier}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} with \
             body {body}"
        );
        // The refusal names what it could not find, in the two members the error shape carries
        // for it.
        assert_eq!(
            text(&body, "ResourceType"),
            Some("Dataset"),
            "case {case:?}: expected the refusal to name the resource type Dataset, got {body}"
        );
        assert_eq!(
            text(&body, "ResourceId"),
            Some(identifier.as_str()),
            "case {case:?}: expected the refusal to name {identifier:?}, got {body}"
        );

        // A refusal removes nothing: every association the store held before the refused call
        // is still held, bar the one the row removed itself.
        let mut expected = before.clone();
        if call_twice {
            expected.remove(region);
        }
        let after = held_keys(&engine);
        assert_eq!(
            after, expected,
            "case {case:?}: a refused disassociation must leave the associations {expected:?}, \
             got {after:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_disassociations_of_one_dataset_converge_on_a_single_removal() {
    // The rows differ along how many callers remove the same association at once: whatever
    // the count, the association is removed once, so exactly one caller carries out the
    // disassociation and every other one finds no key left to remove.
    for callers in [2usize, 3, 5, 8] {
        let region = "us-east-1";
        let (router, engine) = build_router_with_engine();
        let seeded = seed_keys(&engine);
        let router = Arc::new(router);

        let mut racing = Vec::new();
        for caller in 0..callers {
            let router = Arc::clone(&router);
            // Half the callers name the dataset plainly and half by its ARN: one dataset, two
            // spellings, so which caller wins cannot depend on how it named it.
            let identifier = if caller % 2 == 0 {
                "default".to_string()
            } else {
                dataset_arn(region)
            };
            racing.push(tokio::spawn(async move {
                let (status, body) = call_in_region(
                    &router,
                    region,
                    "DisassociateDatasetKmsKey",
                    &json!({"DatasetIdentifier": identifier}),
                )
                .await;
                (caller, status, body)
            }));
        }

        let mut answers = Vec::new();
        for racer in racing {
            answers.push(racer.await.expect("the racing disassociation completed"));
        }

        let case = format!("{callers} concurrent callers");
        let removals = answers.iter().filter(|answer| answer.1 == 200).count();
        assert_eq!(
            removals, 1,
            "case {case:?}: exactly one caller may carry out the disassociation, got \
             {answers:?}"
        );
        for (caller, status, body) in answers.iter().filter(|answer| answer.1 != 200) {
            assert_eq!(
                (*status, error_code(body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: caller {caller} found no key to remove, so it must be refused \
                 HTTP 404 ResourceNotFoundException, got {status} with body {body}"
            );
        }

        // And the dataset converges on having no association, however the race went.
        let reported = reported_key(&router, region).await;
        assert_eq!(
            reported, None,
            "case {case:?}: the dataset must end with no KmsKeyArn, got {reported:?}"
        );
        let after = held_keys(&engine);
        let expected: HashMap<String, String> = seeded
            .iter()
            .filter(|(held, _)| held.as_str() != region)
            .map(|(held, key)| (held.clone(), key.clone()))
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: expected the remaining associations {expected:?}, got {after:?}"
        );
    }
}
