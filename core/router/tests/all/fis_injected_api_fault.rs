// SPDX-License-Identifier: CC0-1.0

//! A RUNNING fault-injection experiment makes the targeted role's calls to the named
//! operations FAIL, and only those calls.
//!
//! RULE: while an experiment's API-fault action is running, a request made with the
//! credentials of one of the IAM roles its target resolved to, naming one of the operations
//! it listed on the service it named, is answered with that fault class's HTTP status and
//! error code instead of being served — for the share of the calls the action's `percentage`
//! asked for — and every other call is served exactly as before: another principal, an
//! operation the action did not name, and any call once the experiment is no longer running.
//!
//! Four services have to agree for that to hold, which is why this is the suite that can pin
//! it end to end: IAM holds the roles and the policies in force on them, STS mints the
//! session the targeted calls are signed with, FIS runs the experiment that publishes the
//! fault, and Amazon EC2 is the service whose API answers it. They all read ONE engine, as
//! the production router mounts them.
//!
//! The rows span what the rule generalises over: the FAULT CLASS the action injects (the
//! internal-error, throttle and unavailable actions of the `aws:fis:inject-api-*` family,
//! each with its own status and code), the FORM the `operations` parameter takes (one name,
//! several names, a list whose named operation is not the first entry), whether the action
//! states a `percentage` at all, and what share it states (the whole share, and none of it).
//! Beside the table: a fault is not live before the run reaches `running` and is gone once
//! the run has ended on its own clock.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder for an `application/x-www-form-urlencoded` component, which
// `all/cloudfront_key_value_store_handshake.rs` already owns.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The Region every request here signs for.
const REGION: &str = "us-east-1";

/// The long-term key every emulator publishes, which is the account's own administrative
/// credential: the CONTROL caller of every row, whose calls no fault affects.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The IAM API version a client sends in an awsQuery body.
const IAM_API_VERSION: &str = "2010-05-08";

/// The STS API version a client sends in an awsQuery body.
const STS_API_VERSION: &str = "2011-06-15";

/// The `RoleSessionName` every `AssumeRole` here asks for.
const SESSION: &str = "probe";

/// The role the experiments run under, which FIS assumes.
const EXECUTION_ROLE: &str = "fis-exec";

/// The role the actions TARGET: the principal whose calls a running fault affects.
const TARGETED_ROLE: &str = "fault-target";

/// Another role, assumable and allowed the same EC2 calls, which no action targets: the
/// principal a fault must spare.
const SPARED_ROLE: &str = "fault-bystander";

/// The operation the rows' actions name, which is the one the recorded experiment named.
const NAMED_OPERATION: &str = "DescribeAvailabilityZones";

/// A second operation some rows name beside it.
const ALSO_NAMED_OPERATION: &str = "DescribeRegions";

/// An operation NO row names: the call a fault must leave alone on the very client whose
/// other calls it fails.
const UNNAMED_OPERATION: &str = "DescribeVpcs";

/// How long a row may wait for the move to `running`: the emulator's transition span is a
/// fraction of a second, so this is generous.
const RUNNING_BUDGET: Duration = Duration::from_secs(10);

/// How long a row may wait for an experiment to reach a terminal status: the whole
/// compressed run window, its two transitions, and slack for a loaded machine.
const TERMINAL_BUDGET: Duration = Duration::from_secs(30);

/// How long a poll waits between reads.
const INTERVAL: Duration = Duration::from_millis(50);

