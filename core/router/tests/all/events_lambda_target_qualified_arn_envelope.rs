// SPDX-License-Identifier: CC0-1.0

//! WHAT an EventBridge rule hands its Lambda target, and WHICH function qualifier that
//! handover addresses — end to end over the real `MultiServiceRouter` with EventBridge and
//! Lambda mounted on one engine.
//!
//! The rule pinned here has two halves, and both are properties of the ENVELOPE EventBridge
//! builds for a matched event rather than of any one event:
//!
//! * the payload a function target is invoked with is the whole event envelope — the fields
//!   an `EventPattern` names (`version`, `id`, `detail-type`, `source`, `account`, `time`,
//!   `region`, `resources`, `detail`), with `detail` the entry's `Detail` parsed and
//!   `resources` the entry's `Resources` as submitted — and nothing is delivered for an
//!   event no rule's pattern matches;
//! * a target `Arn` naming a function AT a qualifier (a published version number or an
//!   alias name, both of which a function ARN carries after the function name) addresses
//!   THAT qualifier, so the invocation is queued against it rather than against the
//!   unpublished version; and because "for Lambda functions … EventBridge relies on
//!   resource-based policies", a function whose policy grants this service nothing at the
//!   addressed qualifier is not invoked at all.
//!
//! The observable is Lambda's own asynchronous queue: EventBridge invokes a function target
//! asynchronously ("Event – Invoke the function asynchronously",
//! `Invoke$InvocationType`), and no modelled Lambda operation reports an accepted
//! asynchronous invocation, so the entry Lambda recorded — its addressed version label and
//! the event bytes it holds — is what a consumer of the invocation receives.
//!
//! The rows differ along what the rule generalises over: the FORM of the target ARN
//! (unqualified, `:<version>`-qualified, `:<alias>`-qualified), whether the published event
//! matches the rule's pattern, whether the addressed qualifier carries a grant for this
//! service, and the shape of the envelope's variable fields (no resources, one, two; an
//! empty detail, a flat one, a nested one).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    ComAmazonawsEventbridgeAwseventsState, InvocationOutcome,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and so the Region the envelope reports.
const REGION: &str = "eu-west-2";

/// The account id every emulator ARN carries, which the envelope reports as its `account`
/// (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The `version` every EventBridge envelope carries — the envelope's own schema version,
/// which is a constant of the event format and not of any one event.
const ENVELOPE_VERSION: &str = "0";

/// The `source` and `detail-type` the rule's pattern names, and a `source` it does not.
const SOURCE: &str = "qualified.target.probe";
const DETAIL_TYPE: &str = "Qualified Target Probe";
const OTHER_SOURCE: &str = "qualified.target.other";

/// The principal EventBridge invokes a resource-policy-governed target as, and the action
/// an invocation of a function performs.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// A deployment package as `Code.ZipFile` carries it. Nothing here asserts what a handler
/// computed: what is pinned is the event the function was handed.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// A well-formed execution role ARN, satisfying that model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The alias every aliased row addresses, and the version it invokes — the first version
/// `PublishVersion` mints for a function.
const ALIAS: &str = "live";
const PUBLISHED_VERSION: &str = "1";

/// The version label an unqualified function ARN addresses.
const LATEST: &str = "$LATEST";

/// How long past a due instant this suite waits before calling.
const SLACK: Duration = Duration::from_millis(200);

/// How long it waits for work the clock owes before giving up on it, and how often it looks
/// while waiting.
const DELIVERY_WAIT: Duration = Duration::from_secs(20);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// How long it waits before concluding that NOTHING was handed to the function.
const QUIET_WAIT: Duration = Duration::from_millis(2_000);

