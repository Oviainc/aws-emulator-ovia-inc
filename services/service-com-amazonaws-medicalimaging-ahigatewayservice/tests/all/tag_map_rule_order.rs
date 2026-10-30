// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ORDER the tag-map rules are applied in, and for where each
//! of them is answered from.
//!
//! RULE: a supplied tag map is judged by the rules of the MAP — its entry count against
//! the per-resource tag bound, and then its keys against every constraint the model puts
//! on `com.amazonaws.medicalimaging#TagKey` (`smithy_query(query="shape",
//! shape="TagMap,TagKey")`: len 1..128, `@pattern` `^(?!aws:)[a-zA-Z+-=._:/]+$`) — in ONE
//! order on every operation that installs one, and entirely before any resource is
//! resolved. So a map that breaks both rules is refused for its entry count whichever
//! operation carries it, and a map no resource could hold earns that same refusal whether
//! or not this emulator holds the resource the request names. How much room the named
//! resource has LEFT is a fact about that resource rather than about the map, so it is
//! answered only once the resource resolves — which is also why an ARN naming no held
//! resource still answers 404 for a map the map rules admit.
//!
//! The suites are TABLES whose rows differ along what the rule generalises over: how far
//! past the bound the map sits, which clause of `TagKey` the illegal key riding with it
//! breaks and where that key sorts among the legal ones, which operation carries the map,
//! which resource the ARN names (a data store, an image set inside it, and ARNs naming
//! neither), and how much room the target resource has left. Every case drives real HTTP
//! through the production front door, so what is asserted is the wire behaviour a
//! `medical-imaging` SDK client sees.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    too_many_tags_message, MAX_TAGS_PER_RESOURCE, TAG_KEY_MAX_LENGTH, TAG_KEY_RESERVED_PREFIX,
};

use crate::datastore_lifecycle::{unheld_id, Answer, Emulator, ACCOUNT};
use crate::image_set_copy::seed_image_set;
use crate::resource_tagging::{
    list_tags, tag, tag_object, unanchored_tag_key_pattern, untag, VALIDATION_ENVELOPE,
};

/// `n` distinct, legal tag keys — `key0`, `key1`, … — with a value each, as the sibling
/// suites build a legal map.
fn legal_pairs(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect()
}

/// A request body's tag map: `legal` legal keys plus `illegal`, whichever rules that
/// leaves broken.
fn map_with(legal: usize, illegal: &str) -> Value {
    let mut pairs = legal_pairs(legal);
    pairs.push((illegal.to_string(), "v".to_string()));
    tag_object(&pairs)
}

/// Assert `answer` is the ENTRY-COUNT refusal: HTTP 400 `ValidationException` stating the
/// bound and the count, and NOT a clause about a key — which is what tells the two rule
/// orders apart for a map that breaks both.
fn assert_count_refusal(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected error code ValidationException, got {answer:?}"
    );
    let message = answer.string(&["message"]);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the entry-count clause {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(unanchored_tag_key_pattern()),
        "case {case:?}: expected the entry-count clause rather than a key clause, \
         got {message:?}"
    );
}

/// Assert `answer` is a KEY refusal: HTTP 400 `ValidationException` in this service's
/// validation envelope, stating the constraint a key failed and nothing about the bound.
fn assert_key_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected error code ValidationException, got {answer:?}"
    );
    let message = answer.string(&["message"]);
    assert!(
        message.starts_with(VALIDATION_ENVELOPE),
        "case {case:?}: expected the message to open with {VALIDATION_ENVELOPE:?}, \
         got {message:?}"
    );
    assert!(
        message.contains("at 'tags'") && message.contains("failed to satisfy constraint"),
        "case {case:?}: expected a constraint clause naming the map member, got {message:?}"
    );
    assert!(
        !message.contains("tags per resource"),
        "case {case:?}: expected a key clause rather than the entry-count clause, \
         got {message:?}"
    );
}

