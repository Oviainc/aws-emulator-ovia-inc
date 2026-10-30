// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's REFUSAL contract.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through
//! the ONE harness this test crate has (`telemetry_rules`), so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! A refusal's status, its error code AND its `Message` are the contract: a client
//! that surfaces or matches the sentence sees the service's own words, and only the
//! parts the service itself interpolates vary with the request. The rules under test:
//!
//! 1. a read of a resource no record answers carries that operation's fixed sentence
//!    with NO part of the request in it, and `ResourceNotFoundException$ResourceType`
//!    — "the type of the resource which could not be found" — names the resource in
//!    the API's own words, while the identifier the request carried is reported in
//!    `ResourceId`, the member modelled for it;
//! 2. a MUTATING telemetry-rule operation refuses an identifier nothing answers by
//!    naming the resource it would have changed by ARN, whichever of the two forms
//!    `RuleIdentifier` admits the request took — a deliberately different sentence
//!    from the read path's;
//! 3. a create refused because the name is already in use carries a fixed sentence
//!    that names the INPUT MEMBER which carried the name, reports that member in
//!    `ResourceType` and the submitted value in `ResourceId`, and never splices the
//!    value into the message;
//! 4. the S3 Table integration is the account's ONE per regional endpoint: a create
//!    while the endpoint holds one is a `ConflictException` with its own fixed
//!    sentence and the `x-amzn-errortype` header, it stores nothing, another
//!    endpoint's slot is its own, and a delete frees the slot;
//! 5. a request that supplies BOTH forms of a multi-region scope is refused with one
//!    fixed sentence wherever this service refuses that pair, naming the two
//!    parameters in their lowerCamel wire form.
//!
//! The sentences and the `ResourceType` values asserted below are written out rather
//! than read from the service's constants, so a change to either is a failure here.
//! The member names they carry are the modelled ones:
//! `smithy_query(query="operation",operation="CreateTelemetryRule,CreateTelemetryPipeline")`.

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::{
    pipeline_arn, rule_arn, s3_table_integration_arn,
    ComAmazonawsObservabilityadminObservabilityadminState,
};

use crate::telemetry_rules::{call, emulator, error_code, request, Emulator, ACCOUNT};

/// The region most requests below sign for.
const HOME_REGION: &str = "us-west-2";

/// A second region, so a refusal keyed on the caller's own endpoint can be shown to
/// be per-endpoint rather than store-wide.
const OTHER_REGION: &str = "eu-west-1";

/// A role ARN every integration create below carries; it satisfies `ResourceArn`'s
/// own modelled `^arn:aws…` pattern.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/telemetry-s3-tables";

/// The smallest rule body the model admits: `TelemetryRule$TelemetryType` is the one
/// required member.
fn rule_body() -> Value {
    json!({ "TelemetryType": "Logs" })
}

/// The smallest pipeline configuration the model admits.
fn pipeline_configuration() -> Value {
    json!({ "Body": "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n" })
}

/// One string member of a response, absent members read as a stand-in that can never
/// equal an expected value.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

