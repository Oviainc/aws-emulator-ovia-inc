// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request by the RESOURCE it names, end to end over
//! the router.
//!
//! Three rules, each asserted over a SET of requests rather than one:
//!
//! 1. **An unreadable GUARDRAIL reference is refused in the service's own
//!    words.** `ApplyGuardrail` names the guardrail it is to apply in its URI
//!    (`guardrailIdentifier`/`guardrailVersion`), and a reference the service
//!    cannot read is refused with `400 ValidationException` carrying the
//!    service's own sentence for THAT reference, and not the per-member
//!    `N validation error(s) detected: …` envelope. A request wrong in both is
//!    refused on the first one READ.
//! 2. **Every other member outside its constraints still gets that envelope** —
//!    body member and URI label alike, including the `modelId` that is a label on
//!    five inference URIs and a body member of `StartAsyncInvoke` — so the rule
//!    above does not swallow the one thing a client needs for a bad value: which
//!    member and which constraint. (How a value SET is listed inside such a
//!    clause is the `guardrail_checks_inline` suite's subject.)
//! 3. **A resource ARN is resolved in one order.** An invocation ARN is read,
//!    then placed in a region, then attributed to an account, then read for the
//!    invocation id it names, and only then looked up: an unreadable reference
//!    and one of another region are client-input refusals
//!    (`400 ValidationException`), one of another account is an authorization
//!    refusal (`403 AccessDeniedException`, reason under the key `Message`), and a
//!    readable in-region ARN of this account that names no invocation is the
//!    service's own absent-job refusal. A minted ARN still reads back.
//!
//! Every assertion names its case and the actual value.
//!
//! Every assertion names its case and the actual value.

use serde_json::json;

use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;

// The harness every suite of this target shares.
use crate::bedrock_runtime_inference::{emulator, encode_path, get, post_json, Call};

/// One request whose only fault is the resource it names.
struct ReferenceRow {
    case: &'static str,
    /// `None` for a GET, `Some(body)` for a POST.
    body: Option<serde_json::Value>,
    path: String,
    /// The sentence the service answers for this kind of reference.
    message: &'static str,
}

/// A `Converse`-shaped body, so a row never fails on the body instead of on the
/// reference under test.
fn converse_body() -> serde_json::Value {
    json!({ "messages": [{ "role": "user", "content": [{ "text": "hello" }] }] })
}

/// A `CountTokens` body over the same conversation.
fn count_tokens_body() -> serde_json::Value {
    json!({ "input": { "converse": converse_body() } })
}

/// An `ApplyGuardrail` body the operation's own constraints accept, so a row
/// fails on the guardrail reference and nothing else.
fn apply_guardrail_body() -> serde_json::Value {
    json!({
        "source": "INPUT",
        "content": [{ "text": { "text": "hello" } }],
    })
}

