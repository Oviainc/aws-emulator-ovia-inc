// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the rule that ONE REQUEST CANNOT SIZE THE RESPONSE IT IS
//! ANSWERED WITH.
//!
//! The RULE under test, asserted over tables rather than one recorded request: a refusal
//! this service words itself names the value the request addressed only through the
//! bounded rendering `service::error_detail::echoed` — the value whole while it is within
//! `MAX_ECHOED_VALUE_LEN` bytes, and past that a prefix of that many bytes followed by the
//! length of the whole value — so the body answering a request is sized by this service,
//! never by the caller, whether or not the member that carried the value declares a
//! modelled bound.
//!
//! The rows differ along what the rule generalises over: WHICH member carries the value
//! (the `resourceArn` label of all three tagging operations, and the `clientToken` body
//! member of `StartPatientInsightsJob`), which refusal names it (404
//! `ResourceNotFoundException` and 409 `ConflictException`), the SIZE of the value (an
//! ordinary one, one byte short of the ceiling, exactly the ceiling, one byte past it, and
//! far past it) and its FORM (ASCII, and multi-byte characters, which may not be cut
//! mid-character).
//!
//! Sources for every bound written by hand here:
//!
//! * `smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `resourceArn` is an `@httpLabel` targeting the prelude `String`: required, and
//!   bounded by neither `@length` nor `@pattern`, so nothing below the service bounds what
//!   a refusal naming it would echo.
//! * `smithy_query(query="operation",operation="StartPatientInsightsJob",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `clientToken` targets `NonEmptyString`, which carries a `@pattern` and no
//!   `@length`: the second member of this service a refusal echoes with no modelled
//!   ceiling behind it.
//!
//! The harness (`Emulator`, `error_code`, `string_at`), the routes and the minimal job
//! body are the sibling suites': one way of driving this service, not two.

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::service::error_detail::MAX_ECHOED_VALUE_LEN;

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::patient_insights_job_lifecycle::{minimal_body, start_path};
use crate::resource_tag_removal::untag_path;
use crate::resource_tagging::tags_path;

/// The elision an over-long value is echoed with: the whole value's length, stated after
/// the prefix that was echoed of it.
fn elision(value: &str) -> String {
    format!("... [{} bytes]", value.len())
}

/// The `Message` member of a refusal body, which every refusal of this service carries.
fn message(case: &str, body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a Message, got {body}"))
        .to_string()
}

/// Assert that `message` names `value` the way the rule says: whole while the value is
/// within the ceiling, and otherwise as a leading prefix of at most the ceiling followed
/// by the whole value's length — never the value itself.
fn names_the_value_bounded(case: &str, message: &str, value: &str) {
    if value.len() <= MAX_ECHOED_VALUE_LEN {
        assert!(
            message.contains(value),
            "case {case:?}: a value within the ceiling must be named whole, got {message:?}"
        );
        return;
    }
    assert!(
        !message.contains(value),
        "case {case:?}: the whole value must not be echoed, got a {}-byte message",
        message.len()
    );
    assert!(
        message.contains(&elision(value)),
        "case {case:?}: the message must state the elided value's whole length, \
         got {message:?}"
    );
    // The prefix that WAS echoed is the value's own leading bytes, cut on a character
    // boundary (so a multi-byte value is not cut mid-character) and no longer than the
    // ceiling.
    let elided = elision(value);
    let cut = message
        .find(&elided)
        .unwrap_or_else(|| panic!("case {case:?}: the elision must be in {message:?}"));
    let prefix: String = message[..cut]
        .chars()
        .rev()
        .take_while(|character| value.contains(*character))
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    assert!(
        !prefix.is_empty() && value.starts_with(&prefix) && prefix.len() <= MAX_ECHOED_VALUE_LEN,
        "case {case:?}: the echoed prefix must be the value's own leading characters \
         within the ceiling, got {prefix:?}"
    );
    // And the sentence as a whole is sized by this service: the ceiling, the elision and
    // the service's own words, with room for a second echoed value, and nothing that grows
    // with the request.
    assert!(
        message.len() <= 2 * (MAX_ECHOED_VALUE_LEN + elided.len()) + 256,
        "case {case:?}: the message must not be sized by the request, got {} bytes for a \
         {}-byte value",
        message.len(),
        value.len()
    );
}

/// One `resourceArn` per size the rule distinguishes, none of them naming a resource this
/// service holds: a well-formed ARN of ordinary length, then ARNs padded in the resource
/// field to each side of the ceiling and far past it.
///
/// ASCII only, because these travel in a URI label; the multi-byte rows ride on
/// `clientToken`, a body member, in the test below.
fn arns_around_the_ceiling() -> Vec<(String, String)> {
    let prefix = "arn:aws:health-agent:us-west-2:000000000000:domain/dom-";
    let pad = |total: usize| -> String {
        let filler = total.saturating_sub(prefix.len());
        format!("{prefix}{}", "a".repeat(filler))
    };
    vec![
        ("an ARN of ordinary length".to_string(), pad(prefix.len() + 20)),
        (
            "one byte short of the ceiling".to_string(),
            pad(MAX_ECHOED_VALUE_LEN - 1),
        ),
        ("exactly the ceiling".to_string(), pad(MAX_ECHOED_VALUE_LEN)),
        (
            "one byte past the ceiling".to_string(),
            pad(MAX_ECHOED_VALUE_LEN + 1),
        ),
        (
            "far past the ceiling".to_string(),
            pad(16 * MAX_ECHOED_VALUE_LEN),
        ),
    ]
}

