// SPDX-License-Identifier: CC0-1.0

//! `EstimateTemplateCost`, end to end over the awsQuery wire.
//!
//! The rule under test: `EstimateTemplateCost` reads the template the request carries
//! (CloudFormation reads a template as JSON or, failing that, as YAML) and answers the
//! Simple Monthly Calculator URL of the estimate it saves for what that template declares:
//! the calculator's estimate page naming an estimate id minted per call, since every call
//! saves its own estimate. It refuses a body on FORM grounds only — past the documented
//! `TemplateBody` maximum, readable as neither JSON nor YAML (refused at the line and
//! column the reader stopped at), or declaring no resource — and writes nothing of this
//! service's own.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out) through
//! the shared wire harness, so what it asserts is what a client calling
//! `EstimateTemplateCost` sees. The tables span the class the rule generalises over: both
//! template spellings (JSON and YAML, long-form and short-form intrinsics), the resource
//! types a template declares, their count, their properties and the values of those
//! properties, the parameter values in force (supplied, defaulted and both), a template
//! carrying a `Description`, a request carrying both `TemplateBody` and `TemplateURL`, and
//! the Region the call is signed for.

use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_in, member, router, router_with_engine, store, xml_text, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::TEMPLATE_BODY_MAX_BYTES;

/// The page a Simple Monthly Calculator estimate is read on, which is what
/// `EstimateTemplateCostOutput$Url` is a URL of ("an Amazon Web Services Simple Monthly
/// Calculator URL with a query string that describes the resources required to run the
/// template").
const CALCULATOR_ESTIMATE_PAGE: &str = "https://calculator.aws/#/estimate";

/// The query string that names the estimate: the producer segment a CloudFormation estimate
/// carries, before the estimate's own hexadecimal id.
const ID_PREFIX: &str = "?id=cloudformation/";

/// How many hexadecimal characters the estimate's own id carries.
const ID_DIGITS: usize = 40;

/// A Region other than the harness's own, for the row that changes only the Region: cost is
/// Regional, so an estimate is taken in the Region the call was signed for.
const OTHER_REGION: &str = "us-west-2";

/// `EstimateTemplateCost` with whatever members the case sends, signed for `region`.
async fn estimate_in(
    router: &MultiServiceRouter,
    region: &str,
    members: &[(String, String)],
) -> (u16, String) {
    call_in(router, region, "EstimateTemplateCost", members).await
}

/// `EstimateTemplateCost` for one template body, signed for the harness's Region.
async fn estimate(router: &MultiServiceRouter, template_body: &str) -> (u16, String) {
    call(
        router,
        "EstimateTemplateCost",
        &[member("TemplateBody", template_body)],
    )
    .await
}

/// One supplied `Parameter`, as the form body spells a list member.
fn parameter(index: usize, key: &str, value: &str) -> Vec<(String, String)> {
    vec![
        member(&format!("Parameters.member.{index}.ParameterKey"), key),
        member(&format!("Parameters.member.{index}.ParameterValue"), value),
    ]
}

/// The request members for a body plus the parameters a case supplies.
fn with_parameters(template_body: &str, parameters: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut members = vec![member("TemplateBody", template_body)];
    for (index, (key, value)) in parameters.iter().enumerate() {
        members.extend(parameter(index + 1, key, value));
    }
    members
}

/// The `Url` the response reports, or `None` when it carries none.
fn reported_url(case: &str, status: u16, body: &str) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body}"
    );
    xml_text(body, "Url").unwrap_or_else(|| {
        panic!("case {case:?}: expected a Url in the response, got {body}");
    })
}

/// A reported URL is the calculator's estimate page plus the estimate's id: the producer
/// segment and [`ID_DIGITS`] lowercase hexadecimal characters.
fn assert_url_shape(case: &str, url: &str) {
    let prefix = format!("{CALCULATOR_ESTIMATE_PAGE}{ID_PREFIX}");
    assert!(
        url.starts_with(&prefix),
        "case {case:?}: expected a URL starting {prefix:?}, got {url:?}"
    );
    let id = &url[prefix.len()..];
    assert_eq!(
        id.len(),
        ID_DIGITS,
        "case {case:?}: expected a {ID_DIGITS}-character estimate id, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "case {case:?}: expected a lowercase-hex estimate id, got {id:?}"
    );
}

/// A template declaring one queue, spelled as JSON.
fn one_queue() -> String {
    r#"{"Resources": {"Queue": {"Type": "AWS::SQS::Queue"}}}"#.to_string()
}

/// One row of the "every template in the class is estimated" table.
struct Case {
    case: &'static str,
    members: Vec<(String, String)>,
    region: &'static str,
}

