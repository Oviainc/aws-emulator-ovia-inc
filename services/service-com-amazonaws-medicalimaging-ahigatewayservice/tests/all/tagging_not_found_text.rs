// SPDX-License-Identifier: CC0-1.0

//! How the three tagging routes WORD a missing resource, end to end over real HTTP.
//!
//! RULE: a request this service cannot resolve to a resource is refused with HTTP 404
//! `ResourceNotFoundException` whose message is a constant of the resource KIND that is
//! absent — the data store when the ARN's data store is not held in the region the
//! request was addressed to, the image set when that data store IS held but owns no such
//! image set — and carries no identifier from the request. The route that reached the
//! resource does not change the sentence: an ARN-keyed tagging call and an id-keyed read
//! of the same absent resource answer the same text.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! resource kind is missing, which clause of a held resource's ARN the request altered
//! (partition, region, account, id), whether the resource never existed or has since
//! gone, and which of the three routes carried the call.
//!
//! The harness is the sibling suites' (`crate::datastore_lifecycle`): real requests
//! through `MultiServiceRouter::dispatch`.

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::DATASTORE_DELETION_DELAY;

use crate::datastore_lifecycle::{unheld_id, Answer, Emulator, ACCOUNT, SETTLE_MULTIPLE};
use crate::image_set_copy::{get_image_set, seed_image_set};
use crate::resource_tagging::{list_tags, tag, untag};

use serde_json::json;

/// The service's constant text for a data store it does not hold, as the wire corpus
/// records it from the id-keyed operations — the same sentence the tagging routes owe.
const DATASTORE_NOT_FOUND: &str = "Datastore resource is not found.";

/// The service's constant text for an image set a held data store does not own, stated
/// by resource kind exactly as [`DATASTORE_NOT_FOUND`] is.
const IMAGE_SET_NOT_FOUND: &str = "ImageSet resource is not found.";

/// The three routes `/tags/{resourceArn}` carries, one per method
/// (`smithy_query(query="digest")`: POST TagResource, GET ListTagsForResource,
/// DELETE UntagResource).
const TAGGING_ROUTES: [&str; 3] = ["POST", "GET", "DELETE"];

/// One tagging request on `route`, carrying the body or query that route requires.
async fn tagging_call(emulator: &Emulator, route: &str, arn: &str) -> Answer {
    match route {
        "POST" => tag(emulator, arn, json!({ "team": "radiology" })).await,
        "GET" => list_tags(emulator, arn).await,
        "DELETE" => untag(emulator, arn, &["team"]).await,
        other => panic!("unknown tagging route {other}"),
    }
}

/// One row of the absent-resource table: the ARN a tagging call names, and the sentence
/// the kind it fails to resolve is stated with.
#[derive(Debug)]
struct AbsentCase {
    case: &'static str,
    arn: String,
    expected: &'static str,
    /// Every identifier the request carried that the refusal must NOT reflect back.
    withheld: Vec<String>,
}

/// A well-formed `[0-9a-z]{32}` resource id that differs from [`unheld_id`], so a case
/// needing a SECOND absent id does not reuse the first.
fn other_unheld_id() -> String {
    "z".repeat(32)
}

