// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag-key rule on the REMOVAL path: `UntagResource`.
//!
//! RULE: every element of `UntagResource.tagKeys` is a
//! `com.amazonaws.medicalimaging#TagKey`
//! (`smithy_query(query="shape",shape="TagKeyList,TagKey")`: len 1..128, `@pattern`
//! `^(?!aws:)[a-zA-Z+-=._:/]+$`, whose leading negative lookahead reserves the `aws:`
//! namespace), so a request naming a key that fails any of those constraints is refused
//! with HTTP 400 `ValidationException` — whatever POSITION the key holds in the list,
//! however many keys travel with it, and whether or not the resource carries it — and
//! the refused call removes nothing. The refusal is a constraint refusal stated from the
//! request: it names the failing element's path (`tagKeys.<1-based index>.member`) and
//! the constraint it failed, in this service's one validation envelope.
//!
//! The sibling suite `tag_map_validation` covers the same key rule on the two WRITE
//! paths (`CreateDatastore`, `TagResource`) with one key per request; this suite is the
//! removal path's own axes, which a tag MAP has none of: the index of the illegal key,
//! the number of keys around it, and the legal keys that must survive the refusal.
//! Every case drives real HTTP through the production front door.

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    TAG_KEY_PATTERN, TAG_KEY_RESERVED_PREFIX,
};

use crate::datastore_lifecycle::{Answer, Emulator, ACCOUNT};
use crate::resource_tagging::{
    illegal_tag_keys, list_tags, tag, tag_object, untag, VALIDATION_ENVELOPE,
};

/// Assert `answer` is the modelled refusal of an illegal tag key: HTTP 400,
/// `ValidationException`, and a message in this service's validation envelope that names
/// the failing element of `tagKeys` and what it failed.
fn assert_refused(case: &str, answer: &Answer, expected_path: &str) {
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
        "case {case:?}: expected the message to open with {VALIDATION_ENVELOPE:?}, got {message:?}"
    );
    assert!(
        message.contains(&format!("at '{expected_path}'")),
        "case {case:?}: expected the refusal to name {expected_path:?}, got {message:?}"
    );
    assert!(
        message.contains("failed to satisfy constraint"),
        "case {case:?}: expected a constraint clause, got {message:?}"
    );
}

/// A data store this emulator holds, and its ARN as the store itself reports it.
async fn held_datastore_arn(emulator: &Emulator, name: &str, token: &str) -> String {
    let datastore_id = emulator.active_datastore(name, token).await;
    let read = emulator.get_datastore(&datastore_id).await;
    assert_eq!(
        read.status, 200,
        "reading back {datastore_id}: expected 200, got {read:?}"
    );
    read.string(&["datastoreProperties", "datastoreArn"])
}

/// RULE (the refusal, and what it states): a `tagKeys` element outside `TagKey`'s
/// modelled constraints is refused with HTTP 400 `ValidationException` naming that
/// element and the constraint it failed — a `@pattern` quoted without its anchors, a
/// `@length` by its bound. The rows are the shared illegal-key corpus
/// (`resource_tagging::illegal_tag_keys`), one per clause of `TagKey` a key can break.
#[tokio::test]
async fn an_illegal_tag_key_is_refused_by_untag_resource_with_the_constraint_it_failed() {
    let emulator = Emulator::new();
    let arn = held_datastore_arn(&emulator, "untag.refusal", "untag-refusal-token").await;

    for row in illegal_tag_keys() {
        let refused = untag(&emulator, &arn, &[row.key.as_str()]).await;
        assert_refused(row.case, &refused, "tagKeys.1.member");
        let message = refused.string(&["message"]);
        assert!(
            message.contains(&row.constraint),
            "case {:?}: expected the refusal to state {:?}, got {message:?}",
            row.case,
            row.constraint
        );
        assert!(
            !message.contains(TAG_KEY_PATTERN),
            "case {:?}: expected any pattern quoted without its anchors, got {message:?}",
            row.case
        );
    }
}

