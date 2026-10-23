// SPDX-License-Identifier: CC0-1.0

//! `TagResource`'s write path, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`),
//! through the same `MultiServiceRouter` the emulator mounts in production. The harness,
//! the domain seeding and the exact-presence tag comparison are the ones
//! `workflow_execution_lifecycle` and `domain_tagging` already build.
//!
//! Two rules, each asserted over a SET of inputs:
//!
//! 1. A tag a request supplies must satisfy the documented tag character class —
//!    `ResourceTag`'s doc: "Tags may only contain unicode letters, digits, whitespace, or
//!    these symbols: `_ . : / = + - @`" — on both sides of the pair, wherever an
//!    operation accepts a tag; a tag outside it is refused with HTTP 400
//!    `ValidationException` and nothing is stored. Each row is driven through BOTH entry
//!    points that store a domain tag (`TagResource` and `RegisterDomain`), so the two can
//!    never disagree about what is storable, and the modelled key/value length bounds are
//!    checked at each published bound and one past it.
//! 2. A string the caller supplies is stored and reported back byte for byte, and the
//!    PRESENCE of an optional one is preserved: a member supplied empty reads back present
//!    and empty, a member not supplied reads back absent. The rows vary the script, the
//!    symbol, the length and the presence transitions a merge makes — for a tag value
//!    through `TagResource`/`ListTagsForResource`, and for a workflow's `input` through
//!    `StartWorkflowExecution`/`GetWorkflowExecutionHistory`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::domain_tagging::{
    list_tags, registered_with_tags, reported_tags, Tag, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
};
use crate::workflow_execution_lifecycle::{
    full_defaults, merge, register_domain_body, seeded, string_at, whole_history, Emulator,
};

/// Tag `tag` onto `resource_arn`, requiring a 200.
async fn tag_resource(emulator: &Emulator, case: &str, resource_arn: &str, tags: &[Tag]) {
    let wire: Vec<Value> = tags.iter().map(Tag::to_json).collect();
    emulator
        .ok(
            case,
            "TagResource",
            &json!({ "resourceArn": resource_arn, "tags": wire }),
        )
        .await;
}

/// The tags `ListTagsForResource` reports for `resource_arn`, keeping each value's
/// presence exactly as stored.
async fn tags_of(emulator: &Emulator, case: &str, resource_arn: &str) -> BTreeSet<Tag> {
    let listed = list_tags(emulator, case, resource_arn).await;
    reported_tags(case, &listed)
}

#[tokio::test]
async fn tag_resource_round_trips_every_tag_it_stores_byte_for_byte() {
    // Rows inside the documented character class, varying what the rule generalises
    // over: script (Latin, Latin with diacritics, Greek, Cyrillic, Han, Kana, Arabic,
    // Devanagari, Hebrew), digits, whitespace, every documented symbol, and each
    // published length bound (key 1..128, value 0..256 — `MAX_TAG_KEY_LEN` /
    // `MAX_TAG_VALUE_LEN`, from `smithy_query(query="shape",shape="ResourceTag")`).
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let rows: Vec<Tag> = vec![
        Tag::new("owner", Some("platform")),
        Tag::new("Ünïcødé", Some("Ünïcødé 42")),
        Tag::new("Ελληνικά", Some("Ελλάδα")),
        Tag::new("Кириллица", Some("Москва")),
        Tag::new("白板", Some("キー")),
        Tag::new("العربية", Some("مرحبا")),
        Tag::new("देवनागरी", Some("भारत")),
        Tag::new("עברית", Some("שלום")),
        Tag::new("digits", Some("0987654321")),
        Tag::new("spaced key", Some("spaced value")),
        Tag::new("symbols_._:_/_=_+_-_@", Some("a_b.c:d/e=f+g-h@i")),
        // `value` is optional with minimum length 0, so "present and empty" and "absent"
        // are two different stored tags.
        Tag::new("empty-value", Some("")),
        Tag::new("absent-value", None),
        Tag::new("key-at-its-bound-value", Some(&longest_value)),
        Tag::new(&longest_key, Some("key at its bound")),
        Tag::new("k", Some("key at its minimum")),
    ];

    let emulator = Emulator::new();
    let arn = registered_with_tags(&emulator, "round trip", "fidelity-domain", &[]).await;

    // Each row is written on its own call, and the whole stored set is read back after
    // each one: a row must arrive exactly as written AND leave every earlier row alone.
    let mut expected: BTreeSet<Tag> = BTreeSet::new();
    for row in &rows {
        let case = format!("{:?} => {:?}", row.key, row.value);
        tag_resource(&emulator, &case, &arn, std::slice::from_ref(row)).await;
        expected.insert(row.clone());
        let reported = tags_of(&emulator, &case, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: TagResource must store the tag exactly as supplied, \
             got {reported:?}"
        );
        let stored = reported
            .iter()
            .find(|tag| tag.key == row.key)
            .unwrap_or_else(|| panic!("case {case:?}: the written key is stored, got {reported:?}"));
        assert_eq!(
            (
                stored.value.as_deref(),
                stored.value.as_deref().map(str::len)
            ),
            (row.value.as_deref(), row.value.as_deref().map(str::len)),
            "case {case:?}: the stored value keeps its presence and its bytes, \
             got {stored:?}"
        );
    }
}