/// One resource a `TagResource` call can be addressed to. A rule of the MAP may not turn
/// on which of them the ARN names, nor on whether this emulator holds it.
#[derive(Debug)]
struct Target {
    case: &'static str,
    arn: String,
}

/// The four resources a tagging ARN names, as the sibling suite spells them
/// (`tag_resource_key_pattern`): a held data store, a held image set inside it, and ARNs
/// naming neither. Answers the table together with the held data store's own ARN and the
/// held image set's, so a case can read either map back.
async fn targets(emulator: &Emulator, name: &str, token: &str) -> (String, String, Vec<Target>) {
    let datastore_id = emulator.active_datastore(name, token).await;
    let read = emulator.get_datastore(&datastore_id).await;
    assert_eq!(
        read.status, 200,
        "reading back {datastore_id}: expected HTTP 200, got {read:?}"
    );
    let datastore_arn = read.string(&["datastoreProperties", "datastoreArn"]);
    // Only a DICOM import mints an image set, which this emulator does not serve yet, so
    // the image set is seeded into the shared engine the way the sibling suites seed one.
    let image_set_id = seed_image_set(emulator, &datastore_id, 1);
    let image_set_arn = format!("{datastore_arn}/imageset/{image_set_id}");

    let table = vec![
        Target {
            case: "a held data store",
            arn: datastore_arn.clone(),
        },
        Target {
            case: "a held image set inside it",
            arn: image_set_arn.clone(),
        },
        Target {
            case: "a data store this emulator does not hold",
            arn: format!(
                "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{}",
                unheld_id()
            ),
        },
        Target {
            case: "an image set the held data store does not own",
            arn: format!("{datastore_arn}/imageset/{}", unheld_id()),
        },
    ];
    (datastore_arn, image_set_arn, table)
}

/// One tag map, how many entries it carries, and which clause of `TagKey` the key riding
/// with it breaks.
#[derive(Debug)]
struct Row {
    case: String,
    tags: Value,
    count: usize,
}

/// A row from the number of LEGAL keys and the illegal one travelling with them.
fn row(case: &str, legal: usize, illegal: String) -> Row {
    Row {
        case: format!("{case} ({} entries)", legal + 1),
        tags: map_with(legal, &illegal),
        count: legal + 1,
    }
}

/// The maps that break BOTH map rules: more entries than a resource may hold AND one key
/// the model refuses. Each is the input on which a path applying the key rule first and a
/// path applying the count rule first visibly disagree.
///
/// The illegal keys are clauses of `TagKey`, from the shape the map's keys target
/// (`smithy_query(query="shape",shape="TagMap,TagKey")`): its `@pattern`'s reserved-`aws:`
/// lookahead, a character outside its class, and its `@length` maximum. Two of them sort
/// BEFORE the legal `key…` keys and two after, because the key rule reports the
/// sorted-first illegal key — so sort position is the axis on which the two orders would
/// report different keys as well as different rules. The row sizes span the first map past
/// the bound, two past it, and far past it.
fn double_breaches() -> Vec<Row> {
    vec![
        row(
            "the reserved aws: prefix, sorting first",
            MAX_TAGS_PER_RESOURCE,
            format!("{TAG_KEY_RESERVED_PREFIX}owner"),
        ),
        row(
            "the reserved prefix alone, sorting first",
            MAX_TAGS_PER_RESOURCE + 1,
            TAG_KEY_RESERVED_PREFIX.to_string(),
        ),
        row(
            "a space, outside the character class, sorting last",
            MAX_TAGS_PER_RESOURCE,
            "owning team".to_string(),
        ),
        row(
            "one character past the length maximum, sorting last",
            MAX_TAGS_PER_RESOURCE * 4,
            "k".repeat(TAG_KEY_MAX_LENGTH + 1),
        ),
    ]
}

