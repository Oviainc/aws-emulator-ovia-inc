// SPDX-License-Identifier: CC0-1.0

//! Authorization on the DynamoDB data plane: three services, one decision.
//!
//! RULE: a read or a write of an item is served only when the policies in force permit
//! `dynamodb:<Action>` on the table's ARN for the principal the request signed as. The
//! caller is the identity the access key it signed with names — the STS session registry,
//! the same source `sts:GetCallerIdentity` answers from — and its permissions are the
//! policies the account holds for that identity (for an assumed-role session, the IAM
//! role's inline and attached policies) together with the table's own resource-based
//! policy. IAM's order decides: an explicit `Deny` in either refuses whatever any grant
//! says; otherwise the call needs a grant from one of them; with neither it is refused —
//! IAM's implicit deny. The refusal is `AccessDeniedException` (HTTP 400, the status the
//! service framework behind DynamoDB answers an authorization refusal with), worded from
//! the request: the principal, the action and the resource.
//!
//! The account's own published credential is the account's administrative identity: the
//! IAM store holds no policies for it and the account owns its tables, so it keeps reading
//! and writing throughout — while an explicit `Deny` in the table's policy still refuses
//! it.
//!
//! Three services have to agree for that to hold (IAM holds the role and its policies, STS
//! mints the session credential and names its principal, DynamoDB makes the decision), so
//! this is the suite that can pin it; a crate-local test can only ever reach one of them.
//!
//! The rows span what the rule generalises over: what the role's own policy says (nothing
//! at all, another service's action, another action of this service, the action on another
//! table, the action on this table, a wildcard covering it), where the grant comes from
//! (the identity-based policy or the table's own), and which of the two documents states a
//! `Deny`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The region every request here signs for.
const REGION: &str = "us-east-1";

/// The table every row acts on.
const TABLE: &str = "Audited";

/// The long-term key every emulator publishes, which is the ambient identity's.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The session name every `AssumeRole` below asks for, which is the last segment of the
/// assumed-role ARN a refusal names.
const SESSION: &str = "probe";

/// A trust policy granting the account root permission to assume a role. It carries none of
/// the characters that are significant in an `application/x-www-form-urlencoded` body.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// The same trust policy, also permitting the mint to carry SESSION TAGS: "to pass session
/// tags … the role's trust policy must allow the `sts:TagSession` action"
/// (`AssumeRoleRequest$Tags`).
const TAGGING_TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"#,
    r#""Action":["sts:AssumeRole","sts:TagSession"]}]}"#
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

/// Create an IAM role and return its ARN.
async fn create_role(router: &MultiServiceRouter, role_name: &str) -> String {
    create_role_trusting(router, role_name, TRUST_POLICY).await
}

/// Create an IAM role whose trust policy is `trust_policy`, and return its ARN.
async fn create_role_trusting(
    router: &MultiServiceRouter,
    role_name: &str,
    trust_policy: &str,
) -> String {
    let (status, body) = query(
        router,
        "iam",
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}\
             &AssumeRolePolicyDocument={trust_policy}"
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
             &PolicyName=probe-policy&PolicyDocument={document}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: PutRolePolicy expected 200, got {status} {body}"
    );
}

/// Assume a role and return the access key id of the session STS minted.
async fn assume_role(router: &MultiServiceRouter, role_arn: &str) -> String {
    let (status, body) = query(
        router,
        "sts",
        format!("Action=AssumeRole&Version=2011-06-15&RoleArn={role_arn}&RoleSessionName={SESSION}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssumeRole({role_arn}) expected 200, got {status} {body}"
    );
    xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("AssumeRole({role_arn}): no <AccessKeyId> in {body}"))
}

/// One identity-based statement, as a document carrying nothing a form body treats
/// specially.
fn policy(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// The table's ARN, which is the resource every statement here names.
fn table_arn() -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{TABLE}")
}

/// Create the table the rows act on, as the ambient identity, and seed one item.
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
        json!({"TableName": TABLE, "Item": {"pk": {"S": "k"}, "v": {"S": "seeded"}}}),
    )
    .await;
    assert_eq!(status, 200, "PutItem expected 200, got {status} {put}");
}

/// A `GetItem` of the seeded item.
fn read_request() -> Value {
    json!({"TableName": TABLE, "Key": {"pk": {"S": "k"}}})
}

