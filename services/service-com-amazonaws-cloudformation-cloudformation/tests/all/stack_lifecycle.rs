// SPDX-License-Identifier: CC0-1.0

//! The stack lifecycle, end to end over the awsQuery wire.
//!
//! Every case here drives the real request path — form-encoded body in, XML envelope out
//! — through the router, so what it asserts is what a client
//! sees: `CreateStack` persists a stack, and `DescribeStacks`, `ListStacks`,
//! `GetTemplate`, `UpdateStack`, `DeleteStack` and `DescribeStackEvents` read and mutate
//! THAT stack rather than echoing the request back. That router carries the services that
//! own the resource types a stack really creates beside this one on one engine — Amazon S3
//! for an `AWS::S3::Bucket`, Amazon SNS for the `AWS::SNS::Topic` most of these templates
//! declare — because a stack provisions them by calling those services in process (see
//! `router_with_engine`).
//!
//! The wire harness here (`call`, `member`, the XML readers, `router`) is this crate's
//! one such harness: `stack_outputs.rs` drives its cases through these same helpers.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::{
    self, status, ComAmazonawsCloudformationCloudformationState, StoredStack,
    MAX_DELETED_STACKS_RETAINED, MAX_EVENTS_PER_STACK, MAX_STACKS, PAGE_SIZE,
    STATUS_TRANSITION_DELAY, TEMPLATE_BODY_MAX_BYTES,
};
use service_com_amazonaws_cloudformation_cloudformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The emulator's canonical account id, as every ARN it emits carries.
pub(crate) const ACCOUNT_ID: &str = "000000000000";
/// The region the test's requests are signed for.
pub(crate) const REGION: &str = "us-east-1";

pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The router AND the engine behind it, for a case that has to inspect the store or seed
/// it with more records than a bound's worth of wire calls could create.
///
/// The services that OWN the resource types `state::provisioning` creates are mounted
/// beside CloudFormation on the SAME engine, exactly as `core/router/src/lib.rs` mounts
/// them: a stack's create calls those services, so a suite that deploys a template
/// declaring one of those types must have them there — Amazon SNS among them, since the
/// `AWS::SNS::Topic` most cases here declare is really created in SNS by the stack. (Amazon
/// Data Firehose is deliberately NOT one of them — `unprovisioned_resources.rs` is the
/// suite that pins what a stack does when an owning service cannot be reached.) No case
/// here calls SNS itself; what a stack and SNS must agree about is
/// `core/router/tests/all/cloudformation_provisions_sns_topics.rs`.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// This service's store in the shared engine, as every handler reaches it.
pub(crate) fn store(
    engine: &Arc<engine::InMemoryEngine>,
) -> engine::ServiceState<ComAmazonawsCloudformationCloudformationState> {
    engine.state::<ComAmazonawsCloudformationCloudformationState>()
}

/// Wait past the instant a stack operation's terminal status becomes due.
pub(crate) async fn wait_for_the_transition() {
    tokio::time::sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// The status a stack must report on a read taken `started` ago: the in-progress one
/// while the transition is not yet due, the settled one once it is.
///
/// The guard is what keeps the case honest on a loaded machine: the emulator owes the
/// in-progress status only until the due instant, so a read that lost the race to the
/// clock must be held to the settled status instead of failing.
pub(crate) fn expected_status(
    started: Instant,
    in_progress: &'static str,
    settled: &'static str,
) -> &'static str {
    if started.elapsed() < STATUS_TRANSITION_DELAY {
        in_progress
    } else {
        settled
    }
}

/// Percent-encode one form value, so a template body carrying `{`, `"`, `&` or a
/// multi-byte character survives the body.
///
/// Everything outside the unreserved set an AWS SDK leaves alone is escaped, which is
/// what the form body an SDK sends looks like.
pub(crate) fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Call one operation: `Action` plus the request's members, form-encoded exactly as an
/// AWS SDK sends them, answered with the HTTP status and the XML body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://cloudformation.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cloudformation/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

pub(crate) fn member(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One awsQuery call to IAM over the same router, answered with the HTTP status and the XML
/// body.
///
/// The ONE IAM caller of this crate's suites: the role records a stack reads (a `RoleARN`'s
/// trust policy) and the roles a stack CREATES are both written and read through IAM's own
/// wire, so a change to the calling convention is made here once.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed IAM request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// Undo the XML text escaping the serializer applies, so a round-tripped value can be
/// compared with what was sent.
pub(crate) fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Every `<tag>` text in document order.
pub(crate) fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(xml_unescape(&after[..end]));
        rest = &after[end + close.len()..];
    }
    found
}

/// The first `<tag>` text, or `None` when the response carries none.
pub(crate) fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// The inner text of every non-nested `<tag>…</tag>` BLOCK, in document order, left exactly
/// as the serializer wrote it.
///
/// [`xml_texts`] is the leaf form of this scan: it unescapes what it finds, which is right
/// for a value and wrong for a block that carries more elements. A caller reading a nested
/// list (the `<member>`s inside one `<Details>`) takes the block first and scans INSIDE it,
/// so a sibling list's members cannot be swept up with it.
pub(crate) fn xml_blocks(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    found
}

pub(crate) async fn create_stack(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("StackName", name), member("TemplateBody", template)];
    members.extend_from_slice(extra);
    call(router, "CreateStack", &members).await
}

pub(crate) async fn describe_stack(router: &MultiServiceRouter, name_or_id: &str) -> (u16, String) {
    call(
        router,
        "DescribeStacks",
        &[member("StackName", name_or_id)],
    )
    .await
}

pub(crate) async fn get_template(router: &MultiServiceRouter, name_or_id: &str) -> (u16, String) {
    call(router, "GetTemplate", &[member("StackName", name_or_id)]).await
}