/// RULE: for a map that breaks both tag-map rules, every write path reports the SAME
/// rule — the entry count, which is applied before the keys — and reports it before any
/// resource is resolved, so the create path, the tag path onto a held resource and the tag
/// path onto an ARN naming no held resource all answer one text. Nothing is installed and
/// nothing is minted.
#[tokio::test]
async fn a_map_breaking_both_rules_is_refused_for_its_entry_count_on_every_write_path() {
    let emulator = Emulator::new();
    let (datastore_arn, image_set_arn, table) =
        targets(&emulator, "map.order", "map-order-token").await;

    for (index, breach) in double_breaches().into_iter().enumerate() {
        let expected = too_many_tags_message(breach.count);

        // The create path, which installs the map as a store is minted.
        let created = emulator
            .create_datastore(json!({
                "datastoreName": format!("map.order.{index}"),
                "clientToken": format!("map-order-{index}"),
                "tags": breach.tags.clone(),
            }))
            .await;
        let case = format!("{} via CreateDatastore", breach.case);
        assert_count_refusal(&case, &created, &expected);
        assert!(
            created.field(&["datastoreId"]).is_null(),
            "case {case:?}: expected no datastoreId in a refusal, got {created:?}"
        );

        // And the tag path, onto every resource an ARN can name: the map is judged before
        // the resource is, so none of them changes the answer.
        for target in &table {
            let case = format!("{} via TagResource onto {}", breach.case, target.case);
            let refused = tag(&emulator, &target.arn, breach.tags.clone()).await;
            assert_count_refusal(&case, &refused, &expected);
        }
    }

    // Both held resources are untouched: every call above was refused.
    for (case, arn) in [
        ("the held data store", &datastore_arn),
        ("the held image set", &image_set_arn),
    ] {
        let listed = list_tags(&emulator, arn).await;
        assert_eq!(
            listed.field(&["tags"]),
            &json!({}),
            "case {case:?}: expected the refusals to install nothing, got {listed:?}"
        );
    }
}

/// RULE (the second rule still applies): a map whose entry count is within the bound is
/// judged by its KEYS, on every write path and whichever resource the ARN names — the
/// count rule running first swallows nothing. Rows span a one-entry map, a map at the
/// bound, and each clause of `TagKey` an illegal key can break.
#[tokio::test]
async fn a_map_within_the_bound_is_still_judged_by_its_keys_on_every_write_path() {
    let emulator = Emulator::new();
    let (datastore_arn, image_set_arn, table) =
        targets(&emulator, "map.keys", "map-keys-token").await;

    let rows = vec![
        row(
            "the reserved aws: prefix, alone in the map",
            0,
            format!("{TAG_KEY_RESERVED_PREFIX}owner"),
        ),
        row(
            "a space, outside the character class, in a map at the bound",
            MAX_TAGS_PER_RESOURCE - 1,
            "owning team".to_string(),
        ),
        row(
            "one character past the length maximum, in a map at the bound",
            MAX_TAGS_PER_RESOURCE - 1,
            "k".repeat(TAG_KEY_MAX_LENGTH + 1),
        ),
    ];

    for (index, row) in rows.into_iter().enumerate() {
        assert!(
            row.count <= MAX_TAGS_PER_RESOURCE,
            "case {:?}: this table's rows are within the bound, got {} entries",
            row.case,
            row.count
        );

        let created = emulator
            .create_datastore(json!({
                "datastoreName": format!("map.keys.{index}"),
                "clientToken": format!("map-keys-{index}"),
                "tags": row.tags.clone(),
            }))
            .await;
        assert_key_refusal(&format!("{} via CreateDatastore", row.case), &created);

        for target in &table {
            let case = format!("{} via TagResource onto {}", row.case, target.case);
            let refused = tag(&emulator, &target.arn, row.tags.clone()).await;
            assert_key_refusal(&case, &refused);
        }
    }

    for (case, arn) in [
        ("the held data store", &datastore_arn),
        ("the held image set", &image_set_arn),
    ] {
        let listed = list_tags(&emulator, arn).await;
        assert_eq!(
            listed.field(&["tags"]),
            &json!({}),
            "case {case:?}: expected the refusals to install nothing, got {listed:?}"
        );
    }
}

