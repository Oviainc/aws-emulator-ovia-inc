// SPDX-License-Identifier: CC0-1.0

//! The SCOPE a credential was minted with, on the DynamoDB data plane.
//!
//! RULE: a request made with a temporary credential is authorized against the INTERSECTION
//! of the permissions of the identity it speaks for and the session policies passed when it
//! was minted — "the resulting session's permissions are the intersection of the role's
//! identity-based policies and the session policies" (`AssumeRoleRequest$Policy`) — with an
//! explicit `Deny` in any document in force refusing whatever any grant says. So a session
//! policy can only take permissions away: a call it does not allow is refused even where the
//! role's own policy or the table's resource policy allows it, a call it explicitly denies is
//! refused while the actions it does not name are still served in the SAME session, and a
//! call nothing else grants is refused however wide the session policy is.
//!
//! The refusal is `AccessDeniedException` (HTTP 400), worded from the request — the
//! principal, the action and the resource — and ends at the document that withheld the
//! grant, because an explicit deny in the scope, a scope that allows nothing and an identity
//! that is granted nothing need different fixes.
//!
//! Three services have to agree for that to hold (IAM holds the role, its policies and the
//! managed policy a scope can name, STS mints the scoped credential and keeps what it was
//! scoped with, DynamoDB makes the decision), so this is the suite that can pin it; the
//! unscoped half of the same decision is `dynamodb_data_plane_authorization.rs`.
//!
//! The rows span what the rule generalises over: what the session policy says (nothing at
//! all, the action on this table, another action, the action on another table, a wildcard, an
//! allow plus an explicit deny, a deny alone), how it was passed (inline, as a managed policy
//! ARN, as an ARN naming a policy this emulator does not hold), where the grant it scopes
//! comes from (the role's identity-based policy, the table's own resource policy, nowhere),
//! and which operation is asked for (`GetItem`, `PutItem`, `Query`, `Scan`).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The region every request here signs for.
const REGION: &str = "us-east-1";

/// The table every row acts on, keyed on a string partition key alone.
const TABLE: &str = "Scoped";

/// The long-term key every emulator publishes, which is the ambient identity's.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The partition the seeded item lives in.
const SEEDED: &str = "k";

