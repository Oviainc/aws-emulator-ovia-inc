// SPDX-License-Identifier: CC0-1.0

//! The refusals a client branches on, over the awsQuery wire.
//!
//! RULE: a request this service will not serve is refused with the taxonomy the service
//! defines for that refusal — the modelled error code and its HTTP status — carrying a
//! message built from the request and the constraint it failed, never with a
//! not-implemented refusal the service does not model.
//!
//! The three suites below are the three refusal surfaces a provisioning client keys its
//! retry and branch logic on, each over a table of inputs:
//!
//! * a template declaring more parameters than the per-template quota allows is refused by
//!   the submitting operation with `ValidationError` (HTTP 400) and a message naming what
//!   one entry of the section IS, the count the REQUEST declares and the fixed maximum —
//!   which is how a client tells a template-content limit apart from the account/Region
//!   resource-count `LimitExceededException`;
//! * a `GetHookResult` for a well-formed `HookResultId` no invocation was issued under is
//!   refused with the one error the operation models, `HookResultNotFoundException`
//!   (HTTP 404, wire code `HookResultNotFound`), so a probe can tell "no such result" from
//!   "service broken";
//! * a `RecordHandlerProgress` whose `BearerToken` names no live invocation is refused with
//!   `ValidationError` (HTTP 400) and a message quoting the token the request carried.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees, and a body AT the quota is read back through
//! `DescribeStacks` to show it was deployed holding everything it declared. The per-section
//! quota rule over every metered section and every submitting operation lives in
//! `template_quotas.rs`, the detailed Hook-result read in `hook_result_detail.rs` and the
//! progress-report state machine in `handler_progress_reports.rs`; the rows here span what
//! those do not — the body SPELLING a count is declared in, and the ids and tokens a client
//! probes with, including one carrying the characters XML reserves.

use service_com_amazonaws_cloudformation_cloudformation::state::handler_progress::operation_status;
use service_com_amazonaws_cloudformation_cloudformation::state::template::MAX_PARAMETERS_PER_TEMPLATE;

use crate::harness::{call, create_stack, member, router, xml_text, xml_texts};

/// The wire code this service refuses a request on input grounds with.
const REFUSAL_CODE: &str = "ValidationError";

/// A JSON template body declaring `parameters` parameters and `resources` resources.
///
/// Each parameter carries a `Default`, so a body at the quota needs no value from the
/// request and is submittable as it stands.
fn json_body(parameters: usize, resources: usize) -> String {
    let declared: serde_json::Map<String, serde_json::Value> = (0..parameters)
        .map(|index| {
            (
                format!("Param{index:04}"),
                serde_json::json!({"Type": "String", "Default": "value"}),
            )
        })
        .collect();
    let mut body = serde_json::Map::new();
    body.insert(
        "Parameters".to_string(),
        serde_json::Value::Object(declared),
    );
    if resources > 0 {
        let topics: serde_json::Map<String, serde_json::Value> = (0..resources)
            .map(|index| {
                (
                    format!("Topic{index:04}"),
                    serde_json::json!({"Type": "AWS::SNS::Topic"}),
                )
            })
            .collect();
        body.insert("Resources".to_string(), serde_json::Value::Object(topics));
    }
    serde_json::Value::Object(body).to_string()
}

/// The same declaration in the OTHER spelling a body may be submitted in: YAML. The counts
/// the refusal reports are taken off the template the service reads, not off the bytes it
/// was spelled in, so both spellings are refused identically.
fn yaml_body(parameters: usize) -> String {
    let mut body = String::from("AWSTemplateFormatVersion: '2010-09-09'\nParameters:\n");
    for index in 0..parameters {
        body.push_str(&format!(
            "  Param{index:04}:\n    Type: String\n    Default: value\n"
        ));
    }
    body
}

