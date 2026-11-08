// SPDX-License-Identifier: CC0-1.0

//! Just-in-time node access end to end: `StartAccessRequest` raises a request and
//! `GetAccessToken` reports it and vends the credentials of an approved one.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over the single request a recorded unit happened to make:
//!
//! 1. **Authorization comes first.** Both operations are signed calls, and such a call is
//!    served only to a caller whose identity-based IAM policies allow THAT operation's own
//!    action (`ssm:<OperationName>`) on the resource the request addresses — the access
//!    request's own ARN for a token read, the account's resources for a start. Every other
//!    caller is refused with the modelled `AccessDeniedException` BEFORE anything is
//!    resolved, so the refusal does not depend on what the account holds. The principals
//!    below are made with IAM's and STS's OWN operations over the same engine, so the
//!    documents this service reads are the ones those services wrote.
//! 2. **Then whether the account has the workflow at all.** An account has just-in-time node
//!    access in a Region once it holds an access-request approval policy there — an SSM
//!    document of type `AutoApprovalPolicy` or `ManualApprovalPolicy` — and an account that
//!    holds none is refused with `ValidationException` carrying the sentence AWS states that
//!    with, before any id or target is looked at.
//! 3. **Then the request itself.** A well-formed id no start of that Region issued is
//!    refused; a request its policy has not approved reports the status it is in and vends
//!    nothing; an approved one reports the credentials it holds, which stay the same set
//!    across reads.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` — and nothing reaches into the state struct, so a
//! response that is not computed from persisted state cannot pass.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{error_code, REGION, TARGET_PREFIX};

/// A second Region, so a rule that is decided per Region is seen to be.
const OTHER_REGION: &str = "eu-central-1";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDTEST";

/// The sentence a just-in-time node access call against an account that has not turned the
/// workflow on is refused with. Fixed text for that error class.
const NOT_ENABLED: &str = "Just-In-Time Node Access service is not enabled in this account.";

/// The sentence a well-formed `AccessRequestId` that names no request the Region issued is
/// refused with.
const NOT_FOUND: &str = "The specified access request ID doesn't exist. Verify the ID and try again.";

/// The transient window an auto-approved request stays `Pending` for, which the service
/// stores as a due instant rather than sleeping for.
const APPROVAL_DELAY: Duration =
    service_com_amazonaws_ssm_amazonssm::state::just_in_time_node_access::APPROVAL_DELAY;

/// How long the credentials vended for an approved request are valid.
const CREDENTIAL_LIFETIME: Duration =
    service_com_amazonaws_ssm_amazonssm::state::just_in_time_node_access::CREDENTIAL_LIFETIME;

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal must
/// blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// A router with this service beside the IAM and STS services the principals and their
/// policies are made with, all on one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing: the `Authorization` header carries only the credential scope,
    // which is where a service reads the caller's access key id and Region from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One call to this service, signed for `region` with `access_key_id` in the credential
/// scope, returning `(status, body)`.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The same call, signed with the account's own published credential.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_as(router, ACCOUNT_KEY, region, operation, body).await
}