#[tokio::test]
async fn an_item_is_read_only_by_a_principal_the_policies_in_force_allow() {
    let router = build_router();
    seed_table(&router).await;

    // (case, role name, the role's inline policy, expected status)
    let rows: Vec<(&str, &str, Option<String>, u16)> = vec![
        (
            "a role the account grants nothing",
            "ddb-no-policy-role",
            None,
            400,
        ),
        (
            "another service's action",
            "ddb-s3-only-role",
            Some(policy("Allow", "s3:GetObject", "*")),
            400,
        ),
        (
            "another action of this service",
            "ddb-put-only-role",
            Some(policy("Allow", "dynamodb:PutItem", "*")),
            400,
        ),
        (
            "the action on another table",
            "ddb-other-table-role",
            Some(policy(
                "Allow",
                "dynamodb:GetItem",
                &format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/Elsewhere"),
            )),
            400,
        ),
        (
            "the action on this table",
            "ddb-exact-role",
            Some(policy("Allow", "dynamodb:GetItem", &table_arn())),
            200,
        ),
        (
            "a wildcard covering the action and the resource",
            "ddb-wildcard-role",
            Some(policy("Allow", "dynamodb:*", "*")),
            200,
        ),
        (
            "an explicit deny over a wildcard allow",
            "ddb-denied-role",
            Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"dynamodb:*","Resource":"*"},{"Effect":"Deny","Action":"dynamodb:GetItem","Resource":"*"}]}"#
                    .to_string(),
            ),
            400,
        ),
    ];

    for (case, role_name, document, expected) in rows {
        let role_arn = create_role(&router, role_name).await;
        if let Some(document) = &document {
            put_role_policy(&router, role_name, document).await;
        }
        let access_key_id = assume_role(&router, &role_arn).await;

        let (status, answer) = ddb(&router, &access_key_id, "GetItem", read_request()).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for the assumed-role caller, got \
             {status} {answer}"
        );
        if expected == 200 {
            assert_eq!(
                answer["Item"]["v"],
                json!({"S": "seeded"}),
                "case {case:?}: the authorized caller reads the stored item, got {answer}"
            );
        } else {
            assert_eq!(
                error_code(&answer),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {answer}"
            );
            let message = message_of(&answer);
            let principal = format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/{SESSION}");
            assert!(
                message.contains(&principal)
                    && message.contains("dynamodb:GetItem")
                    && message.contains(&table_arn()),
                "case {case:?}: the refusal must be built from the request (principal \
                 {principal}, action dynamodb:GetItem, resource {}), got {message:?}",
                table_arn()
            );
        }

        // Whatever the row, the account's own credential keeps reading its table.
        let (status, owner) = ddb(&router, AMBIENT_KEY, "GetItem", read_request()).await;
        assert_eq!(
            (status, owner["Item"]["v"].clone()),
            (200, json!({"S": "seeded"})),
            "case {case:?}: the account's own credential keeps reading its table, got \
             {status} {owner}"
        );
    }
}