/// EventBridge and Lambda over ONE shared engine, as `core/router/src/lib.rs` mounts them.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("authorization", authorization("events"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// A Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Every asynchronous invocation Lambda holds for `function`, as the version label it was
/// addressed to and the event bytes it carries.
fn handed_to(engine: &Arc<engine::InMemoryEngine>, function: &str) -> Vec<(String, Vec<u8>)> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map(|record| {
            record
                .async_invocations
                .iter()
                .map(|entry| (entry.version.clone(), entry.payload.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until `function` holds `expected` asynchronous invocations, or until the wait is
/// spent. A row expecting none waits the quiet budget, so "nothing arrived" is a conclusion
/// and not a race.
async fn settled(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    expected: usize,
) -> Vec<(String, Vec<u8>)> {
    let budget = if expected == 0 {
        QUIET_WAIT
    } else {
        DELIVERY_WAIT
    };
    let looks = budget.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let held = handed_to(engine, function);
        if held.len() >= expected && expected > 0 {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    handed_to(engine, function)
}

/// What EventBridge recorded for each handover of a routed event.
fn handovers(engine: &Arc<engine::InMemoryEngine>) -> Vec<InvocationOutcome> {
    let state = engine.state::<ComAmazonawsEventbridgeAwseventsState>();
    let store = state.lock();
    store
        .invocations
        .iter()
        .map(|invocation| invocation.outcome.clone())
        .collect()
}

/// Which qualifier a row's target ARN carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addressed {
    /// No qualifier at all, so the ARN addresses the unpublished version.
    Unqualified,
    /// A published version number.
    Version,
    /// An alias name.
    Alias,
}

impl Addressed {
    /// The suffix this form puts on the function ARN.
    fn suffix(self) -> String {
        match self {
            Self::Unqualified => String::new(),
            Self::Version => format!(":{PUBLISHED_VERSION}"),
            Self::Alias => format!(":{ALIAS}"),
        }
    }

    /// The version label an invocation of this form is queued against, which is the
    /// qualifier the ARN named.
    fn label(self) -> &'static str {
        match self {
            Self::Unqualified => LATEST,
            Self::Version => PUBLISHED_VERSION,
            Self::Alias => ALIAS,
        }
    }

    /// The `Qualifier` an `AddPermission` has to carry to grant this form's invocation.
    /// "Lambda does not support adding policies to version $LATEST", so the unqualified
    /// form's grant names no qualifier.
    fn permission_qualifier(self) -> Option<&'static str> {
        match self {
            Self::Unqualified => None,
            Self::Version => Some(PUBLISHED_VERSION),
            Self::Alias => Some(ALIAS),
        }
    }
}

/// One row: which ARN form the rule targets, what is published, whether the addressed
/// qualifier carries a grant, and how many invocations the function must end up holding.
struct Row {
    case: &'static str,
    addressed: Addressed,
    granted: bool,
    source: &'static str,
    resources: &'static [&'static str],
    detail: &'static str,
    invocations: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an unqualified target ARN is invoked with the envelope, no resources",
            addressed: Addressed::Unqualified,
            granted: true,
            source: SOURCE,
            resources: &[],
            detail: r#"{}"#,
            invocations: 1,
        },
        Row {
            case: "a version-qualified target ARN resolves to that version",
            addressed: Addressed::Version,
            granted: true,
            source: SOURCE,
            resources: &["arn:aws:s3:::qualified-target-probe"],
            detail: r#"{"stage":"one"}"#,
            invocations: 1,
        },
        Row {
            case: "an alias-qualified target ARN resolves to that alias",
            addressed: Addressed::Alias,
            granted: true,
            source: SOURCE,
            resources: &[
                "arn:aws:s3:::qualified-target-probe",
                "arn:aws:sqs:eu-west-2:000000000000:qualified-target-probe",
            ],
            detail: r#"{"stage":"two","nested":{"depth":[1,2,3]}}"#,
            invocations: 1,
        },
        Row {
            case: "an event the rule's pattern does not match reaches an unqualified target \
                   nowhere",
            addressed: Addressed::Unqualified,
            granted: true,
            source: OTHER_SOURCE,
            resources: &[],
            detail: r#"{"stage":"suppressed"}"#,
            invocations: 0,
        },
        Row {
            case: "an event the rule's pattern does not match reaches an alias-qualified \
                   target nowhere",
            addressed: Addressed::Alias,
            granted: true,
            source: OTHER_SOURCE,
            resources: &["arn:aws:s3:::qualified-target-probe"],
            detail: r#"{"stage":"suppressed"}"#,
            invocations: 0,
        },
        Row {
            case: "an unqualified target whose policy grants this service nothing is not \
                   invoked",
            addressed: Addressed::Unqualified,
            granted: false,
            source: SOURCE,
            resources: &[],
            detail: r#"{"stage":"ungranted"}"#,
            invocations: 0,
        },
        Row {
            case: "a version-qualified target whose policy grants this service nothing is not \
                   invoked",
            addressed: Addressed::Version,
            granted: false,
            source: SOURCE,
            resources: &["arn:aws:s3:::qualified-target-probe"],
            detail: r#"{"stage":"ungranted"}"#,
            invocations: 0,
        },
        Row {
            case: "an alias-qualified target whose policy grants this service nothing is not \
                   invoked",
            addressed: Addressed::Alias,
            granted: false,
            source: SOURCE,
            resources: &["arn:aws:s3:::qualified-target-probe"],
            detail: r#"{"stage":"ungranted"}"#,
            invocations: 0,
        },
    ]
}

