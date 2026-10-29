// SPDX-License-Identifier: CC0-1.0

//! How the APPEND that collides with an output the stored feed already holds is WORDED,
//! end to end over the wire.
//!
//! THE RULE under test: an output's `name` IDENTIFIES it within its feed, and
//! `AssociateFeed` only APPENDS, so an output whose name the feed already holds under a
//! different definition is refused with the modelled `ConflictException` (HTTP 409). That
//! refusal is decided from the STORED output list, so its message is the rule's sentence
//! and is worded once in `crate::state`: it names at most the output name the REQUEST
//! supplied and no value read off the record — not the feed, not its status, nothing of
//! the held output's definition — so it is byte-identical whichever feed refused the
//! append and whatever that feed holds, and a client can match it against a constant.
//!
//! The rows span what the rule generalises over: WHERE the collision sits in the stored
//! list (its only output, its first, its last), HOW the appended definition differs from
//! the held one (the configuration's feature, the output's status, its description, and
//! not at all), where the colliding entry sits in the REQUEST's list, and the FORM and
//! LENGTH of the name (`CreateOutput$name` has pattern
//! `^[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?$`: one character, digits, hyphens and
//! underscores, and the 128-character maximum). The feed differs in every row too, since
//! each row mints its own, which is what catches a message built from the record.
//!
//! Every case drives the real request path: the router picks the service from the SigV4
//! credential scope, the framework deserializes and validates the body, the handler runs,
//! and the response is serialized as `restJson1` — so what these tests assert is what a
//! client sees.

use crate::harness::{call, message_of, output, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    duplicate_output_name_message, output_not_appendable_message,
};
use smithy_server_runtime::MultiServiceRouter;

/// The resource name every claim below carries: one value of `AssociatedResourceName`,
/// which no refusal under test may report.
const RESOURCE: &str = "append-conflict-media";

/// One output named `name`, carrying the `kind` config, `status`, and `description` when
/// given — the four members `CreateOutput` models.
fn described_output(name: &str, kind: &str, status: &str, description: Option<&str>) -> Value {
    let mut built = output(name, kind, status);
    if let Some(description) = description {
        built
            .as_object_mut()
            .expect("an output is a JSON object")
            .insert("description".to_string(), json!(description));
    }
    built
}

/// Create a feed holding `outputs` and return its id and its ARN — the two record values
/// a refusal must not report.
async fn feed_holding(
    router: &MultiServiceRouter,
    label: &str,
    outputs: &[Value],
) -> (String, String) {
    let (status, created) = call(
        router,
        "POST",
        "/v1/feed",
        Some(json!({"name": "append-conflict-feed", "outputs": outputs})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {label:?}: CreateFeed answers 202, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: CreateFeed returns an id, got {created}"))
        .to_string();
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: CreateFeed returns an arn, got {created}"))
        .to_string();
    (id, arn)
}

/// `POST /v1/feed/{id}/associate`, appending `outputs` under [`RESOURCE`].
async fn associate(router: &MultiServiceRouter, id: &str, outputs: &[Value]) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/v1/feed/{id}/associate"),
        Some(json!({"associatedResourceName": RESOURCE, "outputs": outputs})),
    )
    .await
}

/// The names the stored feed reports, in order, read back through `GetFeed`.
async fn stored_output_names(
    router: &MultiServiceRouter,
    id: &str,
    label: &str,
) -> (Vec<String>, String) {
    let (status, feed) = call(router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: GetFeed answers 200, got {status} with {feed}"
    );
    let names = feed["outputs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: a feed reports an outputs array, got {feed}"))
        .iter()
        .map(|output| {
            output["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: an output carries a name, got {output}"))
                .to_string()
        })
        .collect();
    let feed_status = feed["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: a feed reports a status, got {feed}"))
        .to_string();
    (names, feed_status)
}

/// One row: the outputs the feed is minted with, the ones the append carries, the held
/// definition the refusal must not report, and the name that collides.
struct Case {
    label: String,
    held: Vec<Value>,
    held_kind: &'static str,
    held_status: &'static str,
    appended: Vec<Value>,
    colliding_name: String,
}