/// Status, `x-amzn-errortype` header and parsed body of one call. The harness's own
/// [`call`] folds the header into the body; this keeps them apart, so the header can
/// be asserted on its own.
async fn call_with_headers(
    emulator: &Emulator,
    region: &str,
    uri: &str,
    body: &Value,
) -> (u16, Option<String>, Value) {
    let response = emulator.router.dispatch(request(region, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, error_type, parsed)
}

/// How many integration records the shared store holds.
fn stored_integrations(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<ComAmazonawsObservabilityadminObservabilityadminState>()
        .lock()
        .s3_table_integrations
        .len()
}

/// A read of a resource no record answers reports that operation's fixed sentence and
/// the resource's own API name, for every form of identifier the operation admits.
#[tokio::test]
async fn a_read_of_a_missing_resource_reports_the_services_own_sentence() {
    let emulator = emulator();

    // One rule, one pipeline and one integration live in ANOTHER region, so a read
    // signed for the home endpoint has a resource of the right name to miss.
    let (status, created) = call(
        &emulator,
        OTHER_REGION,
        "/CreateTelemetryRule",
        &json!({ "RuleName": "resident", "Rule": rule_body() }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    let (status, created) = call(
        &emulator,
        OTHER_REGION,
        "/CreateTelemetryPipeline",
        &json!({ "Name": "resident-pipeline", "Configuration": pipeline_configuration() }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    let (status, created) = call(
        &emulator,
        OTHER_REGION,
        "/CreateS3TableIntegration",
        &json!({ "Encryption": { "SseAlgorithm": "AES256" }, "RoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    let resident_integration = member(&created, "Arn");

    // The rows differ along what the rule generalises over: which resource is read,
    // which of the identifier forms the request carries (a bare name, the full ARN,
    // an ARN of another region or another account), how long the name is and which
    // of the characters the identifier's pattern admits it uses.
    let unknown_integration = s3_table_integration_arn(HOME_REGION, &"f".repeat(32));
    let rows: Vec<(&str, &str, String, &str, &str)> = vec![
        (
            "a rule name nothing was created under",
            "/GetTelemetryRule",
            "no-such-rule".to_string(),
            "Rule not found.",
            "Rule",
        ),
        (
            "a rule name of one character",
            "/GetTelemetryRule",
            "r".to_string(),
            "Rule not found.",
            "Rule",
        ),
        (
            "a rule name using every character class RuleName admits",
            "/GetTelemetryRule",
            "a.b#c/d-e_f0".to_string(),
            "Rule not found.",
            "Rule",
        ),
        (
            "a rule ARN of this region",
            "/GetTelemetryRule",
            rule_arn(HOME_REGION, "no-such-rule"),
            "Rule not found.",
            "Rule",
        ),
        (
            "a rule that lives in another region, by name",
            "/GetTelemetryRule",
            "resident".to_string(),
            "Rule not found.",
            "Rule",
        ),
        (
            "a rule that lives in another region, by ARN",
            "/GetTelemetryRule",
            rule_arn(OTHER_REGION, "resident"),
            "Rule not found.",
            "Rule",
        ),
        (
            "a pipeline name nothing was created under",
            "/GetTelemetryPipeline",
            "no-such-pipeline".to_string(),
            "Pipeline not found",
            "Pipeline",
        ),
        (
            "a pipeline ARN of this region",
            "/GetTelemetryPipeline",
            pipeline_arn(HOME_REGION, "no-such-pipeline"),
            "Pipeline not found",
            "Pipeline",
        ),
        (
            "a pipeline that lives in another region, by name",
            "/GetTelemetryPipeline",
            "resident-pipeline".to_string(),
            "Pipeline not found",
            "Pipeline",
        ),
        (
            "an integration ARN naming an id nothing was created under",
            "/GetS3TableIntegration",
            unknown_integration,
            "Integration not found",
            "Integration",
        ),
        (
            "an integration that lives in another region",
            "/GetS3TableIntegration",
            resident_integration,
            "Integration not found",
            "Integration",
        ),
        (
            "an integration ARN of another account",
            "/GetS3TableIntegration",
            format!(
                "arn:aws:observabilityadmin:{HOME_REGION}:000000000001:s3tableintegration/\
                 ffffffffffffffffffffffffffffffff"
            ),
            "Integration not found",
            "Integration",
        ),
    ];

    for (case, uri, identifier, expected_message, expected_type) in rows {
        let body = match uri {
            "/GetTelemetryRule" => json!({ "RuleIdentifier": identifier }),
            "/GetTelemetryPipeline" => json!({ "PipelineIdentifier": identifier }),
            _ => json!({ "Arn": identifier }),
        };
        let (status, response) = call(&emulator, HOME_REGION, uri, &body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
        let message = member(&response, "Message");
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected Message {expected_message:?}, got {message:?}"
        );
        assert_eq!(
            member(&response, "ResourceType"),
            expected_type,
            "case {case:?}: expected ResourceType {expected_type:?}, got {:?}",
            member(&response, "ResourceType")
        );
        assert_eq!(
            member(&response, "ResourceId"),
            identifier,
            "case {case:?}: expected the requested identifier in ResourceId, got {:?}",
            member(&response, "ResourceId")
        );
        // Nothing of the request's own is spliced into a sentence the service keeps
        // fixed.
        assert!(
            !message.contains(&identifier) && !message.contains(HOME_REGION),
            "case {case:?}: expected a fixed sentence carrying neither {identifier:?} nor \
             {HOME_REGION:?}, got {message:?}"
        );
    }
}

/// A mutating telemetry-rule operation refuses an identifier nothing answers by
/// naming the resource it would have changed by ARN — and that is a different
/// sentence from the read path's.
#[tokio::test]
async fn a_mutating_rule_refusal_names_the_resource_by_arn() {
    let emulator = emulator();
    let (status, created) = call(
        &emulator,
        OTHER_REGION,
        "/CreateTelemetryRule",
        &json!({ "RuleName": "resident", "Rule": rule_body() }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");

    // The rows differ along what the rule generalises over: which mutating operation
    // is called, which identifier form it carries (a bare name the service resolves
    // into an ARN, or an ARN it reports as given), the characters and length of the
    // name, and whether a rule of that name exists in another region.
    let identifiers: Vec<(&str, String)> = vec![
        ("a bare name nothing was created under", "no-such-rule".to_string()),
        ("a bare name of one character", "r".to_string()),
        (
            "a bare name using every character class RuleName admits",
            "a.b#c/d-e_f0".to_string(),
        ),
        (
            "a bare name of a rule that lives in another region",
            "resident".to_string(),
        ),
        ("this region's ARN for a missing rule", rule_arn(HOME_REGION, "no-such-rule")),
        ("another region's ARN", rule_arn(OTHER_REGION, "resident")),
    ];

    for (uri, label) in [
        ("/DeleteTelemetryRule", "delete"),
        ("/UpdateTelemetryRule", "update"),
    ] {
        for (row, identifier) in &identifiers {
            let case = format!("{label}: {row}");
            // The ARN the refusal reports is the one the identifier names: an ARN is
            // already one, a bare name is that name in the caller's own region.
            let expected_arn = if identifier.starts_with("arn:") {
                identifier.clone()
            } else {
                rule_arn(HOME_REGION, identifier)
            };
            let expected_message = format!("Resource {expected_arn} does not exist.");
            let body = if uri == "/UpdateTelemetryRule" {
                json!({ "RuleIdentifier": identifier, "Rule": rule_body() })
            } else {
                json!({ "RuleIdentifier": identifier })
            };
            let (status, response) = call(&emulator, HOME_REGION, uri, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {response:?}"
            );
            let message = member(&response, "Message");
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected Message {expected_message:?}, got {message:?}"
            );
            assert!(
                message.contains(ACCOUNT) && message.contains("telemetry-rule/"),
                "case {case:?}: expected the resolved rule ARN in the sentence, got {message:?}"
            );
            assert_eq!(
                member(&response, "ResourceType"),
                "Rule",
                "case {case:?}: expected ResourceType \"Rule\", got {:?}",
                member(&response, "ResourceType")
            );

            // The read path of the SAME identifier keeps its own sentence: the two
            // refusals are distinguishable.
            let (status, read) = call(
                &emulator,
                HOME_REGION,
                "/GetTelemetryRule",
                &json!({ "RuleIdentifier": identifier }),
            )
            .await;
            assert_eq!(
                (status, member(&read, "Message").as_str()),
                (404, "Rule not found."),
                "case {case:?}: expected the read refusal to keep its own sentence, got {status} \
                 {read:?}"
            );
        }
    }
}

/// A create refused because the name is in use names the input member that carried
/// the name, and keeps the submitted value out of the sentence.
#[tokio::test]
async fn a_duplicate_name_create_names_the_member_that_collided() {
    // The rows differ along what the rule generalises over: which resource is
    // created, and the length and character classes of the name that collides.
    let rows: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "a two-character rule name",
            "/CreateTelemetryRule",
            "x9",
            "Telemetry Rule already exists.",
            "RuleName",
        ),
        (
            "a rule name using every character class RuleName admits",
            "/CreateTelemetryRule",
            "a.b#c/d-e_f0",
            "Telemetry Rule already exists.",
            "RuleName",
        ),
        (
            "a rule name of the modelled maximum length",
            "/CreateTelemetryRule",
            "rule-00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "Telemetry Rule already exists.",
            "RuleName",
        ),
        (
            "the shortest pipeline name the model admits",
            "/CreateTelemetryPipeline",
            "abc",
            "The specified PipelineName already exists",
            "PipelineName",
        ),
        (
            "a pipeline name with digits and hyphens",
            "/CreateTelemetryPipeline",
            "pipe-07-logs",
            "The specified PipelineName already exists",
            "PipelineName",
        ),
        (
            "a pipeline name of the modelled maximum length",
            "/CreateTelemetryPipeline",
            "pipeline-0000000000000000000",
            "The specified PipelineName already exists",
            "PipelineName",
        ),
    ];

    for (case, uri, name, expected_message, expected_type) in rows {
        let emulator = emulator();
        let body = if uri == "/CreateTelemetryRule" {
            json!({ "RuleName": name, "Rule": rule_body() })
        } else {
            json!({ "Name": name, "Configuration": pipeline_configuration() })
        };

        let (status, created) = call(&emulator, HOME_REGION, uri, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first create to answer 200, got {status} {created:?}"
        );

        let (status, response) = call(&emulator, HOME_REGION, uri, &body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {response:?}"
        );
        let message = member(&response, "Message");
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected Message {expected_message:?}, got {message:?}"
        );
        assert!(
            !message.contains(name),
            "case {case:?}: expected the submitted name to stay out of the sentence, got \
             {message:?}"
        );
        assert_eq!(
            member(&response, "ResourceType"),
            expected_type,
            "case {case:?}: expected ResourceType {expected_type:?}, got {:?}",
            member(&response, "ResourceType")
        );
        assert_eq!(
            member(&response, "ResourceId"),
            name,
            "case {case:?}: expected the submitted name in ResourceId, got {:?}",
            member(&response, "ResourceId")
        );
    }
}

/// The account is served ONE S3 Table integration per regional endpoint: a second
/// create there is the modelled conflict, whatever it carries, and it stores nothing.
#[tokio::test]
async fn the_account_is_served_one_s3_table_integration_per_endpoint() {
    // The rows differ along what the second create carries, since the refusal is
    // about the account's state and not about the request: the same configuration
    // again, the other `SSEAlgorithm` value, a KMS key, and tags.
    let seconds: Vec<(&str, Value)> = vec![
        (
            "the same configuration again",
            json!({ "Encryption": { "SseAlgorithm": "AES256" }, "RoleArn": ROLE_ARN }),
        ),
        (
            "the KMS algorithm instead",
            json!({ "Encryption": { "SseAlgorithm": "aws:kms" }, "RoleArn": ROLE_ARN }),
        ),
        (
            "a customer managed KMS key",
            json!({
                "Encryption": {
                    "SseAlgorithm": "aws:kms",
                    "KmsKeyArn":
                        "arn:aws:kms:us-west-2:000000000000:key/1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d",
                },
                "RoleArn": ROLE_ARN,
            }),
        ),
        (
            "tags beside the configuration",
            json!({
                "Encryption": { "SseAlgorithm": "AES256" },
                "RoleArn": ROLE_ARN,
                "Tags": { "team": "observability" },
            }),
        ),
    ];

    let first_request = json!({ "Encryption": { "SseAlgorithm": "AES256" }, "RoleArn": ROLE_ARN });

    for (case, second) in seconds {
        let emulator = emulator();
        let (status, created) = call(
            &emulator,
            HOME_REGION,
            "/CreateS3TableIntegration",
            &first_request,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first create to answer 200, got {status} {created:?}"
        );
        let held = member(&created, "Arn");

        let (status, error_type, response) = call_with_headers(
            &emulator,
            HOME_REGION,
            "/CreateS3TableIntegration",
            &second,
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {response:?}"
        );
        assert_eq!(
            error_type
                .as_deref()
                .unwrap_or("<no x-amzn-errortype>")
                .rsplit('#')
                .next()
                .unwrap_or_default(),
            "ConflictException",
            "case {case:?}: expected the x-amzn-errortype header to name the error, got \
             {error_type:?}"
        );
        let message = member(&response, "Message");
        assert_eq!(
            message, "Only one S3 Table integration is allowed per account",
            "case {case:?}: expected the singleton sentence, got {message:?}"
        );
        assert_eq!(
            member(&response, "ResourceType"),
            "Integration",
            "case {case:?}: expected ResourceType \"Integration\", got {:?}",
            member(&response, "ResourceType")
        );

        // The refusal allocated nothing and left the one integration in service.
        assert_eq!(
            stored_integrations(&emulator),
            1,
            "case {case:?}: expected the refused create to store nothing, got {} records",
            stored_integrations(&emulator)
        );
        let (status, read) = call(
            &emulator,
            HOME_REGION,
            "/GetS3TableIntegration",
            &json!({ "Arn": held }),
        )
        .await;
        assert_eq!(
            (status, member(&read, "Arn").as_str()),
            (200, held.as_str()),
            "case {case:?}: expected the held integration to still read back, got {status} \
             {read:?}"
        );

        // Another regional endpoint has its own slot, and a delete frees this one.
        let (status, elsewhere) = call(
            &emulator,
            OTHER_REGION,
            "/CreateS3TableIntegration",
            &first_request,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected another endpoint's create to answer 200, got {status} \
             {elsewhere:?}"
        );

        let (status, deleted) = call(
            &emulator,
            HOME_REGION,
            "/DeleteS3TableIntegration",
            &json!({ "Arn": held }),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the delete to answer 204, got {status} {deleted:?}"
        );
        let (status, again) = call(
            &emulator,
            HOME_REGION,
            "/CreateS3TableIntegration",
            &first_request,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the freed slot to serve a create, got {status} {again:?}"
        );
        assert_ne!(
            member(&again, "Arn"),
            held,
            "case {case:?}: expected a fresh integration ARN, got the retired {held:?}"
        );
    }
}

/// Both forms of a multi-region scope at once is one fixed sentence, wherever this
/// service refuses that pair.
#[tokio::test]
async fn both_forms_of_a_multi_region_scope_are_one_sentence() {
    let expected = "Parameters 'regions' and 'allRegions' are mutually exclusive. Provide one or \
                    the other, not both.";

    // The rows differ along what the rule generalises over: which operation carries
    // the pair (the rule's own scope and the telemetry-config feature's, in both its
    // account and its organization form) and how many regions the list names.
    let scopes: Vec<(&str, Value)> = vec![
        ("one region beside AllRegions", json!(["eu-west-1"])),
        (
            "several regions beside AllRegions",
            json!(["eu-west-1", "sa-east-1", "ap-south-1"]),
        ),
        ("the caller's own region beside AllRegions", json!([HOME_REGION])),
    ];

    for (row, regions) in scopes {
        for uri in [
            "/CreateTelemetryRule",
            "/UpdateTelemetryRule",
            "/StartTelemetryEvaluation",
            "/StartTelemetryEvaluationForOrganization",
        ] {
            let case = format!("{uri}: {row}");
            let emulator = emulator();
            // The rule operations need a rule to carry the scope; the update needs one
            // that exists, so the refusal is the scope's and not a missing resource's.
            let (status, created) = call(
                &emulator,
                HOME_REGION,
                "/CreateTelemetryRule",
                &json!({ "RuleName": "scoped", "Rule": rule_body() }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the setup create to answer 200, got {status} {created:?}"
            );

            let body = match uri {
                "/CreateTelemetryRule" => json!({
                    "RuleName": "both-forms",
                    "Rule": { "TelemetryType": "Logs", "Regions": regions, "AllRegions": true },
                }),
                "/UpdateTelemetryRule" => json!({
                    "RuleIdentifier": "scoped",
                    "Rule": { "TelemetryType": "Logs", "Regions": regions, "AllRegions": true },
                }),
                _ => json!({ "Regions": regions, "AllRegions": true }),
            };

            let (status, response) = call(&emulator, HOME_REGION, uri, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
            let message = member(&response, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected Message {expected:?}, got {message:?}"
            );
        }
    }
}
