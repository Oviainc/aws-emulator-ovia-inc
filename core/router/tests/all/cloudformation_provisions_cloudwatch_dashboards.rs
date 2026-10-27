// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::CloudWatch::Dashboard` its template declares — in CloudWatch.
//!
//! The rule under test: **a stack creates every resource its `Resources` section declares
//! through the service that OWNS the type, so after the create completes that service's own
//! read answers for it with the configuration the template declared and the tags
//! CloudFormation gives it, a later deployment that changes either is applied through the
//! same service, and deleting the stack deletes it there.** Here the owning call is
//! CloudWatch's `PutDashboard` ("Creates a dashboard if it does not already exist, or
//! updates an existing dashboard"), the read is `GetDashboard`, which models
//! `DashboardNotFoundError` only for a dashboard that does not exist, and the tags travel on
//! that create and through `TagResource`/`UntagResource` on an update, since `PutDashboard`'s
//! own `Tags` member is documented as ignored when the call updates a dashboard that exists.
//! Two crates have to agree for that to hold (CloudFormation makes the calls, CloudWatch
//! serves them), so the suite lives here: a crate-local test can only ever see one side of
//! the call.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! dashboard or leaves the name to CloudFormation, how long that name is, the shape and size
//! of the `DashboardBody` document (an empty object, one widget, many widgets, a text widget
//! carrying non-ASCII markdown), whether the body is declared as the JSON TEXT of a document
//! or as an inline template document, and how many tags the stack carries. Every expected
//! value is computed here from what the row declares, never read back out of the answer being
//! checked.
//!
//! The last two suites here add the other half of the same rule: what the stack creates is
//! the resource's declaration EVALUATED against the stack's own parameters, so a dashboard
//! whose name, body or tag value is stated through a parameter is created under the value
//! that parameter resolves to, and changing a parameter is applied as the same kind of change
//! a literal rewrite of the template would be.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The test crate's one awsQuery encoder and element readers, and its SigV4 header builder:
// a suite that writes its own drifts from the others, so this one calls them.
use crate::query_wire::{elements, form_encode, opt_element};
use crate::rds_docdb_neptune_signing_name_routing::authorization;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};

/// The region every request here is signed for, and so the region the stack belongs to.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's own clock-owed span to come due. A dashboard has no transient
/// state of its own: `PutDashboard` stores it outright.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// CloudWatch's `X-Amz-Target` prefix, from its model's service shape
/// (`GraniteServiceVersion20100801`), and the signing name its credential scope carries
/// (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The characters a dashboard name may carry: "valid characters are A-Z, a-z, 0-9, \"-\",
/// and \"_\"" (`PutDashboardInput$DashboardName`).
fn is_dashboard_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_'
}

/// The longest a dashboard name may be: "The maximum length is 255"
/// (`PutDashboardInput$DashboardName`).
const MAX_DASHBOARD_NAME_LEN: usize = 255;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call one CloudFormation operation over its awsQuery wire. The form components are encoded
/// by the test crate's one awsQuery encoder, so a dashboard document full of `{`, `"` and `&`
/// survives the body.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
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
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudWatch operation over its awsJson1_0 wire.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{CLOUDWATCH_SIGNING_NAME}.{REGION}.amazonaws.com"),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header("authorization", authorization(CLOUDWATCH_SIGNING_NAME))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code of a CloudWatch refusal: awsJson1_0 carries it as the body's `__type`.
fn cloudwatch_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The dashboard CloudWatch holds under `name`, requiring the read to succeed.
async fn get_dashboard(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, parsed) = cloudwatch(router, "GetDashboard", &json!({"DashboardName": name})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetDashboard for {name:?}, got {status} with \
         body {parsed}"
    );
    parsed
}