/// A created stack's id is the ARN form CloudFormation mints:
/// `arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>`.
pub(crate) fn assert_stack_id_shape(case: &str, stack_id: &str, stack_name: &str) {
    let prefix = format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{stack_name}/");
    assert!(
        stack_id.starts_with(&prefix),
        "case {case:?}: expected a stack id starting {prefix:?}, got {stack_id:?}"
    );
    let uuid = &stack_id[prefix.len()..];
    assert_eq!(
        uuid.len(),
        36,
        "case {case:?}: expected a 36-character uuid after the stack name, got {uuid:?}"
    );
    assert!(
        uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "case {case:?}: expected a hex-and-hyphen uuid, got {uuid:?}"
    );
}

/// One row of the create/describe/get-template table.
struct TemplateCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// The `Description` the stack must report: the template's own, when it has one.
    description: Option<&'static str>,
    parameters: Vec<(String, String)>,
    tags: Vec<(String, String)>,
}

/// A borrowed key/value table as owned pairs.
pub(crate) fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The keys of a key/value table, for comparing against a response's elements.
fn keys(pairs: &[(String, String)]) -> Vec<String> {
    pairs.iter().map(|(key, _)| key.clone()).collect()
}

/// The values of a key/value table.
fn values(pairs: &[(String, String)]) -> Vec<String> {
    pairs.iter().map(|(_, value)| value.clone()).collect()
}

/// The class the create/read rule covers: any template body, of any form and size, with
/// any number of parameters and tags. The issue's own case (a one-resource JSON template
/// with one tag) is the second row.
fn template_cases() -> Vec<TemplateCase> {
    vec![
        TemplateCase {
            case: "minimal json object, no parameters, no tags",
            stack_name: "minimal",
            template: "{}".to_string(),
            description: None,
            parameters: vec![],
            tags: vec![],
        },
        TemplateCase {
            case: "described single-resource template with one tag",
            stack_name: "demo",
            template: r#"{"Description":"demo stack","Resources":{"T":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#
                .to_string(),
            description: Some("demo stack"),
            parameters: vec![],
            tags: kv(&[("env", "dev")]),
        },
        TemplateCase {
            case: "three parameters whose values carry spaces, symbols and an empty string",
            stack_name: "params-3",
            template: r#"{"Description":"parameterised","Parameters":{"Alpha":{"Type":"String"}}}"#
                .to_string(),
            description: Some("parameterised"),
            parameters: kv(&[("Alpha", "a b&c=d"), ("Beta", ""), ("Gamma", "3")]),
            tags: kv(&[("team", "iam"), ("env", "prod")]),
        },
        TemplateCase {
            case: "yaml body with no Description section",
            stack_name: "yaml-body",
            template: "Resources:\n  T:\n    Type: AWS::CloudFormation::WaitConditionHandle\n"
                .to_string(),
            description: None,
            parameters: vec![],
            tags: vec![],
        },
        TemplateCase {
            // CloudFormation reads a template written in JSON *or* YAML, so the
            // `Description` it reports comes from either spelling.
            case: "yaml body declaring a Description",
            stack_name: "yaml-described",
            template: "Description: a yaml stack\nResources:\n  T:\n    Type: AWS::SNS::Topic\n"
                .to_string(),
            description: Some("a yaml stack"),
            parameters: vec![],
            tags: kv(&[("env", "dev")]),
        },
        TemplateCase {
            case: "unicode, quotes and angle brackets in the template",
            stack_name: "escaped",
            template: r#"{"Description":"café \"quoted\" & <angled>"}"#.to_string(),
            description: Some("café \"quoted\" & <angled>"),
            parameters: vec![],
            tags: kv(&[("note", "a<b&c>d")]),
        },
        TemplateCase {
            case: "large template body and a long parameter value",
            stack_name: "large",
            template: format!(
                r#"{{"Description":"large","Metadata":{{"Pad":"{}"}}}}"#,
                "x".repeat(8192)
            ),
            description: Some("large"),
            parameters: vec![("Pad".to_string(), "y".repeat(512))],
            tags: vec![],
        },
    ]
}

#[tokio::test]
async fn create_then_describe_and_get_template_round_trip_every_template_shape() {
    let router = router();

    for case in template_cases() {
        let mut extra = Vec::new();
        for (index, (key, value)) in case.parameters.iter().enumerate() {
            let n = index + 1;
            extra.push(member(&format!("Parameters.member.{n}.ParameterKey"), key));
            extra.push(member(&format!("Parameters.member.{n}.ParameterValue"), value));
        }
        for (index, (key, value)) in case.tags.iter().enumerate() {
            let n = index + 1;
            extra.push(member(&format!("Tags.member.{n}.Key"), key));
            extra.push(member(&format!("Tags.member.{n}.Value"), value));
        }

        let (status, body) =
            create_stack(&router, case.stack_name, &case.template, &extra).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
        let stack_id = xml_text(&body, "StackId")
            .unwrap_or_else(|| panic!("case {:?}: CreateStack answered no StackId: {body}", case.case));
        assert_stack_id_shape(case.case, &stack_id, case.stack_name);

        // Addressed by name and by stack id, a create must be readable back as what was
        // created.
        for identifier in [case.stack_name.to_string(), stack_id.clone()] {
            let (status, body) = describe_stack(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected DescribeStacks({identifier}) to succeed, got {status} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackId").as_deref(),
                Some(stack_id.as_str()),
                "case {:?}: DescribeStacks({identifier}) must report the created stack id, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackName").as_deref(),
                Some(case.stack_name),
                "case {:?}: DescribeStacks({identifier}) must report the created name, got {body}",
                case.case
            );
            assert!(
                xml_text(&body, "CreationTime").is_some(),
                "case {:?}: DescribeStacks({identifier}) must report a CreationTime, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "Description").as_deref(),
                case.description,
                "case {:?}: DescribeStacks({identifier}) must report the template's Description, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackDriftStatus").as_deref(),
                Some("NOT_CHECKED"),
                "case {:?}: a stack never checked for drift is NOT_CHECKED, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "ParameterKey"),
                keys(&case.parameters),
                "case {:?}: DescribeStacks({identifier}) must report the stored parameter keys, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "ParameterValue"),
                values(&case.parameters),
                "case {:?}: DescribeStacks({identifier}) must report the stored parameter values, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "Key"),
                keys(&case.tags),
                "case {:?}: DescribeStacks({identifier}) must report the stored tag keys, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "Value"),
                values(&case.tags),
                "case {:?}: DescribeStacks({identifier}) must report the stored tag values, got {body}",
                case.case
            );

            // The template body is persisted verbatim: a byte-identical round trip.
            let (status, body) = get_template(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected GetTemplate({identifier}) to succeed, got {status} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "TemplateBody").as_deref(),
                Some(case.template.as_str()),
                "case {:?}: GetTemplate({identifier}) must answer the exact body that was submitted, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "member"),
                vec!["Original".to_string(), "Processed".to_string()],
                "case {:?}: both template stages are available for a stack, got {body}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_created_stack_is_in_progress_before_the_transition_and_complete_after_it() {
    let router = router();
    let started = Instant::now();
    let (_, created) = create_stack(&router, "settling", "{}", &[]).await;
    let stack_id = xml_text(&created, "StackId").expect("a stack id");

    let (_, body) = describe_stack(&router, "settling").await;
    let owed = expected_status(started, "CREATE_IN_PROGRESS", "CREATE_COMPLETE");
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(owed),
        "case \"immediately after create\": expected {owed} on a read taken this soon, got {body}"
    );

    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case \"after the transition is due\": the stack must have settled, got {body}"
    );
}