/// The refusal a body declaring `declared` parameters is answered with: the singular noun
/// for one entry of the section, the count the request itself declares, and the fixed
/// maximum it is past.
fn expected_parameter_refusal(declared: usize) -> String {
    format!(
        "Template format error: Parameter count {declared} is greater than max allowed \
         {MAX_PARAMETERS_PER_TEMPLATE}"
    )
}

#[tokio::test]
async fn a_body_past_the_parameter_quota_is_refused_with_the_count_it_declares() {
    let router = router();

    // The rows differ along the body's spelling, the count declared past the quota, and
    // whether the body declares a `Resources` section beside its parameters. Each carries
    // the refusal it expects: the first row writes it out from the PUBLISHED numbers, so it
    // holds the service to them and not to whatever the crate's constant currently says.
    let past_the_quota: Vec<(&str, String, String)> = vec![
        (
            "201 parameters, held to the published numbers",
            json_body(201, 1),
            "Template format error: Parameter count 201 is greater than max allowed 200"
                .to_string(),
        ),
        (
            "one parameter past the quota, parameters only",
            json_body(MAX_PARAMETERS_PER_TEMPLATE + 1, 0),
            expected_parameter_refusal(MAX_PARAMETERS_PER_TEMPLATE + 1),
        ),
        (
            "far past the quota, beside a resource",
            json_body(MAX_PARAMETERS_PER_TEMPLATE + 41, 1),
            expected_parameter_refusal(MAX_PARAMETERS_PER_TEMPLATE + 41),
        ),
        (
            "one parameter past the quota, in a YAML body",
            yaml_body(MAX_PARAMETERS_PER_TEMPLATE + 1),
            expected_parameter_refusal(MAX_PARAMETERS_PER_TEMPLATE + 1),
        ),
        (
            "far past the quota, in a YAML body",
            yaml_body(MAX_PARAMETERS_PER_TEMPLATE + 17),
            expected_parameter_refusal(MAX_PARAMETERS_PER_TEMPLATE + 17),
        ),
    ];

    for (row, (case, body, expected)) in past_the_quota.into_iter().enumerate() {
        let (status, response) =
            create_stack(&router, &format!("past-the-quota-{row}"), &body, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create refused, got {status} {response}"
        );
        assert_eq!(
            xml_text(&response, "Code").as_deref(),
            Some(REFUSAL_CODE),
            "case {case:?}: expected {REFUSAL_CODE}, got {response}"
        );
        let actual = xml_text(&response, "Message");
        assert_eq!(
            actual,
            Some(expected.clone()),
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // AT the quota, in either spelling: accepted, so the refusal is the ceiling and not the
    // section itself.
    for (row, (case, body)) in [
        (
            "at the quota, beside a resource",
            json_body(MAX_PARAMETERS_PER_TEMPLATE, 1),
        ),
        (
            "at the quota, in a YAML body",
            yaml_body(MAX_PARAMETERS_PER_TEMPLATE),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("at-the-quota-{row}");
        let (status, response) = create_stack(&router, &name, &body, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to succeed, got {status} {response}"
        );

        // Read the accepted body back: a body AT the quota is deployed holding everything it
        // declares, so the stack reports one parameter per declaration rather than having
        // been accepted and dropped.
        let (status, described) =
            call(&router, "DescribeStacks", &[member("StackName", &name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stack to be readable back, got {status} {described}"
        );
        let reported = xml_texts(&described, "ParameterKey");
        assert_eq!(
            reported.len(),
            MAX_PARAMETERS_PER_TEMPLATE,
            "case {case:?}: expected {MAX_PARAMETERS_PER_TEMPLATE} parameters on the deployed \
             stack, got {}",
            reported.len()
        );
    }
}

#[tokio::test]
async fn an_unissued_hook_result_id_is_refused_as_not_found() {
    let router = router();

    // The listing half of the hook-result read model answers for an account that has had no
    // Hook invoked, which is what makes a non-modelled refusal from the per-id read visible.
    let (status, listed) = call(&router, "ListHookResults", &[]).await;
    assert_eq!(
        status, 200,
        "case \"the summaries of an account with no invocation\": expected 200, got {status} \
         {listed}"
    );
    let summarised = xml_texts(&listed, "HookResultId");
    assert!(
        summarised.is_empty(),
        "case \"the summaries of an account with no invocation\": expected no summary, got \
         {summarised:?}"
    );

    // Every row is a well-formed `HookResultId` — `HookInvocationId` is `@length(36..36)`
    // with pattern `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$` — that
    // no invocation of this account was recorded under.
    for (case, hook_result_id) in [
        ("the all-zero id", "00000000-0000-0000-0000-000000000000"),
        (
            "a version-4 id no invocation was recorded under",
            "9f3c1b2a-5d6e-4f70-8a91-b2c3d4e5f607",
        ),
        (
            "an id spanning every hex digit",
            "abcdef01-2345-6789-abcd-ef0123456789",
        ),
    ] {
        let (status, response) = call(
            &router,
            "GetHookResult",
            &[member("HookResultId", hook_result_id)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {hook_result_id}, got {status} {response}"
        );
        let code = xml_text(&response, "Code");
        assert_eq!(
            code.as_deref(),
            Some("HookResultNotFound"),
            "case {case:?}: expected the modelled wire code, got {code:?} in {response}"
        );
        // The sentence the service answers this refusal with, written out rather than read
        // off the implementation's own constant.
        let expected = "The hook result was not found.";
        let message = xml_text(&response, "Message");
        assert_eq!(
            message.as_deref(),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_bearer_token_no_invocation_owns_is_refused_naming_the_token() {
    let router = router();

    // The rows differ along the FORM of the token a client reports under: `BearerToken` is a
    // `ClientToken` (`@length(1..128)`) with no pattern, so a token of any spelling in that
    // length names an invocation or names none.
    let tokens: Vec<(&str, String)> = vec![
        (
            "a token shaped like a CLI invocation id",
            "handler-invocation-0000".to_string(),
        ),
        (
            "a UUID-shaped token",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
        (
            "a one-character token, the shortest the model admits",
            "x".to_string(),
        ),
        (
            "a token at the modelled maximum length",
            "t".repeat(128),
        ),
        // The model puts no pattern on `ClientToken`, so a token may carry the characters
        // XML reserves: the refusal quotes it in the envelope, which therefore has to carry
        // it encoded and still parse.
        (
            "a token carrying the characters XML reserves",
            "<token> & \"quoted\" 't'".to_string(),
        ),
    ];

    for (case, token) in &tokens {
        let (status, response) = call(
            &router,
            "RecordHandlerProgress",
            &[
                member("BearerToken", token),
                member("OperationStatus", operation_status::SUCCESS),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the report refused, got {status} {response}"
        );
        assert_eq!(
            xml_text(&response, "Code").as_deref(),
            Some(REFUSAL_CODE),
            "case {case:?}: expected {REFUSAL_CODE}, got {response}"
        );
        // The wording the service answers an unrecognised token with, written out here, with
        // only the token itself coming from the row.
        let expected = format!("BearerToken '{token}' is invalid");
        let message = xml_text(&response, "Message");
        assert_eq!(
            message,
            Some(expected.clone()),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // A token carrying characters XML reserves is quoted back ENCODED, so the envelope
        // the client parses is still one document — each reserved character is checked on
        // its own, so a row carrying only one of them is held to that one.
        for (reserved, encoded) in [('<', "&lt;"), ('>', "&gt;"), ('&', "&amp;")] {
            if !token.contains(reserved) {
                continue;
            }
            assert!(
                !response.contains(token.as_str()),
                "case {case:?}: the quoted token must be encoded in the envelope, got {response}"
            );
            assert!(
                response.contains(encoded),
                "case {case:?}: expected {reserved:?} encoded as {encoded:?}, got {response}"
            );
        }
    }
}