/// The tags CloudWatch holds for the resource `arn` names, as the key/value map a client
/// reads them as.
async fn resource_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, parsed) =
        cloudwatch(router, "ListTagsForResource", &json!({"ResourceARN": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource for {arn:?}, got {status} \
         with body {parsed}"
    );
    parsed
        .get("Tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    Some((
                        tag.get("Key")?.as_str()?.to_string(),
                        tag.get("Value")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ARN a dashboard of `name` carries in this emulator:
/// `arn:aws:cloudwatch::<account>:dashboard/<name>`. The Region segment is EMPTY because
/// "All dashboards in your account are global, not region-specific." (`PutDashboard`).
fn dashboard_arn(name: &str) -> String {
    format!("arn:aws:cloudwatch::{ACCOUNT_ID}:dashboard/{name}")
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The tags a dashboard of `logical_id` in this stack must end up carrying: the ones its own
/// declaration asks for, then the stack-level ones under every key that declaration does not
/// use, plus the three CloudFormation records itself — which are CloudFormation's to set, so
/// a declaration spelling one of them does not displace it.
fn expected_tags(
    stack_name: &str,
    stack_id: &str,
    logical_id: &str,
    stack_tags: &[(&str, &str)],
    declared_tags: &[(&str, &str)],
) -> BTreeMap<String, String> {
    let mut tags: BTreeMap<String, String> = stack_tags
        .iter()
        .chain(declared_tags)
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    tags.insert(
        "aws:cloudformation:stack-name".to_string(),
        stack_name.to_string(),
    );
    tags.insert(
        "aws:cloudformation:stack-id".to_string(),
        stack_id.to_string(),
    );
    tags.insert(
        "aws:cloudformation:logical-id".to_string(),
        logical_id.to_string(),
    );
    tags
}

/// The stack-level `Tags` of a stack request, as the awsQuery form members that carry them.
fn tag_members(stack_tags: &[(&str, &str)]) -> Vec<(String, String)> {
    stack_tags
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let ordinal = index + 1;
            [
                (
                    format!("Tags.member.{ordinal}.Key"),
                    (*key).to_string(),
                ),
                (
                    format!("Tags.member.{ordinal}.Value"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect()
}

/// One stack request's members: the stack name, the template and the stack's tags.
fn stack_request<'a>(
    stack_name: &'a str,
    template: &'a str,
    tags: &'a [(String, String)],
) -> Vec<(&'a str, &'a str)> {
    let mut members: Vec<(&str, &str)> = vec![
        ("StackName", stack_name),
        ("TemplateBody", template),
    ];
    for (key, value) in tags {
        members.push((key.as_str(), value.as_str()));
    }
    members
}

/// A template declaring ONE dashboard under `logical_id`, with an output resolving that
/// resource's `Ref` — which for this type is the dashboard's name.
fn template(logical_id: &str, properties: &Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "one CloudWatch dashboard",
        "Resources": {
            logical_id: {
                "Type": "AWS::CloudWatch::Dashboard",
                "Properties": properties,
            },
        },
        "Outputs": {"DashboardName": {"Value": {"Ref": logical_id}}},
    })
    .to_string()
}

/// A dashboard document holding `count` metric widgets, for the rows that vary the body's
/// size and structure.
fn body_with_widgets(count: usize) -> String {
    let widgets: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "type": "metric",
                "x": 0,
                "y": index * 6,
                "width": 12,
                "height": 6,
                "properties": {
                    "metrics": [["MyApp", format!("Latency{index}")]],
                    "period": 300,
                    "stat": "Average",
                    "region": REGION,
                },
            })
        })
        .collect();
    serde_json::to_string(&json!({"widgets": widgets})).expect("the body serializes")
}

/// Create a stack, wait past its clock-owed span, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    stack_tags: &[(&str, &str)],
) -> String {
    let tags = tag_members(stack_tags);
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &stack_request(stack_name, template, &tags),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    describe_stack(router, case, stack_name).await
}

/// Update a stack, wait past its clock-owed span, and answer the `DescribeStacks` body.
async fn update_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    stack_tags: &[(&str, &str)],
) -> String {
    let tags = tag_members(stack_tags);
    let (status, body) = cloudformation(
        router,
        "UpdateStack",
        &stack_request(stack_name, template, &tags),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    describe_stack(router, case, stack_name).await
}

/// The one stack `stack_name` names, as the response body.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// One row of the table: a template's dashboard declaration, and what the rule says
/// CloudWatch must then hold.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    logical_id: &'static str,
    /// The `Properties` the template declares for the dashboard.
    properties: fn() -> Value,
    /// The name the template declares, or `None` where CloudFormation generates it.
    declared_name: Option<String>,
    /// The document `GetDashboard` must report, as the JSON text of one.
    expected_body: fn() -> String,
    /// Whether the declaration states the body as TEXT, in which case the stored document
    /// has to come back character for character rather than merely as the same document.
    body_declared_as_text: bool,
    /// The stack-level tags the stack operations carry.
    stack_tags: &'static [(&'static str, &'static str)],
    /// The tags the dashboard's OWN declaration asks for, which its `Tags` property carries.
    declared_tags: &'static [(&'static str, &'static str)],
}