#[tokio::test]
async fn create_stack_enforces_the_documented_stack_name_rule() {
    let router = router();

    // The rule, from `CreateStackInput$StackName`: alphanumeric characters and hyphens
    // only, starting with a letter, at most 128 characters. Each bound is tested at the
    // published value and one past it.
    let long_128 = "a".repeat(128);
    let long_129 = "a".repeat(129);
    let accepted: Vec<(&str, &str)> = vec![
        ("single letter", "a"),
        ("letters, digits and hyphens", "Stack-9-x"),
        ("at the published length bound", &long_128),
    ];
    let refused: Vec<(&str, &str)> = vec![
        ("leading digit", "1stack"),
        ("leading hyphen", "-stack"),
        ("underscore", "my_stack"),
        ("space", "my stack"),
        ("dot", "my.stack"),
        ("one past the published length bound", &long_129),
    ];

    for (case, name) in accepted {
        let (status, body) = create_stack(&router, name, "{}", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {name:?} satisfies the stack name rule, got {status} {body}"
        );
    }
    for (case, name) in refused {
        let (status, body) = create_stack(&router, name, "{}", &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {name:?} breaks the stack name rule, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: a refused stack name is a ValidationError, got {body}"
        );
        let (status, _) = describe_stack(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must not have stored a stack"
        );
    }
}

#[tokio::test]
async fn create_stack_needs_exactly_one_template_source() {
    let router = router();
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("neither TemplateBody nor TemplateURL", vec![]),
        (
            "both TemplateBody and TemplateURL",
            vec![
                member("TemplateBody", "{}"),
                member("TemplateURL", "https://example.test/t.json"),
            ],
        ),
    ];
    for (case, extra) in cases {
        let mut members = vec![member("StackName", "no-template")];
        members.extend(extra);
        let (status, body) = call(&router, "CreateStack", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the request names no single template source, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn a_live_stack_name_is_taken_and_is_free_again_once_its_deletion_completes() {
    let router = router();
    let (status, first) = create_stack(&router, "unique", "{}", &[]).await;
    assert_eq!(status, 200, "the first create must succeed, got {first}");
    let first_id = xml_text(&first, "StackId").expect("a stack id");

    let (status, body) = create_stack(&router, "unique", "{}", &[]).await;
    assert_eq!(
        status, 400,
        "case \"name of a live stack\": the second create must be refused, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("AlreadyExistsException"),
        "case \"name of a live stack\": expected AlreadyExistsException, got {body}"
    );

    wait_for_the_transition().await;
    let (status, body) = call(&router, "DeleteStack", &[member("StackName", "unique")]).await;
    assert_eq!(status, 200, "DeleteStack must succeed, got {status} {body}");
    wait_for_the_transition().await;

    let (status, second) = create_stack(&router, "unique", "{}", &[]).await;
    assert_eq!(
        status, 200,
        "case \"name of a deleted stack\": the name is free again, got {status} {second}"
    );
    let second_id = xml_text(&second, "StackId").expect("a stack id");
    assert_ne!(
        first_id, second_id,
        "case \"name of a deleted stack\": the new stack must have its own stack id"
    );
}

#[tokio::test]
async fn an_identifier_that_names_no_live_stack_is_refused_with_validation_error() {
    let router = router();
    let (_, created) = create_stack(&router, "gone", "{}", &[]).await;
    let stack_id = xml_text(&created, "StackId").expect("a stack id");
    wait_for_the_transition().await;
    call(&router, "DeleteStack", &[member("StackName", "gone")]).await;
    wait_for_the_transition().await;

    let cases = vec![
        ("a name never created", "no-such-stack".to_string()),
        ("the NAME of a deleted stack", "gone".to_string()),
        (
            "a stack id no stack carries",
            format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/ghost/00000000-0000-0000-0000-000000000000"),
        ),
    ];
    for (case, identifier) in cases {
        for action in ["DescribeStacks", "GetTemplate", "DescribeStackEvents"] {
            let (status, body) =
                call(&router, action, &[member("StackName", &identifier)]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {action} must refuse {identifier:?}, got {status} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: {action} must answer ValidationError for {identifier:?}, got {body}"
            );
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some(format!("Stack with id {identifier} does not exist").as_str()),
                "case {case:?}: {action} must name the stack it could not find, got {body}"
            );
        }
    }

    // The deleted stack stays addressable by its stack ID, which is the one difference
    // between the two spellings of its identifier.
    let (status, body) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        status, 200,
        "case \"the ID of a deleted stack\": it stays addressable, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("DELETE_COMPLETE"),
        "case \"the ID of a deleted stack\": it reports DELETE_COMPLETE, got {body}"
    );
    assert!(
        xml_text(&body, "DeletionTime").is_some(),
        "case \"the ID of a deleted stack\": it reports a DeletionTime, got {body}"
    );
}