/// A row sending one body in the harness's Region.
fn row(case: &'static str, template_body: &str) -> Case {
    Case {
        case,
        members: vec![member("TemplateBody", template_body)],
        region: REGION,
    }
}

/// The template the parameter rows estimate: what the stack would run with is a property
/// taken from a parameter, so the value in force is part of what is estimated.
const PARAMETERISED: &str = r#"{
    "Parameters": {"TopicName": {"Type": "String", "Default": "defaulted-topic"}},
    "Resources": {
        "Topic": {"Type": "AWS::SNS::Topic", "Properties": {"TopicName": {"Ref": "TopicName"}}}
    }
}"#;

/// The same declarations as [`PARAMETERISED`], spelled as YAML with a short-form `!Ref`:
/// "CloudFormation first checks if the template is valid JSON. If it isn't, CloudFormation
/// checks if the template is valid YAML", so both spellings are estimated.
const PARAMETERISED_YAML: &str = "Parameters:\n\
     \x20 TopicName:\n\
     \x20   Type: String\n\
     \x20   Default: defaulted-topic\n\
     Resources:\n\
     \x20 Topic:\n\
     \x20   Type: AWS::SNS::Topic\n\
     \x20   Properties:\n\
     \x20     TopicName: !Ref TopicName\n";

/// The templates and requests an estimate is taken for: the class the rule generalises
/// over, each row differing from the others in the declared resource's type, the resource
/// COUNT, a declared property, that property's VALUE, the parameter value in force, the
/// template's spelling, the members the request carries, or the Region.
fn cases() -> Vec<Case> {
    vec![
        row("one queue", &one_queue()),
        row(
            "one topic — the same count, another resource type",
            r#"{"Resources": {"Topic": {"Type": "AWS::SNS::Topic"}}}"#,
        ),
        row(
            "a queue and a topic — one resource more",
            r#"{"Resources": {
                "Queue": {"Type": "AWS::SQS::Queue"},
                "Topic": {"Type": "AWS::SNS::Topic"}
            }}"#,
        ),
        row(
            "one queue carrying a property",
            r#"{"Resources": {"Queue": {
                "Type": "AWS::SQS::Queue", "Properties": {"QueueName": "alpha"}
            }}}"#,
        ),
        row(
            "one queue carrying that property with another value",
            r#"{"Resources": {"Queue": {
                "Type": "AWS::SQS::Queue", "Properties": {"QueueName": "beta"}
            }}}"#,
        ),
        row(
            "a template carrying a Description",
            r#"{"Description": "An estimated queue",
                "Resources": {"Queue": {"Type": "AWS::SQS::Queue"}}}"#,
        ),
        Case {
            case: "the parameterised template, with the declared default in force",
            members: with_parameters(PARAMETERISED, &[]),
            region: REGION,
        },
        Case {
            case: "the parameterised template, with a supplied parameter value",
            members: with_parameters(PARAMETERISED, &[("TopicName", "supplied-topic")]),
            region: REGION,
        },
        Case {
            case: "the same declarations spelled as YAML, with a short-form !Ref",
            members: with_parameters(PARAMETERISED_YAML, &[("TopicName", "supplied-topic")]),
            region: REGION,
        },
        Case {
            // "If both are passed, only TemplateBody is used" — the URL this answers is an
            // estimate of the BODY, and the sibling refusal row shows a `TemplateURL` alone
            // is not read at all.
            case: "a body beside an ignored TemplateURL",
            members: vec![
                member("TemplateBody", &one_queue()),
                member(
                    "TemplateURL",
                    "https://s3.amazonaws.com/a-bucket/another-template.json",
                ),
            ],
            region: REGION,
        },
        Case {
            case: "one queue, estimated in another Region",
            members: vec![member("TemplateBody", &one_queue())],
            region: OTHER_REGION,
        },
    ]
}

#[tokio::test]
async fn every_estimate_answers_the_url_of_its_own_saved_estimate() {
    let router = router();
    let mut seen: Vec<(&str, String)> = Vec::new();
    for case in cases() {
        let label = case.case;
        // Called twice, because each call saves its OWN estimate: the second call names a
        // second saved estimate rather than the first one again.
        for attempt in 1..=2 {
            let (status, body) = estimate_in(&router, case.region, &case.members).await;
            let url = reported_url(label, status, &body);
            assert_url_shape(label, &url);
            for (other, other_url) in &seen {
                assert_ne!(
                    &url, other_url,
                    "case {label:?} (call {attempt}): expected an estimate id of its own, \
                     got the one case {other:?} answered ({url:?})"
                );
            }
            seen.push((label, url));
        }
    }
}