/// The table: a collision against a stored output, differing along everything the rule
/// generalises over.
fn cases() -> Vec<Case> {
    // A 128-character name — `CreateOutput$name`'s published maximum — and the shortest
    // forms its pattern admits.
    let longest = "n".repeat(128);
    let mut cases = vec![
        Case {
            label: "the feed's only output, under another feature".to_string(),
            held: vec![output("crop1", "cropping", "ENABLED")],
            held_kind: "cropping",
            held_status: "ENABLED",
            appended: vec![output("crop1", "clipping", "ENABLED")],
            colliding_name: "crop1".to_string(),
        },
        Case {
            label: "the first of three held outputs".to_string(),
            held: vec![
                output("held0", "cropping", "ENABLED"),
                output("held1", "subtitling", "ENABLED"),
                output("held2", "contextualMetadata", "ENABLED"),
            ],
            held_kind: "cropping",
            held_status: "ENABLED",
            appended: vec![output("held0", "subtitling", "ENABLED")],
            colliding_name: "held0".to_string(),
        },
        Case {
            label: "the last of three held outputs".to_string(),
            held: vec![
                output("held0", "cropping", "ENABLED"),
                output("held1", "subtitling", "ENABLED"),
                output("held2", "contextualMetadata", "DISABLED"),
            ],
            held_kind: "contextualMetadata",
            held_status: "DISABLED",
            appended: vec![output("held2", "cropping", "ENABLED")],
            colliding_name: "held2".to_string(),
        },
        Case {
            label: "differing only in the output's status".to_string(),
            held: vec![output("steady1", "cropping", "ENABLED")],
            held_kind: "cropping",
            held_status: "ENABLED",
            appended: vec![output("steady1", "cropping", "DISABLED")],
            colliding_name: "steady1".to_string(),
        },
        Case {
            label: "differing only in the output's description".to_string(),
            held: vec![described_output("note1", "cropping", "ENABLED", None)],
            held_kind: "cropping",
            held_status: "ENABLED",
            appended: vec![described_output(
                "note1",
                "cropping",
                "ENABLED",
                Some("the appended description"),
            )],
            colliding_name: "note1".to_string(),
        },
        Case {
            // Not differing at all: the name still identifies an output the feed holds,
            // and that output is not one an association appended, so the append would
            // still modify it.
            label: "the same definition the create stored".to_string(),
            held: vec![output("same1", "subtitling", "ENABLED")],
            held_kind: "subtitling",
            held_status: "ENABLED",
            appended: vec![output("same1", "subtitling", "ENABLED")],
            colliding_name: "same1".to_string(),
        },
        Case {
            // The colliding entry is the SECOND of the request's list, behind one the
            // feed really does not hold: the refusal names the colliding one.
            label: "the collision behind an output that would be appended".to_string(),
            held: vec![output("crop1", "cropping", "ENABLED")],
            held_kind: "cropping",
            held_status: "ENABLED",
            appended: vec![
                output("brandnew1", "clipping", "ENABLED"),
                output("crop1", "subtitling", "DISABLED"),
            ],
            colliding_name: "crop1".to_string(),
        },
    ];
    // The same collision under the forms and lengths the name itself admits.
    for (how, name) in [
        ("one character", "a".to_string()),
        ("a digit", "0".to_string()),
        ("hyphens and underscores", "Mixed_Case-9-name".to_string()),
        ("the 128-character maximum", longest),
    ] {
        cases.push(Case {
            label: format!("a name of {how}"),
            held: vec![output(&name, "cropping", "ENABLED")],
            held_kind: "cropping",
            held_status: "ENABLED",
            appended: vec![output(&name, "subtitling", "DISABLED")],
            colliding_name: name,
        });
    }
    cases
}