#[tokio::test]
async fn describe_stacks_without_a_stack_name_answers_every_live_stack_in_creation_order() {
    let router = router();
    for name in ["first", "second", "third"] {
        create_stack(&router, name, "{}", &[]).await;
    }
    wait_for_the_transition().await;
    call(&router, "DeleteStack", &[member("StackName", "second")]).await;
    wait_for_the_transition().await;

    let (status, body) = call(&router, "DescribeStacks", &[]).await;
    assert_eq!(status, 200, "expected DescribeStacks to succeed, got {status} {body}");
    assert_eq!(
        xml_texts(&body, "StackName"),
        vec!["first".to_string(), "third".to_string()],
        "case \"one of three stacks deleted\": only the live stacks, in creation order, got {body}"
    );
}

#[tokio::test]
async fn list_stacks_filters_on_stack_status_filter() {
    let router = router();
    // One stack per terminal status this slice produces: created, updated, deleted.
    for name in ["kept", "updated", "removed"] {
        create_stack(&router, name, "{}", &[]).await;
    }
    wait_for_the_transition().await;
    call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "updated"),
            member("TemplateBody", r#"{"Description":"v2"}"#),
        ],
    )
    .await;
    call(&router, "DeleteStack", &[member("StackName", "removed")]).await;
    wait_for_the_transition().await;

    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("no filter: every stack, deleted ones included", vec![], vec!["kept", "updated", "removed"]),
        ("one status", vec!["CREATE_COMPLETE"], vec!["kept"]),
        ("a different status", vec!["UPDATE_COMPLETE"], vec!["updated"]),
        ("deleted stacks are still summarised", vec!["DELETE_COMPLETE"], vec!["removed"]),
        (
            "two statuses",
            vec!["CREATE_COMPLETE", "DELETE_COMPLETE"],
            vec!["kept", "removed"],
        ),
        ("a status no stack is in", vec!["ROLLBACK_FAILED"], vec![]),
    ];

    for (case, filter, expected) in cases {
        let members: Vec<(String, String)> = filter
            .iter()
            .enumerate()
            .map(|(index, status)| {
                member(&format!("StackStatusFilter.member.{}", index + 1), status)
            })
            .collect();
        let (status, body) = call(&router, "ListStacks", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListStacks to succeed, got {status} {body}"
        );
        assert_eq!(
            xml_texts(&body, "StackName"),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: ListStacks must report exactly the matching stacks, got {body}"
        );
    }

    // A deleted stack's summary carries the instant its deletion completed.
    let (_, body) = call(
        &router,
        "ListStacks",
        &[member("StackStatusFilter.member.1", "DELETE_COMPLETE")],
    )
    .await;
    assert!(
        xml_text(&body, "DeletionTime").is_some(),
        "case \"deleted stacks are still summarised\": the summary carries a DeletionTime, got {body}"
    );
    assert_eq!(
        xml_text(&body, "TemplateDescription").as_deref(),
        None,
        "case \"deleted stacks are still summarised\": this template declares no Description, got {body}"
    );
}

/// One row of the update table: what the request restates, and what the stack must look
/// like afterwards.
struct UpdateCase {
    case: &'static str,
    stack_name: &'static str,
    members: Vec<(String, String)>,
    expected_template: &'static str,
    expected_parameters: Vec<(String, String)>,
    expected_tags: Vec<(String, String)>,
}

#[tokio::test]
async fn update_stack_mutates_the_same_stack_and_is_observable_through_the_reads() {
    const ORIGINAL: &str = r#"{"Description":"v1","Parameters":{"Alpha":{"Type":"String"}}}"#;
    const REPLACEMENT: &str = r#"{"Description":"v2","Resources":{"T2":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

    let cases = vec![
        UpdateCase {
            case: "a new template body, parameters and tags left alone",
            stack_name: "upd-body",
            members: vec![member("TemplateBody", REPLACEMENT)],
            expected_template: REPLACEMENT,
            expected_parameters: kv(&[("Alpha", "1"), ("Beta", "2")]),
            expected_tags: kv(&[("env", "dev")]),
        },
        UpdateCase {
            case: "the previous template, one parameter changed and one kept",
            stack_name: "upd-params",
            members: vec![
                member("UsePreviousTemplate", "true"),
                member("Parameters.member.1.ParameterKey", "Alpha"),
                member("Parameters.member.1.ParameterValue", "99"),
                member("Parameters.member.2.ParameterKey", "Beta"),
                member("Parameters.member.2.UsePreviousValue", "true"),
            ],
            expected_template: ORIGINAL,
            expected_parameters: kv(&[("Alpha", "99"), ("Beta", "2")]),
            expected_tags: kv(&[("env", "dev")]),
        },
        UpdateCase {
            case: "a new template body and the tags replaced",
            stack_name: "upd-tags",
            members: vec![
                member("TemplateBody", REPLACEMENT),
                member("Tags.member.1.Key", "env"),
                member("Tags.member.1.Value", "staging"),
                member("Tags.member.2.Key", "owner"),
                member("Tags.member.2.Value", "platform"),
            ],
            expected_template: REPLACEMENT,
            expected_parameters: kv(&[("Alpha", "1"), ("Beta", "2")]),
            expected_tags: kv(&[("env", "staging"), ("owner", "platform")]),
        },
    ];

    let router = router();
    for case in &cases {
        let (status, created) = create_stack(
            &router,
            case.stack_name,
            ORIGINAL,
            &[
                member("Parameters.member.1.ParameterKey", "Alpha"),
                member("Parameters.member.1.ParameterValue", "1"),
                member("Parameters.member.2.ParameterKey", "Beta"),
                member("Parameters.member.2.ParameterValue", "2"),
                member("Tags.member.1.Key", "env"),
                member("Tags.member.1.Value", "dev"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the stack under test must be created, got {created}",
            case.case
        );
    }
    wait_for_the_transition().await;

    for case in &cases {
        let (_, created) = describe_stack(&router, case.stack_name).await;
        let stack_id = xml_text(&created, "StackId").expect("a stack id");

        let mut members = vec![member("StackName", case.stack_name)];
        members.extend(case.members.iter().cloned());
        let started = Instant::now();
        let (status, body) = call(&router, "UpdateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected UpdateStack to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackId").as_deref(),
            Some(stack_id.as_str()),
            "case {:?}: an update must mutate the SAME stack, got {body}",
            case.case
        );

        let (_, body) = describe_stack(&router, case.stack_name).await;
        let owed = expected_status(started, "UPDATE_IN_PROGRESS", "UPDATE_COMPLETE");
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(owed),
            "case {:?}: expected {owed} on a read taken this soon after the update, got {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (_, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {:?}: the update must have settled, got {body}",
            case.case
        );
        assert!(
            xml_text(&body, "LastUpdatedTime").is_some(),
            "case {:?}: an updated stack reports a LastUpdatedTime, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "ParameterKey"),
            keys(&case.expected_parameters),
            "case {:?}: the merged parameter keys, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "ParameterValue"),
            values(&case.expected_parameters),
            "case {:?}: the merged parameter values, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "Key"),
            keys(&case.expected_tags),
            "case {:?}: the stack's tags after the update, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "Value"),
            values(&case.expected_tags),
            "case {:?}: the stack's tag values after the update, got {body}",
            case.case
        );

        let (_, body) = get_template(&router, case.stack_name).await;
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.expected_template),
            "case {:?}: GetTemplate must answer the template the update left in place, got {body}",
            case.case
        );
    }
}