#[tokio::test]
async fn the_tables_own_policy_grants_and_denies_the_principal_it_names() {
    let router = build_router();
    seed_table(&router).await;

    // A role the account grants NOTHING: its only possible permission is one the table's
    // own policy gives it.
    let role_arn = create_role(&router, "ddb-resource-granted-role").await;
    let access_key_id = assume_role(&router, &role_arn).await;
    let session = format!(
        "arn:aws:sts::{ACCOUNT}:assumed-role/ddb-resource-granted-role/{SESSION}"
    );

    // Before any grant exists, the call is refused: IAM's default is deny.
    let (status, refused) = ddb(&router, &access_key_id, "GetItem", read_request()).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "AccessDeniedException"),
        "case before any grant: expected AccessDeniedException, got {status} {refused}"
    );

    /// One resource-policy document naming the role as its principal.
    fn document(effect: &str, actions: Value, principal: &str) -> String {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": effect,
                "Principal": {"AWS": principal},
                "Action": actions,
                "Resource": format!("arn:aws:dynamodb:us-east-1:000000000000:table/{TABLE}"),
            }],
        })
        .to_string()
    }

    // A grant in the TABLE's own policy that names the role is a permission of its own.
    let (status, attached) = ddb(
        &router,
        AMBIENT_KEY,
        "PutResourcePolicy",
        json!({
            "ResourceArn": table_arn(),
            "Policy": document("Allow", json!(["dynamodb:GetItem", "dynamodb:PutItem"]), &role_arn),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutResourcePolicy expected 200, got {status} {attached}"
    );
    // "PutResourcePolicy is an asynchronous operation" and "the policy application is
    // eventually consistent", so the data plane is asked once the application is due.
    tokio::time::sleep(Duration::from_millis(3400)).await;
    let (status, granted) = ddb(&router, &access_key_id, "GetItem", read_request()).await;
    assert_eq!(
        (status, granted["Item"]["v"].clone()),
        (200, json!({"S": "seeded"})),
        "case granted by the table's own policy: expected the stored item, got {status} \
         {granted}"
    );

    // An explicit Deny on ONE action refuses that action and leaves the others alone: the
    // decision is per action, which is what a least-privilege policy is written for.
    let (status, revised) = ddb(
        &router,
        AMBIENT_KEY,
        "PutResourcePolicy",
        json!({
            "ResourceArn": table_arn(),
            "Policy": json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": role_arn},
                        "Action": ["dynamodb:GetItem", "dynamodb:PutItem"],
                        "Resource": table_arn(),
                    },
                    {
                        "Effect": "Deny",
                        "Principal": {"AWS": role_arn},
                        "Action": "dynamodb:GetItem",
                        "Resource": table_arn(),
                    },
                ],
            })
            .to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the revising PutResourcePolicy expected 200, got {status} {revised}"
    );
    tokio::time::sleep(Duration::from_millis(3400)).await;

    let (status, denied) = ddb(&router, &access_key_id, "GetItem", read_request()).await;
    assert_eq!(
        (status, error_code(&denied).as_str()),
        (400, "AccessDeniedException"),
        "case an explicit deny on the read: expected AccessDeniedException, got {status} \
         {denied}"
    );
    let message = message_of(&denied);
    assert!(
        message.contains(&session)
            && message.contains("dynamodb:GetItem")
            && message.contains("explicit deny in a resource-based policy"),
        "case an explicit deny on the read: the refusal names the principal, the action and \
         the document that denied it, got {message:?}"
    );

    // The SAME principal's write is still allowed: the deny named the read alone.
    let (status, written) = ddb(
        &router,
        &access_key_id,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "written-by-the-role"}}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case an explicit deny on the read: the same principal's PutItem is still allowed, \
         got {status} {written}"
    );

    // Removing the document leaves the role with nothing again, which is the revoke half
    // of the flow.
    let (status, removed) = ddb(
        &router,
        AMBIENT_KEY,
        "DeleteResourcePolicy",
        json!({"ResourceArn": table_arn()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteResourcePolicy expected 200, got {status} {removed}"
    );
    tokio::time::sleep(Duration::from_millis(3400)).await;
    let (status, revoked) = ddb(&router, &access_key_id, "GetItem", read_request()).await;
    assert_eq!(
        (status, error_code(&revoked).as_str()),
        (400, "AccessDeniedException"),
        "case after the grant was revoked: expected AccessDeniedException, got {status} \
         {revoked}"
    );
}

/// The table whose partitions are one tenant each, keyed on a STRING partition key.
const TENANTS_TABLE: &str = "Tenants";

/// A second table keyed on a NUMBER partition key, so the rule is exercised over a key
/// type whose values are not text.
const READINGS_TABLE: &str = "Readings";

/// The session tag key every conditioned policy below interpolates.
const TENANT_TAG: &str = "tenant";

/// The ARN of a table of this account and region, which is the resource a statement names.
fn arn_of(table: &str) -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{table}")
}