/// One emulator carrying IAM, STS, FIS and Amazon EC2 over ONE shared engine, exactly as
/// the production router builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` — what routes
/// the request — and whose credential is `access_key_id`, which is the key the serving
/// service reads the caller's identity from.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Post one query-protocol form body to `signing_name`, presenting `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One restJson1 call to FIS, as the ambient caller.
async fn fis(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("fis.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(AMBIENT_KEY, "fis"))
        .body(payload)
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// What one EC2 call answered: its HTTP status, and the error code and message of the
/// ec2Query error envelope when it carries one.
struct Answered {
    status: u16,
    code: String,
    message: String,
}

/// Call one EC2 operation presenting `access_key_id`.
async fn ec2(router: &MultiServiceRouter, access_key_id: &str, action: &str) -> Answered {
    let body = format!("Action={action}&Version={EC2_API_VERSION}");
    let (status, text) = query(router, access_key_id, "ec2", body).await;
    Answered {
        status,
        code: xml_element(&text, "Code").unwrap_or_default(),
        message: xml_element(&text, "Message").unwrap_or_default(),
    }
}

/// Create a role carrying `trust`, as the ambient identity.
async fn create_role(router: &MultiServiceRouter, role_name: &str, trust: &str) {
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "iam",
        format!(
            "Action=CreateRole&Version={IAM_API_VERSION}&RoleName={role_name}\
             &AssumeRolePolicyDocument={}",
            form_encode(trust)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {role_name:?}: CreateRole expected 200, got {status}: {body}"
    );
}

/// Embed one inline policy in a role, as the ambient identity.
async fn put_role_policy(router: &MultiServiceRouter, role_name: &str, document: &str) {
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "iam",
        format!(
            "Action=PutRolePolicy&Version={IAM_API_VERSION}&RoleName={role_name}\
             &PolicyName=granted&PolicyDocument={}",
            form_encode(document)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {role_name:?}: PutRolePolicy expected 200, got {status}: {body}"
    );
}

/// Assume `role_name` as the ambient identity, and answer the access key id of the session
/// STS minted — the credential a targeted call is signed with.
async fn assume_role(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "sts",
        format!(
            "Action=AssumeRole&Version={STS_API_VERSION}&RoleArn={}&RoleSessionName={SESSION}",
            role_arn(role_name)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {role_name:?}: AssumeRole expected 200, got {status}: {body}"
    );
    xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("setup {role_name:?}: no <AccessKeyId> in {body}"))
}

/// The ARN the account holds `role_name` under.
fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")
}

/// A trust policy granting the account root permission to assume a role, so the test's own
/// caller can mint a session of it.
fn account_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// A trust policy granting FIS permission to assume the experiments' execution role.
fn fis_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "fis.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// An identity policy allowing `actions` on every resource.
fn allow(actions: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
    .to_string()
}

/// Create the three roles every row runs with: the execution role FIS assumes and which is
/// allowed to inject an API fault, the role the actions target, and a bystander role. Both
/// of the latter are assumable by this caller and allowed the EC2 reads the rows make, so
/// what a row observes is the FAULT and never a permission.
async fn seed_roles(router: &MultiServiceRouter) {
    create_role(router, EXECUTION_ROLE, &fis_trust()).await;
    put_role_policy(
        router,
        EXECUTION_ROLE,
        &allow(json!([
            "fis:InjectApiInternalError",
            "fis:InjectApiThrottleError",
            "fis:InjectApiUnavailableError"
        ])),
    )
    .await;
    for role in [TARGETED_ROLE, SPARED_ROLE] {
        create_role(router, role, &account_trust()).await;
        put_role_policy(router, role, &allow(json!("ec2:*"))).await;
    }
}

/// A client token for `case`: the modelled `clientToken` admits no whitespace
/// (`^[\S]+$`), so a case's own words travel as one token.
fn token(label: &str, case: &str) -> String {
    format!("{label}-{}", case.replace(' ', "-"))
}