#[tokio::test]
async fn update_stack_refuses_what_it_cannot_apply() {
    let router = router();
    let started = Instant::now();
    create_stack(&router, "refusals", r#"{"Description":"v1"}"#, &[]).await;

    // While the create is still in flight, an update cannot start. Once the create has
    // settled the same call is accepted, so the case is only asserted when the request
    // really did reach the service before the due instant.
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "refusals"),
            member("TemplateBody", r#"{"Description":"v2"}"#),
        ],
    )
    .await;
    if started.elapsed() < STATUS_TRANSITION_DELAY {
        assert_eq!(
            status, 400,
            "case \"while the stack is CREATE_IN_PROGRESS\": the update is refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case \"while the stack is CREATE_IN_PROGRESS\": expected ValidationError, got {body}"
        );
    }

    wait_for_the_transition().await;

    // Whatever the race above did, the stack is back on `{"Description":"v1"}` for the
    // table below: an update that went through is undone by updating back.
    if status == 200 {
        call(
            &router,
            "UpdateStack",
            &[
                member("StackName", "refusals"),
                member("TemplateBody", r#"{"Description":"v1"}"#),
            ],
        )
        .await;
        wait_for_the_transition().await;
    }

    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "an update that changes nothing",
            vec![member("TemplateBody", r#"{"Description":"v1"}"#)],
        ),
        (
            "the previous template with nothing else restated",
            vec![member("UsePreviousTemplate", "true")],
        ),
        ("no template source at all", vec![]),
        (
            "two template sources",
            vec![
                member("TemplateBody", "{}"),
                member("UsePreviousTemplate", "true"),
            ],
        ),
        (
            "a previous value for a parameter the stack does not have",
            vec![
                member("TemplateBody", "{}"),
                member("Parameters.member.1.ParameterKey", "Nope"),
                member("Parameters.member.1.UsePreviousValue", "true"),
            ],
        ),
    ];
    for (case, extra) in cases {
        let mut members = vec![member("StackName", "refusals")];
        members.extend(extra);
        let (status, body) = call(&router, "UpdateStack", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the update must be refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }

    // Every refusal above left the stack exactly as it was.
    let (_, body) = get_template(&router, "refusals").await;
    assert_eq!(
        xml_text(&body, "TemplateBody").as_deref(),
        Some(r#"{"Description":"v1"}"#),
        "a refused update must not have changed the stored template, got {body}"
    );
    let (_, body) = describe_stack(&router, "refusals").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "a refused update must not have moved the stack's status, got {body}"
    );
}

#[tokio::test]
async fn delete_stack_is_refused_while_termination_protection_is_enabled() {
    let router = router();
    create_stack(
        &router,
        "protected",
        "{}",
        &[member("EnableTerminationProtection", "true")],
    )
    .await;
    wait_for_the_transition().await;

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", "protected")]).await;
    assert_eq!(
        status, 400,
        "case \"termination protection enabled\": the delete is refused, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case \"termination protection enabled\": expected ValidationError, got {body}"
    );
    assert!(
        xml_text(&body, "Message").is_some_and(|m| m.contains("TerminationProtection")),
        "case \"termination protection enabled\": the message must say why, got {body}"
    );

    wait_for_the_transition().await;
    let (status, body) = describe_stack(&router, "protected").await;
    assert_eq!(
        status, 200,
        "case \"termination protection enabled\": the stack remains unchanged, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case \"termination protection enabled\": the refused delete moved no status, got {body}"
    );
    assert_eq!(
        xml_text(&body, "EnableTerminationProtection").as_deref(),
        Some("true"),
        "case \"termination protection enabled\": the flag is read back from the store, got {body}"
    );
}

#[tokio::test]
async fn deleting_a_stack_that_is_not_in_the_live_set_succeeds_and_changes_nothing() {
    let router = router();
    let cases = vec![
        ("a name no stack ever had", "never-existed".to_string()),
        (
            "a stack id no stack carries",
            format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/ghost/00000000-0000-0000-0000-000000000000"),
        ),
    ];
    for (case, identifier) in cases {
        let (status, body) =
            call(&router, "DeleteStack", &[member("StackName", &identifier)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting what is already absent succeeds, got {status} {body}"
        );
    }

    let (status, body) = call(&router, "ListStacks", &[]).await;
    assert_eq!(status, 200, "expected ListStacks to succeed, got {status} {body}");
    assert!(
        xml_texts(&body, "StackName").is_empty(),
        "a delete of an absent stack must not have invented one, got {body}"
    );
}

#[tokio::test]
async fn describe_stack_events_reports_the_recorded_events_newest_first() {
    let router = router();
    let started = Instant::now();
    let (_, created) = create_stack(&router, "eventful", r#"{"Description":"v1"}"#, &[]).await;
    let stack_id = xml_text(&created, "StackId").expect("a stack id");

    let (status, body) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", "eventful")],
    )
    .await;
    assert_eq!(status, 200, "expected DescribeStackEvents to succeed, got {status} {body}");
    // Newest first: the opening event, preceded by the settled one once the transition is
    // due — which is the only thing the clock can have added by now.
    let owed_events: Vec<String> = if started.elapsed() < STATUS_TRANSITION_DELAY {
        vec!["CREATE_IN_PROGRESS".to_string()]
    } else {
        vec![
            "CREATE_COMPLETE".to_string(),
            "CREATE_IN_PROGRESS".to_string(),
        ]
    };
    assert_eq!(
        xml_texts(&body, "ResourceStatus"),
        owed_events,
        "case \"immediately after create\": expected {owed_events:?}, got {body}"
    );
    assert_eq!(
        xml_text(&body, "ResourceType").as_deref(),
        Some("AWS::CloudFormation::Stack"),
        "case \"immediately after create\": the stack's own resource type, got {body}"
    );
    assert_eq!(
        xml_text(&body, "LogicalResourceId").as_deref(),
        Some("eventful"),
        "case \"immediately after create\": the stack's logical id is its name, got {body}"
    );
    assert_eq!(
        xml_text(&body, "PhysicalResourceId").as_deref(),
        Some(stack_id.as_str()),
        "case \"immediately after create\": the stack's physical id is its stack id, got {body}"
    );
    assert!(
        xml_text(&body, "EventId").is_some(),
        "case \"immediately after create\": every event carries an EventId, got {body}"
    );

    wait_for_the_transition().await;
    call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "eventful"),
            member("TemplateBody", r#"{"Description":"v2"}"#),
        ],
    )
    .await;
    wait_for_the_transition().await;
    call(&router, "DeleteStack", &[member("StackName", "eventful")]).await;
    wait_for_the_transition().await;

    let (_, body) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", &stack_id)],
    )
    .await;
    assert_eq!(
        xml_texts(&body, "ResourceStatus"),
        vec![
            "DELETE_COMPLETE".to_string(),
            "DELETE_IN_PROGRESS".to_string(),
            "UPDATE_COMPLETE".to_string(),
            "UPDATE_IN_PROGRESS".to_string(),
            "CREATE_COMPLETE".to_string(),
            "CREATE_IN_PROGRESS".to_string(),
        ],
        "case \"the whole lifecycle\": every event, newest first, got {body}"
    );
    let event_ids = xml_texts(&body, "EventId");
    let mut unique = event_ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        event_ids.len(),
        "case \"the whole lifecycle\": every EventId is distinct, got {event_ids:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let router = router();
    create_stack(&router, "paged", "{}", &[]).await;

    for action in ["DescribeStacks", "ListStacks", "DescribeStackEvents"] {
        let mut members = vec![member("NextToken", "not-a-token-we-issued")];
        if action == "DescribeStackEvents" {
            members.push(member("StackName", "paged"));
        }
        let (status, body) = call(&router, action, &members).await;
        assert_eq!(
            status, 400,
            "case {action:?}: a token this service never issued is refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {action:?}: expected ValidationError, got {body}"
        );
    }
}