/// Every row: the envelope the target is handed, and the qualifier the handover addressed.
#[tokio::test]
async fn a_rules_lambda_target_is_handed_the_envelope_at_the_qualifier_its_arn_names() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, engine) = router();
        let function_name = format!("qualified-target-{index}");

        let (status, function) = lambda(
            &router,
            "POST",
            "/2015-03-31/functions",
            json!({
                "FunctionName": function_name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": ZIP_FILE },
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({function})"
        );
        let function_arn = function["FunctionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {function}"))
            .to_string();

        // The created function leaves `Pending` by the clock, so everything that addresses a
        // version of it happens past that span.
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

        // A qualifier has to EXIST before an ARN can name it: a version is published and an
        // alias is created against the version it invokes.
        let (status, published) = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function_name}/versions"),
            json!({}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from PublishVersion, got {status} ({published})"
        );
        assert_eq!(
            published["Version"],
            json!(PUBLISHED_VERSION),
            "case {case:?}: expected the first published version to be \
             {PUBLISHED_VERSION:?}, got {published}"
        );
        let (status, alias) = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function_name}/aliases"),
            json!({ "Name": ALIAS, "FunctionVersion": PUBLISHED_VERSION }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateAlias, got {status} ({alias})"
        );

        // "For Lambda functions … EventBridge relies on resource-based policies", and the
        // document in force over an invocation is the one held at the qualifier the ARN
        // names — so a granting row adds its statement THERE and an ungranted row adds none
        // at all.
        if row.granted {
            let qualifier = row
                .addressed
                .permission_qualifier()
                .map(|value| format!("?Qualifier={value}"))
                .unwrap_or_default();
            let (status, granted) = lambda(
                &router,
                "POST",
                &format!("/2015-03-31/functions/{function_name}/policy{qualifier}"),
                json!({
                    "StatementId": "events-invoke",
                    "Action": INVOKE_ACTION,
                    "Principal": EVENTS_PRINCIPAL,
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected HTTP 201 from AddPermission, got {status} ({granted})"
            );
        }

        let target_arn = format!("{function_arn}{}", row.addressed.suffix());
        let rule_name = format!("qualified-target-rule-{index}");
        events(
            &router,
            case,
            "PutRule",
            &json!({
                "Name": rule_name,
                "EventPattern": json!({
                    "source": [SOURCE],
                    "detail-type": [DETAIL_TYPE],
                })
                .to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
        let wired = events(
            &router,
            case,
            "PutTargets",
            &json!({
                "Rule": rule_name,
                "Targets": [{ "Id": "fn", "Arn": target_arn }],
            }),
        )
        .await;
        assert_eq!(
            wired["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the target {target_arn:?} to be accepted, got {wired}"
        );

        let published = events(
            &router,
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": row.source,
                    "DetailType": DETAIL_TYPE,
                    "Detail": row.detail,
                    "Resources": row.resources,
                }],
            }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the event to be accepted, got {published}"
        );

        let held = settled(&engine, &function_name, row.invocations).await;
        assert_eq!(
            held.len(),
            row.invocations,
            "case {case:?}: expected {} asynchronous invocation(s) on {function_name:?}, got \
             {held:?}",
            row.invocations
        );

        for (label, payload) in &held {
            assert_eq!(
                label,
                row.addressed.label(),
                "case {case:?}: expected the invocation to be queued against {:?}, got {label:?}",
                row.addressed.label()
            );
            let envelope: Value = serde_json::from_slice(payload).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: expected the payload to be the JSON envelope, got {:?} \
                     ({error})",
                    String::from_utf8_lossy(payload)
                )
            });
            assert_eq!(
                envelope["version"],
                json!(ENVELOPE_VERSION),
                "case {case:?}: expected envelope version {ENVELOPE_VERSION:?}, got {envelope}"
            );
            let id = envelope["id"].as_str().unwrap_or_default();
            assert!(
                !id.is_empty(),
                "case {case:?}: expected the envelope to carry an id, got {envelope}"
            );
            assert_eq!(
                envelope["detail-type"],
                json!(DETAIL_TYPE),
                "case {case:?}: expected envelope detail-type {DETAIL_TYPE:?}, got {envelope}"
            );
            assert_eq!(
                envelope["source"],
                json!(row.source),
                "case {case:?}: expected envelope source {:?}, got {envelope}",
                row.source
            );
            assert_eq!(
                envelope["account"],
                json!(ACCOUNT),
                "case {case:?}: expected envelope account {ACCOUNT:?}, got {envelope}"
            );
            assert_eq!(
                envelope["region"],
                json!(REGION),
                "case {case:?}: expected envelope region {REGION:?}, got {envelope}"
            );
            let time = envelope["time"].as_str().unwrap_or_default();
            assert!(
                time.contains('T') && time.ends_with('Z'),
                "case {case:?}: expected the envelope time to be an RFC3339 instant, got \
                 {time:?} in {envelope}"
            );
            assert_eq!(
                envelope["resources"],
                json!(row.resources),
                "case {case:?}: expected envelope resources {:?}, got {envelope}",
                row.resources
            );
            let detail: Value = serde_json::from_str(row.detail)
                .expect("every row's Detail is a JSON object this suite wrote");
            assert_eq!(
                envelope["detail"], detail,
                "case {case:?}: expected envelope detail {detail}, got {envelope}"
            );
        }

        // What EventBridge itself recorded agrees: a matched event the target's policy
        // admits is INVOKED, a matched event it does not is a delivery that FAILED, and an
        // unmatched event is no handover at all.
        let recorded = handovers(&engine);
        if row.source != SOURCE {
            assert!(
                recorded.is_empty(),
                "case {case:?}: expected no handover for an unmatched event, got {recorded:?}"
            );
        } else if row.granted {
            assert_eq!(
                recorded,
                vec![InvocationOutcome::Invoked],
                "case {case:?}: expected one invoked handover, got {recorded:?}"
            );
        } else {
            assert_eq!(
                recorded.len(),
                1,
                "case {case:?}: expected one recorded handover, got {recorded:?}"
            );
            assert!(
                !matches!(recorded[0], InvocationOutcome::Invoked),
                "case {case:?}: expected the handover to a target that granted this service \
                 nothing to be recorded as failed, got {recorded:?}"
            );
        }
    }
}
