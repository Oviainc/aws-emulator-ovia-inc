// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag-key rule on the `TagResource` route, over the two axes
//! that route has of its own: WHAT the refusal states, and WHICH resource the
//! `resourceArn` names.
//!
//! RULE: a key a tag map supplies is a `com.amazonaws.medicalimaging#TagKey`
//! (`smithy_query(query="shape",shape="TagMap,TagKey")`: len 1..128, `@pattern`
//! `^(?!aws:)[a-zA-Z+-=._:/]+$`, whose leading negative lookahead reserves the `aws:`
//! namespace), so a `TagResource` request carrying a key that fails any of those
//! constraints is refused with HTTP 400 `ValidationException` — in this service's one
//! validation envelope, naming the map member, the key it refused and the constraint that
//! key failed — and installs nothing. A constraint is decided from the REQUEST alone, so
//! the answer does not turn on which resource kind the ARN names or on whether this
//! emulator holds that resource at all.
//!
//! The sibling suites cover the other halves of the same key rule: `tag_map_validation`
//! the two write paths with one key per request and the per-resource tag ceiling,
//! `untag_key_validation` the removal path's list positions. This suite is the write
//! path's own two axes, which neither of those drives: the refusal's TEXT per clause of
//! `TagKey`, and the resource the tagged ARN names — a data store, one image set inside
//! it, and ARNs naming neither — each with the legal control its row needs, read back
//! through `ListTagsForResource`. Every case drives real HTTP through the production
//! front door.

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::TAG_KEY_PATTERN;

use crate::datastore_lifecycle::{unheld_id, Answer, Emulator, ACCOUNT};
use crate::image_set_copy::seed_image_set;
use crate::resource_tagging::{
    illegal_tag_keys, list_tags, tag, tag_object, IllegalTagKey, VALIDATION_ENVELOPE,
};

/// A one-entry tag map keyed by `key`, built the way the sibling suites build a request
/// body (`resource_tagging::tag_object`). The rule under test is about the KEY; a tag
/// VALUE is the map's element, which the boundary validator already enforces.
fn one_tag(key: &str) -> serde_json::Value {
    tag_object(&[(key.to_string(), "v".to_string())])
}

/// Assert `answer` refuses `row` stating everything a constraint refusal states: HTTP 400
/// `ValidationException`, this service's validation envelope, the map member the key was
/// supplied at, the key itself, and the constraint that key failed — with any `@pattern`
/// quoted without the anchors the model spells it with.
fn assert_states_constraint(case: &str, answer: &Answer, row: &IllegalTagKey) {
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
        message.contains("at 'tags'"),
        "case {case:?}: expected the refusal to name the map member, got {message:?}"
    );
    assert!(
        message.contains(&format!("Value '{}'", row.key)),
        "case {case:?}: expected the refusal to carry the key it refused, got {message:?}"
    );
    assert!(
        message.contains("failed to satisfy constraint"),
        "case {case:?}: expected a constraint clause, got {message:?}"
    );
    assert!(
        message.contains(&row.constraint),
        "case {case:?}: expected the refusal to state {:?}, got {message:?}",
        row.constraint
    );
    assert!(
        !message.contains(TAG_KEY_PATTERN),
        "case {case:?}: expected any pattern quoted without its anchors, got {message:?}"
    );
}

/// One resource a `TagResource` call can be addressed to: the ARN, and whether this
/// emulator holds the resource it names — which is what a LEGAL key's answer turns on
/// (200 when held, 404 when not), while an ILLEGAL key's answer must not turn on it.
#[derive(Debug)]
struct Target {
    case: &'static str,
    arn: String,
    held: bool,
}

/// RULE (both axes at once): a `TagResource` request carrying a key outside `TagKey`'s
/// modelled constraints is refused with HTTP 400 `ValidationException` naming the map
/// member, the key and the constraint that key failed — whichever resource the ARN names,
/// and whether or not this emulator holds it, where a legal key is a 404. The legal
/// control proves the refusals are not a blanket one: on a held resource it is accepted
/// and reads back verbatim through `ListTagsForResource`, from that resource's own map.
#[tokio::test]
async fn the_tag_key_refusal_states_its_constraint_on_every_resource_a_tagging_arn_names() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("tag.resource.axis", "tag-resource-axis-token")
        .await;
    let read = emulator.get_datastore(&datastore_id).await;
    assert_eq!(
        read.status, 200,
        "reading back {datastore_id}: expected HTTP 200, got {read:?}"
    );
    let arn = read.string(&["datastoreProperties", "datastoreArn"]);
    assert_eq!(
        arn,
        format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{datastore_id}"),
        "reading back {datastore_id}: expected the canonical data store ARN, got {read:?}"
    );
    // Only a DICOM import mints an image set, which this emulator does not serve yet, so
    // the image set is seeded into the shared engine the way the sibling suites seed one.
    let image_set_id = seed_image_set(&emulator, &datastore_id, 1);

    let targets = vec![
        Target {
            case: "a held data store",
            arn: arn.clone(),
            held: true,
        },
        Target {
            case: "a held image set inside it",
            arn: format!("{arn}/imageset/{image_set_id}"),
            held: true,
        },
        Target {
            case: "a data store this emulator does not hold",
            arn: format!(
                "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{}",
                unheld_id()
            ),
            held: false,
        },
        Target {
            case: "an image set the held data store does not own",
            arn: format!("{arn}/imageset/{}", unheld_id()),
            held: false,
        },
    ];

    // The one legal pair every control row installs, in punctuation the class admits.
    let control = vec![("owning_team".to_string(), "radiology".to_string())];

    for target in &targets {
        for row in illegal_tag_keys() {
            let case = format!("{}, {}", target.case, row.case);
            let refused = tag(&emulator, &target.arn, one_tag(&row.key)).await;
            assert_states_constraint(&case, &refused, &row);
        }

        let answer = tag(&emulator, &target.arn, tag_object(&control)).await;
        if target.held {
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected a legal key to answer HTTP 200, got {answer:?}",
                target.case
            );
            let listed = list_tags(&emulator, &target.arn).await;
            assert_eq!(
                listed.field(&["tags"]),
                &tag_object(&control),
                "case {:?}: expected the legal key to read back verbatim, got {listed:?}",
                target.case
            );
        } else {
            assert_eq!(
                answer.status, 404,
                "case {:?}: expected a legal key on an unheld resource to answer HTTP 404, \
                 got {answer:?}",
                target.case
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {:?}: expected ResourceNotFoundException, got {answer:?}",
                target.case
            );
        }
    }

    // Each held resource carries its OWN map: the controls above wrote one tag to the data
    // store and one to the image set, and no refusal left anything in either.
    for (case, held_arn) in [
        ("the held data store", arn.clone()),
        ("the held image set", format!("{arn}/imageset/{image_set_id}")),
    ] {
        let listed = list_tags(&emulator, &held_arn).await;
        assert_eq!(
            listed.field(&["tags"]),
            &tag_object(&control),
            "case {case:?}: expected only the legal control tag, got {listed:?}"
        );
    }
}