/// A JSON template body of exactly `bytes` bytes: `{"Description":"<pad>"}`, whose
/// wrapper is 18 bytes (`{"Description":"` plus `"}`). `pad` must be one ASCII byte.
fn template_of_size(bytes: usize, pad: &str) -> String {
    const WRAPPER_BYTES: usize = 18;
    assert_eq!(pad.len(), 1, "the padding must be a single-byte string");
    format!(
        "{{\"Description\":\"{}\"}}",
        pad.repeat(bytes - WRAPPER_BYTES)
    )
}

#[tokio::test]
async fn a_stored_template_body_is_bounded_by_its_documented_maximum() {
    let router = router();

    // `CreateStackInput$TemplateBody`: "a minimum length of 1 byte and a maximum length of
    // 51,200 bytes". The published bound and one byte past it, on create and on update
    // alike (the model carries no `@length` maximum, so the handler is what enforces it).
    let cases: Vec<(&str, usize, bool)> = vec![
        ("a small body", 64, true),
        ("at the published maximum", TEMPLATE_BODY_MAX_BYTES, true),
        (
            "one byte past the published maximum",
            TEMPLATE_BODY_MAX_BYTES + 1,
            false,
        ),
    ];

    for (case, size, within_the_bound) in cases {
        let name = format!("sized-{size}");
        let (status_code, body) =
            create_stack(&router, &name, &template_of_size(size, "d"), &[]).await;
        if within_the_bound {
            assert_eq!(
                status_code, 200,
                "case {case:?}: a {size}-byte body is within the bound, got {status_code} {body}"
            );
            wait_for_the_transition().await;
            let (update_status, update_body) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", &name),
                    member("TemplateBody", &template_of_size(size, "e")),
                ],
            )
            .await;
            assert_eq!(
                update_status, 200,
                "case {case:?}: UpdateStack accepts the same {size}-byte body, got \
                 {update_status} {update_body}"
            );
        } else {
            assert_eq!(
                status_code, 400,
                "case {case:?}: a {size}-byte body is past the bound, got {status_code} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected ValidationError, got {body}"
            );
            create_stack(&router, "sized-target", "{}", &[]).await;
            wait_for_the_transition().await;
            let (update_status, update_body) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", "sized-target"),
                    member("TemplateBody", &template_of_size(size, "d")),
                ],
            )
            .await;
            assert_eq!(
                update_status, 400,
                "case {case:?}: UpdateStack refuses the same {size}-byte body, got \
                 {update_status} {update_body}"
            );
        }
    }
}

