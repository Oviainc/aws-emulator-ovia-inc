// SPDX-License-Identifier: CC0-1.0

//! What a Config refusal says about the REQUEST that drew it, end to end over the wire.
//!
//! An error code is shared by many legs, so the message is the whole of what a client can
//! branch on — and whether it carries anything of the request is itself the rule. Four
//! rules are pinned here, each over a TABLE of inputs rather than one recorded call:
//!
//! 1. a command this service cannot read as a Config query is a SYNTAX error, reported at
//!    the position of the submitted expression where the reading stopped — the first token
//!    that could not be read as part of a Config query — and nothing else: no prose of
//!    whatever read the text;
//! 2. `GetAggregateResourceConfig` refuses an identifier the aggregator holds no current
//!    configuration item for with ONE fixed sentence, which quotes none of the four
//!    members that identifier carries, whatever they are;
//! 3. the service-linked recorder surface words a refusal for the DIMENSION the request
//!    selected the recorder by: the service-principal legs name the service principal, and
//!    never the recorder name the caller never sent;//! 4. `PutResourceConfig` refuses a `ResourceType` the CloudFormation registry holds no
//!    registration for as the input-validation failure of the value it was sent —
//!    `ValidationException`, client 400 — and records nothing.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — reusing the
//! `Emulator` harness `config_setup_and_rule_loop` defines and the `refusal` helper
//! `config_connectors` shares.

use serde_json::{json, Value};

use crate::config_connectors::refusal;
use crate::config_setup_and_rule_loop::Emulator;
use crate::config_write_path_validation::{recording_emulator, resource_config};

/// The one sentence `GetAggregateResourceConfig` refuses a never-discovered resource
/// with, written out here rather than read from the service's own constant so this suite
/// PINS it instead of restating it.
const NOT_DISCOVERED_TEXT: &str =
    "You have specified a resource that is either unknown or has not been discovered.";

/// The service-principal variant of the sentence Config refuses a configuration recorder
/// it cannot resolve with, pinned here for the same reason.
const UNRECOGNIZED_PRINCIPAL_TEXT: &str =
    "The service principal of the configuration recorder you entered cannot be found. \
     Verify the configuration recorder service principal, and try again.";

/// A service Config DOES link a configuration recorder for, from the catalogue
/// `smithy_query(query="doc", shape="PutServiceLinkedConfigurationRecorder")` points at.
const LINKED_PRINCIPAL: &str = "observabilityadmin.amazonaws.com";

/// `ResourceId`'s length maximum, from `smithy_query(query="shape", shape="ResourceId")`:
/// 1..768.
const RESOURCE_ID_MAX_LENGTH: usize = 768;

/// Entries of `com.amazonaws.configservice#ResourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`.
const EC2_INSTANCE: &str = "AWS::EC2::Instance";
const EC2_VOLUME: &str = "AWS::EC2::Volume";
const S3_BUCKET: &str = "AWS::S3::Bucket";

/// The emulator's own account id, the account its aggregator aggregates.
fn account() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// One `AggregateResourceIdentifier`, with every member the model requires.
fn identifier(account_id: &str, region: &str, resource_type: &str, resource_id: &str) -> Value {
    json!({
        "SourceAccountId": account_id,
        "SourceRegion": region,
        "ResourceType": resource_type,
        "ResourceId": resource_id,
    })
}

/// An emulator holding an aggregator named `name` over this account in every Region.
async fn emulator_with_an_aggregator(name: &str) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationAggregator",
            &json!({
                "ConfigurationAggregatorName": name,
                "AccountAggregationSources": [{
                    "AccountIds": [account()],
                    "AllAwsRegions": true,
                }],
            }),
        )
        .await;
    emulator
}