/// RULE: a tagging call naming a resource this emulator does not hold is refused with the
/// constant sentence of the missing resource's KIND, with no identifier from the request
/// in it — on every one of the three routes.
#[tokio::test]
async fn a_tagging_call_on_an_absent_resource_is_refused_by_resource_kind() {
    let emulator = Emulator::new();

    let held_id = emulator.active_datastore("tag.kind", "token-tag-kind").await;
    let held_arn = emulator
        .get_datastore(&held_id)
        .await
        .string(&["datastoreProperties", "datastoreArn"]);
    let held_image_set = seed_image_set(&emulator, &held_id, 1);

    // A second held data store, so a row can name a REAL image set id under the data
    // store that does not own it.
    let other_id = emulator
        .active_datastore("tag.kind.other", "token-tag-kind-other")
        .await;
    let other_arn = emulator
        .get_datastore(&other_id)
        .await
        .string(&["datastoreProperties", "datastoreArn"]);
    let foreign_image_set = seed_image_set(&emulator, &other_id, 1);

    // A data store whose deletion the clock has since made due: gone, not never-there.
    let doomed_id = emulator
        .active_datastore("tag.kind.doomed", "token-tag-kind-doomed")
        .await;
    let doomed_arn = emulator
        .get_datastore(&doomed_id)
        .await
        .string(&["datastoreProperties", "datastoreArn"]);
    // No image set is seeded into it: a data store that still owns image sets cannot be
    // deleted (`ConflictException`), so this row's store has to be empty.
    let deleted = emulator.delete_datastore(&doomed_id).await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete the doomed store\": expected HTTP 200, got {deleted:?}"
    );
    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_DELETION_DELAY).await;

    let unminted = unheld_id();
    let unminted_arn = format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{unminted}");
    let rows: Vec<AbsentCase> = vec![
        AbsentCase {
            case: "a data store id nothing minted",
            arn: unminted_arn.clone(),
            expected: DATASTORE_NOT_FOUND,
            withheld: vec![unminted_arn.clone(), unminted.clone()],
        },
        AbsentCase {
            case: "an image set under a data store id nothing minted",
            arn: format!("{unminted_arn}/imageset/{}", other_unheld_id()),
            expected: DATASTORE_NOT_FOUND,
            withheld: vec![unminted.clone(), other_unheld_id()],
        },
        AbsentCase {
            case: "the held data store's ARN in another partition",
            arn: held_arn.replacen("arn:aws:", "arn:aws-cn:", 1),
            expected: DATASTORE_NOT_FOUND,
            withheld: vec![held_id.clone(), "aws-cn".to_string()],
        },
        AbsentCase {
            case: "the held data store's ARN in another region",
            arn: held_arn.replacen("us-east-1", "us-west-2", 1),
            expected: DATASTORE_NOT_FOUND,
            withheld: vec![held_id.clone(), "us-west-2".to_string()],
        },
        AbsentCase {
            case: "the held data store's ARN in another account",
            arn: held_arn.replacen(ACCOUNT, "123456789012", 1),
            expected: DATASTORE_NOT_FOUND,
            withheld: vec![held_id.clone(), "123456789012".to_string()],
        },
        AbsentCase {
            case: "a data store whose deletion fell due",
            arn: doomed_arn.clone(),
            expected: DATASTORE_NOT_FOUND,
            withheld: vec![doomed_arn.clone(), doomed_id.clone()],
        },
        AbsentCase {
            case: "an image set of a data store whose deletion fell due",
            arn: format!("{doomed_arn}/imageset/{}", other_unheld_id()),
            expected: DATASTORE_NOT_FOUND,
            withheld: vec![doomed_id.clone(), other_unheld_id()],
        },
        AbsentCase {
            case: "an image set id nothing minted, under a held data store",
            arn: format!("{held_arn}/imageset/{}", unheld_id()),
            expected: IMAGE_SET_NOT_FOUND,
            withheld: vec![held_id.clone(), unheld_id()],
        },
        AbsentCase {
            case: "a second absent image set id, under a held data store",
            arn: format!("{held_arn}/imageset/{}", other_unheld_id()),
            expected: IMAGE_SET_NOT_FOUND,
            withheld: vec![held_id.clone(), other_unheld_id()],
        },
        AbsentCase {
            case: "a real image set id, under the data store that does not own it",
            arn: format!("{held_arn}/imageset/{foreign_image_set}"),
            expected: IMAGE_SET_NOT_FOUND,
            withheld: vec![held_id.clone(), foreign_image_set.clone()],
        },
        AbsentCase {
            case: "the held image set, named under the other held data store",
            arn: format!("{other_arn}/imageset/{held_image_set}"),
            expected: IMAGE_SET_NOT_FOUND,
            withheld: vec![other_id.clone(), held_image_set.clone()],
        },
    ];

    for route in TAGGING_ROUTES {
        for row in &rows {
            let case = format!("{route} {}", row.case);
            let answer = tagging_call(&emulator, route, &row.arn).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
            );
            let message = answer.string(&["message"]);
            assert_eq!(
                message, row.expected,
                "case {case:?}: expected message {:?}, got {message:?}",
                row.expected
            );
            for withheld in &row.withheld {
                assert!(
                    !message.contains(withheld.as_str()),
                    "case {case:?}: {withheld:?} came from the request and must not appear, \
                     got {message:?}"
                );
            }
        }
    }

    // Both held resources are untouched by every refusal above: the data store and the
    // image set it owns still list their tags.
    for (case, arn) in [
        ("the held data store", held_arn.clone()),
        (
            "the held image set",
            format!("{held_arn}/imageset/{held_image_set}"),
        ),
    ] {
        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected the held resource to still list its tags, got {listed:?}"
        );
    }
}

/// RULE, the route-independent half: the sentence depends on the missing resource's kind
/// and NOT on the operation that failed to find it, so each tagging route answers an
/// absent resource with exactly the text the id-keyed read of that same resource answers.
#[tokio::test]
async fn the_tagging_routes_word_a_missing_resource_as_the_id_keyed_routes_do() {
    let emulator = Emulator::new();

    let held_id = emulator
        .active_datastore("tag.agree", "token-tag-agree")
        .await;
    let held_arn = emulator
        .get_datastore(&held_id)
        .await
        .string(&["datastoreProperties", "datastoreArn"]);

    let absent_datastore = unheld_id();
    let absent_datastore_arn =
        format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{absent_datastore}");
    let absent_image_set = other_unheld_id();

    // The id-keyed reads of the same two absent resources, which the tagging routes must
    // agree with: GetDatastore for the data store, GetImageSet for the image set.
    let by_id_datastore = emulator.get_datastore(&absent_datastore).await;
    assert_eq!(
        by_id_datastore.status, 404,
        "case \"GetDatastore on an absent store\": expected HTTP 404, got {by_id_datastore:?}"
    );
    let by_id_image_set = get_image_set(&emulator, &held_id, &absent_image_set, None).await;
    assert_eq!(
        by_id_image_set.status, 404,
        "case \"GetImageSet on an absent image set\": expected HTTP 404, got {by_id_image_set:?}"
    );

    let pairs: [(&str, String, String); 2] = [
        (
            "a data store",
            absent_datastore_arn,
            by_id_datastore.string(&["message"]),
        ),
        (
            "an image set",
            format!("{held_arn}/imageset/{absent_image_set}"),
            by_id_image_set.string(&["message"]),
        ),
    ];

    for (kind, arn, by_id_message) in &pairs {
        for route in TAGGING_ROUTES {
            let case = format!("{route} on {kind}");
            let answer = tagging_call(&emulator, route, arn).await;
            let message = answer.string(&["message"]);
            assert_eq!(
                message, *by_id_message,
                "case {case:?}: expected the id-keyed route's text {by_id_message:?}, \
                 got {message:?}"
            );
        }
    }
}
