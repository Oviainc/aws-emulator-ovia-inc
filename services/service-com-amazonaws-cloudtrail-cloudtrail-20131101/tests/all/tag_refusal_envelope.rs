// SPDX-License-Identifier: CC0-1.0

//! The ENVELOPE a tag refusal leaves in, end to end over the wire.
//!
//! RULE: a tag key or value outside the `@length` its modelled type declares is
//! refused — before the operation runs — as the modelled
//! `InvalidTagParameterException` (HTTP 400), rendered from that SHAPE, so the prose
//! arrives in the `Message` member every CloudTrail error shape carries and in no
//! other key; the sentence names the part of the tag (`key` / `value`) and the bound
//! it broke, and the refused request writes nothing.
//!
//! The rule is the TAG's, not one operation's, so every row runs against every
//! operation whose input carries a tag list, and the rows differ in which part of the
//! tag breaks its bound, by how far, how many tags the request carries and which
//! position in the list the offending tag sits at.
//!
//! Why this is asserted on the body KEY and not only on the code: the framework's
//! pre-dispatch envelope — which is what a constraint refusal renders through unless a
//! service answers ahead of it — writes its prose into a lowercase `message` and names
//! the type `ValidationException`, a shape this model does not declare. A client
//! deserializing the modelled error finds no `Message` in that body at all, so the
//! refusal is not merely mis-worded: the reason is invisible to it.

use serde_json::{json, Value};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// `TagKey`'s modelled length maximum, from
/// `smithy_query(query="shape", shape="TagKey")`: `len 1..128`.
const TAG_KEY_MAX_LEN: usize = 128;

/// `TagValue`'s modelled length maximum, from
/// `smithy_query(query="shape", shape="TagValue")`: `len 1..256`.
const TAG_VALUE_MAX_LEN: usize = 256;

/// The modelled error a tag outside its length is refused with, and the status its
/// `@httpError` declares (`smithy_query(query="operation", operation="AddTags")`).
const INVALID_TAG_PARAMETER: &str = "InvalidTagParameterException";
/// See [`INVALID_TAG_PARAMETER`].
const INVALID_TAG_PARAMETER_STATUS: u16 = 400;

/// One tag, as a request carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// Every operation whose input carries a tag list, with the member the list is bound
/// to — the `TagsList` of the tagging pair and of the three creates that seed tags,
/// and `CreateChannel`'s `Tags`
/// (`smithy_query(query="operation", operation="AddTags,RemoveTags,CreateTrail,CreateDashboard,CreateEventDataStore,CreateChannel")`,
/// each of which declares `InvalidTagParameterException`).
const TAG_CARRYING_OPERATIONS: [(&str, &str); 6] = [
    ("AddTags", "TagsList"),
    ("RemoveTags", "TagsList"),
    ("CreateTrail", "TagsList"),
    ("CreateDashboard", "TagsList"),
    ("CreateEventDataStore", "TagsList"),
    ("CreateChannel", "Tags"),
];

/// The request body `operation` is called with, carrying `tags` under the member that
/// operation binds its tag list to, and every other member the model requires of it.
///
/// `suffix` keeps the resources a row would create distinct from the other rows', so a
/// row that is NOT refused creates its own resource rather than colliding with one an
/// earlier row made.
fn request(operation: &str, member: &str, resource_id: &str, suffix: &str, tags: &[Value]) -> Value {
    let mut body = match operation {
        "AddTags" | "RemoveTags" => json!({"ResourceId": resource_id}),
        "CreateTrail" => json!({
            "Name": format!("tag-envelope-{suffix}"),
            "S3BucketName": "an-emulator-bucket",
        }),
        "CreateDashboard" | "CreateEventDataStore" => {
            json!({"Name": format!("tag-envelope-{suffix}")})
        }
        "CreateChannel" => json!({
            "Name": format!("tag-envelope-{suffix}"),
            "Source": "custom",
            "Destinations": [{
                "Type": "EVENT_DATA_STORE",
                "Location": "arn:aws:cloudtrail:us-east-1:000000000000:eventdatastore/abc",
            }],
        }),
        other => panic!("no request body is written for {other}"),
    };
    body[member] = Value::Array(tags.to_vec());
    body
}