/// RULE (the position axis): the rule is the LIST ELEMENT's, so it holds at every index —
/// an illegal key is refused first, last or in the middle of a list of any size, and the
/// refusal names the 1-based index it sat at. Rows differ along list length and the index
/// the illegal key holds.
#[tokio::test]
async fn an_illegal_tag_key_is_refused_wherever_it_sits_in_the_list() {
    let emulator = Emulator::new();
    let arn = held_datastore_arn(&emulator, "untag.position", "untag-position-token").await;

    // The tags the resource holds throughout: every refusal below must leave this map.
    let held: Vec<(String, String)> = (0..6)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    let seeded = tag(&emulator, &arn, tag_object(&held)).await;
    assert_eq!(seeded.status, 200, "seeding six legal tags: got {seeded:?}");

    let illegal = format!("{TAG_KEY_RESERVED_PREFIX}owner");
    // (how many keys the list carries, the 0-based index the illegal key holds)
    let rows: [(usize, usize); 7] = [(1, 0), (2, 0), (2, 1), (3, 1), (6, 0), (6, 2), (6, 5)];

    for (length, index) in rows {
        let case = format!("{length} keys, the illegal one at index {index}");
        let mut keys: Vec<String> = (0..length).map(|n| format!("key{n}")).collect();
        keys[index] = illegal.clone();
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();

        let refused = untag(&emulator, &arn, &borrowed).await;
        assert_refused(&case, &refused, &format!("tagKeys.{}.member", index + 1));

        // Nothing was removed: the legal keys that travelled with the illegal one are
        // still there, so the refusal is all-or-nothing.
        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.field(&["tags"]),
            &tag_object(&held),
            "case {case:?}: expected the refusal to remove nothing, got {listed:?}"
        );
    }

    // The control: the same list with every key legal removes exactly those keys.
    let removable: Vec<&str> = vec!["key0", "key2", "key5"];
    let removed = untag(&emulator, &arn, &removable).await;
    assert_eq!(
        removed.status, 200,
        "the all-legal control: expected HTTP 200, got {removed:?}"
    );
    let remaining: Vec<(String, String)> = held
        .iter()
        .filter(|(key, _)| !removable.contains(&key.as_str()))
        .cloned()
        .collect();
    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed.field(&["tags"]),
        &tag_object(&remaining),
        "the all-legal control: expected exactly the named keys removed, got {listed:?}"
    );
}

/// RULE (the ordering axis): a `tagKeys` element is refused for what it IS, not for
/// whether the resource carries it — so the constraint decides before the resource's tag
/// map is consulted at all. A well-formed ARN naming nothing this emulator holds is a 404
/// for a legal key and a 400 `ValidationException` for an illegal one.
#[tokio::test]
async fn an_illegal_tag_key_is_refused_before_the_resource_is_resolved() {
    let emulator = Emulator::new();

    // A well-formed `[0-9a-z]{32}` data store id no call ever mints.
    let unheld = format!(
        "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{}",
        "0".repeat(32)
    );

    let absent = untag(&emulator, &unheld, &["team"]).await;
    assert_eq!(
        absent.status, 404,
        "a legal key on an unheld ARN: expected HTTP 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code, "ResourceNotFoundException",
        "a legal key on an unheld ARN: expected ResourceNotFoundException, got {absent:?}"
    );

    for row in illegal_tag_keys() {
        let refused = untag(&emulator, &unheld, &[row.key.as_str()]).await;
        assert_refused(
            &format!("{}, on an ARN naming nothing held", row.case),
            &refused,
            "tagKeys.1.member",
        );
    }

    // And a key the resource does not carry is still refused when it is illegal, where a
    // legal one is a success that changes nothing.
    let arn = held_datastore_arn(&emulator, "untag.order", "untag-order-token").await;
    let unknown = untag(&emulator, &arn, &["nobody-set-this"]).await;
    assert_eq!(
        unknown.status, 200,
        "a legal key the resource does not carry: expected HTTP 200, got {unknown:?}"
    );
    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed.field(&["tags"]),
        &json!({}),
        "a legal key the resource does not carry: expected an empty map, got {listed:?}"
    );
}