/// One awsQuery call to IAM or STS, signed with the account's own credential, which must
/// succeed.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, String)],
) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20240101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action:?}: the setup call must succeed, got {status} with {text}"
    );
    text
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Create an IAM user carrying `policy` as its inline policy (when one is given), mint a
/// long-term access key for it, and answer that key id with the user's own ARN.
async fn user(router: &MultiServiceRouter, user_name: &str, policy: Option<&str>) -> Principal {
    let created = query(
        router,
        "iam",
        "2010-05-08",
        "CreateUser",
        &[("UserName", user_name.to_string())],
    )
    .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
    if let Some(policy) = policy {
        query(
            router,
            "iam",
            "2010-05-08",
            "PutUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }
    let minted = query(
        router,
        "iam",
        "2010-05-08",
        "CreateAccessKey",
        &[("UserName", user_name.to_string())],
    )
    .await;
    Principal {
        access_key_id: xml_element(&minted, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}")),
        arn,
    }
}

/// Create an IAM role carrying `policy` as its inline policy (when one is given), assume it
/// through STS, and answer the session's access key id with the session ARN.
async fn assumed_role(
    router: &MultiServiceRouter,
    role_name: &str,
    policy: Option<&str>,
) -> Principal {
    let account = engine::emulator_account_id();
    let trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    );
    query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    if let Some(policy) = policy {
        query(
            router,
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }
    let assumed = query(
        router,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
            ("RoleSessionName", "probe".to_string()),
        ],
    )
    .await;
    Principal {
        access_key_id: xml_element(&assumed, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
        arn: xml_element(&assumed, "Arn")
            .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
    }
}

/// Register an access-request approval policy of `document_type` in `region`, through this
/// service's own `CreateDocument`.
async fn approval_policy(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    document_type: &str,
) {
    let (status, response) = call(
        router,
        region,
        "CreateDocument",
        &json!({
            "Name": name,
            "DocumentType": document_type,
            "Content": "{\"schemaVersion\": \"1.0\", \"description\": \"who may approve\"}",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {document_type} in {region}: the approval policy must be created, \
         got {status} {response}"
    );
}

/// The ARN an access request id carries in `region` — the resource a token read's
/// authorization decision is made about.
fn access_request_arn(region: &str, access_request_id: &str) -> String {
    format!(
        "arn:aws:ssm:{region}:{}:opsitem/{access_request_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// `StartAccessRequest` with `targets`, as the account's own credential.
async fn start(
    router: &MultiServiceRouter,
    region: &str,
    reason: &str,
    targets: Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "StartAccessRequest",
        &json!({ "Reason": reason, "Targets": targets }),
    )
    .await
}

/// `StartAccessRequest`, asserting it succeeded, and returning the id it minted.
async fn start_ok(router: &MultiServiceRouter, case: &str, region: &str, targets: Value) -> String {
    let (status, response) = start(router, region, &format!("because: {case}"), targets).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartAccessRequest must succeed, got {status} {response}"
    );
    let id = response["AccessRequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a start answers an AccessRequestId, got {response}"))
        .to_string();
    assert!(
        is_access_request_id(&id),
        "case {case:?}: expected an id of the modelled form oi-<12 lowercase hex>, got {id:?}"
    );
    id
}

/// `GetAccessToken` of `access_request_id`, as the account's own credential.
async fn token(router: &MultiServiceRouter, region: &str, access_request_id: &str) -> (u16, Value) {
    call(
        router,
        region,
        "GetAccessToken",
        &json!({ "AccessRequestId": access_request_id }),
    )
    .await
}

/// `GetAccessToken`, asserting it succeeded.
async fn token_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    access_request_id: &str,
) -> Value {
    let (status, response) = token(router, region, access_request_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetAccessToken must succeed, got {status} {response}"
    );
    response
}

/// Whether `id` has the form the modelled `AccessRequestId` pattern requires
/// (`^(oi)-[0-9a-f]{12}$`).
fn is_access_request_id(id: &str) -> bool {
    id.strip_prefix("oi-").is_some_and(|digits| {
        digits.len() == 12
            && digits
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// Whether `value` has the form the modelled `AccessKeyIdType` pattern requires
/// (`^\w{16,128}$`).
fn is_access_key_id(value: &str) -> bool {
    (16..=128).contains(&value.chars().count())
        && value.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Seconds since the epoch, as an awsJson1_1 timestamp renders them.
fn epoch_seconds(now: SystemTime) -> f64 {
    now.duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

/// Assert one refusal: its status, its wire error code and the whole sentence it carries.
fn assert_refusal(case: &str, expected_code: &str, expected_message: &str, got: (u16, Value)) {
    let (status, body) = got;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, expected_code),
        "case {case:?}: expected 400 {expected_code}, got {status} {body}"
    );
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("<no message>");
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {message:?}"
    );
}

/// The targets a start may name, one row per form `Target` accepts
/// (`smithy_query(query="shape", shape="Target")`: a key and the values that map to it),
/// up to the modelled `Targets` length of 5.
fn target_forms() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one node by id",
            json!([{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }]),
        ),
        (
            "two nodes by id",
            json!([{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0", "i-00112233445566778"] }]),
        ),
        (
            "every node of a tag",
            json!([{ "Key": "tag:Env", "Values": ["staging"] }]),
        ),
        (
            "a resource group",
            json!([{ "Key": "ResourceGroup", "Values": ["fleet-a"] }]),
        ),
        (
            "the modelled maximum of five target keys",
            json!([
                { "Key": "tag:Env", "Values": ["staging"] },
                { "Key": "tag:Team", "Values": ["platform"] },
                { "Key": "tag:Tier", "Values": ["web"] },
                { "Key": "tag:Owner", "Values": ["ops"] },
                { "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] },
            ]),
        ),
        ("no target at all", json!([])),
    ]
}

/// RULE 2: while an account holds no access-request approval policy it has no just-in-time
/// node access, so every call of the workflow is refused with `ValidationException` carrying
/// that one sentence — whatever the id, the reason, the targets or the Region.
#[tokio::test]
async fn the_workflow_is_refused_while_the_account_holds_no_approval_policy() {
    let router = emulator();
    let ids = [
        "oi-000000000000",
        "oi-0123456789ab",
        "oi-ffffffffffff",
        "oi-ba9876543210",
    ];
    for region in [REGION, OTHER_REGION] {
        for id in ids {
            let case = format!("GetAccessToken {id} in {region}");
            assert_refusal(
                &case,
                "ValidationException",
                NOT_ENABLED,
                token(&router, region, id).await,
            );
        }
        for (form, targets) in target_forms() {
            let case = format!("StartAccessRequest for {form} in {region}");
            assert_refusal(
                &case,
                "ValidationException",
                NOT_ENABLED,
                start(&router, region, "a reason", targets).await,
            );
        }
    }

    // The policy is a REGIONAL document, so creating one enables the Region it was created
    // in and no other.
    approval_policy(&router, REGION, "JitnaAutoApproval", "AutoApprovalPolicy").await;
    let enabled = start_ok(
        &router,
        "the Region that now holds a policy",
        REGION,
        json!([{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }]),
    )
    .await;
    assert_refusal(
        "the same start in a Region that holds none",
        "ValidationException",
        NOT_ENABLED,
        start(
            &router,
            OTHER_REGION,
            "a reason",
            json!([{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }]),
        )
        .await,
    );
    assert_refusal(
        "a token read in a Region that holds none",
        "ValidationException",
        NOT_ENABLED,
        token(&router, OTHER_REGION, &enabled).await,
    );
}

/// RULE 3: in a Region that HAS the workflow, a well-formed id no start of that Region issued
/// names no access request, and is refused with `ValidationException` — including an id
/// another Region's start minted, because a request is reached through the endpoint it was
/// raised at.
#[tokio::test]
async fn a_well_formed_id_no_start_issued_names_no_access_request() {
    let router = emulator();
    approval_policy(&router, REGION, "JitnaManualApproval", "ManualApprovalPolicy").await;
    approval_policy(
        &router,
        OTHER_REGION,
        "JitnaOtherApproval",
        "ManualApprovalPolicy",
    )
    .await;
    let issued = start_ok(
        &router,
        "a request raised in the first Region",
        REGION,
        json!([{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }]),
    )
    .await;
    let unissued = [
        "oi-000000000000",
        "oi-ffffffffffff",
        "oi-0123456789ab",
        "oi-abcdefabcdef",
    ];
    for id in unissued {
        assert!(
            id != issued,
            "case {id:?}: the row must name an id no start minted, and a start minted {issued:?}"
        );
        assert_refusal(
            &format!("the unissued {id}"),
            "ValidationException",
            NOT_FOUND,
            token(&router, REGION, id).await,
        );
    }
    assert_refusal(
        "an id another Region's start minted",
        "ValidationException",
        NOT_FOUND,
        token(&router, OTHER_REGION, &issued).await,
    );
}

/// RULE: `AccessRequestId`'s `^(oi)-[0-9a-f]{12}$` pattern is modelled, so a value outside it
/// is refused as invalid at the boundary — before the workflow is consulted at all.
#[tokio::test]
async fn a_malformed_access_request_id_is_refused() {
    let router = emulator();
    approval_policy(&router, REGION, "JitnaAutoApproval", "AutoApprovalPolicy").await;
    // (case, the value)
    let rows: &[(&str, &str)] = &[
        ("empty", ""),
        ("the prefix alone", "oi-"),
        ("one hex digit short", "oi-0123456789a"),
        ("one hex digit too many", "oi-0123456789abc"),
        ("uppercase hex", "oi-0123456789AB"),
        ("a digit outside hex", "oi-0123456789ag"),
        ("another prefix", "op-0123456789ab"),
        ("no prefix at all", "0123456789ab"),
        ("an OpsItem ARN rather than an id", &access_request_arn(REGION, "oi-0123456789ab")),
    ];
    for (case, value) in rows {
        let (status, body) = token(&router, REGION, value).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a value outside the modelled pattern is refused, got {status} {body}"
        );
    }
}

/// RULE 1: both operations are decided against the CALLER's identity-based policies before
/// anything is resolved — a caller no readable document allows is refused with
/// `AccessDeniedException` naming the principal, the action, the resource and the rule that
/// withheld the grant, while an allowed caller reaches the workflow and is answered by it.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_action_is_refused() {
    let router = emulator();
    let account = engine::emulator_account_id();
    let id = "oi-0123456789ab";
    let token_resource = access_request_arn(REGION, id);

    let allow_all = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Action":"ssm:*","Resource":"*"}]}"#;
    let allow_elsewhere = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Action":"sts:GetCallerIdentity","Resource":"*"}]}"#;
    let allow_only_the_token_read = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Action":"ssm:GetAccessToken","Resource":"*"}]}"#;
    let allow_all_but_deny_the_start = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Action":"ssm:*","Resource":"*"},
        {"Effect":"Deny","Action":"ssm:StartAccessRequest","Resource":"*"}]}"#;
    let allow_another_access_request = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
        "Action":"ssm:GetAccessToken","Resource":"{}"}}]}}"#,
        access_request_arn(REGION, "oi-ffffffffffff")
    );

    // The principals: the account's own published credential, IAM users and assumed-role
    // sessions, each carrying a different document.
    let principals: Vec<(&str, Principal, Option<String>, Option<String>)> = vec![
        (
            "the account's own published credential",
            Principal {
                access_key_id: ACCOUNT_KEY.to_string(),
                arn: format!("arn:aws:iam::{account}:root"),
            },
            None,
            None,
        ),
        (
            "a user carrying no policy at all",
            user(&router, "jitna-nopolicy", None).await,
            Some("because no identity-based policy allows the ssm:GetAccessToken action".to_string()),
            Some(
                "because no identity-based policy allows the ssm:StartAccessRequest action"
                    .to_string(),
            ),
        ),
        (
            "a user allowed only another service's action",
            user(&router, "jitna-elsewhere", Some(allow_elsewhere)).await,
            Some("because no identity-based policy allows the ssm:GetAccessToken action".to_string()),
            Some(
                "because no identity-based policy allows the ssm:StartAccessRequest action"
                    .to_string(),
            ),
        ),
        (
            "a user allowed the token read alone",
            user(&router, "jitna-tokenonly", Some(allow_only_the_token_read)).await,
            None,
            Some(
                "because no identity-based policy allows the ssm:StartAccessRequest action"
                    .to_string(),
            ),
        ),
        (
            "a user allowed the token read on ANOTHER access request",
            user(&router, "jitna-otherresource", Some(&allow_another_access_request)).await,
            Some("because no identity-based policy allows the ssm:GetAccessToken action".to_string()),
            Some(
                "because no identity-based policy allows the ssm:StartAccessRequest action"
                    .to_string(),
            ),
        ),
        (
            "an assumed-role session carrying no policy",
            assumed_role(&router, "jitna-noperm", None).await,
            Some("because no identity-based policy allows the ssm:GetAccessToken action".to_string()),
            Some(
                "because no identity-based policy allows the ssm:StartAccessRequest action"
                    .to_string(),
            ),
        ),
        (
            "an assumed-role session allowed everything",
            assumed_role(&router, "jitna-admin", Some(allow_all)).await,
            None,
            None,
        ),
        (
            "an assumed-role session whose policy denies the start explicitly",
            assumed_role(&router, "jitna-denystart", Some(allow_all_but_deny_the_start)).await,
            None,
            Some("with an explicit deny in an identity-based policy".to_string()),
        ),
    ];

    for (case, principal, token_reason, start_reason) in principals {
        // The token read. A refused caller is told it is not authorized; an allowed one
        // reaches the workflow, which — this account holds no approval policy — tells it the
        // service is not enabled. Either way the answer proves which decision came first.
        let got = call_as(
            &router,
            &principal.access_key_id,
            REGION,
            "GetAccessToken",
            &json!({ "AccessRequestId": id }),
        )
        .await;
        match token_reason {
            Some(reason) => assert_refusal(
                &format!("{case}: GetAccessToken"),
                "AccessDeniedException",
                &format!(
                    "User: {} is not authorized to perform: ssm:GetAccessToken on resource: \
                     {token_resource} {reason}",
                    principal.arn
                ),
                got,
            ),
            None => assert_refusal(
                &format!("{case}: GetAccessToken"),
                "ValidationException",
                NOT_ENABLED,
                got,
            ),
        }

        // The start, whose decision is about the account's resources rather than one record.
        let got = call_as(
            &router,
            &principal.access_key_id,
            REGION,
            "StartAccessRequest",
            &json!({
                "Reason": "a reason",
                "Targets": [{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }],
            }),
        )
        .await;
        match start_reason {
            Some(reason) => assert_refusal(
                &format!("{case}: StartAccessRequest"),
                "AccessDeniedException",
                &format!(
                    "User: {} is not authorized to perform: ssm:StartAccessRequest on resource: \
                     * {reason}",
                    principal.arn
                ),
                got,
            ),
            None => assert_refusal(
                &format!("{case}: StartAccessRequest"),
                "ValidationException",
                NOT_ENABLED,
                got,
            ),
        }
    }
}