#[tokio::test]
async fn the_arn_not_found_refusal_is_sized_by_the_service() {
    let emulator = Emulator::new("us-west-2");
    // A live domain, so the refusals below are answered by a service that holds records
    // rather than by an empty store.
    let created = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod" } }))
        .await;
    let held_arn = string_at(&created, "arn");

    for (case, arn) in arns_around_the_ceiling() {
        // Every operation addressed by `resourceArn`: the reader, the writer and the
        // remover answer one ARN the same way, so the bound cannot hold on one of them
        // only.
        let requests: Vec<(&str, &str, String, Option<Value>)> = vec![
            ("ListTagsForResource", "GET", tags_path(&arn), None),
            (
                "TagResource",
                "POST",
                tags_path(&arn),
                Some(json!({ "tags": { "env": "prod" } })),
            ),
            (
                "UntagResource",
                "DELETE",
                untag_path(&arn, &["env".to_string()]),
                None,
            ),
        ];
        for (operation, method, path, body) in requests {
            let label = format!("{operation}: {case}");
            let (status, refused) = emulator.send(method, &path, body.as_ref()).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (404, "ResourceNotFoundException"),
                "case {label:?}: expected 404 ResourceNotFoundException, got {status} with \
                 a {}-byte body",
                refused.to_string().len()
            );
            let stated = message(&label, &refused);
            names_the_value_bounded(&label, &stated, &arn);
        }
    }

    // The bound changes nothing about the service's answers: the ARN it does hold is
    // still served, and its own refusal wording for an ordinary ARN is unchanged.
    let (status, listed) = emulator.send("GET", &tags_path(&held_arn), None).await;
    assert_eq!(
        status, 200,
        "the held ARN must still be served, got {status} with {listed}"
    );
    let unheld = format!("{held_arn}-nope");
    let (status, refused) = emulator.send("GET", &tags_path(&unheld), None).await;
    assert_eq!(status, 404, "an unheld ARN must be refused, got {refused}");
    assert_eq!(
        message("an ARN of ordinary length", &refused),
        format!("Resource not found: {unheld}"),
        "an ARN of ordinary length must be named exactly as before, got {refused}"
    );
}

#[tokio::test]
async fn the_idempotency_conflict_is_sized_by_the_service() {
    let emulator = Emulator::new("eu-central-1");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic-tokens" })).await,
        "domainId",
    );

    // One row per size and form the rule distinguishes. `clientToken` rides in the body,
    // so a multi-byte token reaches the handler as sent — which is the case a cut on a
    // byte index rather than a character boundary would break.
    let tokens: Vec<(&str, String)> = vec![
        ("a token of ordinary length", "token-7781".to_string()),
        (
            "exactly the ceiling",
            "t".repeat(MAX_ECHOED_VALUE_LEN),
        ),
        (
            "one byte past the ceiling",
            "t".repeat(MAX_ECHOED_VALUE_LEN + 1),
        ),
        (
            "far past the ceiling",
            "t".repeat(64 * MAX_ECHOED_VALUE_LEN),
        ),
        // Two-byte and three-byte characters: well past a ceiling counted in bytes, and
        // with no character boundary at the ceiling itself.
        (
            "multi-byte characters",
            "é".repeat(MAX_ECHOED_VALUE_LEN),
        ),
        (
            "a character straddling the ceiling",
            format!("{}{}", "t".repeat(MAX_ECHOED_VALUE_LEN - 1), "日".repeat(8)),
        ),
    ];

    for (case, token) in &tokens {
        // The token bound to one job's parameters...
        let mut first = minimal_body();
        first["clientToken"] = json!(token);
        let (status, started) = emulator
            .send("POST", &start_path(&domain_id), Some(&first))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first start must be accepted, got {status} with {started}"
        );
        let job_id = string_at(&started, "jobId");

        // ...then presented again with different parameters, which is the conflict that
        // names the token back.
        let mut conflicting = first.clone();
        conflicting["patientContext"] = json!({ "patientId": "patient-9000" });
        let (status, refused) = emulator
            .send("POST", &start_path(&domain_id), Some(&conflicting))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} with a {}-byte body",
            refused.to_string().len()
        );
        let stated = message(case, &refused);
        names_the_value_bounded(case, &stated, token);
        // The job the first call stored is still what the sentence reports the token
        // against, so bounding the echo did not cost the caller the fact it needs.
        assert!(
            stated.contains(&job_id),
            "case {case:?}: the conflict must name the job the token is bound to, \
             got {stated:?}"
        );
    }
}