#[tokio::test]
async fn tag_resource_preserves_whether_a_value_is_present() {
    let emulator = Emulator::new();
    let arn = registered_with_tags(&emulator, "presence", "presence-domain", &[]).await;

    // Every transition a merge can make between the three forms an optional `value`
    // takes — present, present and empty, absent — applied in order to the SAME key, so
    // each step is checked against the state the one before it left. The overwrite must
    // carry the new form through, never collapse "absent" into "" or the other way.
    let steps = [
        Some("first"),
        Some(""),
        None,
        Some(""),
        Some("second"),
        None,
        Some("third"),
    ];
    for (index, value) in steps.iter().enumerate() {
        let case = format!("step {index}: value {value:?}");
        tag_resource(&emulator, &case, &arn, &[Tag::new("presence", *value)]).await;
        let reported = tags_of(&emulator, &case, &arn).await;
        assert_eq!(
            reported,
            BTreeSet::from([Tag::new("presence", *value)]),
            "case {case:?}: the stored tag must carry exactly the value form supplied, \
             got {reported:?}"
        );
    }

    // And two keys that differ ONLY in the presence of their value coexist, each keeping
    // its own form.
    let pair = [Tag::new("empty", Some("")), Tag::new("absent", None)];
    tag_resource(&emulator, "a pair differing only in presence", &arn, &pair).await;
    let reported = tags_of(&emulator, "a pair differing only in presence", &arn).await;
    assert_eq!(
        reported,
        BTreeSet::from([
            Tag::new("presence", Some("third")),
            Tag::new("empty", Some("")),
            Tag::new("absent", None),
        ]),
        "case \"a pair differing only in presence\": an empty value and an absent one are \
         two different stored tags, got {reported:?}"
    );
}

/// One tag both entry points must refuse, and why it is outside what SWF stores.
struct RefusedTag {
    case: &'static str,
    tag: Tag,
}

