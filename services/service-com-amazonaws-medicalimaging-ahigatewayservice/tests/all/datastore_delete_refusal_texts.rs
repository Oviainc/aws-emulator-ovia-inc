// SPDX-License-Identifier: CC0-1.0

//! How `DeleteDatastore` WORDS each of its three refusals.
//!
//! RULE: every refusal this service answers with states the RULE the request broke, as
//! one fixed sentence, and carries no identifier naming the resource the request
//! addressed — not the data store id in the URI, not an id of anything the store holds.
//! So the three refusals of `DeleteDatastore` are each a constant: the 404 for a data
//! store this region does not hold, the 409 for a store whose creation the clock still
//! owes, and the 409 the model's own precondition states ("before a data store can be
//! deleted, you must first delete all image sets within it") for a store that still
//! holds image sets.
//!
//! The rows span what the rule generalises over: for the image-sets conflict, how many
//! image sets the store holds, how many versions each carries, whether they are in the
//! primary DICOM hierarchy, what the store is named and which region it lives in; for
//! the other two, which id the request names. Each class is also asserted to be ONE
//! sentence across its rows, which is what makes it a constant of the service rather
//! than of a request.
//!
//! The harness is the sibling suites' (`crate::datastore_lifecycle`,
//! `crate::image_set_copy`): real requests through `MultiServiceRouter::dispatch`, with
//! the state struct touched only to SEED an image set (which only a DICOM import mints)
//! and never to assert.

use std::collections::BTreeSet;

use serde_json::json;

use crate::datastore_lifecycle::{unheld_id, Answer, Emulator};
use crate::image_set_copy::seed_image_set_in_hierarchy;

/// The sentence this service refuses the delete of a data store that still holds image
/// sets with: the model's precondition stated as a rule, with no identifier and no count
/// in it.
const STILL_HOLDS_IMAGE_SETS: &str = "The data store still contains image sets, which \
                                      must all be deleted before the data store can be \
                                      deleted.";

/// The sentence this service refuses the delete of a store whose creation is still in
/// flight with — the sibling 409 of the same operation.
const STILL_CREATING: &str = "The data store is still being created, so it cannot be deleted yet.";

/// The sentence this service answers a request naming a data store it does not hold
/// with, as the wire corpus records it — the sibling 404 of the same operation.
const DATASTORE_NOT_FOUND: &str = "Datastore resource is not found.";

/// Whether `text` holds a run of 32 lowercase-alphanumeric characters — the shape of
/// every identifier this service mints (`DatastoreId` and `ImageSetId` are both
/// `^[0-9a-z]{32}$`, `smithy_query(query="shape",shape="DatastoreId,ImageSetId")`). A
/// refusal text that holds one has reflected an identifier back.
fn holds_an_id_shaped_run(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut run = 0_usize;
    for byte in bytes {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            run += 1;
            if run >= 32 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// Assert one refusal: the status and error code the model declares, the exact sentence,
/// and that nothing in the answer reflects an identifier the request named.
fn assert_refusal(
    case: &str,
    answer: &Answer,
    status: u16,
    error_code: &str,
    expected_message: &str,
    identifiers: &[String],
) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, error_code,
        "case {case:?}: expected error code {error_code}, got {:?}",
        answer.error_code
    );
    let message = answer.string(&["message"]);
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected the refusal text {expected_message:?}, got {message:?}"
    );
    let body = answer.body.to_string();
    for identifier in identifiers {
        assert!(
            !body.contains(identifier.as_str()),
            "case {case:?}: the refusal reflected the identifier {identifier:?} back, body \
             {body:?}"
        );
    }
    assert!(
        !holds_an_id_shaped_run(&message),
        "case {case:?}: the refusal text carries an id-shaped run, got {message:?}"
    );
}

/// One row of the image-sets-remaining table.
#[derive(Debug)]
struct HoldingCase {
    case: &'static str,
    /// The region the data store is created in and the delete is addressed to: a store
    /// exists in one region, and the refusal does not vary with which.
    region: &'static str,
    datastore_name: &'static str,
    /// How many image sets the store holds when the delete arrives.
    held: usize,
    /// How many versions each held image set carries.
    versions: u32,
    /// Whether the held image sets are in the primary DICOM hierarchy.
    primary: bool,
}