/// One row of the malformed-body table: a body the reader stops in, and where it stops.
struct MalformedCase {
    case: &'static str,
    body: &'static str,
    /// The 1-based line the refusal must name, which is the line the body's syntax error is
    /// on.
    line: usize,
    /// The 1-based column, where the row can state it: a body that ends mid-document is
    /// stopped one character past its last, whatever that column happens to be.
    column: Option<usize>,
}

#[tokio::test]
async fn a_malformed_body_is_refused_where_the_reader_stopped() {
    let router = router();
    // "CloudFormation first checks if the template is valid JSON. If it isn't,
    // CloudFormation checks if the template is valid YAML. If both these checks fail,
    // CloudFormation returns a template validation error" — and the error names the position
    // the reader stopped at, so the caller can find it. The rows put that position on
    // different LINES of the body.
    const TRUNCATED: &str = "{\"Resources\": {\"Queue\": {\"Type\": ";
    let cases = vec![
        MalformedCase {
            case: "a one-line body whose JSON object never closes",
            body: TRUNCATED,
            line: 1,
            // Stopped at the end of the body, which is one past its last character.
            column: Some(TRUNCATED.len() + 1),
        },
        MalformedCase {
            case: "a YAML body whose third line carries two mapping values",
            body: "Resources:\n  Queue:\n    Type: AWS::SQS::Queue: extra\n",
            line: 3,
            column: None,
        },
        MalformedCase {
            case: "a YAML body whose fifth line carries two mapping values",
            body: "Resources:\n  Queue:\n    Type: AWS::SQS::Queue\n  Topic:\n    \
                   Type: AWS::SNS::Topic: extra\n",
            line: 5,
            column: None,
        },
    ];

    for case in &cases {
        let label = case.case;
        let (status, body) =
            estimate_in(&router, REGION, &[member("TemplateBody", case.body)]).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        let expected = format!(
            "Template format error: YAML not well-formed. (line {}, column ",
            case.line
        );
        assert!(
            message.starts_with(&expected) && message.ends_with(')'),
            "case {label:?}: expected a refusal of the form {expected:?}…), got {message:?}"
        );
        // The column is a position too, so it is a number, and it is a position ON the line
        // the refusal names: at most one past that line's last character.
        let column = message
            .rsplit_once("column ")
            .and_then(|(_, rest)| rest.strip_suffix(')'))
            .and_then(|column| column.parse::<usize>().ok())
            .unwrap_or_default();
        let line_length = case
            .body
            .lines()
            .nth(case.line - 1)
            .map_or(0, |line| line.chars().count());
        assert!(
            (1..=line_length + 1).contains(&column),
            "case {label:?}: expected a column on line {} (1..={}), got {message:?}",
            case.line,
            line_length + 1
        );
        if let Some(expected_column) = case.column {
            assert_eq!(
                column, expected_column,
                "case {label:?}: expected the refusal to stop at column \
                 {expected_column}, got {message:?}"
            );
        }
    }
}

/// One row of the refusal table: a request `EstimateTemplateCost` must not answer a URL
/// for, and the status, wire code and message the refusal carries.
struct RefusalCase {
    case: &'static str,
    members: Vec<(String, String)>,
    status: u16,
    code: &'static str,
    /// Text the message must contain, so a row cannot pass by failing for another reason.
    message: String,
}