/// The rows: every guardrail reference member, each with more than one
/// unreadable value, so neither the member nor the value a recording happened to
/// carry is what the rule is keyed on.
///
/// The values are unreadable in DIFFERENT ways — a case the reference's
/// `@pattern` admits nowhere, a separator it admits nowhere, a value past the
/// reference's length, a version that is neither a number nor `DRAFT`, a version
/// numbered from zero, a version past its published digits — and the sentence is
/// the same for every value of one reference.
fn reference_rows() -> Vec<ReferenceRow> {
    let mut rows = vec![
        ReferenceRow {
            case: "a guardrail identifier in the wrong case",
            body: Some(apply_guardrail_body()),
            path: "/guardrail/INVALID_GUARDRAIL_ID/version/DRAFT/apply".to_string(),
            message: state::GUARDRAIL_REFERENCE_FORMAT_MESSAGE,
        },
        ReferenceRow {
            case: "a guardrail identifier carrying a separator",
            body: Some(apply_guardrail_body()),
            path: "/guardrail/my-guardrail/version/1/apply".to_string(),
            message: state::GUARDRAIL_REFERENCE_FORMAT_MESSAGE,
        },
        ReferenceRow {
            case: "a guardrail version that is neither a number nor DRAFT",
            body: Some(apply_guardrail_body()),
            path: "/guardrail/abc123/version/latest/apply".to_string(),
            message: state::INVALID_REQUEST_MESSAGE,
        },
        ReferenceRow {
            case: "a guardrail version numbered from zero",
            body: Some(apply_guardrail_body()),
            path: "/guardrail/abc123/version/0/apply".to_string(),
            message: state::INVALID_REQUEST_MESSAGE,
        },
        ReferenceRow {
            case: "a guardrail version past its published digits",
            body: Some(apply_guardrail_body()),
            path: "/guardrail/abc123/version/123456789/apply".to_string(),
            message: state::INVALID_REQUEST_MESSAGE,
        },
        ReferenceRow {
            // The guardrail is read BEFORE the version that selects one of its
            // versions, so a request wrong in both is refused on the guardrail.
            case: "a guardrail identifier and version both unreadable",
            body: Some(apply_guardrail_body()),
            path: "/guardrail/NOT_AN_ID/version/latest/apply".to_string(),
            message: state::GUARDRAIL_REFERENCE_FORMAT_MESSAGE,
        },
    ];
    // One value past the reference's own published bound: `GuardrailIdentifier`
    // carries `@length max 2048` (`smithy_query(query="shape",
    // shape="GuardrailIdentifier")`), so 2049 admissible characters break the
    // length rather than the pattern — still the same sentence.
    rows.push(ReferenceRow {
        case: "a guardrail identifier one character past its length",
        body: Some(apply_guardrail_body()),
        path: format!("/guardrail/{}/version/DRAFT/apply", "a".repeat(2049)),
        message: state::GUARDRAIL_REFERENCE_FORMAT_MESSAGE,
    });
    rows
}

/// The `message` member of a modelled refusal.
fn message_of(call: &Call) -> String {
    call.json()["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE 1: a request whose guardrail reference the service cannot read is
/// refused with `400 ValidationException` and the service's own sentence for
/// that reference — never the per-member constraint envelope.
#[tokio::test]
async fn an_unreadable_guardrail_reference_is_refused_in_the_services_own_words() {
    let router = emulator();
    for row in reference_rows() {
        let case = row.case;
        let call = match &row.body {
            Some(body) => post_json(&router, &row.path, body).await,
            None => get(&router, &row.path).await,
        };
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
        assert_eq!(
            message_of(&call),
            row.message,
            "case {case:?}: expected {:?}, got {:?}",
            row.message,
            message_of(&call)
        );
    }
}

/// RULE 2: every other member outside its constraints is still stated as the
/// per-member envelope, which names the member and the requirement it broke.
///
/// The rows are three different constraint kinds on a body member, including the
/// `modelId` that `StartAsyncInvoke` carries in its BODY; the block after them is
/// the same member as a URI LABEL, which its own operation resolves. So the
/// guardrail wording above reaches neither surface of a member it does not name.
#[tokio::test]
async fn a_member_outside_its_constraints_still_names_the_member_and_the_requirement() {
    let router = emulator();
    // (case, method, path, body, the member path the clause must name, the
    // requirement text it must carry)
    let rows: Vec<(&str, &str, serde_json::Value, &str, &str)> = vec![
        (
            "a pattern on the output destination",
            "/async-invoke",
            json!({
                "modelId": "luma.ray-v2:0",
                "modelInput": { "prompt": "a kite" },
                "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": "not-a-valid-s3-uri" } },
            }),
            "outputDataConfig.s3OutputDataConfig.s3Uri",
            "Member must satisfy regular expression pattern",
        ),
        (
            "an enum value set on an inline check category",
            "/guardrail-checks/invoke",
            json!({
                "messages": [{ "role": "user", "content": [{ "text": "hello" }] }],
                "checks": { "contentFilter": { "categories": [{ "category": "NOT_A_CATEGORY" }] } },
            }),
            "checks.contentFilter.categories.1.member.category",
            "Member must satisfy enum value set",
        ),
        (
            "an absent required member",
            "/async-invoke",
            json!({ "modelId": "luma.ray-v2:0", "modelInput": { "prompt": "a kite" } }),
            "outputDataConfig",
            "Member must not be null",
        ),
        (
            "a pattern on the model identifier in the body",
            "/async-invoke",
            json!({
                "modelId": "not a model id!",
                "modelInput": { "prompt": "a kite" },
                "outputDataConfig": { "s3OutputDataConfig": {
                    "s3Uri": "s3://emulator-bucket/async-out",
                } },
            }),
            "modelId",
            "Member must satisfy regular expression pattern",
        ),
    ];

    for (case, path, body, member, requirement) in rows {
        let call = post_json(&router, path, &body).await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let message = message_of(&call);
        assert!(
            message.contains("validation error") && message.contains(member),
            "case {case:?}: expected the per-member envelope naming {member:?}, got {message:?}"
        );
        assert!(
            message.contains(requirement),
            "case {case:?}: expected the clause to carry {requirement:?}, got {message:?}"
        );
    }

    // The same member as a URI LABEL. Nothing in this change touches it: a value
    // the model's own constraints reject keeps the per-member envelope, and one
    // they admit is resolved by the OPERATION
    // (`state::resolve_model_for_inference`), which answers the service's own
    // sentence for an identifier it cannot interpret. The three rows are the two
    // outcomes and the two members the model gives this label
    // (`smithy_query(query="operation",operation="Converse,CountTokens,\
    // InvokeModel")`: `CountTokens` constrains it with a `@pattern`, `Converse`
    // does not).
    for (case, path, body, expected) in [
        (
            "a model id carrying a '!', on Converse",
            "/model/claude!v1/converse",
            converse_body(),
            state::INVALID_MODEL_IDENTIFIER_MESSAGE.to_string(),
        ),
        (
            "a model id no inventory carries, on InvokeModel",
            "/model/vendor.not-a-model-v9/invoke",
            json!({ "prompt": "hello" }),
            state::INVALID_MODEL_IDENTIFIER_MESSAGE.to_string(),
        ),
        (
            "a model id carrying an escaped '#', on CountTokens",
            "/model/anthropic%23claude/count-tokens",
            count_tokens_body(),
            "1 validation error detected: Value 'anthropic#claude' at 'modelId' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: \
             [a-zA-Z_\\.\\-/0-9:]+"
                .to_string(),
        ),
    ] {
        let call = post_json(&router, path, &body).await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            message_of(&call),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message_of(&call)
        );
    }
}