/// A trust policy granting the account root permission to assume a role. It carries none of
/// the characters that are significant in an `application/x-www-form-urlencoded` body.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// The three services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
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
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key the service reads the caller's identity from.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Post one awsQuery form body to a service as the ambient identity.
async fn query(router: &MultiServiceRouter, signing_name: &str, body: String) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(AMBIENT_KEY, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Send one DynamoDB operation as `access_key_id` and return `(status, body)`.
async fn ddb(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("authorization", authorization(access_key_id, "dynamodb"))
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the test body serializes"),
        ))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code (`__type`) of a refusal.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message`/`Message` member of a refusal, whichever casing it carries.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The ARN of a table of this account and region, which is the resource a statement names.
fn arn_of(table: &str) -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{table}")
}

/// Create an IAM role whose trust policy admits the account root, and return its ARN.
async fn create_role(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = query(
        router,
        "iam",
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}\
             &AssumeRolePolicyDocument={TRUST_POLICY}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole expected 200, got {status} {body}"
    );
    xml_element(&body, "Arn").unwrap_or_else(|| panic!("case {role_name:?}: no <Arn> in {body}"))
}

/// Embed an inline policy in an IAM role.
async fn put_role_policy(router: &MultiServiceRouter, role_name: &str, document: &str) {
    let (status, body) = query(
        router,
        "iam",
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={role_name}\
             &PolicyName=scope-probe&PolicyDocument={document}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: PutRolePolicy expected 200, got {status} {body}"
    );
}

/// Create a customer managed IAM policy, the kind a mint names as a MANAGED session policy,
/// and return its ARN.
async fn create_policy(router: &MultiServiceRouter, policy_name: &str, document: &str) -> String {
    let (status, body) = query(
        router,
        "iam",
        format!(
            "Action=CreatePolicy&Version=2010-05-08&PolicyName={policy_name}\
             &PolicyDocument={document}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: CreatePolicy expected 200, got {status} {body}"
    );
    xml_element(&body, "Arn").unwrap_or_else(|| panic!("case {policy_name:?}: no <Arn> in {body}"))
}

/// Assume `role_arn` under the session name `session_name`, scoping the credential with the
/// inline session policy `inline` and the managed session policy `managed` when either is
/// given, and return the access key id of the session STS minted.
async fn assume_role_scoped(
    router: &MultiServiceRouter,
    role_arn: &str,
    session_name: &str,
    inline: Option<&str>,
    managed: Option<&str>,
) -> String {
    let mut form = format!(
        "Action=AssumeRole&Version=2011-06-15&RoleArn={role_arn}&RoleSessionName={session_name}"
    );
    if let Some(document) = inline {
        form.push_str(&format!("&Policy={document}"));
    }
    if let Some(policy_arn) = managed {
        form.push_str(&format!("&PolicyArns.member.1.arn={policy_arn}"));
    }
    let (status, body) = query(router, "sts", form).await;
    assert_eq!(
        status, 200,
        "AssumeRole({role_arn}, {session_name}) expected 200, got {status} {body}"
    );
    xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("AssumeRole({role_arn}, {session_name}): no key in {body}"))
}

/// An identity-shaped policy document — the grammar a session policy and an identity-based
/// policy share, naming no principal — carrying nothing a form body treats specially.
fn document(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// One `Allow` of `actions` on `resources`.
fn allow(actions: Value, resources: Value) -> String {
    document(json!([{"Effect": "Allow", "Action": actions, "Resource": resources}]))
}

/// Create the table the rows act on, as the account's own credential, and seed one item.
async fn seed_table(router: &MultiServiceRouter) {
    let (status, created) = ddb(
        router,
        AMBIENT_KEY,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTable expected 200, got {status} {created}"
    );
    // Wait out the documented CREATING span, then seed the item the reads report.
    tokio::time::sleep(Duration::from_millis(3400)).await;
    let (status, put) = ddb(
        router,
        AMBIENT_KEY,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": SEEDED}, "v": {"S": "seeded"}}}),
    )
    .await;
    assert_eq!(status, 200, "PutItem expected 200, got {status} {put}");
}

/// Attach a resource-based policy to the table that names `principal`, and wait out the
/// application span: "PutResourcePolicy is an asynchronous operation" and "the policy
/// application is eventually consistent".
async fn put_resource_policy(router: &MultiServiceRouter, principal: &str) {
    let (status, attached) = ddb(
        router,
        AMBIENT_KEY,
        "PutResourcePolicy",
        json!({
            "ResourceArn": arn_of(TABLE),
            "Policy": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": principal},
                    "Action": ["dynamodb:GetItem", "dynamodb:PutItem"],
                    "Resource": arn_of(TABLE),
                }],
            })
            .to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutResourcePolicy expected 200, got {status} {attached}"
    );
    tokio::time::sleep(Duration::from_millis(3400)).await;
}

/// A `GetItem` of the seeded item.
fn read_request() -> Value {
    json!({"TableName": TABLE, "Key": {"pk": {"S": SEEDED}}})
}

/// A `PutItem` of a fresh item in the partition `partition`.
fn write_request(partition: &str) -> Value {
    json!({"TableName": TABLE, "Item": {"pk": {"S": partition}, "v": {"S": "written"}}})
}

/// A `Query` of the seeded partition.
fn query_request() -> Value {
    json!({
        "TableName": TABLE,
        "KeyConditionExpression": "pk = :p",
        "ExpressionAttributeValues": {":p": {"S": SEEDED}},
    })
}

/// One row: the call one scoped credential makes, and what the policies in force make of it.
struct Row {
    /// What the row is about, named in every assertion it makes.
    case: &'static str,
    /// Which session (and so which scope) makes the call, by its index in the mints below.
    session: usize,
    /// The operation and its body.
    operation: &'static str,
    body: Value,
    /// The status the policies in force earn the call.
    expected: u16,
    /// The clause a refusal ends at: the document that withheld the grant. Empty for a
    /// served call.
    reason: &'static str,
}

/// Which minted session a row speaks through, by its position in the mints below.
const UNSCOPED: usize = 0;
const READ_WRITE: usize = 1;
const WRITE_ONLY: usize = 2;
const ELSEWHERE: usize = 3;
const EVERYTHING: usize = 4;
const MINUS_READ: usize = 5;
const DENY_ONLY: usize = 6;
const MANAGED_READ: usize = 7;
const MANAGED_WRITE: usize = 8;
const UNHELD: usize = 9;
const RES_READ_WRITE: usize = 10;
const RES_MINUS_READ: usize = 11;
const RES_WRITE_ONLY: usize = 12;
const UNGRANTED: usize = 13;

/// The refusal of a call the session policies allow nothing of.
const NO_SCOPE_GRANT: &str = "because no session policy allows the";

/// The refusal of a call a session policy explicitly denies.
const SCOPE_DENY: &str = "with an explicit deny in a session policy";

/// The refusal of a call nothing in force grants at all.
const NO_IDENTITY_GRANT: &str = "because no identity-based policy allows the";

/// RULE: a scoped credential's permissions are the intersection of the identity's and the
/// session policies' — see the module docs.
#[tokio::test]
async fn a_session_policy_scopes_down_what_the_credential_may_do() {
    let router = build_router();
    seed_table(&router).await;

    // The role the account grants every item action on the table: whatever a row is
    // refused for, it is not the identity's own policy.
    let granted_role = "ddb-scoped-granted-role";
    let granted_role_arn = create_role(&router, granted_role).await;
    put_role_policy(
        &router,
        granted_role,
        &allow(json!("dynamodb:*"), json!(arn_of(TABLE))),
    )
    .await;

    // A role the account grants NOTHING, admitted by the TABLE's own policy instead: the
    // other place a grant can come from, which a session policy scopes just the same.
    let resource_role = "ddb-scoped-resource-role";
    let resource_role_arn = create_role(&router, resource_role).await;
    put_resource_policy(&router, &resource_role_arn).await;

    // A role nothing grants at all, to show a session policy grants nothing of its own.
    let ungranted_role = "ddb-scoped-ungranted-role";
    let ungranted_role_arn = create_role(&router, ungranted_role).await;

    // The managed policy a mint names as a managed session policy, and one that scopes a
    // session to another action.
    let managed_read = create_policy(
        &router,
        "scope-read",
        &allow(
            json!(["dynamodb:GetItem", "dynamodb:Query"]),
            json!(arn_of(TABLE)),
        ),
    )
    .await;
    let managed_write = create_policy(
        &router,
        "scope-write",
        &allow(json!("dynamodb:PutItem"), json!(arn_of(TABLE))),
    )
    .await;

    let read_write = allow(
        json!(["dynamodb:GetItem", "dynamodb:PutItem"]),
        json!(arn_of(TABLE)),
    );
    let write_only = allow(json!("dynamodb:PutItem"), json!(arn_of(TABLE)));
    let other_table = allow(json!("dynamodb:*"), json!(arn_of("Elsewhere")));
    let everything = allow(json!("dynamodb:*"), json!("*"));
    // A scoped-down credential as a broker hands one out: a grant, and a deny that takes
    // one action of it back.
    let read_write_minus_read = document(json!([
        {
            "Effect": "Allow",
            "Action": ["dynamodb:GetItem", "dynamodb:PutItem"],
            "Resource": arn_of(TABLE),
        },
        {"Effect": "Deny", "Action": "dynamodb:GetItem", "Resource": arn_of(TABLE)},
    ]));
    let deny_only = document(json!([{
        "Effect": "Deny", "Action": "dynamodb:GetItem", "Resource": "*",
    }]));

    // (the role a session is of, the session name, its inline scope, its managed scope), in
    // the order the row constants above name them.
    let mints: Vec<(&str, &str, Option<&str>, Option<&str>)> = vec![
        (&granted_role_arn, "unscoped", None, None),
        (&granted_role_arn, "readwrite", Some(&read_write), None),
        (&granted_role_arn, "writeonly", Some(&write_only), None),
        (&granted_role_arn, "elsewhere", Some(&other_table), None),
        (&granted_role_arn, "everything", Some(&everything), None),
        (
            &granted_role_arn,
            "minusread",
            Some(&read_write_minus_read),
            None,
        ),
        (&granted_role_arn, "denyonly", Some(&deny_only), None),
        (&granted_role_arn, "managedread", None, Some(&managed_read)),
        (&granted_role_arn, "managedwrite", None, Some(&managed_write)),
        (
            // A managed session policy whose document this emulator does not hold: a scope
            // it cannot read is no evidence for a refusal, so it withholds nothing.
            &granted_role_arn,
            "unheld",
            None,
            Some("arn:aws:iam::000000000000:policy/not-held-here"),
        ),
        (&resource_role_arn, "resreadwrite", Some(&read_write), None),
        (
            &resource_role_arn,
            "resminusread",
            Some(&read_write_minus_read),
            None,
        ),
        (&resource_role_arn, "reswriteonly", Some(&write_only), None),
        (&ungranted_role_arn, "ungranted", Some(&everything), None),
    ];
    let names = [granted_role, resource_role, ungranted_role];
    let mut sessions: Vec<(String, &str)> = Vec::new();
    for (role_arn, session_name, inline, managed) in &mints {
        let key = assume_role_scoped(&router, role_arn, session_name, *inline, *managed).await;
        let role_name = names
            .iter()
            .find(|name| role_arn.ends_with(*name))
            .unwrap_or_else(|| panic!("the mint names one of the roles created here"));
        sessions.push((key, role_name));
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "a credential no mint scoped",
            session: UNSCOPED,
            operation: "GetItem",
            body: read_request(),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a scope naming the action on this table",
            session: READ_WRITE,
            operation: "GetItem",
            body: read_request(),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a scope naming only another action",
            session: WRITE_ONLY,
            operation: "GetItem",
            body: read_request(),
            expected: 400,
            reason: NO_SCOPE_GRANT,
        },
        Row {
            case: "a scope naming the action on another table",
            session: ELSEWHERE,
            operation: "GetItem",
            body: read_request(),
            expected: 400,
            reason: NO_SCOPE_GRANT,
        },
        Row {
            case: "a scope of wildcards",
            session: EVERYTHING,
            operation: "GetItem",
            body: read_request(),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a scope that allows the read and then denies it",
            session: MINUS_READ,
            operation: "GetItem",
            body: read_request(),
            expected: 400,
            reason: SCOPE_DENY,
        },
        Row {
            case: "the write of the SAME session, which that deny does not name",
            session: MINUS_READ,
            operation: "PutItem",
            body: write_request("written-under-the-scope"),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a scope that only denies, on the action it denies",
            session: DENY_ONLY,
            operation: "GetItem",
            body: read_request(),
            expected: 400,
            reason: SCOPE_DENY,
        },
        Row {
            case: "a scope that only denies, on an action it does not name",
            session: DENY_ONLY,
            operation: "PutItem",
            body: write_request("denied-scope-write"),
            expected: 400,
            reason: NO_SCOPE_GRANT,
        },
        Row {
            case: "a managed scope naming the action",
            session: MANAGED_READ,
            operation: "GetItem",
            body: read_request(),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a managed scope naming the query",
            session: MANAGED_READ,
            operation: "Query",
            body: query_request(),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a managed scope naming neither the scan",
            session: MANAGED_READ,
            operation: "Scan",
            body: json!({"TableName": TABLE}),
            expected: 400,
            reason: NO_SCOPE_GRANT,
        },
        Row {
            case: "a managed scope naming another action",
            session: MANAGED_WRITE,
            operation: "GetItem",
            body: read_request(),
            expected: 400,
            reason: NO_SCOPE_GRANT,
        },
        Row {
            case: "a managed scope this emulator holds no document for",
            session: UNHELD,
            operation: "GetItem",
            body: read_request(),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a query the inline scope does not name",
            session: READ_WRITE,
            operation: "Query",
            body: query_request(),
            expected: 400,
            reason: NO_SCOPE_GRANT,
        },
        Row {
            case: "a query a wildcard scope names",
            session: EVERYTHING,
            operation: "Query",
            body: query_request(),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a scan a wildcard scope names",
            session: EVERYTHING,
            operation: "Scan",
            body: json!({"TableName": TABLE}),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a grant from the table's own policy, within the scope",
            session: RES_READ_WRITE,
            operation: "GetItem",
            body: read_request(),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a grant from the table's own policy the scope denies",
            session: RES_MINUS_READ,
            operation: "GetItem",
            body: read_request(),
            expected: 400,
            reason: SCOPE_DENY,
        },
        Row {
            case: "a grant from the table's own policy the scope does not name",
            session: RES_WRITE_ONLY,
            operation: "GetItem",
            body: read_request(),
            expected: 400,
            reason: NO_SCOPE_GRANT,
        },
        Row {
            case: "a write the table's own policy grants and the scope denies nothing of",
            session: RES_MINUS_READ,
            operation: "PutItem",
            body: write_request("written-by-the-resource-grant"),
            expected: 200,
            reason: "",
        },
        Row {
            case: "a wide scope on a role nothing grants",
            session: UNGRANTED,
            operation: "GetItem",
            body: read_request(),
            expected: 400,
            reason: NO_IDENTITY_GRANT,
        },
    ];

    for row in rows {
        let (key, role_name) = &sessions[row.session];
        let (status, answer) = ddb(&router, key, row.operation, row.body.clone()).await;
        assert_eq!(
            status, row.expected,
            "case {:?}: {} expected HTTP {}, got {status} {answer}",
            row.case, row.operation, row.expected
        );
        if row.expected != 400 {
            continue;
        }
        assert_eq!(
            error_code(&answer),
            "AccessDeniedException",
            "case {:?}: expected AccessDeniedException, got {answer}",
            row.case
        );
        let message = message_of(&answer);
        let session_prefix = format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/");
        let action = format!("dynamodb:{}", row.operation);
        assert!(
            message.contains(&session_prefix)
                && message.contains(&action)
                && message.contains(&arn_of(TABLE)),
            "case {:?}: the refusal is built from the request (a session of \
             {session_prefix}…, action {action}, resource {}), got {message:?}",
            row.case,
            arn_of(TABLE)
        );
        let ends_at = if row.reason == SCOPE_DENY {
            row.reason.to_string()
        } else {
            format!("{} {action} action", row.reason)
        };
        assert!(
            message.ends_with(&ends_at),
            "case {:?}: the refusal names the document that withheld the grant \
             ({ends_at:?}), got {message:?}",
            row.case
        );
    }

    // The writes the scopes admitted are really there, read back through the account's own
    // credential: a grant inside the scope served a write, not just a 200.
    for partition in ["written-under-the-scope", "written-by-the-resource-grant"] {
        let (status, stored) = ddb(
            &router,
            AMBIENT_KEY,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": partition}}}),
        )
        .await;
        assert_eq!(
            (status, stored["Item"]["v"].clone()),
            (200, json!({"S": "written"})),
            "case {partition:?}: the admitted write is read back, got {status} {stored}"
        );
    }
    // And the write a scope refused wrote nothing.
    let (status, absent) = ddb(
        &router,
        AMBIENT_KEY,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "denied-scope-write"}}}),
    )
    .await;
    assert_eq!(
        (status, absent.get("Item").cloned()),
        (200, None),
        "case the refused write: nothing was stored for it, got {status} {absent}"
    );
}
