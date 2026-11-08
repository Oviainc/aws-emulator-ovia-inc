// SPDX-License-Identifier: CC0-1.0

//! The log group a deployed stack DECLARES, read back through Amazon CloudWatch Logs' own
//! API, and the records the function pointed at it leaves there.
//!
//! RULE under test: **a stack that declares an `AWS::Logs::LogGroup` creates that group in
//! Amazon CloudWatch Logs — under the same name it publishes through `Ref`, with the declared
//! `RetentionInDays` applied by that service's own `PutRetentionPolicy` — before it reports
//! `CREATE_COMPLETE`, so `DescribeLogGroups` narrowed to the published name resolves exactly
//! one group whose `retentionInDays` is the declared number of days and which carries no
//! `retentionInDays` at all where the declaration states none (the group never expires); and
//! an AWS Lambda function whose `LoggingConfig.LogGroup` names that group writes the records
//! of its runs into THAT group.**
//!
//! The name a client can find the group by is the one the stack hands it, so a group created
//! under any other name is the same defect as a group created nowhere: both leave the
//! published name resolving to nothing, and every later call that takes a group as a handle
//! (`PutRetentionPolicy`, `CreateLogStream`, `PutLogEvents`, `FilterLogEvents`, a function's
//! `LoggingConfig`) answers `ResourceNotFoundException` for it.
//!
//! The rows differ along what the rule generalises over rather than restating one template:
//!
//! * where the group's NAME comes from — a template that states `LogGroupName` and one that
//!   states none, so CloudFormation generates the name and the row can only learn it from
//!   the stack's own output;
//! * the RETENTION declared — the smallest and the largest number of days the owning service
//!   publishes, and no retention at all, which is the never-expire group whose read carries
//!   no `retentionInDays` member;
//! * the SPELLING of the template — JSON with the long-form `Ref`, and YAML with the
//!   short-form `!Ref` tag.
//!
//! The two retention values are the ends of that service's own published list, quoted in
//! [`SHORTEST_RETENTION`]/[`LONGEST_RETENTION`]; no expected value here is a literal from a
//! recording — the group name each row asserts against is read out of the SAME stack's
//! outputs, and the marker each run logs is built by the row.
//!
//! Three crates have to agree for the rule to hold (CloudFormation provisions, AWS Lambda runs
//! the function under the logging configuration the stack gave it, Amazon CloudWatch Logs
//! holds the group and its events), so the suite lives here rather than in any one of them.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{form_encode, outputs_of, physical_ids_of};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The `StackStatus` a completed create reports.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The shortest and the longest retention Amazon CloudWatch Logs publishes for a log group:
/// "Possible values are: 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096,
/// 1827, 2192, 2557, 2922, 3288, and 3653" (that model's `Days` shape,
/// `smithy_query(query="doc",shape="Days")`).
const SHORTEST_RETENTION: i64 = 1;
const LONGEST_RETENTION: i64 = 3653;

/// How long one invocation of this handler is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
const POLL: Duration = Duration::from_millis(50);
const POLLS: usize = 400;

/// The fixed framing of the record the Lambda runtime writes for every run it finishes.
const REPORT_PREFIX: &str = "REPORT RequestId: ";

/// The handler every row deploys: it prints the marker the event carries and answers it, so
/// what the run left in the group can be told apart from any other run's records.
const HANDLER_SOURCE: &str = r#"
def handler(event, context):
    print("logged %s" % event["marker"])
    return {"logged": event["marker"]}
"#;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The emulator as it is ASSEMBLED, on one engine: the group the stack creates and the group
/// the function's run writes to are the same service's.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: each call carries a credential scope so the
    // router can name the target service, and the signature itself is not under test here.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answered {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        "POST",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("cloudformation")),
        ],
        Bytes::from(body),
    )
    .await
}

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await
}