/// RULE (where each rule is answered from): once the map's own rules admit it, what is
/// left is the ROOM the named resource has — a fact about the resource, so it is answered
/// from the resource's current map, and an ARN naming no held resource answers 404 instead.
/// Rows differ along how many keys the request carries and how many of them the resource
/// already holds, around a resource two entries short of the bound.
#[tokio::test]
async fn the_room_a_resource_has_left_is_answered_once_the_map_itself_is_legal() {
    let emulator = Emulator::new();
    let (arn, _image_set_arn, _table) = targets(&emulator, "map.room", "map-room-token").await;

    // The baseline this suite returns to between rows: two entries short of the bound.
    let baseline = legal_pairs(MAX_TAGS_PER_RESOURCE - 2);
    let seeded = tag(&emulator, &arn, tag_object(&baseline)).await;
    assert_eq!(
        seeded.status, 200,
        "seeding {} tags: expected HTTP 200, got {seeded:?}",
        baseline.len()
    );

    // (the case, the keys the request carries, whether the resource has room for them)
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        (
            "one new key, landing one short of the bound",
            vec!["new0".to_string()],
            true,
        ),
        (
            "two new keys, landing exactly on the bound",
            vec!["new0".to_string(), "new1".to_string()],
            true,
        ),
        (
            "three new keys, one past the bound",
            vec!["new0".to_string(), "new1".to_string(), "new2".to_string()],
            false,
        ),
        (
            "one key already held and three new, one past the bound",
            vec![
                "key0".to_string(),
                "new0".to_string(),
                "new1".to_string(),
                "new2".to_string(),
            ],
            false,
        ),
        (
            "two keys already held and one new, which grows the map by one",
            vec!["key0".to_string(), "key1".to_string(), "new0".to_string()],
            true,
        ),
    ];

    for (case, keys, admitted) in rows {
        let pairs: Vec<(String, String)> = keys
            .iter()
            .map(|key| (key.clone(), "v".to_string()))
            .collect();
        let answer = tag(&emulator, &arn, tag_object(&pairs)).await;

        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {answer:?}"
            );
            // Back to the baseline for the next row.
            let added: Vec<&str> = keys
                .iter()
                .filter(|key| key.starts_with("new"))
                .map(String::as_str)
                .collect();
            let removed = untag(&emulator, &arn, &added).await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: restoring the baseline, got {removed:?}"
            );
        } else {
            // The count the ROOM rule states is the one the resource would end up with,
            // because this rule is measured against the resource's own map.
            assert_count_refusal(
                case,
                &answer,
                &too_many_tags_message(MAX_TAGS_PER_RESOURCE + 1),
            );
        }

        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.field(&["tags"]).as_object().map(serde_json::Map::len),
            Some(baseline.len()),
            "case {case:?}: expected the baseline map of {} tags, got {listed:?}",
            baseline.len()
        );
    }

    // From the other side: a map the map rules admit meets the RESOURCE, so an ARN naming
    // none of one is the not-found refusal …
    let unheld = format!(
        "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{}",
        unheld_id()
    );
    let legal = tag(
        &emulator,
        &unheld,
        tag_object(&[("team".to_string(), "radiology".to_string())]),
    )
    .await;
    assert_eq!(
        legal.status, 404,
        "a legal map onto a data store this emulator does not hold: expected HTTP 404, \
         got {legal:?}"
    );
    assert_eq!(
        legal.error_code, "ResourceNotFoundException",
        "a legal map onto a data store this emulator does not hold: expected \
         ResourceNotFoundException, got {legal:?}"
    );

    // … while a map past the bound never reaches a resource at all, so it is the
    // entry-count refusal, stating the count the MAP carries, even with every key legal.
    let oversized = legal_pairs(MAX_TAGS_PER_RESOURCE + 1);
    let refused = tag(&emulator, &unheld, tag_object(&oversized)).await;
    assert_count_refusal(
        "a map past the bound onto a data store this emulator does not hold",
        &refused,
        &too_many_tags_message(MAX_TAGS_PER_RESOURCE + 1),
    );
}