/// RULE 3: a resource ARN is RESOLVED in one order — read, placed in a region,
/// attributed to an account, read for the invocation id it names, and only then
/// looked up — so a reference that is wrong in more than one way is reported on
/// the first rule it breaks, and a minted ARN still reads back.
///
/// The rows break each rung in turn, with more than one value per rung, so the
/// ladder is not satisfied by one ARN's special case. The request is signed for
/// `us-east-1` (the harness's own credential scope), which is the region the
/// second rung is about.
#[tokio::test]
async fn a_resource_arn_is_resolved_in_one_order_before_the_record_is_looked_up() {
    let router = emulator();
    let own_account = "000000000000";
    let region = "us-east-1";

    // (case, the ARN, the status expected, the error code expected, the member the
    // reason is carried under, the reason)
    let mut rows: Vec<(String, String, u16, &str, &str, &str)> = vec![
        (
            "no ARN at all".to_string(),
            "not-an-arn".to_string(),
            400,
            "ValidationException",
            "message",
            state::INVALID_ASYNC_INVOKE_ARN_MESSAGE,
        ),
        (
            "an ARN of another service".to_string(),
            format!("arn:aws:lambda:{region}:{own_account}:async-invoke/abcdef123456"),
            400,
            "ValidationException",
            "message",
            state::INVALID_ASYNC_INVOKE_ARN_MESSAGE,
        ),
        (
            "an ARN of another resource of this service".to_string(),
            format!("arn:aws:bedrock:{region}:{own_account}:guardrail/abcdef123456"),
            400,
            "ValidationException",
            "message",
            state::INVALID_ASYNC_INVOKE_ARN_MESSAGE,
        ),
        (
            "an ARN carrying no invocation id".to_string(),
            format!("arn:aws:bedrock:{region}:{own_account}:async-invoke/"),
            400,
            "ValidationException",
            "message",
            state::INVALID_ASYNC_INVOKE_ARN_MESSAGE,
        ),
        (
            "another region, this account".to_string(),
            format!("arn:aws:bedrock:eu-west-1:{own_account}:async-invoke/abcdef123456"),
            400,
            "ValidationException",
            "message",
            state::FOREIGN_REGION_RESOURCE_ARN_MESSAGE,
        ),
        (
            "another region, another account — the region is answered first".to_string(),
            "arn:aws:bedrock:ap-south-1:123456789012:async-invoke/abcdef123456".to_string(),
            400,
            "ValidationException",
            "message",
            state::FOREIGN_REGION_RESOURCE_ARN_MESSAGE,
        ),
        (
            "this region, another account".to_string(),
            format!("arn:aws:bedrock:{region}:123456789012:async-invoke/abcdef123456"),
            403,
            "AccessDeniedException",
            "Message",
            state::CROSS_ACCOUNT_RESOURCE_ARN_MESSAGE,
        ),
        (
            "this region, another account, an id of another form".to_string(),
            format!("arn:aws:bedrock:{region}:210987654321:async-invoke/NOT-AN-ID"),
            403,
            "AccessDeniedException",
            "Message",
            state::CROSS_ACCOUNT_RESOURCE_ARN_MESSAGE,
        ),
        (
            "this region and account, an id no job has".to_string(),
            format!("arn:aws:bedrock:{region}:{own_account}:async-invoke/abcdef123456"),
            400,
            "ValidationException",
            "message",
            state::ASYNC_INVOKE_NOT_FOUND_MESSAGE,
        ),
        (
            "this region and account, an id outside the minted form".to_string(),
            format!("arn:aws:bedrock:{region}:{own_account}:async-invoke/ABCDEF123456"),
            400,
            "ValidationException",
            "message",
            state::INVALID_ASYNC_INVOKE_ARN_MESSAGE,
        ),
        (
            "this region and account, an id of the wrong length".to_string(),
            format!("arn:aws:bedrock:{region}:{own_account}:async-invoke/abc123"),
            400,
            "ValidationException",
            "message",
            state::INVALID_ASYNC_INVOKE_ARN_MESSAGE,
        ),
    ];
    // One value past the ARN length the model publishes.
    rows.push((
        "a value past the published ARN length".to_string(),
        format!(
            "arn:aws:bedrock:{region}:{own_account}:async-invoke/{}",
            "a".repeat(state::MAX_RESOURCE_ARN_LENGTH)
        ),
        400,
        "ValidationException",
        "message",
        state::INVALID_ASYNC_INVOKE_ARN_MESSAGE,
    ));

    for (case, arn, status, code, member, reason) in rows {
        let call = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
        assert_eq!(
            call.status,
            status,
            "case {case:?}: expected {status}, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            call.error_code()
        );
        let body = call.json();
        assert_eq!(
            body[member].as_str(),
            Some(reason),
            "case {case:?}: expected {reason:?} under the key {member:?}, got {body}"
        );
    }

    // And the ladder refuses nothing this account minted in this region: the round
    // trip through the implemented reader still answers the job.
    let started = post_json(
        &router,
        "/async-invoke",
        &json!({
            "modelId": "luma.ray-v2:0",
            "modelInput": {
                "prompt": "a kite over a lake at dawn",
                "aspect_ratio": "16:9",
                "loop": false,
                "duration": "9s",
                "resolution": "720p",
            },
            "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": "s3://emulator-bucket/out" } },
        }),
    )
    .await;
    assert_eq!(
        started.status,
        200,
        "case \"a job this account mints\": expected 200, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let arn = started.json()["invocationArn"]
        .as_str()
        .expect("StartAsyncInvoke answers an invocationArn")
        .to_string();
    let read_back = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
    assert_eq!(
        read_back.status,
        200,
        "case \"a job this account mints\": expected the read back to answer 200, got {} / {}",
        read_back.status,
        String::from_utf8_lossy(&read_back.body)
    );
    assert_eq!(
        read_back.json()["invocationArn"].as_str(),
        Some(arn.as_str()),
        "case \"a job this account mints\": expected the same ARN read back, got {}",
        read_back.json()
    );
}
