// SPDX-License-Identifier: CC0-1.0

//! What `PostAgentProfile` accepts as profiling data, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule every case here asserts: submitted profiling data is PARSED, and only a
//! document whose root is a struct carrying a `start` member is an agent profile —
//! anything else is refused with the operation's modelled `ValidationException` (HTTP
//! 400) and stored nowhere, so the group still reports no profile reported and no read
//! serves one. The data is judged on what it holds and not on the media type it was
//! announced under: the `Content-Type` says how to read the bytes, it cannot make bytes
//! that carry no profile into one.
//!
//! Rows vary what the rule generalises over: the representation (JSON and Amazon Ion's
//! binary encoding, the two the model's `contentType` doc names, plus a media type that
//! names neither), the root's type (every scalar type a document can be rooted at, a
//! list, a struct), whether the root carries `start`, where in the document it sits, and
//! bytes that are no document at all.
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds.

use ion_rs::{Element, Sequence, Struct, Symbol, Value, v1_0};

use crate::profiling_group_lifecycle::{Answer, Harness, PROFILE_START_MILLIS};

/// The two media types the model's `PostAgentProfile$contentType` doc names:
/// "application/json — standard JSON format" and "application/x-amzn-ion — the Amazon Ion
/// data format".
const JSON: &str = "application/json";
const ION: &str = "application/x-amzn-ion";

/// A media type that names NEITHER representation, which is what the recorded refusal of
/// a bare symbol payload was submitted under.
const TEXT: &str = "text/plain";

/// The member a profile has to carry, as the service's refusal of a profile without it
/// names it.
const START: &str = "start";

async fn create_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            JSON,
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await
}

/// A JSON text, as bytes.
fn json(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("a serializable document")
}

/// `element` in Amazon Ion's binary encoding.
fn ion(element: &Element) -> Vec<u8> {
    element
        .encode_as(v1_0::Binary)
        .expect("an encodable Ion document")
}

/// What the service must answer for one submission.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Stored: the modelled success status of this operation is 204.
    Stored,
    /// Refused 400 `ValidationException`, with this message.
    Refused(String),
}

/// The refusal of data whose root is not a struct, naming the type the root has.
fn wrong_root(ion_type: &str) -> Outcome {
    Outcome::Refused(format!(
        "Invalid agent profile: Unexpected type for profile root. Expected STRUCT, got \
         {ion_type}.."
    ))
}

/// The refusal of a struct that carries no `start`.
fn no_start() -> Outcome {
    Outcome::Refused(format!(
        "Invalid agent profile: Profile must contain a {START} field.."
    ))
}

/// The refusal of data that is no document at all.
fn unparseable() -> Outcome {
    Outcome::Refused("Invalid agent profile: Profile could not be parsed..".to_string())
}

/// One submission: the media type it is announced under, the bytes, and what must come
/// back.
struct Case {
    label: &'static str,
    content_type: &'static str,
    bytes: Vec<u8>,
    outcome: Outcome,
}

