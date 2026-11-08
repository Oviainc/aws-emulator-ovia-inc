// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateDatasetKmsKey`, `GetDataset` and
//! `DisassociateDatasetKmsKey`: the default dataset every account has in every Region, the
//! customer managed KMS key a caller associates with it, and what removing that association
//! leaves behind.
//!
//! Every table row is a distinct input along what the rule under test generalises over (the
//! form of the identifier, the Region the request is signed for, the partition, Region and
//! account the identifier's ARN carries, the Region the key ARN names, which key the Region
//! holds, and how many callers remove it at once), so the recorded happy path is one row among
//! many rather than the specification.

use std::collections::HashMap;
use std::sync::Arc;

use crate::harness::{build_router_with_engine, call_in_region, error_code, error_message};
use serde_json::{json, Value};

/// The account id every emulated ARN carries, which is the only account whose dataset this
/// emulator serves.
const ACCOUNT: &str = "000000000000";

/// An account OTHER than the one this emulator serves, for the rows whose identifier names a
/// resource across an account boundary.
const FOREIGN_ACCOUNT: &str = "123456789012";

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

/// Associate every Region of [`SEEDED_REGIONS`] with its own key THROUGH THE OPERATION that
/// writes an association, and answer the associations that leaves held.
///
/// Driven over the wire rather than by writing the store, so what every later assertion rests
/// on is the association `AssociateDatasetKmsKey` really made.
async fn seed_keys(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &engine::InMemoryEngine,
) -> HashMap<String, String> {
    for (region, key_id) in SEEDED_REGIONS {
        let (status, body) = call_in_region(
            router,
            region,
            "AssociateDatasetKmsKey",
            &json!({
                "DatasetIdentifier": "default",
                "KmsKeyArn": key_arn(region, key_id),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: associating the key must answer HTTP 200, got {status} with \
             body {body}"
        );
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
async fn an_identifier_outside_the_callers_own_dataset_is_refused_as_unauthorized() {
    let (router, _engine) = build_router_with_engine();

    // Every row satisfies the modelled `DatasetIdentifier` pattern, so the boundary validator
    // passes it through: each names a dataset in a scope this caller has no grant on (another
    // Region, another account, another partition, or no Region at all). A caller that may not
    // see a resource is never told whether it is there, so the authorization layer in front of
    // the service answers `AccessDenied` — a code no shape of the model declares — rather than
    // the not-found the operation models for a dataset of its own scope.
    //
    // The last column is WHAT the refusal says, which follows from where the resource lives: a
    // resource of the caller's own account is refused on the policy that granted nothing, naming
    // the action and the resource, while one in ANOTHER ACCOUNT is refused on the accounts
    // themselves and names no action at all.
    let rows = [
        (
            "another Region",
            "arn:aws:cloudwatch:eu-west-1:000000000000:dataset/default",
            false,
        ),
        (
            "another account",
            "arn:aws:cloudwatch:us-east-1:123456789012:dataset/default",
            true,
        ),
        (
            "another partition",
            "arn:aws-cn:cloudwatch:us-east-1:000000000000:dataset/default",
            false,
        ),
        (
            "no Region",
            "arn:aws:cloudwatch::000000000000:dataset/default",
            false,
        ),
    ];

    for (case, identifier, cross_account) in rows {
        let (status, body) = call_in_region(
            &router,
            "us-east-1",
            "GetDataset",
            &json!({"DatasetIdentifier": identifier}),
        )
        .await;

        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {body}"
        );
        let message = error_message(&body);
        if cross_account {
            assert_eq!(
                message,
                format!(
                    "Caller account {ACCOUNT} is not authorized to access resource in account \
                     {FOREIGN_ACCOUNT}"
                ),
                "case {case:?}: the refusal must name the caller's account and the account the \
                 resource lives in, got {message:?}"
            );
        } else {
            assert!(
                message.contains("is not authorized to perform: cloudwatch:GetDataset on resource: ")
                    && message.contains(identifier),
                "case {case:?}: the refusal must name the action and the resource it refused, got \
                 {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_associated_kms_key_is_reported_for_the_region_that_holds_it() {
    let (router, engine) = build_router_with_engine();

    // The association is made through the operation that writes one, in ONE Region: what this
    // reader reports is the association that call really made.
    let key = key_arn("us-east-1", "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d");
    let (status, body) = call_in_region(
        &router,
        "us-east-1",
        "AssociateDatasetKmsKey",
        &json!({"DatasetIdentifier": "default", "KmsKeyArn": key.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "associating the key must answer HTTP 200, got {status} with body {body}"
    );
    let _ = &engine;

    // Each row is the same read in a different Region: the key belongs to the dataset of the
    // Region it was associated in, and nowhere else.
    let rows = [("us-east-1", Some(key.as_str())), ("eu-west-1", None)];
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
    // exactly the one entry the association made and a later caller sees the same answer.
    let after = held_keys(&engine);
    assert_eq!(
        after.len(),
        1,
        "reading a dataset must not add or remove an association, got {after:?}"
    );
    assert_eq!(
        after.get("us-east-1").map(String::as_str),
        Some(key.as_str()),
        "reading a dataset must not change the association it reports, got {after:?}"
    );
}

#[tokio::test]
async fn associating_writes_the_key_of_the_dataset_the_request_names() {
    // Each row is a distinct association: which Region the request is signed for (so which
    // dataset it names), which of the two accepted forms of `DatasetIdentifier` names it, and
    // which key id the key ARN carries. "Only the default dataset is supported. The default
    // dataset is implicit for every account in every Region", so every Region's dataset takes
    // a key of its own and no Region's association reaches another.
    let (router, engine) = build_router_with_engine();
    let mut expected: HashMap<String, String> = HashMap::new();
    for (index, (region, key_id)) in SEEDED_REGIONS.into_iter().enumerate() {
        let identifier = if index % 2 == 0 {
            "default".to_string()
        } else {
            dataset_arn(region)
        };
        let case = format!("{region} / {identifier}");
        let key = key_arn(region, key_id);
        let (status, body) = call_in_region(
            &router,
            region,
            "AssociateDatasetKmsKey",
            &json!({"DatasetIdentifier": identifier, "KmsKeyArn": key.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // Round trip: the key the association wrote is the one the reader reports, for the
        // dataset of THAT Region.
        let reported = reported_key(&router, region).await;
        assert_eq!(
            reported.as_deref(),
            Some(key.as_str()),
            "case {case:?}: expected GetDataset to report the key just associated, got \
             {reported:?}"
        );
        expected.insert(region.to_string(), key);
        let held = held_keys(&engine);
        assert_eq!(
            held, expected,
            "case {case:?}: expected the associations {expected:?}, got {held:?}"
        );
    }

    // "You can call AssociateDatasetKmsKey on a dataset that is already associated with a KMS
    // key to replace the existing key with a different one." — the replacement is one entry,
    // not a second one, and it leaves every other Region's association where it was.
    let (region, _) = SEEDED_REGIONS[0];
    let replacement = key_arn(region, "11111111-2222-4333-8444-555555555555");
    let (status, body) = call_in_region(
        &router,
        region,
        "AssociateDatasetKmsKey",
        &json!({"DatasetIdentifier": "default", "KmsKeyArn": replacement.clone()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "replacing an existing association must answer HTTP 200, got {status} with body {body}"
    );
    expected.insert(region.to_string(), replacement.clone());
    let held = held_keys(&engine);
    assert_eq!(
        held, expected,
        "a replacement must leave one association per Region, got {held:?}"
    );
    let reported = reported_key(&router, region).await;
    assert_eq!(
        reported.as_deref(),
        Some(replacement.as_str()),
        "the reader must report the key the replacement wrote, got {reported:?}"
    );
}

#[tokio::test]
async fn an_association_outside_the_callers_own_scope_is_refused_and_writes_nothing() {
    // (case, the Region the request is signed for, the identifier, the key ARN, the HTTP
    //  status and error code the refusal carries). The rows differ along WHICH half of the
    //  request is out of scope: the dataset the identifier names, or the Region the key lives
    //  in — "It must be in the same Amazon Web Services Region as the dataset."
    //  (`AssociateDatasetKmsKeyInput$KmsKeyArn`)
    let rows: Vec<(&str, &str, String, String, u16, &str)> = vec![
        (
            "a dataset of another Region",
            "us-east-1",
            dataset_arn("eu-west-1"),
            key_arn("us-east-1", "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"),
            403,
            "AccessDenied",
        ),
        (
            "a dataset of another account",
            "us-east-1",
            "arn:aws:cloudwatch:us-east-1:123456789012:dataset/default".to_string(),
            key_arn("us-east-1", "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"),
            403,
            "AccessDenied",
        ),
        (
            "a dataset of another partition",
            "us-east-1",
            "arn:aws-cn:cloudwatch:us-east-1:000000000000:dataset/default".to_string(),
            key_arn("us-east-1", "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"),
            403,
            "AccessDenied",
        ),
        (
            "a key of another Region",
            "us-east-1",
            "default".to_string(),
            key_arn("eu-west-1", "f0e1d2c3-b4a5-4968-8778-69a5b4c3d2e1"),
            400,
            "KmsKeyNotFoundException",
        ),
        (
            "a key of the Region the dataset is NOT in",
            "ap-southeast-2",
            "default".to_string(),
            key_arn("us-east-1", "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"),
            400,
            "KmsKeyNotFoundException",
        ),
    ];

    for (case, region, identifier, key, status_wanted, code) in rows {
        let (router, engine) = build_router_with_engine();
        let (status, body) = call_in_region(
            &router,
            region,
            "AssociateDatasetKmsKey",
            &json!({"DatasetIdentifier": identifier, "KmsKeyArn": key}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (status_wanted, code),
            "case {case:?}: expected HTTP {status_wanted} {code}, got {status} with body {body}"
        );
        // A refused association writes nothing, so the dataset still carries no key at all.
        let held = held_keys(&engine);
        assert!(
            held.is_empty(),
            "case {case:?}: a refused association must write nothing, got {held:?}"
        );
        let reported = reported_key(&router, region).await;
        assert_eq!(
            reported, None,
            "case {case:?}: the dataset must still report no key, got {reported:?}"
        );
    }
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
            let seeded = seed_keys(&router, &engine).await;

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
async fn disassociating_a_dataset_with_no_associated_key_is_refused_and_removes_nothing() {
    // (case, the Region the request is signed for, the identifier it names, whether every
    // Region's dataset was associated with a key first, whether the call is made twice,
    // whether the identifier is one this caller has a grant on).
    //
    // Rows differ along WHY the call removes nothing: the Region never had a key, the Region's
    // own key was already removed by the call before — both of which are an in-scope dataset
    // with no association, refused as the missing KEY — or the identifier names a dataset in a
    // scope this caller has no grant on (another Region, another account, another partition,
    // or no Region), which is refused BEFORE the association is looked at, exactly as every
    // other operation over this dataset refuses it. Every identifier satisfies the modelled
    // `DatasetIdentifier` pattern, so the handler is what decides.
    let rows: [(&str, &str, String, bool, bool, bool); 8] = [
        (
            "never associated, named plainly",
            "us-east-1",
            "default".to_string(),
            false,
            false,
            true,
        ),
        (
            "never associated, named by ARN",
            "eu-west-1",
            dataset_arn("eu-west-1"),
            false,
            false,
            true,
        ),
        (
            "already disassociated, named plainly",
            "us-east-1",
            "default".to_string(),
            true,
            true,
            true,
        ),
        (
            "already disassociated, named by ARN",
            "ap-southeast-2",
            dataset_arn("ap-southeast-2"),
            true,
            true,
            true,
        ),
        (
            "another Region's dataset",
            "us-east-1",
            dataset_arn("eu-west-1"),
            true,
            false,
            false,
        ),
        (
            "another account's dataset",
            "us-east-1",
            "arn:aws:cloudwatch:us-east-1:123456789012:dataset/default".to_string(),
            true,
            false,
            false,
        ),
        (
            "another partition's dataset",
            "us-east-1",
            "arn:aws-cn:cloudwatch:us-east-1:000000000000:dataset/default".to_string(),
            true,
            false,
            false,
        ),
        (
            "a dataset ARN with no Region",
            "us-east-1",
            format!("arn:aws:cloudwatch::{ACCOUNT}:dataset/default"),
            true,
            false,
            false,
        ),
    ];

    for (case, region, identifier, associate_first, call_twice, in_scope) in rows {
        let (router, engine) = build_router_with_engine();
        let before = if associate_first {
            seed_keys(&router, &engine).await
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
        if in_scope {
            // The dataset is this caller's own and simply carries no key: the refusal names
            // what it could not find, in the two members the error shape carries for it.
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} with \
                 body {body}"
            );
            assert_eq!(
                text(&body, "ResourceType"),
                Some("Dataset"),
                "case {case:?}: expected the refusal to name the resource type Dataset, got \
                 {body}"
            );
            assert_eq!(
                text(&body, "ResourceId"),
                Some(identifier.as_str()),
                "case {case:?}: expected the refusal to name {identifier:?}, got {body}"
            );
        } else {
            // The identifier names a dataset this caller has no grant on, so it is refused
            // before the association is looked at — and the refusal says nothing about
            // whether a key is there.
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDenied"),
                "case {case:?}: expected HTTP 403 AccessDenied, got {status} with body {body}"
            );
            let message = error_message(&body);
            if identifier.contains(FOREIGN_ACCOUNT) {
                // A resource in ANOTHER ACCOUNT is refused on the accounts themselves rather
                // than on a policy of either of them, so that refusal names no action.
                assert_eq!(
                    message,
                    format!(
                        "Caller account {ACCOUNT} is not authorized to access resource in \
                         account {FOREIGN_ACCOUNT}"
                    ),
                    "case {case:?}: the refusal must name the caller's account and the account \
                     the resource lives in, got {message:?}"
                );
            } else {
                assert!(
                    message.contains(
                        "is not authorized to perform: cloudwatch:DisassociateDatasetKmsKey on \
                         resource: "
                    ) && message.contains(&identifier),
                    "case {case:?}: the refusal must name the action and the resource it refused, \
                     got {message:?}"
                );
            }
        }

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
        let seeded = seed_keys(&router, &engine).await;
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