/// Every row earns the rule's sentence from `crate::state`, naming the output name the
/// request supplied and nothing read out of the stored feed — and appending nothing.
#[tokio::test]
async fn an_append_colliding_with_a_held_output_answers_the_rules_sentence() {
    let router = router();
    let mut by_name: Vec<(String, String)> = Vec::new();

    for case in cases() {
        let Case {
            label,
            held,
            held_kind,
            held_status,
            appended,
            colliding_name,
        } = case;
        let (id, arn) = feed_holding(&router, &label, &held).await;
        let (status, error) = associate(&router, &id, &appended).await;

        assert_eq!(
            status, 409,
            "case {label:?}: an append that would modify a held output is a 409, got \
             {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ConflictException"),
            "case {label:?}: that 409 is a ConflictException, got {error}"
        );
        let message = message_of(&error).to_string();
        assert_eq!(
            message,
            output_not_appendable_message(&colliding_name),
            "case {label:?}: expected the rule's sentence from crate::state, got {message:?}"
        );
        assert!(
            message.contains(colliding_name.as_str()),
            "case {label:?}: the refusal names the output name the request supplied \
             ({colliding_name:?}), got {message:?}"
        );
        assert!(
            !message.contains(id.as_str()) && !message.contains(arn.as_str()),
            "case {label:?}: the refusal must not report the stored feed ({id:?}, {arn:?}), \
             got {message:?}"
        );
        assert!(
            !message.contains(held_kind) && !message.contains(held_status),
            "case {label:?}: nor anything of the held output's definition ({held_kind}, \
             {held_status}), got {message:?}"
        );
        assert!(
            !message.contains(RESOURCE) && !message.contains("AVAILABLE"),
            "case {label:?}: nor the resource the claim named or the feed's status, got \
             {message:?}"
        );

        // The refusal is the same refusal it was: nothing appended, no claim taken.
        let expected_names: Vec<String> = held
            .iter()
            .map(|output| output["name"].as_str().unwrap_or_default().to_string())
            .collect();
        let (stored, feed_status) = stored_output_names(&router, &id, &label).await;
        assert_eq!(
            stored, expected_names,
            "case {label:?}: the refused append left the stored list alone, got {stored:?}"
        );
        assert_eq!(
            feed_status, "AVAILABLE",
            "case {label:?}: and took no claim, got {feed_status:?}"
        );

        by_name.push((colliding_name, message));
    }

    // Byte-identical across records: every row minted its own feed, so two rows that
    // collide on one name must still answer one message, and two names must differ only
    // by the name the request supplied.
    for (name, message) in &by_name {
        let same: Vec<&String> = by_name
            .iter()
            .filter(|(other, _)| other == name)
            .map(|(_, message)| message)
            .collect();
        assert!(
            same.iter().all(|other| *other == message),
            "case {name:?}: one name answers one sentence whatever the feed holds, got \
             {same:?}"
        );
    }
    let mut distinct_names: Vec<&String> = by_name.iter().map(|(name, _)| name).collect();
    distinct_names.sort();
    distinct_names.dedup();
    let mut distinct_messages: Vec<&String> = by_name.iter().map(|(_, message)| message).collect();
    distinct_messages.sort();
    distinct_messages.dedup();
    assert_eq!(
        distinct_messages.len(),
        distinct_names.len(),
        "the sentence varies with the supplied name and nothing else, got \
         {distinct_messages:?}"
    );
}

/// Both halves of the one rule — a name the stored feed holds, and a name the REQUEST
/// lists twice — are worded in `crate::state`, in the same convention: one 409 naming the
/// request's own value and no record value.
#[tokio::test]
async fn both_halves_of_the_output_name_rule_are_worded_alike() {
    let router = router();

    // (how the name is not unique, the appended list, the sentence crate::state words
    // that half with).
    let rows: Vec<(&str, Vec<Value>, String)> = vec![
        (
            "a name the stored feed already holds",
            vec![output("held1", "clipping", "ENABLED")],
            output_not_appendable_message("held1"),
        ),
        (
            "a name the request itself lists twice",
            vec![
                output("twice1", "clipping", "ENABLED"),
                output("twice1", "cropping", "DISABLED"),
            ],
            duplicate_output_name_message("twice1"),
        ),
    ];

    for (label, appended, expected) in rows {
        let (id, arn) =
            feed_holding(&router, label, &[output("held1", "cropping", "ENABLED")]).await;
        let (status, error) = associate(&router, &id, &appended).await;
        assert_eq!(
            status, 409,
            "case {label:?}: a name that does not identify one output is a 409, got {status} \
             with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ConflictException"),
            "case {label:?}: that 409 is a ConflictException, got {error}"
        );
        let message = message_of(&error);
        assert_eq!(
            message, expected,
            "case {label:?}: expected the sentence crate::state words this half with, got \
             {message:?}"
        );
        assert!(
            !message.contains(id.as_str())
                && !message.contains(arn.as_str())
                && !message.contains(RESOURCE),
            "case {label:?}: neither half reports a record value ({id:?}, {arn:?}) or the \
             resource named, got {message:?}"
        );
    }
}