fn cases() -> Vec<Case> {
    // Every Ion type a document can be rooted at, as Ion names them — the type names the
    // refusal states. SOURCE: the Ion data model's types, which `ion_rs::IonType`
    // enumerates: NULL, BOOL, INT, FLOAT, DECIMAL, TIMESTAMP, SYMBOL, STRING, CLOB,
    // BLOB, LIST, SEXP, STRUCT. Only STRUCT is a profile.
    vec![
        Case {
            label: "the smallest profile there is",
            content_type: JSON,
            bytes: json(&serde_json::json!({ START: PROFILE_START_MILLIS })),
            outcome: Outcome::Stored,
        },
        Case {
            label: "a profile carrying frames beside its start",
            content_type: JSON,
            bytes: json(&serde_json::json!({
                START: PROFILE_START_MILLIS,
                "callgraph": { "A": { "RUNNABLE": 2 } },
                "samplesPerSecond": 1,
            })),
            outcome: Outcome::Stored,
        },
        Case {
            label: "a profile under a media type that names no representation",
            content_type: TEXT,
            bytes: json(&serde_json::json!({ START: PROFILE_START_MILLIS })),
            outcome: Outcome::Stored,
        },
        Case {
            label: "a profile in Ion's binary encoding",
            content_type: ION,
            bytes: ion(&Element::from(Struct::from_iter([(
                Symbol::from(START),
                Element::from(PROFILE_START_MILLIS),
            )]))),
            outcome: Outcome::Stored,
        },
        Case {
            label: "a struct carrying nothing",
            content_type: JSON,
            bytes: json(&serde_json::json!({})),
            outcome: no_start(),
        },
        Case {
            label: "a struct carrying every member but start",
            content_type: JSON,
            bytes: json(&serde_json::json!({ "callgraph": {}, "end": 1 })),
            outcome: no_start(),
        },
        Case {
            label: "start nested inside a member rather than at the root",
            content_type: JSON,
            bytes: json(&serde_json::json!({ "agentMetadata": { START: 1 } })),
            outcome: no_start(),
        },
        Case {
            label: "start spelled in another case",
            content_type: JSON,
            bytes: json(&serde_json::json!({ "START": PROFILE_START_MILLIS })),
            outcome: no_start(),
        },
        Case {
            label: "an Ion struct carrying no start",
            content_type: ION,
            bytes: ion(&Element::from(Struct::from_iter([(
                Symbol::from("end"),
                Element::from(1),
            )]))),
            outcome: no_start(),
        },
        Case {
            label: "a list of profiles rather than one",
            content_type: JSON,
            bytes: json(&serde_json::json!([{ START: PROFILE_START_MILLIS }])),
            outcome: wrong_root("LIST"),
        },
        Case {
            label: "an Ion list",
            content_type: ION,
            bytes: ion(&Element::from(Value::List(Sequence::new(
                Vec::<Element>::new(),
            )))),
            outcome: wrong_root("LIST"),
        },
        Case {
            label: "the recorded instance: a bare symbol under text/plain",
            content_type: TEXT,
            bytes: b"not-a-profile".to_vec(),
            outcome: wrong_root("SYMBOL"),
        },
        Case {
            label: "a quoted string",
            content_type: JSON,
            bytes: json(&serde_json::json!("a profile, surely")),
            outcome: wrong_root("STRING"),
        },
        Case {
            label: "a whole number",
            content_type: JSON,
            bytes: json(&serde_json::json!(PROFILE_START_MILLIS)),
            outcome: wrong_root("INT"),
        },
        Case {
            label: "a real number",
            content_type: JSON,
            bytes: b"1.5".to_vec(),
            outcome: wrong_root("DECIMAL"),
        },
        Case {
            label: "a boolean",
            content_type: JSON,
            bytes: json(&serde_json::json!(true)),
            outcome: wrong_root("BOOL"),
        },
        Case {
            label: "a null",
            content_type: JSON,
            bytes: json(&serde_json::Value::Null),
            outcome: wrong_root("NULL"),
        },
        Case {
            label: "an Ion timestamp",
            content_type: ION,
            bytes: b"2020-09-13T12:26:40Z".to_vec(),
            outcome: wrong_root("TIMESTAMP"),
        },
        Case {
            label: "an Ion blob",
            content_type: ION,
            bytes: b"{{aGk=}}".to_vec(),
            outcome: wrong_root("BLOB"),
        },
        Case {
            label: "a truncated document",
            content_type: JSON,
            bytes: b"{\"start\":".to_vec(),
            outcome: unparseable(),
        },
        Case {
            label: "one opening brace",
            content_type: JSON,
            bytes: vec![b'{'],
            outcome: unparseable(),
        },
        Case {
            label: "every byte value, announced as Ion",
            content_type: ION,
            bytes: (0..=255u8).collect(),
            outcome: unparseable(),
        },
    ]
}