/// Every form of dashboard declaration round-trips: the stack creates it in CloudWatch under
/// the name its `Ref` resolves to, carrying the document and the tags CloudFormation gives
/// it, and deleting the stack deletes it there.
#[tokio::test]
async fn every_declared_dashboard_is_created_in_cloudwatch_and_deleted_with_the_stack() {
    let longest_name = "n".repeat(MAX_DASHBOARD_NAME_LEN);
    let rows: Vec<Row> = vec![
        Row {
            case: "a named dashboard with an empty document",
            stack_name: "cwdash-empty",
            logical_id: "Dashboard",
            properties: || {
                json!({"DashboardName": "cwdash-empty-board", "DashboardBody": "{}"})
            },
            declared_name: Some("cwdash-empty-board".to_string()),
            expected_body: || "{}".to_string(),
            body_declared_as_text: true,
            stack_tags: &[("team", "platform")],
            declared_tags: &[],
        },
        Row {
            case: "a name using every documented character class, and one widget",
            stack_name: "cwdash-one-widget",
            logical_id: "Prod_Dashboard42",
            properties: || {
                json!({
                    "DashboardName": "Prod-App_Dashboard-42",
                    "DashboardBody": body_with_widgets(1),
                })
            },
            declared_name: Some("Prod-App_Dashboard-42".to_string()),
            expected_body: || body_with_widgets(1),
            body_declared_as_text: true,
            stack_tags: &[("team", "platform"), ("env", "staging")],
            declared_tags: &[],
        },
        Row {
            case: "a document with many widgets",
            stack_name: "cwdash-many-widgets",
            logical_id: "Fleet",
            properties: || {
                json!({
                    "DashboardName": "cwdash-many-widgets-board",
                    "DashboardBody": body_with_widgets(40),
                })
            },
            declared_name: Some("cwdash-many-widgets-board".to_string()),
            expected_body: || body_with_widgets(40),
            body_declared_as_text: true,
            stack_tags: &[],
            declared_tags: &[],
        },
        Row {
            case: "a text widget carrying non-ASCII markdown",
            stack_name: "cwdash-text-widget",
            logical_id: "Notes",
            properties: || {
                json!({
                    "DashboardName": "cwdash-text-widget-board",
                    "DashboardBody": text_widget_body(),
                })
            },
            declared_name: Some("cwdash-text-widget-board".to_string()),
            expected_body: text_widget_body,
            body_declared_as_text: true,
            stack_tags: &[("owner", "sre")],
            declared_tags: &[],
        },
        Row {
            case: "a body declared as an inline template document rather than as text",
            stack_name: "cwdash-inline-body",
            logical_id: "Inline",
            properties: || {
                json!({
                    "DashboardName": "cwdash-inline-board",
                    "DashboardBody": {"widgets": [], "periodOverride": "auto"},
                })
            },
            declared_name: Some("cwdash-inline-board".to_string()),
            expected_body: || {
                serde_json::to_string(&json!({"periodOverride": "auto", "widgets": []}))
                    .expect("the body serializes")
            },
            body_declared_as_text: false,
            stack_tags: &[("team", "data")],
            declared_tags: &[],
        },
        Row {
            case: "a 255-character name, the documented maximum",
            stack_name: "cwdash-longest-name",
            logical_id: "Longest",
            properties: move || {
                json!({
                    "DashboardName": "n".repeat(MAX_DASHBOARD_NAME_LEN),
                    "DashboardBody": body_with_widgets(2),
                })
            },
            declared_name: Some(longest_name),
            expected_body: || body_with_widgets(2),
            body_declared_as_text: true,
            stack_tags: &[("team", "platform")],
            declared_tags: &[],
        },
        Row {
            case: "a dashboard CloudFormation names, the template declaring none",
            stack_name: "cwdash-generated-name",
            logical_id: "Unnamed",
            properties: || json!({"DashboardBody": body_with_widgets(1)}),
            declared_name: None,
            expected_body: || body_with_widgets(1),
            body_declared_as_text: true,
            stack_tags: &[("team", "platform")],
            declared_tags: &[],
        },
        Row {
            case: "a declaration carrying tags of its own, one of them the stack's key",
            stack_name: "cwdash-declared-tags",
            logical_id: "Tagged",
            properties: || {
                json!({
                    "DashboardName": "cwdash-declared-tags-board",
                    "DashboardBody": body_with_widgets(1),
                    "Tags": [
                        {"Key": "team", "Value": "payments"},
                        {"Key": "tier", "Value": "gold"},
                    ],
                })
            },
            declared_name: Some("cwdash-declared-tags-board".to_string()),
            expected_body: || body_with_widgets(1),
            body_declared_as_text: true,
            // The declaration is the more specific statement about this one resource, so its
            // `team` wins over the stack's.
            stack_tags: &[("team", "platform"), ("env", "staging")],
            declared_tags: &[("team", "payments"), ("tier", "gold")],
        },
    ];

    for row in &rows {
        let router = build_router();
        let case = row.case;
        let properties = (row.properties)();
        let body = create_and_settle(
            &router,
            case,
            row.stack_name,
            &template(row.logical_id, &properties),
            row.stack_tags,
        )
        .await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );

        // `Ref` for this type is the dashboard's name, so the output is the handle a client
        // reads the deployment through — and the name CloudWatch must answer for.
        let reported_name = outputs_of(&body)
            .get("DashboardName")
            .cloned()
            .unwrap_or_default();
        match &row.declared_name {
            Some(declared) => assert_eq!(
                &reported_name, declared,
                "case {case:?}: expected Ref to resolve to the declared name, got \
                 {reported_name:?}"
            ),
            None => {
                // A generated name is `<stack>-<logical id>-<unique>` fitted to the owning
                // service's own rule for the member the resource is created under.
                assert!(
                    reported_name.starts_with(row.stack_name)
                        && reported_name.contains(row.logical_id)
                        && reported_name.chars().count() <= MAX_DASHBOARD_NAME_LEN
                        && reported_name.chars().all(is_dashboard_name_character),
                    "case {case:?}: expected a generated name naming the stack and the \
                     resource and inside CloudWatch's own rule, got {reported_name:?}"
                );
            }
        }

        let reported = get_dashboard(&router, case, &reported_name).await;
        assert_eq!(
            reported.get("DashboardName").and_then(Value::as_str),
            Some(reported_name.as_str()),
            "case {case:?}: expected the dashboard to answer under the name the stack \
             published, got {reported}"
        );
        assert_eq!(
            reported.get("DashboardArn").and_then(Value::as_str),
            Some(dashboard_arn(&reported_name).as_str()),
            "case {case:?}: expected the global dashboard ARN of that name, got {reported}"
        );
        let expected_body = (row.expected_body)();
        let actual_body = reported
            .get("DashboardBody")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            serde_json::from_str::<Value>(&actual_body).ok(),
            serde_json::from_str::<Value>(&expected_body).ok(),
            "case {case:?}: expected the declared document, got {actual_body:?}"
        );
        if row.body_declared_as_text {
            assert_eq!(
                actual_body, expected_body,
                "case {case:?}: a body declared as text must come back character for \
                 character, got {actual_body:?}"
            );
        }

        let stack_id = opt_element(&body, "StackId").unwrap_or_default();
        let tags = resource_tags(&router, case, &dashboard_arn(&reported_name)).await;
        assert_eq!(
            tags,
            expected_tags(
                row.stack_name,
                &stack_id,
                row.logical_id,
                row.stack_tags,
                row.declared_tags,
            ),
            "case {case:?}: expected the dashboard to carry the tags CloudFormation gives \
             it, got {tags:?}"
        );

        let (status, deleted) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, gone) = cloudwatch(
            &router,
            "GetDashboard",
            &json!({"DashboardName": reported_name}),
        )
        .await;
        assert_eq!(
            (status, cloudwatch_error_code(&gone).as_str()),
            (404, "DashboardNotFoundError"),
            "case {case:?}: expected the stack's deletion to delete the dashboard in \
             CloudWatch, got {status} with {gone}"
        );
    }
}