/// Create a table keyed on a partition key of type `key_type` — with a string sort key
/// when `sorted`, the shape a single-table multi-tenant design uses — and seed one item in
/// each of `partitions`, as the account's own credential.
async fn seed(
    router: &MultiServiceRouter,
    table: &str,
    key_type: &str,
    sorted: bool,
    partitions: &[Value],
) {
    let mut key_schema = vec![json!({"AttributeName": "pk", "KeyType": "HASH"})];
    let mut definitions = vec![json!({"AttributeName": "pk", "AttributeType": key_type})];
    if sorted {
        key_schema.push(json!({"AttributeName": "sk", "KeyType": "RANGE"}));
        definitions.push(json!({"AttributeName": "sk", "AttributeType": "S"}));
    }
    let (status, created) = ddb(
        router,
        AMBIENT_KEY,
        "CreateTable",
        json!({
            "TableName": table,
            "KeySchema": key_schema,
            "AttributeDefinitions": definitions,
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {table:?}: CreateTable expected 200, got {status} {created}"
    );
    tokio::time::sleep(Duration::from_millis(3400)).await;
    for partition in partitions {
        let mut item = serde_json::Map::new();
        item.insert("pk".to_string(), partition.clone());
        if sorted {
            item.insert("sk".to_string(), json!({"S": "seed"}));
        }
        item.insert("v".to_string(), json!({"S": "seeded"}));
        let (status, put) = ddb(
            router,
            AMBIENT_KEY,
            "PutItem",
            json!({"TableName": table, "Item": Value::Object(item)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {table:?} {partition}: PutItem expected 200, got {status} {put}"
        );
    }
}

/// Assume `role_arn` under the session name `session_name`, carrying `tag` as a session tag
/// when one is given, and return the access key id of the session STS minted.
async fn assume_role_tagged(
    router: &MultiServiceRouter,
    role_arn: &str,
    session_name: &str,
    tag: Option<(&str, &str)>,
) -> String {
    let mut form = format!(
        "Action=AssumeRole&Version=2011-06-15&RoleArn={role_arn}&RoleSessionName={session_name}"
    );
    if let Some((key, value)) = tag {
        form.push_str(&format!(
            "&Tags.member.1.Key={key}&Tags.member.1.Value={value}"
        ));
    }
    let (status, body) = query(router, "sts", form).await;
    assert_eq!(
        status, 200,
        "AssumeRole({role_arn}, {session_name}, {tag:?}) expected 200, got {status} {body}"
    );
    xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("AssumeRole({role_arn}, {session_name}): no key in {body}"))
}

/// One identity-based statement whose `Condition` scopes `actions` on `resources` to the
/// leading keys `leading` — written as the document a fine-grained DynamoDB policy is: "the
/// `dynamodb:LeadingKeys` condition key … limits the user's access to only the items whose
/// partition key value matches the user's identity", under `ForAllValues:StringEquals` so
/// that EVERY partition the request addresses must satisfy it.
fn conditioned(effect: &str, actions: Value, resources: Value, leading: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Action": actions,
            "Resource": resources,
            "Condition": {"ForAllValues:StringEquals": {"dynamodb:LeadingKeys": leading}},
        }],
    })
    .to_string()
}

/// A `GetItem` of one partition of the tenant table.
fn tenant_read(partition: &str) -> Value {
    json!({
        "TableName": TENANTS_TABLE,
        "Key": {"pk": {"S": partition}, "sk": {"S": "seed"}},
    })
}