/// Create a template whose single action injects `action_id` with `parameters` against the
/// targeted role, and answer its id.
async fn create_template(
    router: &MultiServiceRouter,
    case: &str,
    action_id: &str,
    parameters: Value,
) -> String {
    let (status, body) = fis(
        router,
        "POST",
        "/experimentTemplates",
        Some(json!({
            "clientToken": token("template", case),
            "description": format!("the injected-fault rule: {case}"),
            "roleArn": role_arn(EXECUTION_ROLE),
            "stopConditions": [{ "source": "none" }],
            "targets": {
                "roleTarget": {
                    "resourceType": "aws:iam:role",
                    "resourceArns": [role_arn(TARGETED_ROLE)],
                    "selectionMode": "ALL"
                }
            },
            "actions": {
                "fault": {
                    "actionId": action_id,
                    "parameters": parameters,
                    "targets": { "Roles": "roleTarget" }
                }
            }
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateExperimentTemplate expected 200, got {status}: {body}"
    );
    body["experimentTemplate"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no template id in {body}"))
        .to_string()
}

/// Start an experiment of `template_id` and answer its id.
async fn start_experiment(router: &MultiServiceRouter, case: &str, template_id: &str) -> String {
    let (status, body) = fis(
        router,
        "POST",
        "/experiments",
        Some(json!({
            "clientToken": token("start", case),
            "experimentTemplateId": template_id
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartExperiment expected 200, got {status}: {body}"
    );
    body["experiment"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {body}"))
        .to_string()
}

/// The status `GetExperiment` reports for `id`.
async fn experiment_status(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let (status, body) = fis(router, "GET", &format!("/experiments/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetExperiment expected 200, got {status}: {body}"
    );
    body["experiment"]["state"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Poll `id` until it reports one of `wanted`, and answer the status it reached.
async fn poll_for_status(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    wanted: &[&str],
    budget: Duration,
) -> String {
    let deadline = Instant::now() + budget;
    loop {
        let status = experiment_status(router, case, id).await;
        if wanted.contains(&status.as_str()) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the experiment to reach one of {wanted:?} within \
             {budget:?}, it is {status:?}"
        );
        tokio::time::sleep(INTERVAL).await;
    }
}

/// Stop `id` and wait for it to leave the running state.
async fn stop_experiment(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, body) = fis(router, "DELETE", &format!("/experiments/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StopExperiment expected 200, got {status}: {body}"
    );
    poll_for_status(
        router,
        case,
        id,
        &["stopped", "completed", "failed"],
        TERMINAL_BUDGET,
    )
    .await;
}

/// One row: the fault class an action injects, the `parameters` it carries, and what a
/// targeted call to a named operation is answered with while it runs.
struct Row {
    case: &'static str,
    action_id: &'static str,
    /// The `operations` parameter exactly as the action states it.
    operations: &'static str,
    /// The `percentage` parameter, or `None` for an action that states none.
    percentage: Option<&'static str>,
    /// The operations the row's `operations` parameter names.
    named: &'static [&'static str],
    /// The status, error code and message a targeted call to a named operation is answered
    /// with, or `None` for a row whose share affects no call at all.
    affected: Option<(u16, &'static str, &'static str)>,
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: a full injection of the internal-error class over one
            // operation.
            case: "an internal error over the whole share",
            action_id: "aws:fis:inject-api-internal-error",
            operations: NAMED_OPERATION,
            percentage: Some("100"),
            named: &[NAMED_OPERATION],
            affected: Some((500, "InternalError", "An internal error has occurred")),
        },
        Row {
            // Another fault class, over SEVERAL operations, with no `percentage` stated —
            // the parameter is optional, and an action that states none affects every call
            // it names.
            case: "a throttle over two operations with no share stated",
            action_id: "aws:fis:inject-api-throttle-error",
            operations: "DescribeAvailabilityZones,DescribeRegions",
            percentage: None,
            named: &[NAMED_OPERATION, ALSO_NAMED_OPERATION],
            affected: Some((503, "RequestLimitExceeded", "Request limit exceeded.")),
        },
        Row {
            // The third fault class, over a list whose NAMED operation is not the first
            // entry: the order the action lists its operations in decides nothing.
            case: "an unavailable error over a list naming it last",
            action_id: "aws:fis:inject-api-unavailable-error",
            operations: "DescribeRegions,DescribeAvailabilityZones",
            percentage: Some("100"),
            named: &[ALSO_NAMED_OPERATION, NAMED_OPERATION],
            affected: Some((
                503,
                "Unavailable",
                "The server is overloaded and cannot handle the request.",
            )),
        },
        Row {
            // A share of none: the action runs, and no call is affected by it.
            case: "a share of none",
            action_id: "aws:fis:inject-api-internal-error",
            operations: NAMED_OPERATION,
            percentage: Some("0"),
            named: &[NAMED_OPERATION],
            affected: None,
        },
    ]
}

/// The `parameters` a row's action carries.
fn parameters(row: &Row) -> Value {
    let mut parameters = json!({
        "duration": "PT2M",
        "service": "ec2",
        "operations": row.operations
    });
    if let Some(percentage) = row.percentage {
        parameters["percentage"] = json!(percentage);
    }
    parameters
}

/// Assert that `access_key_id`'s call to `action` is served, which is what every call a
/// fault does not cover must be.
async fn assert_served(
    router: &MultiServiceRouter,
    case: &str,
    who: &str,
    access_key_id: &str,
    action: &str,
) {
    let answered = ec2(router, access_key_id, action).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected {who}'s {action} to be served, got {} with code {:?} and \
         message {:?}",
        answered.status, answered.code, answered.message
    );
}

#[tokio::test]
async fn a_running_api_fault_fails_the_targeted_calls_and_only_those() {
    let router = emulator();
    seed_roles(&router).await;
    let targeted_key = assume_role(&router, TARGETED_ROLE).await;
    let spared_key = assume_role(&router, SPARED_ROLE).await;

    for row in rows() {
        let case = row.case;
        // The BASELINE: before any experiment runs, the targeted role's call is served.
        assert_served(&router, case, "the targeted role", &targeted_key, NAMED_OPERATION).await;

        let template_id = create_template(&router, case, row.action_id, parameters(&row)).await;
        let experiment_id = start_experiment(&router, case, &template_id).await;
        poll_for_status(&router, case, &experiment_id, &["running"], RUNNING_BUDGET).await;

        for operation in row.named {
            let answered = ec2(&router, &targeted_key, operation).await;
            match row.affected {
                Some((status, code, message)) => {
                    assert_eq!(
                        (answered.status, answered.code.as_str(), answered.message.as_str()),
                        (status, code, message),
                        "case {case:?}: expected the targeted role's {operation} to be \
                         answered {status} {code} {message:?}, got {} {:?} {:?}",
                        answered.status,
                        answered.code,
                        answered.message
                    );
                }
                None => {
                    assert_eq!(
                        answered.status, 200,
                        "case {case:?}: expected the targeted role's {operation} to be \
                         served, got {} with code {:?}",
                        answered.status, answered.code
                    );
                }
            }
        }

        // An operation the action did not name, on the very client whose other calls it
        // fails; and the two principals it does not target.
        assert_served(
            &router,
            case,
            "the targeted role",
            &targeted_key,
            UNNAMED_OPERATION,
        )
        .await;
        assert_served(&router, case, "the bystander role", &spared_key, NAMED_OPERATION).await;
        assert_served(&router, case, "the control caller", AMBIENT_KEY, NAMED_OPERATION).await;

        // Once the experiment is no longer running, nothing is affected any more.
        stop_experiment(&router, case, &experiment_id).await;
        for operation in row.named {
            assert_served(&router, case, "the targeted role", &targeted_key, operation).await;
        }
    }
}

#[tokio::test]
async fn a_fault_is_live_only_while_the_run_is() {
    let router = emulator();
    seed_roles(&router).await;
    let targeted_key = assume_role(&router, TARGETED_ROLE).await;
    let case = "the run's own clock";

    let template_id = create_template(
        &router,
        case,
        "aws:fis:inject-api-internal-error",
        json!({
            "duration": "PT2M",
            "service": "ec2",
            "operations": NAMED_OPERATION,
            "percentage": "100"
        }),
    )
    .await;
    let experiment_id = start_experiment(&router, case, &template_id).await;

    // The start answers `initiating`: the action has not started, so no call is affected
    // yet.
    let started = experiment_status(&router, case, &experiment_id).await;
    assert_eq!(
        started, "initiating",
        "case {case:?}: expected a started experiment to be initiating, got {started:?}"
    );
    assert_served(
        &router,
        case,
        "the targeted role before the run",
        &targeted_key,
        NAMED_OPERATION,
    )
    .await;

    poll_for_status(&router, case, &experiment_id, &["running"], RUNNING_BUDGET).await;
    let answered = ec2(&router, &targeted_key, NAMED_OPERATION).await;
    assert_eq!(
        (answered.status, answered.code.as_str()),
        (500, "InternalError"),
        "case {case:?}: expected the running fault to fail the targeted call with 500 \
         InternalError, got {} with {:?}",
        answered.status,
        answered.code
    );

    // Nothing is stopped: the run ends on its own clock, and the fault ends with it.
    let reached = poll_for_status(
        &router,
        case,
        &experiment_id,
        &["completed", "stopped", "failed"],
        TERMINAL_BUDGET,
    )
    .await;
    assert_eq!(
        reached, "completed",
        "case {case:?}: expected the run to end completed, got {reached:?}"
    );
    assert_served(
        &router,
        case,
        "the targeted role after the run",
        &targeted_key,
        NAMED_OPERATION,
    )
    .await;
}