/// RULE: the delete of a data store that still holds image sets is refused with ONE
/// fixed sentence stating the precondition the model documents — the same text whatever
/// the store is called, whichever region it lives in, however many image sets it holds
/// and whatever those image sets look like, with no identifier from the request in it.
#[tokio::test]
async fn image_sets_remaining_refusal_is_one_identifier_free_sentence() {
    let cases = [
        HoldingCase {
            case: "one image set, one version",
            region: "us-east-1",
            datastore_name: "holding.one",
            held: 1,
            versions: 1,
            primary: false,
        },
        HoldingCase {
            case: "one image set in the primary hierarchy, several versions",
            region: "us-east-1",
            datastore_name: "holding.primary",
            held: 1,
            versions: 3,
            primary: true,
        },
        HoldingCase {
            case: "several image sets",
            region: "us-east-1",
            datastore_name: "holding.many",
            held: 4,
            versions: 2,
            primary: false,
        },
        HoldingCase {
            case: "another region, another name",
            region: "eu-west-1",
            datastore_name: "holding/other#region-1",
            held: 2,
            versions: 1,
            primary: true,
        },
    ];

    let mut texts: BTreeSet<String> = BTreeSet::new();
    for case in &cases {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore_in(case.region, case.datastore_name, "token-holding-store")
            .await;
        let mut identifiers = vec![datastore_id.clone()];
        for _ in 0..case.held {
            identifiers.push(seed_image_set_in_hierarchy(
                &emulator,
                &datastore_id,
                case.versions,
                case.primary,
            ));
        }

        let refused = emulator
            .call_in(
                case.region,
                "DELETE",
                &format!("/datastore/{datastore_id}"),
                None,
            )
            .await;
        assert_refusal(
            case.case,
            &refused,
            409,
            "ConflictException",
            STILL_HOLDS_IMAGE_SETS,
            &identifiers,
        );
        texts.insert(refused.string(&["message"]));

        // Nothing was written: the store the refusal protected is still ACTIVE, so the
        // caller can delete its image sets and come back.
        let read = emulator
            .call_in(case.region, "GET", &format!("/datastore/{datastore_id}"), None)
            .await;
        assert_eq!(
            read.string(&["datastoreProperties", "datastoreStatus"]),
            "ACTIVE",
            "case {:?}: a refused delete changes nothing, got {read:?}",
            case.case
        );
    }

    assert_eq!(
        texts.len(),
        1,
        "the image-sets-remaining refusal is one constant sentence, got {texts:?}"
    );
}

/// RULE, over the whole operation: each of `DeleteDatastore`'s three refusals is one
/// fixed sentence carrying no identifier from the request — so none of them varies with
/// the id the request named, and the three are distinct sentences because they state
/// three different rules.
#[tokio::test]
async fn every_delete_datastore_refusal_states_only_its_rule() {
    let emulator = Emulator::new();

    // 1. A data store this region does not hold: several well-formed ids, one answer.
    let mut not_found: BTreeSet<String> = BTreeSet::new();
    for (index, id) in [
        unheld_id(),
        "z".repeat(32),
        "abcdefghijklmnopqrstuvwxyz012345".to_string(),
    ]
    .iter()
    .enumerate()
    {
        let refused = emulator.delete_datastore(id).await;
        assert_refusal(
            "absent data store",
            &refused,
            404,
            "ResourceNotFoundException",
            DATASTORE_NOT_FOUND,
            std::slice::from_ref(id),
        );
        not_found.insert(refused.string(&["message"]));
        assert_eq!(
            not_found.len(),
            1,
            "row {index}: the not-found text does not vary with the id, got {not_found:?}"
        );
    }

    // 2. A store whose creation the clock still owes: the delete arrives before the
    //    activation span is out, so the store is CREATING.
    let mut still_creating: BTreeSet<String> = BTreeSet::new();
    for index in 0..2_usize {
        let created = emulator
            .create_datastore(json!({
                "datastoreName": format!("creating.store-{index}"),
                "clientToken": format!("token-creating-{index}"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "row {index}: expected HTTP 200 from CreateDatastore, got {created:?}"
        );
        let datastore_id = created.string(&["datastoreId"]);
        let refused = emulator.delete_datastore(&datastore_id).await;
        assert_refusal(
            "creation still in flight",
            &refused,
            409,
            "ConflictException",
            STILL_CREATING,
            &[datastore_id],
        );
        still_creating.insert(refused.string(&["message"]));
    }
    assert_eq!(
        still_creating.len(),
        1,
        "the still-creating text does not vary with the store, got {still_creating:?}"
    );

    // 3. A settled store that still holds an image set.
    let datastore_id = emulator
        .active_datastore("holding.store", "token-holding-rule")
        .await;
    let image_set_id = seed_image_set_in_hierarchy(&emulator, &datastore_id, 1, false);
    let refused = emulator.delete_datastore(&datastore_id).await;
    assert_refusal(
        "image sets remaining",
        &refused,
        409,
        "ConflictException",
        STILL_HOLDS_IMAGE_SETS,
        &[datastore_id, image_set_id],
    );

    // Three rules, three sentences — built from what the SERVICE answered on each of the
    // three paths, not from the literals above, so a handler that collapsed two of them
    // onto one text fails here.
    let observed: BTreeSet<String> = not_found
        .into_iter()
        .chain(still_creating)
        .chain([refused.string(&["message"])])
        .collect();
    assert_eq!(
        observed.len(),
        3,
        "each rule is answered with its own sentence, got {observed:?}"
    );
}