/// RULE: a conditioned identity-based statement is decided against the condition keys the
/// data-plane request itself carries — `dynamodb:LeadingKeys` holding the partition-key
/// value(s) the call addresses, and `aws:PrincipalTag/<key>` holding the session tag STS
/// attached to the calling session, which is what a `${aws:PrincipalTag/<key>}` policy
/// variable stands for. So one document admits each tagged session to its OWN partition and
/// refuses it another tenant's, an untagged session of the same role is refused everywhere,
/// and a conditioned `Deny` refuses only the partitions its condition names.
///
/// The rows span what the rule generalises over: the tag value (two tenants, and a numeric
/// one), the partition-key TYPE (`S` and `N`), the operation (`GetItem`, `PutItem`,
/// `Query`, `BatchGetItem`, `Scan`), how many partitions one call addresses (one, two), how
/// the condition's value is written (a principal-tag variable, a literal), the effect it
/// conditions (`Allow`, `Deny`), and whether the session carries the tag at all.
#[tokio::test]
async fn a_conditioned_statement_is_decided_against_the_partition_and_the_session_tags() {
    let router = build_router();
    seed(
        &router,
        TENANTS_TABLE,
        "S",
        true,
        &[json!({"S": "tenant-one"}), json!({"S": "tenant-two"})],
    )
    .await;
    seed(
        &router,
        READINGS_TABLE,
        "N",
        false,
        &[json!({"N": "7"}), json!({"N": "8"})],
    )
    .await;

    // ONE role, whose single statement scopes every item action it grants to the partition
    // named by the caller's own session tag.
    let tenant_role = "ddb-leading-keys-tenant-role";
    let tenant_role_arn =
        create_role_trusting(&router, tenant_role, TAGGING_TRUST_POLICY).await;
    put_role_policy(
        &router,
        tenant_role,
        &conditioned(
            "Allow",
            json!([
                "dynamodb:GetItem",
                "dynamodb:PutItem",
                "dynamodb:Query",
                "dynamodb:BatchGetItem",
                "dynamodb:Scan",
            ]),
            json!([arn_of(TENANTS_TABLE), arn_of(READINGS_TABLE)]),
            json!([format!("${{aws:PrincipalTag/{TENANT_TAG}}}")]),
        ),
    )
    .await;

    // A second role conditions the same key on a LITERAL partition instead of a variable:
    // the request's leading key is what the clause is decided against either way.
    let literal_role = "ddb-leading-keys-literal-role";
    let literal_role_arn = create_role(&router, literal_role).await;
    put_role_policy(
        &router,
        literal_role,
        &conditioned(
            "Allow",
            json!("dynamodb:GetItem"),
            json!(arn_of(TENANTS_TABLE)),
            json!(["tenant-two"]),
        ),
    )
    .await;

    // A third role is granted everything and DENIED the read of one partition: a deny whose
    // condition does not hold for the addressed partition refuses nothing.
    let denied_role = "ddb-leading-keys-denied-role";
    let denied_role_arn = create_role(&router, denied_role).await;
    put_role_policy(
        &router,
        denied_role,
        &json!({
            "Version": "2012-10-17",
            "Statement": [
                {"Effect": "Allow", "Action": "dynamodb:*", "Resource": "*"},
                {
                    "Effect": "Deny",
                    "Action": "dynamodb:GetItem",
                    "Resource": "*",
                    "Condition": {
                        "ForAllValues:StringEquals": {"dynamodb:LeadingKeys": ["tenant-two"]},
                    },
                },
            ],
        })
        .to_string(),
    )
    .await;

    let one = assume_role_tagged(
        &router,
        &tenant_role_arn,
        "one",
        Some((TENANT_TAG, "tenant-one")),
    )
    .await;
    let two = assume_role_tagged(
        &router,
        &tenant_role_arn,
        "two",
        Some((TENANT_TAG, "tenant-two")),
    )
    .await;
    let seven = assume_role_tagged(&router, &tenant_role_arn, "seven", Some((TENANT_TAG, "7")))
        .await;
    let untagged = assume_role_tagged(&router, &tenant_role_arn, "untagged", None).await;
    let literal = assume_role_tagged(&router, &literal_role_arn, "literal", None).await;
    let denied = assume_role_tagged(&router, &denied_role_arn, "denied", None).await;

    // (case, the session's key, the role behind it, the operation, its body, the status)
    let rows: Vec<(&str, &String, &str, &str, Value, u16)> = vec![
        (
            "the tagged session reads its own partition",
            &one,
            tenant_role,
            "GetItem",
            tenant_read("tenant-one"),
            200,
        ),
        (
            "the tagged session reads another tenant's partition",
            &one,
            tenant_role,
            "GetItem",
            tenant_read("tenant-two"),
            400,
        ),
        (
            "the other tenant's session reads its own partition",
            &two,
            tenant_role,
            "GetItem",
            tenant_read("tenant-two"),
            200,
        ),
        (
            "the other tenant's session reads the first tenant's partition",
            &two,
            tenant_role,
            "GetItem",
            tenant_read("tenant-one"),
            400,
        ),
        (
            "an untagged session of the same role reads one partition",
            &untagged,
            tenant_role,
            "GetItem",
            tenant_read("tenant-one"),
            400,
        ),
        (
            "an untagged session of the same role reads the other partition",
            &untagged,
            tenant_role,
            "GetItem",
            tenant_read("tenant-two"),
            400,
        ),
        (
            "the tagged session writes its own partition",
            &one,
            tenant_role,
            "PutItem",
            json!({
                "TableName": TENANTS_TABLE,
                "Item": {"pk": {"S": "tenant-one"}, "sk": {"S": "written"}},
            }),
            200,
        ),
        (
            "the tagged session writes another tenant's partition",
            &one,
            tenant_role,
            "PutItem",
            json!({
                "TableName": TENANTS_TABLE,
                "Item": {"pk": {"S": "tenant-two"}, "sk": {"S": "written"}},
            }),
            400,
        ),
        (
            "the tagged session queries its own partition",
            &one,
            tenant_role,
            "Query",
            json!({
                "TableName": TENANTS_TABLE,
                "KeyConditionExpression": "pk = :p",
                "ExpressionAttributeValues": {":p": {"S": "tenant-one"}},
            }),
            200,
        ),
        (
            "the tagged session queries another tenant's partition",
            &one,
            tenant_role,
            "Query",
            json!({
                "TableName": TENANTS_TABLE,
                "KeyConditionExpression": "pk = :p",
                "ExpressionAttributeValues": {":p": {"S": "tenant-two"}},
            }),
            400,
        ),
        (
            "a batch addressing two keys of the session's own partition",
            &one,
            tenant_role,
            "BatchGetItem",
            json!({"RequestItems": {TENANTS_TABLE: {"Keys": [
                {"pk": {"S": "tenant-one"}, "sk": {"S": "seed"}},
                {"pk": {"S": "tenant-one"}, "sk": {"S": "absent"}},
            ]}}}),
            200,
        ),
        (
            "a batch addressing one key of each tenant's partition",
            &one,
            tenant_role,
            "BatchGetItem",
            json!({"RequestItems": {TENANTS_TABLE: {"Keys": [
                {"pk": {"S": "tenant-one"}, "sk": {"S": "seed"}},
                {"pk": {"S": "tenant-two"}, "sk": {"S": "seed"}},
            ]}}}),
            400,
        ),
        (
            // A table walk names no partition, so the request carries no value for the key
            // AND declares that it models none: the clause stays undecided and the grant
            // does not reach the call. The request is NOT read as carrying the empty set
            // of leading keys, which `ForAllValues:` would hold on and so would turn this
            // conditioned grant into table-wide access.
            "a scan, which addresses no partition the condition could be decided on",
            &one,
            tenant_role,
            "Scan",
            json!({"TableName": TENANTS_TABLE}),
            400,
        ),
        (
            "a numeric partition the session's tag names",
            &seven,
            tenant_role,
            "GetItem",
            json!({"TableName": READINGS_TABLE, "Key": {"pk": {"N": "7"}}}),
            200,
        ),
        (
            "the same number spelled with a fraction, which names that same partition",
            &seven,
            tenant_role,
            "GetItem",
            json!({"TableName": READINGS_TABLE, "Key": {"pk": {"N": "7.0"}}}),
            200,
        ),
        (
            "a numeric partition the session's tag does not name",
            &seven,
            tenant_role,
            "GetItem",
            json!({"TableName": READINGS_TABLE, "Key": {"pk": {"N": "8"}}}),
            400,
        ),
        (
            "a literal condition value, on the partition it names",
            &literal,
            literal_role,
            "GetItem",
            tenant_read("tenant-two"),
            200,
        ),
        (
            "a literal condition value, on a partition it does not name",
            &literal,
            literal_role,
            "GetItem",
            tenant_read("tenant-one"),
            400,
        ),
        (
            "a conditioned deny, on a partition its condition does not name",
            &denied,
            denied_role,
            "GetItem",
            tenant_read("tenant-one"),
            200,
        ),
        (
            "a conditioned deny, on the partition its condition names",
            &denied,
            denied_role,
            "GetItem",
            tenant_read("tenant-two"),
            400,
        ),
    ];

    for (case, key, role_name, operation, body, expected) in rows {
        let table = body
            .get("TableName")
            .and_then(Value::as_str)
            .unwrap_or(TENANTS_TABLE)
            .to_string();
        let (status, answer) = ddb(&router, key, operation, body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: {operation} expected HTTP {expected}, got {status} {answer}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&answer),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {answer}"
            );
            let message = message_of(&answer);
            let session = format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/");
            assert!(
                message.contains(&session)
                    && message.contains(&format!("dynamodb:{operation}"))
                    && message.contains(&arn_of(&table)),
                "case {case:?}: the refusal is built from the request (a session of \
                 {session}…, action dynamodb:{operation}, resource {}), got {message:?}",
                arn_of(&table)
            );
        }
    }

    // The write the tagged session was allowed to make is really there, read back through
    // the reader the same policy admits: the grant served a write, not just a 200.
    let (status, written) = ddb(
        &router,
        &one,
        "GetItem",
        json!({
            "TableName": TENANTS_TABLE,
            "Key": {"pk": {"S": "tenant-one"}, "sk": {"S": "written"}},
        }),
    )
    .await;
    assert_eq!(
        (status, written["Item"]["sk"].clone()),
        (200, json!({"S": "written"})),
        "case the admitted write: the item it stored is read back, got {status} {written}"
    );
    // And the write the refused row attempted left the other tenant's partition untouched.
    let (status, other) = ddb(
        &router,
        AMBIENT_KEY,
        "GetItem",
        json!({
            "TableName": TENANTS_TABLE,
            "Key": {"pk": {"S": "tenant-two"}, "sk": {"S": "written"}},
        }),
    )
    .await;
    assert_eq!(
        (status, other.get("Item").cloned()),
        (200, None),
        "case the refused write: nothing was written to the other tenant's partition, got \
         {status} {other}"
    );
}