#[tokio::test]
async fn a_paginated_read_carries_one_bounded_page_and_a_token_that_resumes_after_it() {
    let router = router();
    let total = PAGE_SIZE + 3;
    for index in 0..total {
        let name = format!("page-{index:04}");
        let (status_code, body) = create_stack(&router, &name, "{}", &[]).await;
        assert_eq!(
            status_code, 200,
            "case {name:?}: the stack must be created, got {status_code} {body}"
        );
    }

    // Both `@paginated` stack reads: one response carries at most PAGE_SIZE items and a
    // token, and following the token enumerates the rest exactly once.
    for action in ["ListStacks", "DescribeStacks"] {
        let (status_code, first) = call(&router, action, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {action:?}: the first page is served, got {status_code} {first}"
        );
        let first_names = xml_texts(&first, "StackName");
        assert_eq!(
            first_names.len(),
            PAGE_SIZE,
            "case {action:?}: one page carries at most {PAGE_SIZE} items, got {}",
            first_names.len()
        );
        let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
            panic!("case {action:?}: a page with more behind it must carry a NextToken, got {first}")
        });

        let (status_code, second) = call(&router, action, &[member("NextToken", &token)]).await;
        assert_eq!(
            status_code, 200,
            "case {action:?}: the token resumes the read, got {status_code} {second}"
        );
        let second_names = xml_texts(&second, "StackName");
        assert_eq!(
            second_names.len(),
            total - PAGE_SIZE,
            "case {action:?}: the last page carries the remainder, got {second_names:?}"
        );
        assert_eq!(
            xml_text(&second, "NextToken"),
            None,
            "case {action:?}: the last page carries no NextToken, got {second}"
        );

        let mut paged = first_names.clone();
        paged.extend(second_names);
        let distinct: std::collections::BTreeSet<String> = paged.iter().cloned().collect();
        assert_eq!(
            distinct.len(),
            total,
            "case {action:?}: the pages enumerate every stack exactly once, got {} names",
            paged.len()
        );
    }

    // A cursor past the end of the store is not one this service handed out for it, and
    // neither is one issued for a different collection.
    let beyond_the_end = state::encode_next_token(state::PageOf::StackSummaries, total + 1);
    let other_collection = state::encode_next_token(state::PageOf::StackEvents, 0);
    for (case, token) in [
        ("a cursor past the end", beyond_the_end),
        ("a cursor issued for another collection", other_collection),
    ] {
        let (status_code, body) = call(&router, "ListStacks", &[member("NextToken", &token)]).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: refused, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn create_stack_refuses_a_stack_past_the_stores_stack_quota() {
    let (router, engine) = router_with_engine();
    let now = SystemTime::now();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..MAX_STACKS {
            let name = format!("filler-{index:05}");
            held.stacks.push(StoredStack::new(
                format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{name}/{index:05}"),
                name,
                Some("{}".to_string()),
                now,
            ));
        }
    }

    let (status_code, body) = create_stack(&router, "one-too-many", "{}", &[]).await;
    assert_eq!(
        status_code, 400,
        "case \"the store is at its stack quota\": the create is refused, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("LimitExceededException"),
        "case \"the store is at its stack quota\": expected LimitExceededException, got {body}"
    );
    assert_eq!(
        store(&engine).lock().stacks.len(),
        MAX_STACKS,
        "case \"the store is at its stack quota\": the refused create stored nothing"
    );
}

#[tokio::test]
async fn one_stacks_event_log_is_bounded_and_read_back_one_page_at_a_time() {
    let (router, engine) = router_with_engine();
    create_stack(&router, "bounded-events", "{}", &[]).await;

    let appended = MAX_EVENTS_PER_STACK + 25;
    {
        let state = store(&engine);
        let mut held = state.lock();
        let stack = held
            .resolve_mut("bounded-events")
            .expect("the stack just created");
        for index in 0..appended {
            stack.record_event(
                SystemTime::now(),
                "UPDATE_IN_PROGRESS",
                Some(format!("event {index}")),
                None,
                None,
            );
        }
        assert_eq!(
            stack.events.len(),
            MAX_EVENTS_PER_STACK,
            "case \"past the event bound\": the log holds at most {MAX_EVENTS_PER_STACK} events"
        );
        assert!(
            stack
                .events
                .iter()
                .all(|event| event.resource_status_reason.as_deref() != Some("event 0")),
            "case \"past the event bound\": the oldest events are the ones reclaimed"
        );
    }

    let (status_code, first) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", "bounded-events")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a long event log\": the first page is served, got {status_code} {first}"
    );
    assert_eq!(
        xml_texts(&first, "EventId").len(),
        PAGE_SIZE,
        "case \"a long event log\": one response carries at most {PAGE_SIZE} events"
    );
    let token = xml_text(&first, "NextToken")
        .unwrap_or_else(|| panic!("case \"a long event log\": expected a NextToken, got {first}"));
    let (status_code, second) = call(
        &router,
        "DescribeStackEvents",
        &[
            member("StackName", "bounded-events"),
            member("NextToken", &token),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a long event log\": the token resumes the read, got {status_code} {second}"
    );
    assert_eq!(
        xml_texts(&second, "EventId").len(),
        PAGE_SIZE,
        "case \"a long event log\": the second page is bounded too, got {second}"
    );
    let first_ids = xml_texts(&first, "EventId");
    assert!(
        xml_texts(&second, "EventId")
            .iter()
            .all(|id| !first_ids.contains(id)),
        "case \"a long event log\": the pages do not repeat an event"
    );
}