/// The tag set `ListTags` reports for `resource_id`.
async fn tags_of(emulator: &Emulator, case: &str, resource_id: &str) -> Value {
    let reply = emulator
        .call("ListTags", &json!({"ResourceIdList": [resource_id]}))
        .await;
    reply
        .expect_ok(case)
        .get("ResourceTagList")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(|entry| entry.get("TagsList"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Assert `reply` is the modelled tag refusal, carrying `message` in the member the
/// modelled shape declares and nowhere else.
fn expect_modelled_tag_refusal(reply: &Reply, case: &str, message: &str) {
    reply.expect_error(case, INVALID_TAG_PARAMETER_STATUS, INVALID_TAG_PARAMETER);
    let carried = reply.body.get("Message").and_then(Value::as_str);
    assert_eq!(
        carried,
        Some(message),
        "case {case:?}: expected the refusal in the modelled `Message` member as \
         {message:?}, got {reply:?}"
    );
    let framework_key = reply.body.get("message");
    assert!(
        framework_key.is_none(),
        "case {case:?}: expected the refusal to be rendered from the modelled shape, \
         whose member is `Message`, not from the framework's pre-dispatch envelope; \
         got a lowercase `message` of {framework_key:?} in {reply:?}"
    );
}

/// RULE: a tag key or value past its modelled `@length` maximum is refused as the
/// modelled `InvalidTagParameterException`, in this service's own error envelope, by
/// every operation that carries a tag list — and the refused request writes nothing.
#[tokio::test]
async fn an_over_long_tag_is_refused_in_the_modelled_envelope() {
    let emulator = Emulator::new();

    // The resource the tagging pair addresses, carrying a tag set the refused calls
    // must leave exactly as it is.
    let seeded = json!([tag("seeded", "1")]);
    let resource_id = emulator
        .call(
            "CreateTrail",
            &json!({
                "Name": "tag-envelope-trail",
                "S3BucketName": "an-emulator-bucket",
                "TagsList": seeded,
            }),
        )
        .await
        .expect_ok("the trail the rows tag")
        .get("TrailARN")
        .and_then(Value::as_str)
        .expect("the create reports the trail ARN")
        .to_string();

    // (case, the tag list the request carries, the refusal the service states)
    let over_long: Vec<(&str, Vec<Value>, String)> = vec![
        (
            "a key one character past its maximum",
            vec![tag(&"k".repeat(TAG_KEY_MAX_LEN + 1), "v")],
            format!("Tag key length cannot be more than {TAG_KEY_MAX_LEN} characters."),
        ),
        (
            "a key far past its maximum",
            vec![tag(&"k".repeat(TAG_KEY_MAX_LEN * 8), "v")],
            format!("Tag key length cannot be more than {TAG_KEY_MAX_LEN} characters."),
        ),
        (
            "a value one character past its maximum",
            vec![tag("k", &"v".repeat(TAG_VALUE_MAX_LEN + 1))],
            format!("Tag value length cannot be more than {TAG_VALUE_MAX_LEN} characters."),
        ),
        (
            "a value far past its maximum",
            vec![tag("k", &"v".repeat(TAG_VALUE_MAX_LEN * 8))],
            format!("Tag value length cannot be more than {TAG_VALUE_MAX_LEN} characters."),
        ),
        (
            // The offending tag is the THIRD element, so the refusal is read off a
            // list position the first row never reaches.
            "an over-long key on the third tag of a longer list",
            vec![
                tag("a", "1"),
                tag("b", "2"),
                tag(&"k".repeat(TAG_KEY_MAX_LEN + 1), "v"),
                tag("d", "4"),
            ],
            format!("Tag key length cannot be more than {TAG_KEY_MAX_LEN} characters."),
        ),
        (
            "an over-long value on the second tag of a longer list",
            vec![
                tag("a", "1"),
                tag("b", &"v".repeat(TAG_VALUE_MAX_LEN + 1)),
                tag("c", "3"),
            ],
            format!("Tag value length cannot be more than {TAG_VALUE_MAX_LEN} characters."),
        ),
    ];

    for (index, (case, tags, message)) in over_long.iter().enumerate() {
        for (operation, member) in TAG_CARRYING_OPERATIONS {
            let suffix = format!("{operation}-{index}");
            let reply = emulator
                .call_in(
                    operation,
                    HOME_REGION,
                    &request(operation, member, &resource_id, &suffix, tags),
                )
                .await;
            expect_modelled_tag_refusal(&reply, &format!("{operation}: {case}"), message);
        }
    }

    // A refused request is not a partial one: the trail's tag set is the seeded one,
    // neither added to by the refused `AddTags` calls nor emptied by the refused
    // `RemoveTags` calls.
    let held = tags_of(&emulator, "after every refused call", &resource_id).await;
    assert_eq!(
        held, seeded,
        "expected the refused calls to leave the seeded tag set, got {held:?}"
    );
}

/// RULE (the other side of it): a tag INSIDE its modelled length is not a tag
/// refusal — the layer that states the refusal hands the request on to the operation,
/// which answers for itself.
///
/// The rows sit AT each published bound (the largest key and value a caller may send)
/// and at an ordinary tag, over every operation that carries a tag list, so the
/// refusal cannot be a blanket answer to "this request carries tags".
#[tokio::test]
async fn a_tag_within_its_modelled_length_reaches_the_operation() {
    let emulator = Emulator::new();
    let resource_id = emulator
        .call(
            "CreateTrail",
            &json!({
                "Name": "within-bounds-trail",
                "S3BucketName": "an-emulator-bucket",
                "TagsList": [tag("seeded", "1")],
            }),
        )
        .await
        .expect_ok("the trail the rows tag")
        .get("TrailARN")
        .and_then(Value::as_str)
        .expect("the create reports the trail ARN")
        .to_string();

    // (case, the tag list the request carries)
    let within: Vec<(&str, Vec<Value>)> = vec![
        ("one ordinary tag", vec![tag("ordinary", "value")]),
        (
            "a key AT its maximum",
            vec![tag(&"k".repeat(TAG_KEY_MAX_LEN), "v")],
        ),
        (
            "a value AT its maximum",
            vec![tag("k", &"v".repeat(TAG_VALUE_MAX_LEN))],
        ),
        ("no tags at all", vec![]),
    ];

    for (index, (case, tags)) in within.iter().enumerate() {
        for (operation, member) in TAG_CARRYING_OPERATIONS {
            let suffix = format!("ok-{operation}-{index}");
            let reply = emulator
                .call_in(
                    operation,
                    HOME_REGION,
                    &request(operation, member, &resource_id, &suffix, tags),
                )
                .await;
            assert_ne!(
                reply.error_code(),
                INVALID_TAG_PARAMETER,
                "case {operation}: {case:?}: every tag is inside its modelled length, so \
                 the request must reach the operation, got {reply:?}"
            );
        }
    }
}