/// The message `SelectResourceConfig` refuses `expression` with, requiring the modelled
/// `InvalidExpressionException` and its client status.
async fn refused_expression(case: &str, emulator: &Emulator, expression: &str) -> String {
    let (status, code, message) = refusal(
        emulator,
        "SelectResourceConfig",
        &json!({ "Expression": expression }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidExpressionException"),
        "case {case:?}: expected HTTP 400 InvalidExpressionException, got {status} {code} with \
         message {message:?}"
    );
    message
}

#[tokio::test]
async fn an_unreadable_query_is_a_syntax_error_at_the_token_the_reading_stopped_at() {
    let emulator = Emulator::new();

    // Rows differ along what makes the command unreadable — a clause cut short, a clause
    // the Config query form has no place for, a keyword standing where a property
    // belongs, a selected item that is no property, a command that is not a SELECT at all
    // — and along WHERE in the text that happens: the offending token is named per row,
    // and the column expected is the one that token starts at in the expression the row
    // sends, so no row carries a position written by hand.
    let cases: [(&str, &str, &str); 8] = [
        (
            "a WHERE clause with nothing to compare",
            "SELECT resourceId WHERE (",
            "WHERE",
        ),
        (
            "a WHERE clause cut off after the keyword",
            "SELECT resourceId WHERE",
            "WHERE",
        ),
        (
            "a FROM clause, which a Config query has no place for",
            "SELECT resourceId FROM aws",
            "FROM",
        ),
        (
            "a HAVING clause, which a Config query has no place for",
            "SELECT resourceType GROUP BY resourceType HAVING COUNT(*) > 1",
            "HAVING",
        ),
        (
            "a DISTINCT a Config query has no place for",
            "SELECT DISTINCT resourceId",
            "DISTINCT",
        ),
        (
            "a keyword standing where the first property belongs",
            "SELECT WHERE",
            "WHERE",
        ),
        (
            "an alias on a selected property",
            "SELECT resourceId AS id",
            "AS",
        ),
        (
            "a command that is not a SELECT at all",
            "DELETE resourceId",
            "DELETE",
        ),
    ];

    for (case, expression, token) in cases {
        let column = expression
            .find(token)
            .unwrap_or_else(|| panic!("case {case:?}: {token:?} is part of {expression:?}"))
            + 1;
        let expected = format!("syntax error at line 1, column {column}");
        let message = refused_expression(case, &emulator, expression).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected the reading of {expression:?} to stop at {token:?}, got \
             {message:?}"
        );
        // Nothing of the expression, and nothing of whatever read it, travels in the
        // sentence: the position is the whole refusal.
        assert!(
            !message.contains(token),
            "case {case:?}: the refusal quotes no text of the expression, got {message:?}"
        );
    }

    // The line is counted too, so the same fault reads differently when the command is
    // written over several lines. Rows differ along which line the reading stops on and
    // the column within it.
    for (case, expression, line, column) in [
        (
            "a clause on the second line",
            "SELECT resourceId\nWHERE (",
            2,
            1,
        ),
        (
            "a clause indented on the second line",
            "SELECT resourceId\n  FROM aws",
            2,
            3,
        ),
        (
            "a clause on the third line",
            "SELECT resourceId\n\nORDER",
            3,
            1,
        ),
    ] {
        let expected = format!("syntax error at line {line}, column {column}");
        let message = refused_expression(case, &emulator, expression).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A command that runs out of text stopped where the text ran out: one past its last
    // character. Rows differ along the length of what was written.
    for (case, expression) in [
        ("the keyword alone", "SELECT"),
        ("the keyword and a space", "SELECT "),
        ("a second line opened and left empty", "SELECT\n"),
    ] {
        let message = refused_expression(case, &emulator, expression).await;
        let lines = expression.split('\n').count();
        let column = expression
            .rsplit('\n')
            .next()
            .map_or(1, |last| last.chars().count() + 1);
        let expected = format!("syntax error at line {lines}, column {column}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The control: a command that IS a Config query is read and served, so the rows above
    // are refused for what they carry and not because the reading refuses everything.
    for (case, expression) in [
        ("one property", "SELECT resourceId"),
        (
            "every clause of the documented form",
            "SELECT resourceType, COUNT(*) WHERE resourceType = 'AWS::EC2::Volume' GROUP BY \
             resourceType ORDER BY resourceType LIMIT 10",
        ),
        ("a command written over two lines", "SELECT resourceId\nWHERE resourceId = 'vol-1'"),
    ] {
        let (status, parsed) = emulator
            .call("SelectResourceConfig", &json!({ "Expression": expression }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a Config query is served, got {status} with body {parsed}"
        );
    }
}

#[tokio::test]
async fn the_same_syntax_error_position_is_reported_wherever_the_language_is_read() {
    // `SelectAggregateResourceConfig` runs the same language over the aggregated
    // inventory, and `PutStoredQuery` saves a query written in it, so one unreadable
    // command is refused the same way by all three.
    let aggregator = "cross-read";
    let emulator = emulator_with_an_aggregator(aggregator).await;
    let expression = "SELECT resourceId WHERE (";
    let expected = "syntax error at line 1, column 19";

    let selected = refused_expression("the select path", &emulator, expression).await;
    assert_eq!(
        selected, expected,
        "the select path reports where the reading stopped, got {selected:?}"
    );

    let (status, code, message) = refusal(
        &emulator,
        "SelectAggregateResourceConfig",
        &json!({ "ConfigurationAggregatorName": aggregator, "Expression": expression }),
    )
    .await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (400, "InvalidExpressionException", expected),
        "the aggregate select path reports the same position, got {status} {code} {message:?}"
    );

    let (status, code, message) = refusal(
        &emulator,
        "PutStoredQuery",
        &json!({ "StoredQuery": { "QueryName": "unreadable", "Expression": expression } }),
    )
    .await;
    assert_eq!(
        (status, message.as_str()),
        (400, expected),
        "the stored-query path reports the same position, got {status} {code} {message:?}"
    );
}

#[tokio::test]
async fn a_never_discovered_aggregate_resource_is_refused_without_quoting_the_identifier() {
    let aggregator = "org-wide";
    let emulator = emulator_with_an_aggregator(aggregator).await;
    let long_id = format!("i-{}", "a".repeat(RESOURCE_ID_MAX_LENGTH - 2));

    // Rows differ along every member of the identifier: the resource type, the id (an
    // ordinary one, a single character at the `ResourceId` length minimum, and one at its
    // 768-character maximum), the source account, and the source Region. The sentence is
    // the same for all of them, and quotes none of them.
    let cases: [(&str, Value); 6] = [
        (
            "an instance that was never recorded",
            identifier(&account(), "us-west-2", EC2_INSTANCE, "i-0123456789abcdef0"),
        ),
        (
            "another resource type",
            identifier(&account(), "us-west-2", EC2_VOLUME, "vol-0123456789abcdef0"),
        ),
        (
            "an id of one character, the length minimum",
            identifier(&account(), "us-west-2", S3_BUCKET, "b"),
        ),
        (
            "an id at the length maximum",
            identifier(&account(), "us-west-2", EC2_INSTANCE, &long_id),
        ),
        (
            "another source account",
            identifier("222222222222", "us-west-2", EC2_INSTANCE, "i-0abc"),
        ),
        (
            "another source Region",
            identifier(&account(), "eu-west-1", EC2_INSTANCE, "i-0abc"),
        ),
    ];

    for (case, sent) in cases {
        let (status, code, message) = refusal(
            &emulator,
            "GetAggregateResourceConfig",
            &json!({
                "ConfigurationAggregatorName": aggregator,
                "ResourceIdentifier": sent,
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ResourceNotDiscoveredException", NOT_DISCOVERED_TEXT),
            "case {case:?}: expected the fixed not-discovered refusal, got {status} {code} \
             {message:?}"
        );
        for member in ["ResourceId", "ResourceType", "SourceAccountId", "SourceRegion"] {
            let value = sent[member].as_str().expect("the row sends every member");
            // A value of a few characters can be a substring of ordinary words, so the
            // echo is checked on the values long enough for the check to mean anything;
            // the sentence asserted above is the whole of the message for every row,
            // which is what says the identifier is not in it.
            assert!(
                value.len() < 4 || !message.contains(value),
                "case {case:?}: the refusal quotes nothing of {member} ({value:?}), got \
                 {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_service_linked_recorder_refusal_names_the_dimension_the_request_used() {
    // A delete that resolves the recorder by SERVICE PRINCIPAL states the account and the
    // principal it looked for, naming that member — never the recorder name, which such a
    // request cannot carry. Rows differ along the principal and along whether a recorder
    // for it was created and deleted first, or never existed at all.
    for (case, principal, create_first) in [
        ("a linked service whose recorder was deleted", LINKED_PRINCIPAL, true),
        ("a linked service with no recorder at all", LINKED_PRINCIPAL, false),
        (
            "another linked service with no recorder",
            "telemetry-enablement.observabilityadmin.amazonaws.com",
            false,
        ),
    ] {
        let emulator = Emulator::new();
        if create_first {
            emulator
                .ok(
                    "PutServiceLinkedConfigurationRecorder",
                    &json!({ "ServicePrincipal": principal }),
                )
                .await;
            emulator
                .ok(
                    "DeleteServiceLinkedConfigurationRecorder",
                    &json!({ "ServicePrincipal": principal }),
                )
                .await;
        }

        let (status, code, message) = refusal(
            &emulator,
            "DeleteServiceLinkedConfigurationRecorder",
            &json!({ "ServicePrincipal": principal }),
        )
        .await;
        let expected = format!(
            "Configuration recorder (AccountId: {}, Service Principal: {principal}) cannot be \
             found",
            account()
        );
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                400,
                "NoSuchConfigurationRecorderException",
                expected.as_str()
            ),
            "case {case:?}: expected the refusal to name the service principal, got {status} \
             {code} {message:?}"
        );
        assert!(
            !message.contains("name of the configuration recorder"),
            "case {case:?}: a request that sent no recorder name is not told to verify one, got \
             {message:?}"
        );
    }

    // A ServicePrincipal that names a service Config links no recorder for is refused
    // with the service-principal variant of the recorder-not-found sentence, which quotes
    // nothing of the value. Rows differ along how the value misses: a well-formed
    // principal of no such integration, a principal of an Amazon Web Services service
    // Config links no recorder for, and a value that is no service principal at all.
    for (case, principal) in [
        ("a well-formed principal of no integration", "not-a-real-integration.amazonaws.com"),
        ("a service Config links no recorder for", "ec2.amazonaws.com"),
        ("a value that is no service principal", "not-a-principal"),
    ] {
        let emulator = Emulator::new();
        let (status, code, message) = refusal(
            &emulator,
            "PutServiceLinkedConfigurationRecorder",
            &json!({ "ServicePrincipal": principal }),
        )
        .await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ValidationException", UNRECOGNIZED_PRINCIPAL_TEXT),
            "case {case:?}: expected the service-principal sentence, got {status} {code} \
             {message:?}"
        );
        assert!(
            !message.contains(principal),
            "case {case:?}: the refusal quotes nothing of the value, got {message:?}"
        );
        // Nothing was created for it: the listing reports no recorder at all.
        let listed = emulator.ok("ListConfigurationRecorders", &json!({})).await;
        assert_eq!(
            listed["ConfigurationRecorderSummaries"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused create leaves no recorder behind, got {listed}"
        );
    }

    // The control: the linked service IS created and deleted, so the refusals above are
    // about the recorder the request named and not about the operation.
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutServiceLinkedConfigurationRecorder",
            &json!({ "ServicePrincipal": LINKED_PRINCIPAL }),
        )
        .await;
    emulator
        .ok(
            "DeleteServiceLinkedConfigurationRecorder",
            &json!({ "ServicePrincipal": LINKED_PRINCIPAL }),
        )
        .await;
}

#[tokio::test]
async fn an_unregistered_resource_type_is_refused_by_the_registry_lookups_own_class() {
    // Rows differ along WHY the CloudFormation registry holds no registration for the
    // type: organizations nothing registered a type under, a name that is no registry type
    // name at all, a module name, and a registered type misspelled or spelled in another
    // case. The class is the input-validation one, because what failed is the value the
    // request carried at that member.
    let cases: [(&str, &str); 6] = [
        ("an organization nothing registered", "Sim2Real::PrecisionTest::Resource"),
        ("another unregistered organization", "Acme::Network::Switch"),
        ("a type name of one segment", "Switch"),
        ("a registry module name", "Acme::Network::Switch::MODULE"),
        ("a misspelled registered type", "AWS::EC2::Volumes"),
        ("a registered type in another case", "aws::ec2::volume"),
    ];

    for (case, resource_type) in cases {
        let emulator = recording_emulator().await;
        let (status, code, message) = refusal(
            &emulator,
            "PutResourceConfig",
            &resource_config(resource_type, "r-1", r#"{"state":"new"}"#),
        )
        .await;
        let expected = format!(
            "Cannot find registered Resource Type with name {resource_type}. Please register \
             the Resource Type with Cloudformation and try again."
        );
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (
                400,
                "ValidationException",
                expected.as_str()
            ),
            "case {case:?}: expected the registration precondition for {resource_type:?}, got \
             {status} with {code} and {message:?}"
        );

        // Nothing is recorded: a refused write leaves no configuration item behind, so
        // the inventory counts nothing of the type it named.
        let counted = emulator.ok("GetDiscoveredResourceCounts", &json!({})).await;
        assert_eq!(
            counted["resourceCounts"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: the refused write recorded nothing, got {counted}"
        );
    }

    // The control: a type the registry DOES publish is recorded and read back, so the
    // rows above are refused for their type and not by the write path as such.
    let emulator = recording_emulator().await;
    emulator
        .ok(
            "PutResourceConfig",
            &resource_config(EC2_VOLUME, "vol-0001", r#"{"state":"in-use"}"#),
        )
        .await;
    let history = emulator
        .ok(
            "GetResourceConfigHistory",
            &json!({ "resourceType": EC2_VOLUME, "resourceId": "vol-0001" }),
        )
        .await;
    assert_eq!(
        history["configurationItems"].as_array().map(Vec::len),
        Some(1),
        "the registered type is recorded and read back, got {history}"
    );
}