#[tokio::test]
async fn only_a_struct_stating_its_start_is_an_agent_profile() {
    let harness = Harness::new();

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("submitted-{index}");
        assert_eq!(
            create_group(&harness, &name).await.status,
            201,
            "case {label:?}: create must succeed"
        );

        let answer = harness
            .post_agent_profile(&name, case.content_type, None, case.bytes.clone())
            .await;
        match &case.outcome {
            Outcome::Stored => {
                assert_eq!(
                    answer.status,
                    204,
                    "case {label:?}: expected HTTP 204, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                // Stored means STORED: the group reports the profile as reported, and
                // the aggregated profile reads back the submitted bytes.
                let described = harness.describe(&name).await;
                assert!(
                    described
                        .json()
                        .pointer("/profilingStatus/latestAgentProfileReportedAt")
                        .is_some(),
                    "case {label:?}: an accepted profile must be recorded as reported, \
                     got {}",
                    String::from_utf8_lossy(&described.body)
                );
                harness.complete_aggregation(&name);
                let served = harness.get_profile(&name, "").await;
                assert_eq!(
                    served.status,
                    200,
                    "case {label:?}: the stored profile must read back, got {} {}",
                    served.status,
                    String::from_utf8_lossy(&served.body)
                );
                assert_eq!(
                    served.body,
                    case.bytes,
                    "case {label:?}: expected the submitted bytes back, got {:?}",
                    String::from_utf8_lossy(&served.body)
                );
            }
            Outcome::Refused(message) => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {label:?}: expected HTTP 400, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {label:?}: expected ValidationException, got {}",
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    &answer.error_message(),
                    message,
                    "case {label:?}: expected {message:?}, got {:?}",
                    answer.error_message()
                );
                // Refused means NOTHING WAS WRITTEN: the group reports no profile
                // reported, and no read serves one — before and after the aggregation
                // a stored profile would have been owed.
                let described = harness.describe(&name).await;
                assert_eq!(
                    described
                        .json()
                        .pointer("/profilingStatus/latestAgentProfileReportedAt"),
                    None,
                    "case {label:?}: a refused submission must leave the profiling \
                     status alone, got {}",
                    String::from_utf8_lossy(&described.body)
                );
                harness.complete_aggregation(&name);
                let served = harness.get_profile(&name, "").await;
                assert_eq!(
                    served.status,
                    404,
                    "case {label:?}: a refused submission must leave nothing to serve, \
                     got {} {}",
                    served.status,
                    String::from_utf8_lossy(&served.body)
                );
            }
        }
    }
}

#[tokio::test]
async fn deeply_nested_data_is_refused_without_descending_into_it() {
    let harness = Harness::new();
    let name = "deeply-nested";
    assert_eq!(create_group(&harness, name).await.status, 201);

    // How deeply submitted data nests is the CALLER's choice, so the rule above must hold
    // without the service following that nesting: a document rooted at a list is refused
    // on its root, whatever sits inside it, and nesting past the depth this service reads
    // a document at is data it cannot read as one. Both are the same 400; neither may
    // consume the host or bring the emulator down.
    for levels in [8usize, 200, 5_000, 100_000] {
        let bytes = format!("{}{}", "[".repeat(levels), "]".repeat(levels)).into_bytes();
        let answer = harness
            .post_agent_profile(name, JSON, None, bytes)
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {levels} levels: expected HTTP 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {levels} levels: expected ValidationException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // And nothing of any of them was stored.
    let described = harness.describe(name).await;
    assert_eq!(
        described
            .json()
            .pointer("/profilingStatus/latestAgentProfileReportedAt"),
        None,
        "a refused submission must leave the profiling status alone, got {}",
        String::from_utf8_lossy(&described.body)
    );
}

#[tokio::test]
async fn a_refused_submission_consumes_no_idempotency_token() {
    let harness = Harness::new();
    let name = "token-after-refusal";
    assert_eq!(create_group(&harness, name).await.status, 201);

    // The same `profileToken` as the refused submission below: a submission the service
    // refused is no submission, so the token it carried names none and the profile that
    // carries it next is stored rather than treated as a replay.
    let token = "ptok-1";
    let refused = harness
        .post_agent_profile(name, JSON, Some(token), b"{}".to_vec())
        .await;
    assert_eq!(
        refused.status,
        400,
        "a struct with no start must be refused, got {} {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );

    let profile = json(&serde_json::json!({ START: PROFILE_START_MILLIS }));
    let stored = harness
        .post_agent_profile(name, JSON, Some(token), profile.clone())
        .await;
    assert_eq!(
        stored.status,
        204,
        "the profile must be stored, got {} {}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );
    harness.complete_aggregation(name);
    let served = harness.get_profile(name, "").await;
    assert_eq!(
        served.body,
        profile,
        "the stored profile must read back, got {:?}",
        String::from_utf8_lossy(&served.body)
    );
}

#[tokio::test]
async fn the_group_is_resolved_before_the_data_is_parsed() {
    let harness = Harness::new();

    // The profile is submitted TO a profiling group, so a name that names none is
    // answered with the agent plane's 404 whatever the data holds — the refusals above
    // are about the data of a request that reached a group.
    for (label, bytes) in [
        ("a profile", json(&serde_json::json!({ START: PROFILE_START_MILLIS }))),
        ("a struct with no start", b"{}".to_vec()),
        ("bytes that are no document", vec![b'{']),
    ] {
        let answer = harness
            .post_agent_profile("no-such-group", JSON, None, bytes)
            .await;
        assert_eq!(
            answer.status,
            404,
            "case {label:?}: expected HTTP 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}