/// RULE 3: an auto-approval policy approves what it covers without anybody acting — a request
/// is `Pending` until the approval the clock owes falls due and `Approved` after — and an
/// approved request is answered with the credentials of its window, the same set across
/// reads and a different set per request.
#[tokio::test]
async fn an_approved_request_is_exchanged_for_credentials() {
    let router = emulator();
    approval_policy(&router, REGION, "JitnaAutoApproval", "AutoApprovalPolicy").await;

    // Every form of start raises a request of its own, and each is pending at once.
    let mut raised: Vec<(&str, String)> = Vec::new();
    for (form, targets) in target_forms() {
        let id = start_ok(&router, form, REGION, targets).await;
        let read = token_ok(&router, form, REGION, &id).await;
        assert_eq!(
            read["AccessRequestStatus"],
            json!("Pending"),
            "case {form:?}: a request read straight after the start is Pending, got {read}"
        );
        assert_eq!(
            read["Credentials"],
            Value::Null,
            "case {form:?}: a request nothing has approved vends no credentials, got {read}"
        );
        assert!(
            raised.iter().all(|(_, held)| held != &id),
            "case {form:?}: each start must mint its own id, and {id:?} was minted twice"
        );
        raised.push((form, id));
    }

    // The approval is materialised from the stored due instant, with nothing sleeping in the
    // service: past the window every one of those requests is approved.
    tokio::time::sleep(APPROVAL_DELAY + Duration::from_millis(250)).await;

    let mut vended: Vec<String> = Vec::new();
    for (form, id) in &raised {
        let read = token_ok(&router, form, REGION, id).await;
        assert_eq!(
            read["AccessRequestStatus"],
            json!("Approved"),
            "case {form:?}: past the approval window the request is Approved, got {read}"
        );
        let credentials = &read["Credentials"];
        let access_key_id = credentials["AccessKeyId"].as_str().unwrap_or_default();
        assert!(
            is_access_key_id(access_key_id),
            "case {form:?}: expected an AccessKeyId of the modelled form ^\\w{{16,128}}$, \
             got {credentials}"
        );
        assert!(
            !credentials["SecretAccessKey"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
                && !credentials["SessionToken"]
                    .as_str()
                    .unwrap_or_default()
                    .is_empty(),
            "case {form:?}: an approved request vends both secrets, got {credentials}"
        );
        let expires = credentials["ExpirationTime"].as_f64().unwrap_or_default();
        let now = epoch_seconds(SystemTime::now());
        assert!(
            expires > now && expires <= now + CREDENTIAL_LIFETIME.as_secs_f64(),
            "case {form:?}: the credentials expire within their window, got {expires} at {now}"
        );
        // A second read reports the set the request holds rather than a fresh one.
        let again = token_ok(&router, form, REGION, id).await;
        assert_eq!(
            again["Credentials"]["AccessKeyId"],
            credentials["AccessKeyId"],
            "case {form:?}: two reads of one approved request report one credential set, \
             got {again} after {read}"
        );
        assert!(
            !vended.iter().any(|held| held == access_key_id),
            "case {form:?}: each access request holds its own credentials, and \
             {access_key_id:?} was vended twice"
        );
        vended.push(access_key_id.to_string());
    }
}

/// RULE: the store holds a bounded number of access requests — the emulator's own ceiling,
/// not a published quota — and a start past it is refused through the modelled
/// `ServiceQuotaExceededException` and writes nothing.
#[tokio::test]
async fn a_start_past_the_stores_ceiling_is_refused() {
    let router = emulator();
    approval_policy(&router, REGION, "JitnaManualApproval", "ManualApprovalPolicy").await;
    let ceiling =
        service_com_amazonaws_ssm_amazonssm::state::just_in_time_node_access::MAX_ACCESS_REQUESTS;
    let mut last = String::new();
    for round in 1..=ceiling {
        last = start_ok(
            &router,
            &format!("request {round} of the ceiling"),
            REGION,
            json!([{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }]),
        )
        .await;
    }
    // One past it: refused, and the request that was already held is untouched.
    let (status, body) = start(
        &router,
        REGION,
        "one past the ceiling",
        json!([{ "Key": "InstanceIds", "Values": ["i-0123456789abcdef0"] }]),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ServiceQuotaExceededException"),
        "case \"one past the ceiling\": expected the quota refusal, got {status} {body}"
    );
    assert_eq!(
        body["Message"].as_str().unwrap_or("<no message>"),
        format!(
            "This emulator holds at most {ceiling} just-in-time node access requests across \
             every Region"
        ),
        "case \"one past the ceiling\": the refusal names the emulator's own ceiling, got {body}"
    );
    let read = token_ok(&router, "the last request raised", REGION, &last).await;
    assert_eq!(
        read["AccessRequestStatus"],
        json!("Pending"),
        "case \"the last request raised\": a refused start leaves what is held alone, got {read}"
    );
}

/// RULE 3: a manual approval policy leaves the decision to an approver, and no operation of
/// this API records one — so a request raised under it stays `Pending` however long it is
/// left, and vends nothing.
#[tokio::test]
async fn a_request_awaiting_an_approver_stays_pending() {
    let router = emulator();
    approval_policy(&router, REGION, "JitnaManualApproval", "ManualApprovalPolicy").await;
    let id = start_ok(
        &router,
        "a request awaiting an approver",
        REGION,
        json!([{ "Key": "tag:Env", "Values": ["staging"] }]),
    )
    .await;
    // Read at once, and again past the window an AUTO-approval policy's approval would have
    // fallen due in: the policy's type is what decides, not the clock alone.
    for case in ["read at once", "read past the auto-approval window"] {
        let read = token_ok(&router, case, REGION, &id).await;
        assert_eq!(
            read["AccessRequestStatus"],
            json!("Pending"),
            "case {case:?}: a request awaiting an approver is Pending, got {read}"
        );
        assert_eq!(
            read["Credentials"],
            Value::Null,
            "case {case:?}: a request awaiting an approver vends nothing, got {read}"
        );
        tokio::time::sleep(APPROVAL_DELAY + Duration::from_millis(250)).await;
    }
}