#[tokio::test]
async fn completed_deletions_are_retained_only_up_to_the_retention_bound() {
    let (router, engine) = router_with_engine();
    let retired = MAX_DELETED_STACKS_RETAINED + 10;
    let base = SystemTime::now() - Duration::from_secs(10_000);
    {
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..retired {
            let name = format!("retired-{index:05}");
            let deleted_at = base + Duration::from_secs(index as u64);
            let mut stack = StoredStack::new(
                format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{name}/{index:05}"),
                name,
                Some("{}".to_string()),
                deleted_at,
            );
            stack.status = status::DELETE_COMPLETE.to_string();
            stack.deletion_time = Some(deleted_at);
            held.stacks.push(stack);
        }
    }

    // Any call settles the store, which is also where the retention bound is enforced.
    let (status_code, body) = call(&router, "ListStacks", &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"past the retention bound\": the list is served, got {status_code} {body}"
    );
    assert_eq!(
        store(&engine).lock().stacks.len(),
        MAX_DELETED_STACKS_RETAINED,
        "case \"past the retention bound\": the store keeps at most \
         {MAX_DELETED_STACKS_RETAINED} completed deletions"
    );
    let kept: Vec<String> = store(&engine)
        .lock()
        .stacks
        .iter()
        .map(|stack| stack.stack_name.clone())
        .collect();
    assert!(
        !kept.contains(&"retired-00000".to_string()),
        "case \"past the retention bound\": the oldest deletion is reclaimed first"
    );
    assert!(
        kept.contains(&format!("retired-{:05}", retired - 1)),
        "case \"past the retention bound\": the newest deletion is retained, got {kept:?}"
    );
}

#[tokio::test]
async fn a_stack_policy_round_trips_and_a_policy_url_is_answered_501() {
    let router = router();

    // The class: any policy document a caller supplies on create or update is the one
    // `GetStackPolicy` reports back; a stack with no policy reports none ("If a stack
    // doesn't have a policy, a null value is returned").
    let created = r#"{"Statement":[{"Effect":"Deny","Action":"Update:*","Principal":"*","Resource":"*"}]}"#;
    let updated = r#"{"Statement":[{"Effect":"Allow","Action":"Update:Modify","Principal":"*","Resource":"*"}]}"#;

    let (status_code, body) = create_stack(
        &router,
        "policed",
        "{}",
        &[member("StackPolicyBody", created)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"create with a policy\": expected success, got {status_code} {body}"
    );
    let (status_code, body) = call(&router, "GetStackPolicy", &[member("StackName", "policed")]).await;
    assert_eq!(
        status_code, 200,
        "case \"create with a policy\": GetStackPolicy answers, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackPolicyBody").as_deref(),
        Some(created),
        "case \"create with a policy\": the stored policy is answered verbatim, got {body}"
    );

    wait_for_the_transition().await;
    let (status_code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "policed"),
            member("TemplateBody", r#"{"Description":"v2"}"#),
            member("StackPolicyBody", updated),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"update with a new policy\": expected success, got {status_code} {body}"
    );
    let (_, body) = call(&router, "GetStackPolicy", &[member("StackName", "policed")]).await;
    assert_eq!(
        xml_text(&body, "StackPolicyBody").as_deref(),
        Some(updated),
        "case \"update with a new policy\": the update replaced the stored policy, got {body}"
    );

    create_stack(&router, "unpoliced", "{}", &[]).await;
    let (status_code, body) = call(
        &router,
        "GetStackPolicy",
        &[member("StackName", "unpoliced")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a stack with no policy\": GetStackPolicy answers, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackPolicyBody"),
        None,
        "case \"a stack with no policy\": no policy is reported, got {body}"
    );

    let (status_code, body) = call(
        &router,
        "GetStackPolicy",
        &[member("StackName", "no-such-stack")],
    )
    .await;
    assert_eq!(
        status_code, 400,
        "case \"a stack that does not exist\": refused, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case \"a stack that does not exist\": expected ValidationError, got {body}"
    );

    // A policy named by URL would have to be fetched from Amazon S3, which this emulator
    // does not do: 501, the same honest answer TemplateURL gets — never a 200 that
    // discards the policy.
    struct UrlCase {
        case: &'static str,
        action: &'static str,
        members: Vec<(String, String)>,
    }
    let url_cases = vec![
        UrlCase {
            case: "CreateStack with StackPolicyURL",
            action: "CreateStack",
            members: vec![
                member("StackName", "policy-by-url"),
                member("TemplateBody", "{}"),
                member("StackPolicyURL", "https://example.s3.amazonaws.com/policy.json"),
            ],
        },
        UrlCase {
            case: "UpdateStack with StackPolicyURL",
            action: "UpdateStack",
            members: vec![
                member("StackName", "unpoliced"),
                member("TemplateBody", r#"{"Description":"v3"}"#),
                member("StackPolicyURL", "https://example.s3.amazonaws.com/policy.json"),
            ],
        },
        UrlCase {
            case: "UpdateStack with StackPolicyDuringUpdateURL",
            action: "UpdateStack",
            members: vec![
                member("StackName", "unpoliced"),
                member("TemplateBody", r#"{"Description":"v4"}"#),
                member(
                    "StackPolicyDuringUpdateURL",
                    "https://example.s3.amazonaws.com/policy.json",
                ),
            ],
        },
    ];
    for UrlCase {
        case,
        action,
        members,
    } in url_cases
    {
        let (status_code, body) = call(&router, action, &members).await;
        assert_eq!(
            status_code, 501,
            "case {case:?}: expected 501 NotImplemented, got {status_code} {body}"
        );
    }
}
