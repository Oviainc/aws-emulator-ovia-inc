// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEMPLATES this service's refusals are worded from,
//! driven over its own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes in `$.message`.
//!
//! A refusal's status and code say WHICH class a request fell in; the message is what
//! tells a caller what to change, so each rule below is about which parts of a message
//! are the service's own fixed text and which are taken from the request:
//!
//! 1. the refusal of a logging destination whose log-group ARN is not the qualified
//!    form the member documents states the QUALIFIER RULE in one fixed sentence,
//!    carrying no value of the request at all;
//! 2. the refusal of a state machine `definition` the service could not parse quotes
//!    the parse diagnostic with the DEFINITION it was reading in each of its clauses,
//!    the mark it left unmatched, and the position the parse ended at — one past the
//!    document's last character;
//! 3. the refusal of an execution `input` it could not parse quotes the same reading
//!    and WITHHOLDS the document, whose member the model marks `@sensitive`;
//! 4. an alias MUTATION that resolves nothing words the refusal for which resource the
//!    ARN resolved nothing for — the state machine it qualifies, or the alias name
//!    inside it.
//!
//! The harness (`Emulator`, `create_machine`, `error_code`, `string`,
//! `PAST_THE_DELETE_DELAY`) is the one `state_machine_lifecycle` builds, `failure` is
//! `error_messages`' own, the published versions come from `state_machine_versions`'
//! `machine_with_versions` and the routing entries from `state_machine_aliases`'
//! `route`; `tests/all.rs` is this binary's crate root, so each is reached through
//! `crate::`.

use serde_json::{json, Value};

use crate::error_messages::failure;
use crate::state_machine_aliases::route;
use crate::state_machine_lifecycle::{
    create_machine, error_code, string, Emulator, ACCOUNT, HELLO_DEFINITION,
    PAST_THE_DELETE_DELAY, REGION, ROLE_ARN,
};
use crate::state_machine_versions::machine_with_versions;

// ---------------------------------------------------------------------------
// Rule 1: the log-group qualifier sentence
// ---------------------------------------------------------------------------

/// What the service answers for a `cloudWatchLogsLogGroup.logGroupArn` that is not the
/// qualified log-group ARN the member documents ("The ARN must end with `:*`").
///
/// RECORDED text, and ENTIRELY FIXED: it states the rule the ARN broke and nothing
/// about the request, so it is spelled out here rather than read off the service.
const QUALIFIER_SENTENCE: &str =
    "Invalid Logging Configuration: Log Group ARN must be provided with '*' qualifier.";

/// A logging configuration that logs everything to `log_group_arn`.
fn logging_to(log_group_arn: &str) -> Value {
    json!({
        "level": "ALL",
        "includeExecutionData": true,
        "destinations": [{ "cloudWatchLogsLogGroup": { "logGroupArn": log_group_arn } }],
    })
}

/// The ARN of a CloudWatch Logs log group, unqualified.
fn log_group(path: &str) -> String {
    format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:{path}")
}