/// The body of the text-widget row, kept in one place so the declaration and the expectation
/// cannot drift.
fn text_widget_body() -> String {
    serde_json::to_string(&json!({"widgets": [{
        "type": "text",
        "x": 0,
        "y": 0,
        "width": 24,
        "height": 2,
        "properties": {"markdown": "# Übersicht — 95ᵗʰ ≤ 200 ms ✅"},
    }]}))
    .expect("the body serializes")
}

/// One row of the update table: the case, the `DashboardBody` this deployment declares, the
/// `Tags` the dashboard's own declaration carries, and the stack-level tags.
type Change = (
    &'static str,
    Value,
    &'static [(&'static str, &'static str)],
    &'static [(&'static str, &'static str)],
);

/// The `Tags` property a declaration states as the `[{Key, Value}]` list the type publishes.
fn tags_property(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// A deployment that declares a DIFFERENT document, or different stack tags, for a dashboard
/// the stack already has reaches CloudWatch as that change: the dashboard keeps its name and
/// its ARN, its contents are replaced, and its tags are brought back into step.
#[tokio::test]
async fn a_changed_dashboard_declaration_reaches_cloudwatch() {
    let router = build_router();
    let stack_name = "cwdash-updates";
    let logical_id = "Dashboard";
    let dashboard = "cwdash-updates-board";
    let declaration = |body: &Value, declared_tags: &[(&str, &str)]| {
        json!({
            "DashboardName": dashboard,
            "DashboardBody": body.clone(),
            "Tags": tags_property(declared_tags),
        })
    };

    let first = Value::String("{\"widgets\":[]}".to_string());
    let created = create_and_settle(
        &router,
        "the first deployment",
        stack_name,
        &template(logical_id, &declaration(&first, &[("tier", "gold")])),
        &[("team", "platform"), ("env", "staging")],
    )
    .await;
    assert_eq!(
        opt_element(&created, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "the first deployment: expected the stack to complete, got {created}"
    );
    let stack_id = opt_element(&created, "StackId").unwrap_or_default();

    // Rows differ along what the update changes: the document only, the stack's tags only,
    // the declaration's own tags only, several at once, tags dropped, and a document that
    // grows and then shrinks again.
    let changes: Vec<Change> = vec![
        (
            "one widget added to the document",
            Value::String(body_with_widgets(1)),
            &[("tier", "gold")],
            &[("team", "platform"), ("env", "staging")],
        ),
        (
            "a stack tag re-valued, the document unchanged",
            Value::String(body_with_widgets(1)),
            &[("tier", "gold")],
            &[("team", "data"), ("env", "staging")],
        ),
        (
            "the declaration's own tag re-valued, nothing else changed",
            Value::String(body_with_widgets(1)),
            &[("tier", "silver")],
            &[("team", "data"), ("env", "staging")],
        ),
        (
            "a bigger document, a stack tag added and a declared tag dropped at once",
            Value::String(body_with_widgets(6)),
            &[],
            &[("team", "data"), ("env", "prod"), ("owner", "sre")],
        ),
        (
            "the declaration overriding the stack's own key",
            Value::String(body_with_widgets(6)),
            &[("team", "payments")],
            &[("team", "data"), ("env", "prod"), ("owner", "sre")],
        ),
        (
            "two stack tags dropped and the document shrunk back",
            Value::String("{\"widgets\":[]}".to_string()),
            &[("team", "payments")],
            &[("env", "prod")],
        ),
        (
            "the body restated as an inline template document",
            json!({"widgets": []}),
            &[("team", "payments")],
            &[("env", "prod")],
        ),
    ];

    for (case, body, declared_tags, stack_tags) in changes {
        let described = update_and_settle(
            &router,
            case,
            stack_name,
            &template(logical_id, &declaration(&body, declared_tags)),
            stack_tags,
        )
        .await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the update to complete, got {described}"
        );

        let reported = get_dashboard(&router, case, dashboard).await;
        let expected = match &body {
            Value::String(text) => text.clone(),
            document => serde_json::to_string(document).expect("the body serializes"),
        };
        let actual = reported
            .get("DashboardBody")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            serde_json::from_str::<Value>(&actual).ok(),
            serde_json::from_str::<Value>(&expected).ok(),
            "case {case:?}: expected the document the update declared, got {actual:?}"
        );
        // The dashboard was updated in place, so it keeps the identity the stack published
        // for it.
        assert_eq!(
            reported.get("DashboardArn").and_then(Value::as_str),
            Some(dashboard_arn(dashboard).as_str()),
            "case {case:?}: expected the dashboard to keep its ARN, got {reported}"
        );

        let tags = resource_tags(&router, case, &dashboard_arn(dashboard)).await;
        assert_eq!(
            tags,
            expected_tags(stack_name, &stack_id, logical_id, stack_tags, declared_tags),
            "case {case:?}: expected the dashboard's tags to be the ones this deployment \
             gives it, got {tags:?}"
        );
    }
}