/// One Amazon CloudWatch Logs call over its `awsJson1_1` wire.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> Answered {
    dispatch(
        router,
        "POST",
        &format!("logs.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("authorization", &authorization("logs")),
            ("x-amz-target", &format!("Logs_20140328.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await
}

/// Every group `DescribeLogGroups` resolves for the prefix `name` whose own `logGroupName` is
/// exactly `name`: what a client holding the name the stack published can find by it.
async fn groups_named(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<Value> {
    let answered = logs(
        router,
        "DescribeLogGroups",
        json!({ "logGroupNamePrefix": name }),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from DescribeLogGroups for {name:?}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered
        .json()
        .get("logGroups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter(|group| {
                    group.get("logGroupName").and_then(Value::as_str) == Some(name)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`, and
/// answer the configuration it reported then. The span is that service's own, so nothing here
/// sleeps a fixed span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    for _ in 0..POLLS {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        let configuration = reported.json();
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return configuration;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("case {case:?}: Lambda never reported {name} Active");
}

/// Every message CloudWatch Logs holds in `group`, polled until the group holds at least
/// `at_least` of them or the polls run out.
async fn messages_in(router: &MultiServiceRouter, group: &str, at_least: usize) -> Vec<String> {
    let mut held: Vec<String> = Vec::new();
    for _ in 0..POLLS {
        let answered = logs(router, "FilterLogEvents", json!({ "logGroupName": group })).await;
        if answered.status == 200 {
            held = answered
                .json()
                .get("events")
                .and_then(Value::as_array)
                .map(|events| {
                    events
                        .iter()
                        .filter_map(|event| event.get("message").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if held.len() >= at_least {
                return held;
            }
        }
        tokio::time::sleep(POLL).await;
    }
    held
}

/// How a row spells its template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// JSON, with the long-form `{"Ref": …}`.
    Json,
    /// YAML, with the short-form `!Ref` tag.
    Yaml,
}

/// One row: how the group is named, what retention it declares, and how the template that
/// declares it is spelled.
struct Row {
    case: &'static str,
    spelling: Spelling,
    /// The `LogGroupName` the template states, or `None` where it states none and
    /// CloudFormation names the group.
    group_name: Option<&'static str>,
    /// The `RetentionInDays` the template states, or `None` for a group that never expires.
    retention: Option<i64>,
}

const ROWS: &[Row] = &[
    Row {
        case: "a named group retained for the shortest published span, declared in JSON",
        spelling: Spelling::Json,
        group_name: Some("/declared/named-shortest-retention"),
        retention: Some(SHORTEST_RETENTION),
    },
    Row {
        case: "a group CloudFormation names, retained for the longest published span, \
               declared in YAML",
        spelling: Spelling::Yaml,
        group_name: None,
        retention: Some(LONGEST_RETENTION),
    },
    Row {
        case: "a named group declaring no retention at all, declared in JSON",
        spelling: Spelling::Json,
        group_name: Some("/declared/never-expires"),
        retention: None,
    },
];

/// The template one row deploys: the log group it declares, and the function whose
/// `LoggingConfig` points at that group by `Ref`.
fn template(row: &Row) -> String {
    match row.spelling {
        Spelling::Json => {
            let mut properties = serde_json::Map::new();
            if let Some(name) = row.group_name {
                properties.insert("LogGroupName".to_string(), json!(name));
            }
            if let Some(days) = row.retention {
                properties.insert("RetentionInDays".to_string(), json!(days));
            }
            json!({
                "Transform": "AWS::Serverless-2016-10-31",
                "Resources": {
                    "Group": {
                        "Type": "AWS::Logs::LogGroup",
                        "Properties": Value::Object(properties)
                    },
                    "Logger": {
                        "Type": "AWS::Serverless::Function",
                        "Properties": {
                            "Runtime": "python3.12",
                            "Handler": "index.handler",
                            "Timeout": FUNCTION_TIMEOUT,
                            "InlineCode": HANDLER_SOURCE,
                            "LoggingConfig": { "LogGroup": { "Ref": "Group" } }
                        }
                    }
                },
                "Outputs": {
                    "GroupName": { "Value": { "Ref": "Group" } },
                    "FunctionName": { "Value": { "Ref": "Logger" } }
                }
            })
            .to_string()
        }
        Spelling::Yaml => {
            let indented: Vec<String> = HANDLER_SOURCE
                .lines()
                .map(|line| {
                    if line.is_empty() {
                        String::new()
                    } else {
                        format!("        {line}")
                    }
                })
                .collect();
            let mut yaml = vec![
                "Transform: AWS::Serverless-2016-10-31".to_string(),
                "Resources:".to_string(),
                "  Group:".to_string(),
                "    Type: AWS::Logs::LogGroup".to_string(),
            ];
            if row.group_name.is_some() || row.retention.is_some() {
                yaml.push("    Properties:".to_string());
            }
            if let Some(name) = row.group_name {
                yaml.push(format!("      LogGroupName: {name}"));
            }
            if let Some(days) = row.retention {
                yaml.push(format!("      RetentionInDays: {days}"));
            }
            yaml.extend([
                "  Logger:".to_string(),
                "    Type: AWS::Serverless::Function".to_string(),
                "    Properties:".to_string(),
                "      Runtime: python3.12".to_string(),
                "      Handler: index.handler".to_string(),
                format!("      Timeout: {FUNCTION_TIMEOUT}"),
                "      LoggingConfig:".to_string(),
                "        LogGroup: !Ref Group".to_string(),
                "      InlineCode: |".to_string(),
            ]);
            yaml.extend(indented);
            yaml.extend([
                "Outputs:".to_string(),
                "  GroupName:".to_string(),
                "    Value: !Ref Group".to_string(),
                "  FunctionName:".to_string(),
                "    Value: !Ref Logger".to_string(),
                String::new(),
            ]);
            yaml.join("\n")
        }
    }
}

#[tokio::test]
async fn a_declared_log_group_exists_under_the_published_name_and_takes_its_functions_logs() {
    // ONE emulator for every row: each row deploys a stack of its own, so the group and the
    // function a row reads are still that row's alone.
    let router = emulator();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;

        // 1. The stack: the declared group, and the function configured to log into it.
        let stack = format!("declared-log-group-{index}");
        let body = template(row);
        let created = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &body),
                // A template declaring a macro is acknowledged with CAPABILITY_AUTO_EXPAND,
                // and the role the transform synthesizes for the function with CAPABILITY_IAM.
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {} with {:?}",
            created.status,
            created.text()
        );
        let (reached, described) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        assert_eq!(
            reached, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
        );

        // 2. What the stack published for the group: the name the client goes looking by, and
        // the physical id `Ref` resolves to, which have to be the same name.
        let outputs = outputs_of(&described);
        let published = outputs
            .get("GroupName")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the stack published no group name in {described}")
            });
        if let Some(declared) = row.group_name {
            assert_eq!(
                published, declared,
                "case {case:?}: expected the published name to be the declared one"
            );
        }
        let resources =
            cloudformation(&router, "DescribeStackResources", &[("StackName", &stack)]).await;
        let physical = physical_ids_of(&resources.text());
        let group_id = physical
            .get("Group")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no physical id for Group in {:?}", physical));
        assert_eq!(
            published, group_id,
            "case {case:?}: expected the published name to be the group's physical id"
        );

        // 3. The group exists in CloudWatch Logs under exactly that name — one group, not
        // none and not a second one under a name of the provisioner's own.
        let resolved = groups_named(&router, case, &published).await;
        assert_eq!(
            resolved.len(),
            1,
            "case {case:?}: expected DescribeLogGroups for the prefix {published:?} to resolve \
             exactly 1 group with that exact name, got {resolved:?}"
        );
        let group = &resolved[0];

        // 4. ... carrying the retention the declaration asked for, and none where it asked
        // for none: "To set a log group so that its log events do not expire, use
        // DeleteRetentionPolicy" — a never-expiring group reports no retention at all.
        let reported = group.get("retentionInDays").and_then(Value::as_i64);
        assert_eq!(
            reported, row.retention,
            "case {case:?}: expected the group {published:?} to report the declared retention \
             {:?}, got {group}",
            row.retention
        );

        // 5. The group is the one the function logs to: Lambda reports it as the function's
        // own log group, and the records of a run are in it.
        let function = outputs.get("FunctionName").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack published no function name in {described}")
        });
        let configuration = wait_until_active(&router, case, &function).await;
        assert_eq!(
            configuration
                .get("LoggingConfig")
                .and_then(|logging| logging.get("LogGroup"))
                .and_then(Value::as_str),
            Some(published.as_str()),
            "case {case:?}: expected Lambda to report the declared group as the function's log \
             group, got {configuration}"
        );
        let marker = format!("marker-{index}");
        let invoked = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function}/invocations"),
            Some(json!({ "marker": marker })),
        )
        .await;
        assert_eq!(
            invoked.status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {} with {:?}",
            invoked.status,
            invoked.text()
        );
        assert_eq!(
            invoked.json().get("logged").and_then(Value::as_str),
            Some(marker.as_str()),
            "case {case:?}: expected the handler's own result for {marker:?}, got {:?}",
            invoked.text()
        );
        let printed = format!("logged {marker}");
        let held = messages_in(&router, &published, 2).await;
        assert!(
            held.iter().any(|message| message.contains(&printed)),
            "case {case:?}: expected the group {published:?} to hold the printed line \
             {printed:?}, got {held:?}"
        );
        assert!(
            held.iter().any(|message| message.contains(REPORT_PREFIX)),
            "case {case:?}: expected the group {published:?} to hold a {REPORT_PREFIX:?} \
             record for the run, got {held:?}"
        );
    }
}