#[tokio::test]
async fn estimate_template_cost_refuses_a_body_it_cannot_estimate() {
    let over_long_body = format!(
        r#"{{"Description": "{}", "Resources": {{"Q": {{"Type": "AWS::SQS::Queue"}}}}}}"#,
        "d".repeat(TEMPLATE_BODY_MAX_BYTES)
    );
    let refusals = vec![
        RefusalCase {
            case: "neither TemplateBody nor TemplateURL",
            members: Vec::new(),
            status: 400,
            code: "ValidationError",
            message: "Either TemplateBody or TemplateURL must be specified".to_string(),
        },
        RefusalCase {
            case: "a body that is neither valid JSON nor valid YAML",
            members: vec![member("TemplateBody", "{\"Resources\": ")],
            status: 400,
            code: "ValidationError",
            message: "Template format error".to_string(),
        },
        RefusalCase {
            case: "a template whose Resources section is empty",
            members: vec![member("TemplateBody", r#"{"Resources": {}}"#)],
            status: 400,
            code: "ValidationError",
            message: "At least one Resources member must be defined".to_string(),
        },
        RefusalCase {
            case: "a template whose only resource declares no Type",
            members: vec![member(
                "TemplateBody",
                r#"{"Resources": {"Queue": {"Properties": {}}}}"#,
            )],
            status: 400,
            code: "ValidationError",
            // The entry is named by its own path: "Type: required" is a rule about THAT
            // entry, not about the section, so the two refusals differ.
            message: "[/Resources/Queue] Every Resources object must contain a Type member."
                .to_string(),
        },
        RefusalCase {
            case: "a template declaring one resource beside an untyped sibling",
            members: vec![member(
                "TemplateBody",
                r#"{"Resources": {
                    "Queue": {"Type": "AWS::SQS::Queue"},
                    "Untyped": {"Properties": {}}
                }}"#,
            )],
            status: 400,
            code: "ValidationError",
            message: "[/Resources/Untyped] Every Resources object must contain a Type member."
                .to_string(),
        },
        RefusalCase {
            case: "a body one byte past the documented TemplateBody maximum",
            members: vec![member("TemplateBody", &over_long_body)],
            status: 400,
            code: "ValidationError",
            message: "templateBody".to_string(),
        },
        RefusalCase {
            case: "a supplied parameter carrying no ParameterValue",
            members: vec![
                member("TemplateBody", PARAMETERISED),
                member("Parameters.member.1.ParameterKey", "TopicName"),
            ],
            status: 400,
            code: "ValidationError",
            message: "Parameters: [TopicName] must have values".to_string(),
        },
        RefusalCase {
            // A `TemplateURL` is resolved by reading the object it addresses out of this
            // emulator's own Amazon S3, so one naming a bucket nothing created is the
            // REQUEST's own bad reference — not an estimate of a template nobody read.
            case: "a TemplateURL naming no object this account can read",
            members: vec![member(
                "TemplateURL",
                "https://s3.amazonaws.com/a-bucket/a-template.json",
            )],
            status: 400,
            code: "ValidationError",
            message: "S3 error: The specified bucket does not exist\nFor more information \
                      check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html"
                .to_string(),
        },
    ];

    let router = router();
    for refusal in &refusals {
        let label = refusal.case;
        let (status, body) = estimate_in(&router, REGION, &refusal.members).await;
        assert_eq!(
            status, refusal.status,
            "case {label:?}: expected {}, got {status} {body}",
            refusal.status
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some(refusal.code),
            "case {label:?}: expected the wire code {:?}, got {body}",
            refusal.code
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert!(
            message.contains(refusal.message.as_str()),
            "case {label:?}: expected a message naming {:?}, got {message:?}",
            refusal.message
        );
    }
}

#[tokio::test]
async fn a_body_at_its_documented_maximum_is_still_estimated() {
    let router = router();
    // The sibling of the over-long refusal row: the SAME shape, exactly at the documented
    // maximum instead of one byte past it, so what separates the two rows is the body's
    // LENGTH and nothing else.
    let padding = r#"{"Description": "", "Resources": {"Q": {"Type": "AWS::SQS::Queue"}}}"#.len();
    let body = format!(
        r#"{{"Description": "{}", "Resources": {{"Q": {{"Type": "AWS::SQS::Queue"}}}}}}"#,
        "d".repeat(TEMPLATE_BODY_MAX_BYTES - padding)
    );
    assert_eq!(
        body.len(),
        TEMPLATE_BODY_MAX_BYTES,
        "case \"a body at the documented maximum\": the row must send exactly the \
         documented maximum, got {} bytes",
        body.len()
    );
    let (status, response) = estimate(&router, &body).await;
    let url = reported_url("a body at the documented maximum", status, &response);
    assert_url_shape("a body at the documented maximum", &url);
}

#[tokio::test]
async fn estimating_a_template_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let template = r#"{
        "Parameters": {"Name": {"Type": "String", "Default": "a-name"}},
        "Resources": {"Queue": {"Type": "AWS::SQS::Queue", "Properties": {"QueueName": {"Ref": "Name"}}}}
    }"#;
    let (first_status, first) = estimate(&router, template).await;
    let (second_status, second) = estimate(&router, template).await;
    let first_url = reported_url("estimate twice", first_status, &first);
    let second_url = reported_url("estimate twice", second_status, &second);
    // Each call saves its own estimate, so the two name different ones — and neither leaves
    // anything in this service's store for a later caller to find.
    assert_ne!(
        first_url, second_url,
        "case \"estimate twice\": expected each call to name its own saved estimate, got \
         {first_url:?} twice"
    );
    let held = store(&engine);
    let state = held.lock();
    assert!(
        state.stacks.is_empty() && state.change_sets.is_empty(),
        "case \"estimate twice\": expected an estimate to persist nothing, got {} stack(s) \
         and {} change set(s)",
        state.stacks.len(),
        state.change_sets.len()
    );
}