/// A log-group ARN that is not the qualified form is refused in ONE fixed sentence
/// that names neither the ARN, the log group nor the state machine — on every
/// operation that takes a `loggingConfiguration`.
///
/// The rows differ along WHY the ARN is not a qualified log group ARN — no qualifier
/// at all, a qualifier that is not the one the member documents, a log group path of
/// another shape, another service's ARN, an ARN with no resource at all and text that
/// is not an ARN — because one sentence answers for the whole class.
#[tokio::test]
async fn an_unqualified_log_group_arn_is_refused_in_one_fixed_sentence() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "logging-qualifier", HELLO_DEFINITION).await;

    // (case, the log group ARN the destination names)
    let rows: Vec<(&str, String)> = vec![
        // The recorded row: the qualified ARN with its trailing ":*" removed.
        (
            "a log group ARN with no qualifier",
            log_group("/aws/vendedlogs/states/orders"),
        ),
        (
            "a log group ARN of another path with no qualifier",
            log_group("/aws/vendedlogs/states/orders.v2/retry"),
        ),
        (
            "a log group ARN qualified with something else",
            format!("{}:log-stream", log_group("/aws/vendedlogs/states/orders")),
        ),
        (
            "a log group ARN qualified with a star that is not the last field",
            format!("{}:*:1", log_group("/aws/vendedlogs/states/orders")),
        ),
        (
            "another service's ARN, qualified",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:orders:*"),
        ),
        (
            "a Logs ARN that names no log group",
            format!("arn:aws:logs:{REGION}:{ACCOUNT}:destination:orders:*"),
        ),
        ("text that is not an ARN", "orders".to_string()),
    ];

    for (index, (case, log_group_arn)) in rows.iter().enumerate() {
        for (operation, body) in [
            (
                "CreateStateMachine",
                json!({
                    "name": format!("logging-qualifier-{index}"),
                    "definition": HELLO_DEFINITION,
                    "roleArn": ROLE_ARN,
                    "loggingConfiguration": logging_to(log_group_arn),
                }),
            ),
            (
                "UpdateStateMachine",
                json!({
                    "stateMachineArn": &machine,
                    "loggingConfiguration": logging_to(log_group_arn),
                }),
            ),
        ] {
            let case = format!("{operation} with {case}");
            let (status, code, message) = failure(&emulator, &case, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidLoggingConfiguration"),
                "case {case:?}: expected 400 InvalidLoggingConfiguration, got {status} {code} \
                 with message {message:?}"
            );
            assert_eq!(
                message, QUALIFIER_SENTENCE,
                "case {case:?}: expected the fixed sentence {QUALIFIER_SENTENCE:?}, got \
                 {message:?}"
            );
            // The sentence has no variable part, so nothing the request chose is in it.
            for value in [log_group_arn.as_str(), "orders", "logging-qualifier"] {
                assert!(
                    !message.contains(value),
                    "case {case:?}: expected no value of the request in the sentence, got \
                     {message:?} (carrying {value:?})"
                );
            }
        }
    }

    // The control: the SAME ARNs with the documented qualifier are accepted, so the
    // sentence is about the qualifier and not about refusing logging configurations.
    for (index, path) in [
        "/aws/vendedlogs/states/orders",
        "/aws/vendedlogs/states/orders.v2/retry",
    ]
    .into_iter()
    .enumerate()
    {
        let qualified = format!("{}:*", log_group(path));
        let case = format!("a qualified log group ARN {qualified:?}");
        let created = emulator
            .ok(
                "CreateStateMachine",
                &json!({
                    "name": format!("logging-qualified-{index}"),
                    "definition": HELLO_DEFINITION,
                    "roleArn": ROLE_ARN,
                    "loggingConfiguration": logging_to(&qualified),
                }),
            )
            .await;
        let arn = string(&created, "stateMachineArn");
        // The configuration is readable back: the create kept what it accepted.
        let described = emulator
            .ok("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
            .await;
        let reported = described["loggingConfiguration"]["destinations"][0]
            ["cloudWatchLogsLogGroup"]["logGroupArn"]
            .as_str();
        assert_eq!(
            reported,
            Some(qualified.as_str()),
            "case {case:?}: expected the accepted destination to be reported, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the definition's parse locator
// ---------------------------------------------------------------------------

/// One past the last character of `document`, which is where a parse that ran out of
/// input stopped: there is no character there, so the position named is where the next
/// one would have gone.
fn end_of(document: &str) -> (usize, usize) {
    let lines = document.split('\n').count();
    let last = document.rsplit('\n').next().unwrap_or(document);
    (lines, last.len() + 1)
}

/// The clause a parse quotes the definition it was reading in, at one position.
fn source_clause(definition: &str, (line, column): (usize, usize)) -> String {
    format!("[Source: (String)\"{definition}\"; line: {line}, column: {column}]")
}

/// The whole `InvalidDefinition` message for a `definition` that is not JSON: the
/// diagnostic's code, the reading of where the parse stopped, the START MARKER of the
/// container it was still inside (when there is one), the position it ended at, and
/// that same position as the place the diagnostic was found.
fn definition_refusal(
    definition: &str,
    reading: &str,
    start_marker: Option<(usize, usize)>,
    stopped: (usize, usize),
) -> String {
    let marked = match start_marker {
        Some(position) => format!(
            "{reading} (start marker at {})",
            source_clause(definition, position)
        ),
        None => reading.to_string(),
    };
    let clause = source_clause(definition, stopped);
    format!(
        "Invalid State Machine Definition: 'INVALID_JSON_DESCRIPTION: {marked}\n at {clause} at \
         {clause}'"
    )
}

/// One row of the table below: what a failing assertion calls it, the definition, the
/// reading of where the parse stopped, the position the container it was inside opens
/// at (`None` when the fault left none open), and the position the parse ended at.
type DefinitionRow = (
    &'static str,
    &'static str,
    &'static str,
    Option<(usize, usize)>,
    (usize, usize),
);

/// A `definition` the service cannot parse is refused with the parse diagnostic, the
/// mark the document left unmatched and the position the parse ended at — on both
/// operations that take a definition, and as the diagnostic the validation reports.
///
/// The rows differ along which container was left open, where that container opens
/// (the document itself, a nested object, a later line), how LONG the definition is —
/// so the end position is read off the request rather than fixed — and one fault that
/// is not a container left open at all, which carries no start marker.
#[tokio::test]
async fn a_definition_that_is_not_json_names_where_it_stopped_and_what_it_was_reading() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "definition-locator", HELLO_DEFINITION).await;

    const OBJECT_LEFT_OPEN: &str = "Unexpected end-of-input: expected close marker for Object";
    const ARRAY_LEFT_OPEN: &str = "Unexpected end-of-input: expected close marker for Array";

    // The recorded row: a definition one closing brace short, whose unclosed object is
    // the document itself.
    let recorded = r#"{"StartAt": "Pass", "States": {"Pass": {"Type": "Pass", "End": true}}"#;
    // The same shape written longer, so the end position is a function of the request.
    let longer =
        r#"{"StartAt": "PassThrough", "States": {"PassThrough": {"Type": "Pass", "End": true}}"#;
    // An inner object left open: the innermost unmatched mark is the one reported.
    let inner = r#"{"StartAt": "Pass", "States": {"Pass": {"Type": "Pass""#;
    // An array left open inside the definition: its one element is closed, so the mark
    // left unmatched is the array's own.
    let array = r#"{"StartAt": "Pass", "States": [{"Type": "Pass"}"#;
    // A definition whose container opens on its second line.
    let multiline = "{\n  \"States\": [\n";

    let rows: Vec<DefinitionRow> = vec![
        (
            "a definition one closing brace short",
            recorded,
            OBJECT_LEFT_OPEN,
            Some((1, 1)),
            end_of(recorded),
        ),
        (
            "a longer definition one closing brace short",
            longer,
            OBJECT_LEFT_OPEN,
            Some((1, 1)),
            end_of(longer),
        ),
        (
            // `{"StartAt": "Pass", "States": {"Pass": ` is 39 characters, so the state
            // object's own mark is the 40th.
            "a definition whose innermost object is left open",
            inner,
            OBJECT_LEFT_OPEN,
            Some((1, 40)),
            end_of(inner),
        ),
        (
            // `{"StartAt": "Pass", "States": ` is 30 characters.
            "a definition whose array is left open",
            array,
            ARRAY_LEFT_OPEN,
            Some((1, 31)),
            end_of(array),
        ),
        (
            // Line 2 is `  "States": [`, whose bracket is its 13th byte.
            "a definition whose container opens on its second line",
            multiline,
            ARRAY_LEFT_OPEN,
            Some((2, 13)),
            end_of(multiline),
        ),
        (
            // Not a container left open: there is no mark to report, so the sentence
            // carries no start marker and the locator alone says where it stopped.
            "a definition with no colon after a field name",
            r#"{"StartAt" "Pass"}"#,
            "Unexpected character ('\"' (code 34)): was expecting a colon to separate field name \
             and value",
            None,
            (1, 12),
        ),
    ];

    for (index, (case, definition, reading, start_marker, stopped)) in rows.iter().enumerate() {
        let expected = definition_refusal(definition, reading, *start_marker, *stopped);

        for (operation, body) in [
            (
                "CreateStateMachine",
                json!({
                    "name": format!("definition-locator-{index}"),
                    "definition": definition,
                    "roleArn": ROLE_ARN,
                }),
            ),
            (
                "UpdateStateMachine",
                json!({ "stateMachineArn": &machine, "definition": definition }),
            ),
        ] {
            let case = format!("{operation} with {case}");
            let (status, code, message) = failure(&emulator, &case, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidDefinition"),
                "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with message \
                 {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }

        // The validation reports the same problem as a diagnostic of its own: the
        // reading as its message, and the position the parse ended at as its location.
        let case = format!("ValidateStateMachineDefinition with {case}");
        let reported = emulator
            .ok(
                "ValidateStateMachineDefinition",
                &json!({ "definition": definition }),
            )
            .await;
        assert_eq!(
            reported.get("result").and_then(Value::as_str),
            Some("FAIL"),
            "case {case:?}: expected a FAIL result, got {reported:?}"
        );
        let diagnostics = reported["diagnostics"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected diagnostics in {reported:?}"))
            .clone();
        let codes: Vec<&str> = diagnostics
            .iter()
            .filter_map(|found| found.get("code").and_then(Value::as_str))
            .collect();
        assert_eq!(
            codes,
            vec!["INVALID_JSON_DESCRIPTION"],
            "case {case:?}: expected the one JSON-syntax diagnostic, got {reported:?}"
        );
        let locations: Vec<&str> = diagnostics
            .iter()
            .filter_map(|found| found.get("location").and_then(Value::as_str))
            .collect();
        assert_eq!(
            locations,
            vec![source_clause(definition, *stopped).as_str()],
            "case {case:?}: expected the diagnostic located where the parse ended, got \
             {reported:?}"
        );
    }

    // The control: a definition that IS JSON is read, so the rule is about documents
    // the service cannot parse and not about refusing definitions.
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "definition-locator-control",
                "definition": r#"{"StartAt": "Pass", "States": {"Pass": {"Type": "Pass", "End": true}}}"#,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    assert!(
        string(&created, "stateMachineArn").contains(":stateMachine:"),
        "a definition that parses is created, got {created:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the input the refusal withholds
// ---------------------------------------------------------------------------

/// The source a location clause names where the document it read is WITHHELD.
///
/// RECORDED text: the service reports the source as redacted and names the parser
/// feature that would otherwise have put the document there.
const REDACTED_SOURCE: &str = "REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled)";

/// An `input` the service cannot parse is refused with the reading and the mark the
/// document left unmatched, and NO clause quotes the document itself.
///
/// The `input` member is `@sensitive` on every operation that takes one, so the
/// refusal a caller sees — and the log it lands in — carries the position and the
/// unmatched mark but never the payload. The rows differ along the fault class and
/// along what the payload carries: each one holds a sentinel that must not come back.
#[tokio::test]
async fn a_refused_input_withholds_the_document_it_could_not_read() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "input-redaction", HELLO_DEFINITION).await;
    let pass_state = r#"{"Type":"Pass","End":true}"#;

    // (case, the input, the whole diagnostic the refusal quotes)
    let rows: Vec<(&str, String, String)> = vec![
        (
            // The recorded row: an object left open names the mark that opened it,
            // with the source withheld.
            "an object that ends early",
            r#"{"sentinel-payload": 1"#.to_string(),
            format!(
                "Unexpected end-of-input: expected close marker for Object (start marker at \
                 [Source: {REDACTED_SOURCE}; line: 1, column: 1])"
            ),
        ),
        (
            "an array that ends early",
            r#"["sentinel-payload""#.to_string(),
            format!(
                "Unexpected end-of-input: expected close marker for Array (start marker at \
                 [Source: {REDACTED_SOURCE}; line: 1, column: 1])"
            ),
        ),
        (
            // The recorded row: a document whose first token is not a value names the
            // whole word and carries no clause at all.
            "an unquoted word where a value belongs",
            "this is definitely not { json".to_string(),
            "Unrecognized token 'this': was expecting (JSON String, Number, Array, Object or \
             token 'null', 'true' or 'false')"
                .to_string(),
        ),
        (
            "a word a template left unsubstituted",
            r#"{"id": $sentinel-payload}"#.to_string(),
            "Unrecognized token '$sentinel': was expecting (JSON String, Number, Array, Object \
             or token 'null', 'true' or 'false')"
                .to_string(),
        ),
    ];

    for (case, document, diagnostic) in &rows {
        let (status, code, message) = failure(
            &emulator,
            case,
            "TestState",
            &json!({ "definition": pass_state, "roleArn": ROLE_ARN, "input": document }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExecutionInput"),
            "case {case:?}: expected 400 InvalidExecutionInput, got {status} {code} with message \
             {message:?}"
        );
        let expected = format!("Invalid State Machine Execution Input: '{diagnostic}'");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        // The operations that START an execution with an input read the same member,
        // and withhold the document for the same reason.
        let case = format!("StartExecution with {case}");
        let (_, code, started) = failure(
            &emulator,
            &case,
            "StartExecution",
            &json!({ "stateMachineArn": &machine, "input": document }),
        )
        .await;
        assert_eq!(
            code, "InvalidExecutionInput",
            "case {case:?}: expected InvalidExecutionInput, got {code} with {started:?}"
        );
        assert!(
            !started.contains("payload") && !started.contains("(String)"),
            "case {case:?}: expected the refusal to withhold the input, got {started:?}"
        );
    }

    // The control: an input that IS JSON is read and echoed as the state's output, so
    // the rule is about an input the service cannot parse.
    let tested = emulator
        .ok(
            "TestState",
            &json!({
                "definition": pass_state,
                "roleArn": ROLE_ARN,
                "input": r#"{"sentinel-payload": 1}"#,
            }),
        )
        .await;
    assert_eq!(
        tested.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "an input that parses is read, got {tested:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: which resource an alias mutation could not resolve
// ---------------------------------------------------------------------------

/// An alias MUTATION whose state machine was DELETED under it reports the alias ARN,
/// where one whose alias alone is gone states the condition as a sentence of its own.
///
/// An alias ARN resolves in two steps — the state machine it qualifies, then the alias
/// name inside it — so the refusal names the step that failed, which is what lets a
/// deploy tell "this alias is gone" from "the state machine behind it is gone". The
/// read path reports the ARN in both situations and is the control here.
///
/// The rows differ along which member the update names and along the alias name, and
/// the two situations are driven against the SAME alias shape, so the only thing that
/// differs between them is which resource went away.
#[tokio::test]
async fn an_alias_mutation_words_the_resource_its_arn_could_not_resolve() {
    let emulator = Emulator::new();
    let orphaned = machine_with_versions(&emulator, "alias-orphaned", 1).await;
    let surviving = machine_with_versions(&emulator, "alias-surviving", 1).await;

    // (case, the state machine, the alias name, the member the update names)
    let cases: Vec<(&str, &String, &str, Value)> = vec![
        (
            "repointed",
            &orphaned,
            "PROD",
            json!({ "routingConfiguration": [route(&format!("{orphaned}:1"), 100)] }),
        ),
        ("re-described", &orphaned, "BETA", json!({ "description": "new" })),
        (
            "repointed",
            &surviving,
            "PROD",
            json!({ "routingConfiguration": [route(&format!("{surviving}:1"), 100)] }),
        ),
        ("re-described", &surviving, "BETA", json!({ "description": "new" })),
    ];

    // Each alias is created and read back, so what the refusals below report is an
    // alias that really existed.
    let mut aliases: Vec<String> = Vec::new();
    for (_, machine, name, _) in &cases {
        let created = emulator
            .ok(
                "CreateStateMachineAlias",
                &json!({
                    "name": name,
                    "routingConfiguration": [route(&format!("{machine}:1"), 100)],
                }),
            )
            .await;
        let arn = string(&created, "stateMachineAliasArn");
        let described = emulator
            .ok("DescribeStateMachineAlias", &json!({ "stateMachineAliasArn": &arn }))
            .await;
        assert_eq!(
            string(&described, "stateMachineAliasArn"),
            arn,
            "case {name:?}: the created alias reads back, got {described:?}"
        );
        aliases.push(arn);
    }

    // One state machine is deleted with its aliases still pointing at it, and the
    // OTHER machine's aliases are deleted on their own: two ways for the same
    // mutation to resolve nothing.
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": &orphaned }))
        .await;
    for (index, (_, machine, _, _)) in cases.iter().enumerate() {
        if *machine == &surviving {
            emulator
                .ok(
                    "DeleteStateMachineAlias",
                    &json!({ "stateMachineAliasArn": &aliases[index] }),
                )
                .await;
        }
    }
    std::thread::sleep(PAST_THE_DELETE_DELAY);

    for (index, (case, machine, name, member)) in cases.iter().enumerate() {
        let alias = &aliases[index];
        let orphan = *machine == &orphaned;
        let case = format!(
            "an alias {name:?} {case} whose {} was deleted",
            if orphan { "state machine" } else { "own record" }
        );
        let expected = if orphan {
            format!("Resource not found: '{alias}'")
        } else {
            "Request references a resource that does not exist.".to_string()
        };

        // The control: the READ path reports the ARN in both situations.
        let (status, code, read) = failure(
            &emulator,
            &case,
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": alias }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound from the read, got {status} {code} \
             with message {read:?}"
        );
        assert_eq!(
            read,
            format!("Resource not found: '{alias}'"),
            "case {case:?}: expected the read to report the ARN, got {read:?}"
        );

        let mut body = json!({ "stateMachineAliasArn": alias });
        for (member_name, value) in member.as_object().expect("a member object") {
            body[member_name] = value.clone();
        }
        let (status, parsed) = emulator.call("UpdateStateMachineAlias", &body).await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} {code} with body \
             {parsed:?}"
        );
        assert_eq!(
            parsed.get("message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {parsed:?}"
        );
        // The refused alias travels in `resourceName` whichever wording the message
        // takes, so a caller reads it from there either way.
        assert_eq!(
            parsed.get("resourceName").and_then(Value::as_str),
            Some(alias.as_str()),
            "case {case:?}: expected the refused alias in resourceName, got {parsed:?}"
        );
    }

    // The control: an alias of a state machine that is still there, re-created, is
    // updated and read back — so the refusals above are about resources that went
    // away and not about refusing alias updates.
    let created = emulator
        .ok(
            "CreateStateMachineAlias",
            &json!({
                "name": "LIVE",
                "routingConfiguration": [route(&format!("{surviving}:1"), 100)],
            }),
        )
        .await;
    let alias = string(&created, "stateMachineAliasArn");
    emulator
        .ok(
            "UpdateStateMachineAlias",
            &json!({ "stateMachineAliasArn": &alias, "description": "live" }),
        )
        .await;
    let described = emulator
        .ok(
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": &alias }),
        )
        .await;
    assert_eq!(
        described.get("description").and_then(Value::as_str),
        Some("live"),
        "a live alias takes its update, got {described:?}"
    );
}