#[tokio::test]
async fn a_tag_outside_what_swf_accepts_is_refused_by_every_entry_point() {
    let emulator = Emulator::new();
    let seed = [Tag::new("owner", Some("platform"))];
    let arn = registered_with_tags(&emulator, "charset", "charset-domain", &seed).await;

    // Source for the character class: `ResourceTag`'s own doc — "Tags may only contain
    // unicode letters, digits, whitespace, or these symbols: _ . : / = + - @". A
    // character outside it is refused on EITHER side of the pair, since the doc states
    // the rule for the tag and not for one member of it. Source for the lengths:
    // `smithy_query(query="shape",shape="ResourceTag")` — key 1..128, value 0..256, so a
    // key one past its maximum, an empty key (one below its minimum) and a value one
    // past its maximum are each refused.
    let over_long_key = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let over_long_value = "v".repeat(MAX_TAG_VALUE_LEN + 1);
    let rows = vec![
        RefusedTag {
            case: "an exclamation mark in the key",
            tag: Tag::new("bad!key", Some("v")),
        },
        RefusedTag {
            case: "a hash in the key",
            tag: Tag::new("bad#key", Some("v")),
        },
        RefusedTag {
            case: "a comma in the key",
            tag: Tag::new("bad,key", Some("v")),
        },
        RefusedTag {
            case: "brackets in the key",
            tag: Tag::new("key(1)", Some("v")),
        },
        RefusedTag {
            case: "an exclamation mark in the value",
            tag: Tag::new("ok", Some("bad!value")),
        },
        RefusedTag {
            case: "a control character in the value",
            tag: Tag::new("ok", Some("bad\u{1}value")),
        },
        RefusedTag {
            case: "a semicolon in the value",
            tag: Tag::new("ok", Some("semi;colon")),
        },
        RefusedTag {
            case: "a percent sign in the value",
            tag: Tag::new("ok", Some("100%")),
        },
        // Neither a unicode letter, nor a digit, nor whitespace, nor one of the listed
        // symbols — so outside the class even though it is valid UTF-8 text.
        RefusedTag {
            case: "an emoji in the value",
            tag: Tag::new("ok", Some("ship it 🚀")),
        },
        RefusedTag {
            case: "a key one past its modelled maximum",
            tag: Tag::new(&over_long_key, Some("v")),
        },
        RefusedTag {
            case: "an empty key, one below its modelled minimum",
            tag: Tag::new("", Some("v")),
        },
        RefusedTag {
            case: "a value one past its modelled maximum",
            tag: Tag::new("ok", Some(&over_long_value)),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // The tagging call refuses it...
        emulator
            .refused(
                row.case,
                "TagResource",
                &json!({ "resourceArn": arn, "tags": [row.tag.to_json()] }),
                "ValidationException",
            )
            .await;
        // ...and so does the other entry point that stores a domain tag, so one cannot
        // store what the other rejects.
        let domain = format!("charset-reject-{index}");
        let mut body = register_domain_body(&domain, "7");
        merge(&mut body, json!({ "tags": [row.tag.to_json()] }));
        emulator
            .refused(row.case, "RegisterDomain", &body, "ValidationException")
            .await;
        emulator
            .refused(
                row.case,
                "DescribeDomain",
                &json!({ "name": domain }),
                "UnknownResourceFault",
            )
            .await;
    }

    // Not one of the refusals changed what a later caller sees.
    let reported = tags_of(&emulator, "after the refusals", &arn).await;
    assert_eq!(
        reported,
        seed.iter().cloned().collect::<BTreeSet<Tag>>(),
        "case \"after the refusals\": a refused tagging call must store nothing, \
         got {reported:?}"
    );

    // A tag AT each published bound is still accepted: the refusals above are the bound
    // being enforced, not the bound being narrowed.
    let at_bounds = [
        Tag::new(&"k".repeat(MAX_TAG_KEY_LEN), Some("")),
        Tag::new("value-at-its-bound", Some(&"v".repeat(MAX_TAG_VALUE_LEN))),
    ];
    tag_resource(&emulator, "at the published bounds", &arn, &at_bounds).await;
    let reported = tags_of(&emulator, "at the published bounds", &arn).await;
    for tag in &at_bounds {
        assert!(
            reported.contains(tag),
            "case \"at the published bounds\": a tag at the bound is stored, \
             got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_workflow_input_round_trips_with_its_presence_preserved() {
    // The same rule on the other member the recorded units exercise: `input` is an
    // optional string the caller supplies, so the started event must report it byte for
    // byte, present-and-empty when it was supplied empty and absent when it was not
    // supplied at all. `Data` carries no documented character class, so a non-BMP
    // codepoint is a legitimate row here.
    let rows: [(&str, Option<&str>); 7] = [
        ("no input member at all", None),
        ("an empty input", Some("")),
        ("a JSON payload", Some("{\"n\":1}")),
        ("mixed scripts", Some("白板 Ünïcødé Кириллица")),
        ("a non-BMP codepoint", Some("ship it 🚀")),
        ("leading and trailing whitespace", Some("  padded  ")),
        ("punctuation the tag class bans", Some("a!b#c,d;e%f")),
    ];

    let emulator = Emulator::new();
    seeded(
        &emulator,
        "workflow input",
        "input-fidelity",
        ("demo-wf", "1.0"),
        full_defaults("demo-tasks", "TERMINATE"),
    )
    .await;

    for (index, (case, input)) in rows.iter().enumerate() {
        let workflow_id = format!("wf-{index}");
        let mut body = json!({
            "domain": "input-fidelity",
            "workflowId": workflow_id,
            "workflowType": { "name": "demo-wf", "version": "1.0" },
        });
        if let Some(input) = input {
            merge(&mut body, json!({ "input": input }));
        }
        let started = emulator.ok(case, "StartWorkflowExecution", &body).await;
        let run_id = string_at(&started, "/runId");

        let events = whole_history(&emulator, "input-fidelity", &workflow_id, &run_id).await;
        let attributes = events
            .first()
            .and_then(|event| event.pointer("/workflowExecutionStartedEventAttributes"))
            .unwrap_or_else(|| panic!("case {case:?}: no start attributes in {events:?}"))
            .clone();
        let reported = attributes.get("input").map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: input is a string, got {attributes:?}"))
                .to_string()
        });
        assert_eq!(
            reported.as_deref(),
            *input,
            "case {case:?}: the start event must carry the supplied input, with its \
             presence preserved, got {attributes:?}"
        );
    }
}
