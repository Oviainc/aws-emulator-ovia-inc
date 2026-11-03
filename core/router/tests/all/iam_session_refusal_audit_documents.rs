// SPDX-License-Identifier: CC0-1.0

//! What the audit record of an IAM call made with an assumed-role SESSION carries, read
//! back the way a client reads event history: `cloudtrail:LookupEvents`.
//!
//! RULE: a call a service's permission check stops BEFORE dispatch is recorded with no
//! request parameters and no response elements — `requestParameters` and
//! `responseElements` are both JSON `null` — because those documents report what a call
//! that was authorized and DISPATCHED carried. A call that WAS dispatched records the
//! parameters it was dispatched with, whether the operation then served it or refused it of
//! its own. So a reader of event history tells a call whose body reached the service from
//! one stopped at the permission check, and no member of a denied request reaches the audit
//! trail.
//!
//! Only a cross-service suite can pin it on a SESSION caller, which is the caller a client
//! audits denials of: the role and its one grant are written by `iam:CreateRole` and
//! `iam:PutRolePolicy`, the credential is minted by `sts:AssumeRole`, the records are
//! written by IAM as it serves each call, and they are read back through
//! `cloudtrail:LookupEvents` — three services over ONE engine, as `core/router/src/lib.rs`
//! mounts them.
//!
//! The rows span what the rule generalises over rather than the one call that motivated it:
//! which operation was refused (a create, a listing, a tag write, a delete), what the
//! request carried (one member, none at all, a nested list of them) and — on the other side
//! of the rule — four calls the role's grant admits: a read, a listing that carries no
//! member either, a tag write carrying the nested list, and a read the OPERATION itself
//! refused for an entity nothing created.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The Region every request here is signed for, and the Region IAM files its records in
/// (`service::management_events::EVENT_REGION`).
const REGION: &str = "us-east-1";

/// The long-term key every emulator publishes: the account's own credential, which the
/// fixtures are created with and which holds no IAM policies.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The `RoleSessionName` the assume asks for — the last segment of the assumed-role ARN a
/// refusal of this session names.
const SESSION: &str = "audit-probe";

/// The role whose session every row below calls with.
const ROLE: &str = "AuditedSessionRole";

/// A user name the account holds no user for, for the row whose authorized read the
/// operation itself refuses.
const ABSENT_USER: &str = "audited-session-nobody";

/// A trust policy granting the account root permission to assume a role.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// The actions the role's ONE inline policy allows: everything the dispatched rows ask for,
/// and nothing the refused rows ask for.
const GRANTED: &[&str] = &[
    "iam:GetRole",
    "iam:ListRoles",
    "iam:TagRole",
    "iam:GetUser",
];