/// A deployment that declares a DIFFERENT `DashboardName` for the same logical resource
/// REPLACES the dashboard: the new name is created in CloudWatch and the displaced one is
/// deleted there, so no dashboard the stack no longer declares is left behind.
#[tokio::test]
async fn a_renamed_dashboard_replaces_the_one_it_displaces() {
    let router = build_router();
    let stack_name = "cwdash-renames";
    let logical_id = "Dashboard";
    let declaration = |name: &str| {
        json!({"DashboardName": name, "DashboardBody": body_with_widgets(1)})
    };

    let created = create_and_settle(
        &router,
        "the first deployment",
        stack_name,
        &template(logical_id, &declaration("cwdash-renames-first")),
        &[("team", "platform")],
    )
    .await;
    assert_eq!(
        opt_element(&created, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "the first deployment: expected the stack to complete, got {created}"
    );

    // Rows differ along the name the deployment moves TO: another ordinary name, one using
    // the other documented separator, and the longest a name may be.
    let renames: Vec<(&str, String)> = vec![
        (
            "a plainly different name",
            "cwdash-renames-second".to_string(),
        ),
        (
            "a name using the underscore separator",
            "cwdash_renames_3".to_string(),
        ),
        (
            "the longest name a dashboard may carry",
            "r".repeat(MAX_DASHBOARD_NAME_LEN),
        ),
    ];
    let mut previous = "cwdash-renames-first".to_string();
    for (case, next) in renames {
        let described = update_and_settle(
            &router,
            case,
            stack_name,
            &template(logical_id, &declaration(&next)),
            &[("team", "platform")],
        )
        .await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the update to complete, got {described}"
        );
        assert_eq!(
            outputs_of(&described).get("DashboardName").map(String::as_str),
            Some(next.as_str()),
            "case {case:?}: expected Ref to resolve to the name now declared, got {described}"
        );

        let reported = get_dashboard(&router, case, &next).await;
        assert_eq!(
            reported.get("DashboardArn").and_then(Value::as_str),
            Some(dashboard_arn(&next).as_str()),
            "case {case:?}: expected the new dashboard to carry its own ARN, got {reported}"
        );

        let (status, gone) = cloudwatch(
            &router,
            "GetDashboard",
            &json!({"DashboardName": &previous}),
        )
        .await;
        assert_eq!(
            (status, cloudwatch_error_code(&gone).as_str()),
            (404, "DashboardNotFoundError"),
            "case {case:?}: expected the displaced dashboard {previous:?} to be deleted, got \
             {status} with {gone}"
        );
        previous = next;
    }
}