/// IAM, STS and CloudTrail over ONE engine, as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
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
    router.register_service(
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key a service reads the caller's identity from. The
/// signature is never checked: this router runs with authentication off.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request to `signing_name`, presenting `access_key_id`.
async fn dispatch(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    headers: &[(&str, &str)],
    body: String,
) -> (u16, String) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header(
            "authorization",
            authorization(access_key_id, signing_name),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One awsQuery form POST, which is how IAM and STS are called.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    dispatch(
        router,
        access_key_id,
        signing_name,
        &[("content-type", "application/x-www-form-urlencoded")],
        body,
    )
    .await
}

/// One IAM action, presenting `access_key_id`.
async fn iam(router: &MultiServiceRouter, access_key_id: &str, body: String) -> (u16, String) {
    query(
        router,
        access_key_id,
        "iam",
        format!("Version=2010-05-08&{body}"),
    )
    .await
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One identity-based policy allowing exactly `actions`, each in its own statement, written
/// so it carries nothing an `application/x-www-form-urlencoded` body treats specially.
fn allow(actions: &[&str]) -> String {
    let statements: Vec<String> = actions
        .iter()
        .map(|action| format!(r#"{{"Effect":"Allow","Action":"{action}","Resource":"*"}}"#))
        .collect();
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
        statements.join(",")
    )
}

/// Create the role and embed its one grant, as the account's own credential.
async fn create_role(router: &MultiServiceRouter) {
    let (status, body) = iam(
        router,
        AMBIENT_KEY,
        format!("Action=CreateRole&RoleName={ROLE}&AssumeRolePolicyDocument={TRUST_POLICY}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole must succeed, got {status}: {body}"
    );
    let (status, body) = iam(
        router,
        AMBIENT_KEY,
        format!(
            "Action=PutRolePolicy&RoleName={ROLE}&PolicyName=the-one-grant&PolicyDocument={}",
            allow(GRANTED)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: PutRolePolicy must succeed, got {status}: {body}"
    );
}

/// Assume the role and answer the access key id of the session STS minted.
async fn assume_role(router: &MultiServiceRouter) -> String {
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}");
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "sts",
        format!("Action=AssumeRole&Version=2011-06-15&RoleArn={role_arn}&RoleSessionName={SESSION}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: AssumeRole must succeed, got {status}: {body}"
    );
    xml_element(&body, "AccessKeyId").unwrap_or_else(|| panic!("no <AccessKeyId> in {body}"))
}

/// The ARN of the session the assume minted, which is the principal a refusal of it names.
fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION}")
}

/// `cloudtrail:LookupEvents` for one event name, answering each matching event's parsed
/// `CloudTrailEvent` document — the way a client reads a record back.
async fn lookup(router: &MultiServiceRouter, event_name: &str) -> Vec<Value> {
    let (status, body) = dispatch(
        router,
        AMBIENT_KEY,
        "cloudtrail",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", "CloudTrail_20131101.LookupEvents"),
        ],
        json!({
            "LookupAttributes": [{
                "AttributeKey": "EventName",
                "AttributeValue": event_name,
            }],
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "LookupEvents(EventName={event_name}) must succeed, got {status}: {body}"
    );
    let answered: Value =
        serde_json::from_str(&body).unwrap_or_else(|error| panic!("{error}: {body}"));
    answered["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("LookupEvents reports an Events list, got {body}"))
        .iter()
        .map(|event| {
            let document = event["CloudTrailEvent"]
                .as_str()
                .unwrap_or_else(|| panic!("an event carries its CloudTrailEvent: {event}"));
            serde_json::from_str(document)
                .unwrap_or_else(|error| panic!("CloudTrailEvent is JSON ({error}): {document}"))
        })
        .collect()
}

/// One row: the call the session makes, what the caller must see, and what the record of
/// that call must carry.
struct Case {
    case: &'static str,
    /// The awsQuery `Action`, which is also the `eventName` the record is found under.
    action: &'static str,
    /// The form parameters the call carries.
    params: Vec<(String, String)>,
    /// The HTTP status the caller must see.
    status: u16,
    /// The `errorCode` the record must carry, and `None` for a served call.
    error_code: Option<&'static str>,
    /// The `iam:` action a refusal at the permission check names, for the rows that are
    /// refused there.
    refused_action: Option<&'static str>,
    /// For a DISPATCHED call, the `requestParameters` the record must carry, each as a
    /// pointer into the record and the value held there.
    recorded: Vec<(&'static str, Value)>,
    /// A value the request carried that the recorded documents may not hold, for a call
    /// stopped at the permission check.
    withheld: Option<&'static str>,
}

impl Case {
    /// Whether the call reached the operation at all. A call that did NOT is the one whose
    /// record carries no parameters.
    fn dispatched(&self) -> bool {
        self.refused_action.is_none()
    }
}

/// One `Tags.member.1` pair, as a tag write sends it.
fn tag_params() -> Vec<(String, String)> {
    vec![
        ("Tags.member.1.Key".to_string(), "team".to_string()),
        (
            "Tags.member.1.Value".to_string(),
            "audited-session-tag".to_string(),
        ),
    ]
}

/// The rows. Every one is made with the SAME session credential, so what differs between a
/// refused row and a dispatched one is only whether the role's one grant admits the call.
fn cases() -> Vec<Case> {
    vec![
        // The class the finding was filed on: a create refused for want of permission.
        // Nothing of the refused request is in its record — not even the name it asked for.
        Case {
            case: "a create stopped at authorization",
            action: "CreateUser",
            params: vec![("UserName".to_string(), "audited-session-user".to_string())],
            status: 403,
            error_code: Some("AccessDenied"),
            refused_action: Some("iam:CreateUser"),
            recorded: Vec::new(),
            withheld: Some("audited-session-user"),
        },
        // A request that carries NO member at all: its record carries `null` rather than
        // an empty document, which is what tells it from the dispatched listing below.
        Case {
            case: "a listing stopped at authorization",
            action: "ListUsers",
            params: Vec::new(),
            status: 403,
            error_code: Some("AccessDenied"),
            refused_action: Some("iam:ListUsers"),
            recorded: Vec::new(),
            withheld: None,
        },
        // A request carrying a nested LIST of members: none of it is recorded either.
        Case {
            case: "a tag write stopped at authorization",
            action: "TagUser",
            params: {
                let mut params = vec![("UserName".to_string(), ABSENT_USER.to_string())];
                params.extend(tag_params());
                params
            },
            status: 403,
            error_code: Some("AccessDenied"),
            refused_action: Some("iam:TagUser"),
            recorded: Vec::new(),
            withheld: Some("audited-session-tag"),
        },
        // A removal stopped at the same check: the kind of call differs, the record does
        // not.
        Case {
            case: "a delete stopped at authorization",
            action: "DeleteUser",
            params: vec![("UserName".to_string(), ABSENT_USER.to_string())],
            status: 403,
            error_code: Some("AccessDenied"),
            refused_action: Some("iam:DeleteUser"),
            recorded: Vec::new(),
            withheld: Some(ABSENT_USER),
        },
        // The other side of the rule, and the contrast a monitoring rule is written
        // against: the SAME session's authorized read records the member its request
        // carried.
        Case {
            case: "an authorized read",
            action: "GetRole",
            params: vec![("RoleName".to_string(), ROLE.to_string())],
            status: 200,
            error_code: None,
            refused_action: None,
            recorded: vec![("/requestParameters/roleName", json!(ROLE))],
            withheld: None,
        },
        // A dispatched call whose request carries no member either: it still records a
        // document, which is what the refused listing above records none of.
        Case {
            case: "an authorized listing",
            action: "ListRoles",
            params: Vec::new(),
            status: 200,
            error_code: None,
            refused_action: None,
            recorded: Vec::new(),
            withheld: None,
        },
        // A dispatched write carrying the nested list: every member of it is recorded.
        Case {
            case: "an authorized tag write",
            action: "TagRole",
            params: {
                let mut params = vec![("RoleName".to_string(), ROLE.to_string())];
                params.extend(tag_params());
                params
            },
            status: 200,
            error_code: None,
            refused_action: None,
            recorded: vec![
                ("/requestParameters/roleName", json!(ROLE)),
                ("/requestParameters/tags/0/key", json!("team")),
                (
                    "/requestParameters/tags/0/value",
                    json!("audited-session-tag"),
                ),
            ],
            withheld: None,
        },
        // A call the grant admits and the OPERATION then refused: the parameters stay,
        // because the request did reach the service.
        Case {
            case: "an authorized read of an entity nothing created",
            action: "GetUser",
            params: vec![("UserName".to_string(), ABSENT_USER.to_string())],
            status: 404,
            error_code: Some("NoSuchEntityException"),
            refused_action: None,
            recorded: vec![("/requestParameters/userName", json!(ABSENT_USER))],
            withheld: None,
        },
    ]
}

/// RULE: the record of a call the permission check stopped carries no request parameters
/// and no response elements, and the record of a call that was authorized and DISPATCHED
/// carries the parameters it was dispatched with — whether the operation then served it or
/// refused it itself. Every row is one call of one session, read back through
/// `LookupEvents`.
#[tokio::test]
async fn a_session_call_stopped_at_authorization_is_audited_without_its_parameters() {
    let router = build_router();
    create_role(&router).await;
    let session_key = assume_role(&router).await;

    for case in cases() {
        let body = case
            .params
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<String>>()
            .join("&");
        let request = if body.is_empty() {
            format!("Action={}", case.action)
        } else {
            format!("Action={}&{body}", case.action)
        };
        let (status, answered) = iam(&router, &session_key, request).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status}: {answered}",
            case.case, case.status
        );

        let found = lookup(&router, case.action).await;
        assert_eq!(
            found.len(),
            1,
            "case {:?}: exactly one {} was made, got {found:?}",
            case.case,
            case.action
        );
        let record = &found[0];
        assert_eq!(
            record.get("errorCode"),
            case.error_code.map(|code| json!(code)).as_ref(),
            "case {:?}: expected errorCode {:?}, got {record}",
            case.case,
            case.error_code
        );
        assert_eq!(
            record.get("awsRegion"),
            Some(&json!(REGION)),
            "case {:?}: an IAM record is filed in the partition's leader Region, got \
             {record}",
            case.case
        );
        // Whichever side of the rule the row is on, the record names the SESSION that made
        // the call: what the rule withholds is the request's own members, not the event.
        assert_eq!(
            record.pointer("/userIdentity/type"),
            Some(&json!("AssumedRole")),
            "case {:?}: the call was made with a role session, got {record}",
            case.case
        );
        assert_eq!(
            record.pointer("/userIdentity/arn"),
            Some(&json!(session_arn())),
            "case {:?}: the record names the session's ARN, got {record}",
            case.case
        );
        assert_eq!(
            record.pointer("/userIdentity/accessKeyId"),
            Some(&json!(session_key)),
            "case {:?}: the record names the credential the call was signed with, got \
             {record}",
            case.case
        );
        assert_eq!(
            record.pointer("/userIdentity/sessionContext/sessionIssuer/userName"),
            Some(&json!(ROLE)),
            "case {:?}: the record names the role that issued the session, got {record}",
            case.case
        );

        if case.dispatched() {
            assert_ne!(
                record.get("requestParameters"),
                Some(&Value::Null),
                "case {:?}: a dispatched call records the parameters it was dispatched \
                 with, got {record}",
                case.case
            );
            for (pointer, expected) in &case.recorded {
                assert_eq!(
                    record.pointer(pointer),
                    Some(expected),
                    "case {:?}: expected {pointer} to be {expected}, got {record}",
                    case.case
                );
            }
            continue;
        }

        assert_eq!(
            record.get("requestParameters"),
            Some(&Value::Null),
            "case {:?}: a call stopped at authorization records no parameters, got {record}",
            case.case
        );
        assert_eq!(
            record.get("responseElements"),
            Some(&Value::Null),
            "case {:?}: a call stopped at authorization answered no values, got {record}",
            case.case
        );
        // The refusal itself is recorded in full: the message the record carries is the one
        // the caller was answered with, naming the action asked for and the session it was
        // refused for.
        let refusal = xml_element(&answered, "Message")
            .unwrap_or_else(|| panic!("case {:?}: no <Message> in {answered}", case.case));
        assert_eq!(
            record.get("errorMessage").and_then(Value::as_str),
            Some(refusal.as_str()),
            "case {:?}: the record carries the refusal the caller saw, got {record}",
            case.case
        );
        let refused_action = case
            .refused_action
            .expect("a row stopped at authorization names the action it asked for");
        assert!(
            refusal.contains(refused_action) && refusal.contains(&session_arn()),
            "case {:?}: the refusal names {refused_action} and {}, got {refusal:?}",
            case.case,
            session_arn()
        );
        if let Some(withheld) = case.withheld {
            let documents = json!({
                "requestParameters": record.get("requestParameters"),
                "responseElements": record.get("responseElements"),
            })
            .to_string();
            assert!(
                !documents.contains(withheld),
                "case {:?}: the recorded documents may carry no part of the refused \
                 request, got {withheld:?} in {documents}",
                case.case
            );
        }
    }
}