/// One row of the refusal table: the case, the stack's name, the dashboard's declared
/// `Properties`, the fragment of CloudWatch's own message the failing resource's event must
/// carry, and the `(status, code)` the read-back then answers.
type Refused = (
    &'static str,
    &'static str,
    Value,
    &'static str,
    (u16, &'static str),
);

/// A dashboard CloudWatch REFUSES to create fails the stack with the refusal that service
/// answered, and nothing is created — never a stack that completed while no dashboard exists.
///
/// Where each half of that failure is reported is part of the rule: the STACK's own
/// `StackStatusReason` is the summary sentence naming the logical resources the operation
/// could not create and nothing more, while CloudWatch's own refusal is the failing
/// resource's diagnosis and travels on that resource's `StackEvent$ResourceStatusReason`.
#[tokio::test]
async fn a_dashboard_cloudwatch_refuses_fails_the_stack() {
    // Rows differ along WHICH of the operation's modelled refusals the declaration earns:
    // a body that is not a dashboard document at all, and a name outside the documented
    // character set. The last column is what the read-back then answers — the 404 for a name
    // that could have named a dashboard, and `GetDashboard`'s own 400 for one no dashboard
    // could ever be stored under.
    let rows: Vec<Refused> = vec![
        (
            "a body that is not a JSON object",
            "cwdash-bad-body",
            json!({
                "DashboardName": "cwdash-bad-body-board",
                "DashboardBody": "not a dashboard document",
            }),
            "The field DashboardBody must be a valid JSON object",
            (404, "DashboardNotFoundError"),
        ),
        (
            "a name carrying a character the rule does not admit",
            "cwdash-bad-name",
            json!({"DashboardName": "cwdash.bad.name", "DashboardBody": "{}"}),
            "The value for field DashboardName contains invalid characters",
            (400, "InvalidParameterValueException"),
        ),
    ];

    for (case, stack_name, properties, refusal, read_back) in rows {
        let router = build_router();
        let body = create_and_settle(
            &router,
            case,
            stack_name,
            &template("Dashboard", &properties),
            &[("team", "platform")],
        )
        .await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back rather than report \
             a dashboard nothing holds, got {body}"
        );
        let reason = opt_element(&body, "StackStatusReason").unwrap_or_default();
        assert_eq!(
            reason,
            resource_failure_summary(failed_action::CREATE, ["Dashboard"]),
            "case {case:?}: expected the stack reason to be the summary of which resources \
             failed, got {reason:?}"
        );
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
        assert!(
            elements(&events, "ResourceStatusReason")
                .iter()
                .any(|reported| reported.contains(refusal)),
            "case {case:?}: expected the failing resource's own event to carry CloudWatch's \
             refusal {refusal:?}, got {events}"
        );

        let name = properties
            .get("DashboardName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (status, gone) =
            cloudwatch(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        assert_eq!(
            (status, cloudwatch_error_code(&gone).as_str()),
            read_back,
            "case {case:?}: expected no dashboard of that name, got {status} with {gone}"
        );
    }
}

/// The `Parameters.member.N.ParameterKey` / `.ParameterValue` pairs a stack operation carries
/// for the parameter values a row supplies, as an SDK flattens the list.
fn parameter_members(parameters: &[(&str, &str)]) -> Vec<(String, String)> {
    parameters
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let ordinal = index + 1;
            [
                (
                    format!("Parameters.member.{ordinal}.ParameterKey"),
                    (*key).to_string(),
                ),
                (
                    format!("Parameters.member.{ordinal}.ParameterValue"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect()
}

/// A template declaring a `Parameters` section and ONE dashboard whose properties are stated
/// through those parameters, with an output resolving the resource's `Ref`.
fn parameterised_template(logical_id: &str, declarations: &Value, properties: &Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "one CloudWatch dashboard declared through the stack's parameters",
        "Parameters": declarations.clone(),
        "Resources": {
            logical_id: {
                "Type": "AWS::CloudWatch::Dashboard",
                "Properties": properties.clone(),
            },
        },
        "Outputs": {"DashboardName": {"Value": {"Ref": logical_id}}},
    })
    .to_string()
}

/// Run one stack operation that supplies parameter values, wait past the stack's clock-owed
/// span, and answer the `DescribeStacks` body.
async fn deploy_with_parameters(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    stack_name: &str,
    template: &str,
    stack_tags: &[(&str, &str)],
    parameters: &[(&str, &str)],
) -> String {
    let mut members = tag_members(stack_tags);
    members.extend(parameter_members(parameters));
    let (status, body) = cloudformation(
        router,
        action,
        &stack_request(stack_name, template, &members),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    describe_stack(router, case, stack_name).await
}

/// The dashboard document the `Fn::Sub` row declares, as the template states it: one text
/// widget whose markdown is a variable the stack's parameters supply.
const SUBSTITUTED_BODY: &str =
    r#"{"widgets":[{"type":"text","properties":{"markdown":"${Title}"}}]}"#;

/// The variable that body names, and the value its row's parameter gives it — so the
/// declaration and the expectation are built from the one pair and cannot drift.
const SUBSTITUTED_BODY_VARIABLE: &str = "${Title}";
const SUBSTITUTED_BODY_TITLE: &str = "Requests";

/// That same document with the variable substituted: what `Fn::Sub` resolves the declared
/// body to, and so what CloudWatch must hold.
fn substituted_body() -> String {
    SUBSTITUTED_BODY.replace(SUBSTITUTED_BODY_VARIABLE, SUBSTITUTED_BODY_TITLE)
}

/// One row of the parameter table: how the declaration reaches a parameter, and what the
/// supplied values make the dashboard.
struct ParameterisedRow {
    case: &'static str,
    stack_name: &'static str,
    logical_id: &'static str,
    /// The template's own `Parameters` section.
    declarations: fn() -> Value,
    /// The parameter values the stack operation supplies, which a row may leave empty where
    /// the declaration carries a `Default`.
    parameters: &'static [(&'static str, &'static str)],
    /// The dashboard's `Properties`, stating its name, body or tags through those parameters.
    properties: fn() -> Value,
    /// The name those values resolve the declaration to.
    expected_name: &'static str,
    /// The document text they resolve its body to.
    expected_body: fn() -> String,
    /// The stack-level tags the stack operation carries.
    stack_tags: &'static [(&'static str, &'static str)],
    /// The tags the dashboard's own `Tags` property resolves to.
    expected_declared_tags: &'static [(&'static str, &'static str)],
}

/// A dashboard whose declaration is stated through the stack's PARAMETERS is created in
/// CloudWatch under the values those parameters resolve to — the name its `Ref` publishes,
/// the document it holds and the tags it carries — and deleting the stack deletes it there.
#[tokio::test]
async fn a_dashboard_declared_through_the_stacks_parameters_is_created_under_their_values() {
    // Rows differ along HOW a parameter reaches the declaration: a `Ref` straight to one, a
    // name joined out of one, a value the request omits so the declaration's own `Default`
    // supplies it, a name selected out of a list-typed one, and a body substituted into by
    // `Fn::Sub` — and along WHICH property reads it (the name, the body, a tag's value).
    let rows: Vec<ParameterisedRow> = vec![
        ParameterisedRow {
            case: "a name and a tag value each a Ref to a parameter",
            stack_name: "cwdash-params-ref",
            logical_id: "Dashboard",
            declarations: || {
                json!({"DashboardName": {"Type": "String"}, "Team": {"Type": "String"}})
            },
            parameters: &[
                ("DashboardName", "cwdash-params-ref-board"),
                ("Team", "platform"),
            ],
            properties: || {
                json!({
                    "DashboardName": {"Ref": "DashboardName"},
                    "DashboardBody": body_with_widgets(1),
                    "Tags": [{"Key": "team", "Value": {"Ref": "Team"}}],
                })
            },
            expected_name: "cwdash-params-ref-board",
            expected_body: || body_with_widgets(1),
            stack_tags: &[("env", "prod")],
            expected_declared_tags: &[("team", "platform")],
        },
        ParameterisedRow {
            case: "a name joined from a parameter and a literal suffix",
            stack_name: "cwdash-params-join",
            logical_id: "Joined",
            declarations: || json!({"Prefix": {"Type": "String"}}),
            parameters: &[("Prefix", "cwdash-params-join")],
            properties: || {
                json!({
                    "DashboardName": {"Fn::Join": ["-", [{"Ref": "Prefix"}, "board"]]},
                    "DashboardBody": body_with_widgets(3),
                })
            },
            expected_name: "cwdash-params-join-board",
            expected_body: || body_with_widgets(3),
            stack_tags: &[("team", "platform")],
            expected_declared_tags: &[],
        },
        ParameterisedRow {
            case: "a name from the parameter's own default, the request supplying no value",
            stack_name: "cwdash-params-default",
            logical_id: "Defaulted",
            declarations: || {
                json!({
                    "DashboardName": {
                        "Type": "String",
                        "Default": "cwdash-params-default-board",
                    },
                })
            },
            parameters: &[],
            properties: || {
                json!({"DashboardName": {"Ref": "DashboardName"}, "DashboardBody": "{}"})
            },
            expected_name: "cwdash-params-default-board",
            expected_body: || "{}".to_string(),
            stack_tags: &[("team", "platform")],
            expected_declared_tags: &[],
        },
        ParameterisedRow {
            case: "a name selected out of a list-typed parameter, and a substituted body",
            stack_name: "cwdash-params-select",
            logical_id: "Selected",
            declarations: || {
                json!({
                    "Names": {"Type": "CommaDelimitedList"},
                    "Title": {"Type": "String"},
                })
            },
            parameters: &[
                ("Names", "cwdash-params-zero,cwdash-params-one"),
                ("Title", SUBSTITUTED_BODY_TITLE),
            ],
            properties: || {
                json!({
                    "DashboardName": {"Fn::Select": ["1", {"Ref": "Names"}]},
                    "DashboardBody": {"Fn::Sub": SUBSTITUTED_BODY},
                })
            },
            expected_name: "cwdash-params-one",
            expected_body: substituted_body,
            stack_tags: &[("team", "platform"), ("env", "staging")],
            expected_declared_tags: &[],
        },
    ];

    for row in &rows {
        let router = build_router();
        let case = row.case;
        let described = deploy_with_parameters(
            &router,
            case,
            "CreateStack",
            row.stack_name,
            &parameterised_template(
                row.logical_id,
                &(row.declarations)(),
                &(row.properties)(),
            ),
            row.stack_tags,
            row.parameters,
        )
        .await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {described}"
        );
        assert_eq!(
            outputs_of(&described)
                .get("DashboardName")
                .map(String::as_str),
            Some(row.expected_name),
            "case {case:?}: expected Ref to resolve to the name the parameters make, got \
             {described}"
        );

        let reported = get_dashboard(&router, case, row.expected_name).await;
        assert_eq!(
            reported.get("DashboardName").and_then(Value::as_str),
            Some(row.expected_name),
            "case {case:?}: expected the dashboard to answer under that name, got {reported}"
        );
        assert_eq!(
            reported.get("DashboardArn").and_then(Value::as_str),
            Some(dashboard_arn(row.expected_name).as_str()),
            "case {case:?}: expected the global dashboard ARN of that name, got {reported}"
        );
        let expected_body = (row.expected_body)();
        let actual_body = reported
            .get("DashboardBody")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // Every row states its body as the JSON TEXT of a document (`Fn::Sub` resolves to
        // text too), so the stored document comes back character for character.
        assert_eq!(
            actual_body, expected_body,
            "case {case:?}: expected the document the parameters make, got {actual_body:?}"
        );

        let stack_id = opt_element(&described, "StackId").unwrap_or_default();
        let tags = resource_tags(&router, case, &dashboard_arn(row.expected_name)).await;
        assert_eq!(
            tags,
            expected_tags(
                row.stack_name,
                &stack_id,
                row.logical_id,
                row.stack_tags,
                row.expected_declared_tags,
            ),
            "case {case:?}: expected the tags the parameters and the stack give it, got \
             {tags:?}"
        );

        let (status, deleted) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, gone) = cloudwatch(
            &router,
            "GetDashboard",
            &json!({"DashboardName": row.expected_name}),
        )
        .await;
        assert_eq!(
            (status, cloudwatch_error_code(&gone).as_str()),
            (404, "DashboardNotFoundError"),
            "case {case:?}: expected the stack's deletion to delete the dashboard in \
             CloudWatch, got {status} with {gone}"
        );
    }
}

/// A deployment that changes only a parameter VALUE, the template text unchanged, reaches
/// CloudWatch as the change that value makes: a tag parameter re-tags the dashboard the stack
/// already has, in place and under the same name and ARN, while the name parameter replaces
/// it — the new dashboard created and the displaced one deleted — and the stack's deletion
/// then removes whichever one it currently has.
#[tokio::test]
async fn a_changed_parameter_value_reaches_cloudwatch_as_the_change_it_makes() {
    let router = build_router();
    let stack_name = "cwdash-param-changes";
    let logical_id = "Dashboard";
    let first = "cwdash-param-changes-first";
    let second = "cwdash-param-changes_2";
    let stack_tags: &[(&str, &str)] = &[("env", "prod")];
    let template = parameterised_template(
        logical_id,
        &json!({"DashboardName": {"Type": "String"}, "Team": {"Type": "String"}}),
        &json!({
            "DashboardName": {"Ref": "DashboardName"},
            "DashboardBody": body_with_widgets(2),
            "Tags": [{"Key": "team", "Value": {"Ref": "Team"}}],
        }),
    );

    let case = "the first deployment";
    let created = deploy_with_parameters(
        &router,
        case,
        "CreateStack",
        stack_name,
        &template,
        stack_tags,
        &[("DashboardName", first), ("Team", "platform")],
    )
    .await;
    assert_eq!(
        opt_element(&created, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {created}"
    );
    let stack_id = opt_element(&created, "StackId").unwrap_or_default();
    assert_eq!(
        outputs_of(&created).get("DashboardName").map(String::as_str),
        Some(first),
        "case {case:?}: expected Ref to resolve to the name the parameter gives, got {created}"
    );
    assert_eq!(
        resource_tags(&router, case, &dashboard_arn(first)).await,
        expected_tags(
            stack_name,
            &stack_id,
            logical_id,
            stack_tags,
            &[("team", "platform")],
        ),
        "case {case:?}: expected the tag the parameter's value makes"
    );

    // A change to the TAG parameter alone: the dashboard is the same one, re-tagged.
    let case = "a changed tag parameter";
    let retagged = deploy_with_parameters(
        &router,
        case,
        "UpdateStack",
        stack_name,
        &template,
        stack_tags,
        &[("DashboardName", first), ("Team", "payments")],
    )
    .await;
    assert_eq!(
        opt_element(&retagged, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {retagged}"
    );
    assert_eq!(
        outputs_of(&retagged)
            .get("DashboardName")
            .map(String::as_str),
        Some(first),
        "case {case:?}: expected Ref to stay the name it was, got {retagged}"
    );
    let reported = get_dashboard(&router, case, first).await;
    assert_eq!(
        reported.get("DashboardArn").and_then(Value::as_str),
        Some(dashboard_arn(first).as_str()),
        "case {case:?}: expected the same dashboard under the same ARN, got {reported}"
    );
    assert_eq!(
        reported.get("DashboardBody").and_then(Value::as_str),
        Some(body_with_widgets(2).as_str()),
        "case {case:?}: expected the document to be the one still declared, got {reported}"
    );
    assert_eq!(
        resource_tags(&router, case, &dashboard_arn(first)).await,
        expected_tags(
            stack_name,
            &stack_id,
            logical_id,
            stack_tags,
            &[("team", "payments")],
        ),
        "case {case:?}: expected the tag the parameter's new value makes"
    );

    // A change to the NAME parameter: the new dashboard is created, the displaced one deleted.
    let case = "a changed name parameter";
    let renamed = deploy_with_parameters(
        &router,
        case,
        "UpdateStack",
        stack_name,
        &template,
        stack_tags,
        &[("DashboardName", second), ("Team", "payments")],
    )
    .await;
    assert_eq!(
        opt_element(&renamed, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {renamed}"
    );
    assert_eq!(
        outputs_of(&renamed).get("DashboardName").map(String::as_str),
        Some(second),
        "case {case:?}: expected Ref to resolve to the name now parameterised, got {renamed}"
    );
    let reported = get_dashboard(&router, case, second).await;
    assert_eq!(
        reported.get("DashboardArn").and_then(Value::as_str),
        Some(dashboard_arn(second).as_str()),
        "case {case:?}: expected the new dashboard to carry its own ARN, got {reported}"
    );
    assert_eq!(
        reported.get("DashboardBody").and_then(Value::as_str),
        Some(body_with_widgets(2).as_str()),
        "case {case:?}: expected the declared document on the new dashboard, got {reported}"
    );
    let (status, gone) =
        cloudwatch(&router, "GetDashboard", &json!({"DashboardName": first})).await;
    assert_eq!(
        (status, cloudwatch_error_code(&gone).as_str()),
        (404, "DashboardNotFoundError"),
        "case {case:?}: expected the displaced dashboard {first:?} to be deleted, got {status} \
         with {gone}"
    );

    // The stack's deletion removes whichever dashboard it currently has.
    let case = "the stack's deletion";
    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (status, gone) =
        cloudwatch(&router, "GetDashboard", &json!({"DashboardName": second})).await;
    assert_eq!(
        (status, cloudwatch_error_code(&gone).as_str()),
        (404, "DashboardNotFoundError"),
        "case {case:?}: expected the dashboard the stack last declared to be deleted, got \
         {status} with {gone}"
    );
}
