// SPDX-License-Identifier: CC0-1.0

//! The SignIn resource-permission statement and console-authorization configuration,
//! end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones the service states for its two
//! resources: a permission statement is whatever the put stored, every reader
//! (`ListResourcePermissionStatements`, `GetResourcePolicy`) computes its answer from
//! those records, and a console-authorization configuration is reported exactly as the
//! put persisted it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_signin_signin::state::{
    CONSOLE_AUTHORIZATION_TARGET_QUOTA, PERMISSION_STATEMENT_QUOTA,
};
use service_com_amazonaws_signin_signin::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service — what routes a request to signin.
const SIGNED_FOR_SIGNIN: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/signin/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account id the emulator answers for, and the target a console-authorization call
/// addresses when it names none.
const EMULATOR_ACCOUNT: &str = "000000000000";

/// The policy language version an AWS resource-based policy document declares.
const POLICY_VERSION: &str = "2012-10-17";

/// The condition operator and AWS global condition key each `PutResourcePermissionStatement`
/// member is expressed with, as `(member, operator, condition key)`.
///
/// Spelled out here rather than imported from the crate, so a wrong mapping in
/// `src/state.rs` fails this test. Source: `ConditionBlock` is modelled
/// `map<ConditionType, map<String, list<String>>>`, which is the IAM policy-document
/// condition grammar `"Condition": {"<operator>": {"<condition key>": [values]}}`, and
/// the condition keys are the AWS global ones naming each member's subject. The SignIn
/// and the console VPC endpoint are two values of the one `aws:SourceVpce` key.
const CONDITION_MAPPING: &[(&str, &str, &str)] = &[
    ("sourceVpc", "StringEquals", "aws:SourceVpc"),
    ("signinSourceVpce", "StringEquals", "aws:SourceVpce"),
    ("consoleSourceVpce", "StringEquals", "aws:SourceVpce"),
    ("requestedRegion", "StringEquals", "aws:RequestedRegion"),
    ("vpcSourceIp", "IpAddress", "aws:VpcSourceIp"),
    ("sourceIp", "IpAddress", "aws:SourceIp"),
    ("excludedPrincipal", "ArnNotEquals", "aws:PrincipalArn"),
];

/// The scope each form of `TargetId` implies, as `(target, scope)`.
///
/// Written here from `TargetId`'s own pattern
/// `^(\d{12}|o-[a-z0-9]{10}|r-[0-9a-z]{4,32})$` — an account id, an AWS Organizations
/// organization id and an organizational root id — with a root id at each published bound
/// of `TargetId`'s `@length(min: 12, max: 32)`, which is the narrower of the two
/// constraints on a root id.
fn target_scope_cases() -> Vec<(&'static str, String, &'static str)> {
    vec![
        ("a 12-digit account id", "123456789012".to_string(), "ACCOUNT"),
        (
            "the emulator's own account id",
            EMULATOR_ACCOUNT.to_string(),
            "ACCOUNT",
        ),
        (
            "an organization id",
            "o-abc1234567".to_string(),
            "ORGANIZATION",
        ),
        (
            "another organization id",
            "o-0zzzzzzzz9".to_string(),
            "ORGANIZATION",
        ),
        (
            "a root id at the 12-character length floor",
            "r-0123456789".to_string(),
            "ROOT",
        ),
        (
            "a root id at the 32-character length ceiling",
            format!("r-{}", "a".repeat(30)),
            "ROOT",
        ),
    ]
}

/// `StatementId`'s `^[A-Za-z0-9+/]{64}=?$`, checked here rather than through the crate's
/// own predicate so a wrong predicate cannot make this test pass.
fn matches_statement_id_pattern(value: &str) -> bool {
    let body = value.strip_suffix('=').unwrap_or(value);
    body.len() == 64
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let code = self.body["__type"].as_str().unwrap_or_default();
        code.rsplit('#').next().unwrap_or_default()
    }

    /// The `statementId` a put reported, or the empty string.
    fn statement_id(&self) -> String {
        self.body["statementId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The `sid` of every statement a list page carries, in order.
    fn sids(&self) -> Vec<String> {
        self.body["permissionStatements"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["sid"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `nextToken` a list page carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }

    /// The `Statement` list of a rendered policy document.
    fn policy_statements(&self) -> Vec<Value> {
        self.body["signinResourceBasedPolicy"]["Statement"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "signin.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_SIGNIN)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn put_statement(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/put-resource-permission-statement", request).await
}

async fn list_statements(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/list-resource-permission-statements", request).await
}

async fn get_policy(router: &MultiServiceRouter) -> Answer {
    call(router, "/get-resource-policy", json!({})).await
}

async fn delete_statement(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/delete-resource-permission-statement", request).await
}

async fn put_configuration(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/put-console-authorization-configuration", request).await
}

async fn get_configuration(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/get-console-authorization-configuration", request).await
}

/// The condition block a put request's members must be reported with: `Value::Null` when
/// the request carries no condition member at all.
fn expected_condition(request: &Value) -> Value {
    let mut block: Map<String, Value> = Map::new();
    for (member, operator, key) in CONDITION_MAPPING {
        let Some(value) = request.get(*member).and_then(Value::as_str) else {
            continue;
        };
        let operator_entry = block
            .entry((*operator).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let keys = operator_entry
            .as_object_mut()
            .expect("a condition operator holds a map of condition keys");
        keys.entry((*key).to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("a condition key holds a list of values")
            .push(json!(value));
    }
    if block.is_empty() {
        Value::Null
    } else {
        Value::Object(block)
    }
}

/// Every statement the store holds, listed with the largest page the model admits.
async fn all_statements(router: &MultiServiceRouter) -> Vec<Value> {
    let answer = list_statements(router, json!({"maxResults": 100})).await;
    assert_eq!(
        answer.status, 200,
        "a list must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["permissionStatements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A put stores the statement it reports: the id it answers names a statement
/// `ListResourcePermissionStatements` reports with exactly the condition keys the request
/// supplied, and `GetResourcePolicy` renders that same condition block into the account's
/// policy document.
///
/// The rows differ in which condition members the request carries, how many, and of which
/// kind — the dimensions the projection generalises over.
#[tokio::test]
async fn a_put_stores_the_statement_every_reader_reports() {
    let cases = [
        ("no condition member at all", json!({})),
        (
            "a VPC and its region",
            json!({"sourceVpc": "vpc-0123456789abcdef0", "requestedRegion": "us-east-1"}),
        ),
        (
            "a short-form VPC id and its region",
            json!({"sourceVpc": "vpc-0123abcd", "requestedRegion": "eu-west-1"}),
        ),
        (
            "both VPC endpoints and the region",
            json!({
                "signinSourceVpce": "vpce-0123abcd",
                "consoleSourceVpce": "vpce-abcdef0123456789",
                "requestedRegion": "ap-southeast-2",
            }),
        ),
        (
            "only the SignIn endpoint",
            json!({"signinSourceVpce": "vpce-9876543210", "requestedRegion": "us-west-2"}),
        ),
        ("only a source address", json!({"sourceIp": "203.0.113.0/24"})),
        (
            "both addresses",
            json!({"sourceIp": "198.51.100.7", "vpcSourceIp": "10.0.0.0/8"}),
        ),
        (
            "only an excluded principal",
            json!({"excludedPrincipal": "arn:aws:iam::123456789012:role/Breakglass"}),
        ),
        (
            "every condition member at once",
            json!({
                "sourceVpc": "vpc-00112233445566778",
                "signinSourceVpce": "vpce-aaaa1111bbbb",
                "consoleSourceVpce": "vpce-cccc2222dddd",
                "vpcSourceIp": "172.16.0.0/12",
                "sourceIp": "192.0.2.1",
                "requestedRegion": "ca-central-1",
                "excludedPrincipal": "arn:aws:iam::123456789012:root",
            }),
        ),
    ];

    let router = emulator();
    let mut created = Vec::new();
    for (case, request) in &cases {
        let answer = put_statement(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a put must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        let statement_id = answer.statement_id();
        assert!(
            matches_statement_id_pattern(&statement_id),
            "case {case:?}: statementId must satisfy ^[A-Za-z0-9+/]{{64}}=?$, got \
             {statement_id:?}"
        );
        created.push((case, request.clone(), statement_id));
    }

    let mut ids: Vec<&String> = created.iter().map(|(_, _, id)| id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        created.len(),
        "each put must store its own statement, got {created:?}"
    );

    // Every stored statement is reported by the list, with the conditions its put
    // supplied — read in a request of its own, so the answer can only come from state.
    let listed = all_statements(&router).await;
    assert_eq!(
        listed.len(),
        created.len(),
        "the list must report exactly the {} statements that were put, got {listed:?}",
        created.len()
    );
    for (case, request, statement_id) in &created {
        let summary = listed
            .iter()
            .find(|item| item["sid"].as_str() == Some(statement_id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the list must report the statement {statement_id:?}")
            });
        assert_eq!(
            summary["condition"],
            expected_condition(request),
            "case {case:?}: expected the condition keys the put supplied, got {:?}",
            summary["condition"]
        );
    }

    // And the policy document is rendered from the same records: one statement each,
    // carrying the same condition blocks.
    let policy = get_policy(&router).await;
    assert_eq!(
        policy.status, 200,
        "the policy must be served, got {} with {:?}",
        policy.status, policy.body
    );
    assert_eq!(
        policy.body["signinResourceBasedPolicy"]["Version"],
        json!(POLICY_VERSION),
        "the policy must declare its language version, got {:?}",
        policy.body
    );
    let rendered = policy.policy_statements();
    assert_eq!(
        rendered.len(),
        created.len(),
        "the policy must carry one statement per stored permission statement, got \
         {rendered:?}"
    );
    for (case, request, _) in &created {
        let expected = expected_condition(request);
        let matches = rendered
            .iter()
            .filter(|statement| statement["Condition"] == expected)
            .count();
        assert!(
            matches >= 1,
            "case {case:?}: the policy must carry a statement whose Condition is \
             {expected:?}, got {rendered:?}"
        );
    }
}

/// `clientToken` is an `@idempotencyToken` on an `@idempotent` operation: the same token
/// always names the statement it first created, a different token names a different one,
/// and no token at all stores a new statement every time.
#[tokio::test]
async fn a_repeated_put_with_one_client_token_stores_one_statement() {
    // `ClientToken` is `@length(1..64)` with pattern `^[!-~]+$`, so the rows span a
    // one-character token, a 64-character one and punctuation the pattern admits, each
    // repeated a different number of times.
    let cases: Vec<(&str, String, usize)> = vec![
        ("a one-character token", "t".to_string(), 2),
        ("a 64-character token", "k".repeat(64), 3),
        ("punctuation only", "!~/+=".to_string(), 4),
        ("a token repeated once", "single-attempt".to_string(), 1),
    ];

    let router = emulator();
    let mut distinct = Vec::new();
    for (case, token, attempts) in &cases {
        let request = json!({
            "sourceIp": "203.0.113.1",
            "clientToken": token,
        });
        let mut ids = Vec::new();
        for attempt in 0..*attempts {
            let answer = put_statement(&router, request.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: attempt {attempt} must be accepted, got {} with {:?}",
                answer.status, answer.body
            );
            ids.push(answer.statement_id());
        }
        let first = ids[0].clone();
        assert!(
            ids.iter().all(|id| id == &first),
            "case {case:?}: every attempt under one clientToken must answer the same \
             statementId, got {ids:?}"
        );
        distinct.push(first);
    }

    // Two puts carrying no token at all are two statements.
    let untokened_first = put_statement(&router, json!({"sourceIp": "203.0.113.2"}))
        .await
        .statement_id();
    let untokened_second = put_statement(&router, json!({"sourceIp": "203.0.113.2"}))
        .await
        .statement_id();
    assert_ne!(
        untokened_first, untokened_second,
        "a put with no clientToken must store a new statement every time"
    );
    distinct.push(untokened_first);
    distinct.push(untokened_second);

    let mut unique = distinct.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        distinct.len(),
        "distinct client tokens must name distinct statements, got {distinct:?}"
    );

    let listed: Vec<String> = all_statements(&router)
        .await
        .iter()
        .map(|item| item["sid"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected = distinct.clone();
    expected.sort();
    let mut actual = listed.clone();
    actual.sort();
    assert_eq!(
        actual, expected,
        "the store must hold exactly one statement per distinct token, got {listed:?}"
    );
}

/// `maxResults` bounds a page and `nextToken` resumes exactly where the previous page
/// ended, with no token on the last page; paging reports every stored statement once, in
/// the order the list reports them, and listing is a read: it leaves the store alone.
#[tokio::test]
async fn the_list_pages_at_max_results() {
    // Rows differ in how many statements are stored and how large a page is asked for,
    // including the published bounds of `maxResults`' `@range(min: 1, max: 100)`.
    let cases = [
        ("nothing stored, smallest page", 0usize, Some(1)),
        ("one statement, smallest page", 1, Some(1)),
        ("five statements, smallest page", 5, Some(1)),
        ("five statements, page of two", 5, Some(2)),
        ("six statements, page of three", 6, Some(3)),
        ("four statements, largest page", 4, Some(100)),
        ("seven statements, no page size", 7, None),
    ];

    for (case, stored, max_results) in cases {
        let router = emulator();
        let mut minted = Vec::new();
        for index in 0..stored {
            let answer = put_statement(
                &router,
                json!({"sourceIp": format!("198.51.100.{index}")}),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: put {index} must be accepted, got {} with {:?}",
                answer.status, answer.body
            );
            minted.push(answer.statement_id());
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = Map::new();
            if let Some(max) = max_results {
                request.insert("maxResults".to_string(), json!(max));
            }
            if let Some(token) = token.as_ref() {
                request.insert("nextToken".to_string(), json!(token));
            }
            let answer = list_statements(&router, Value::Object(request)).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: page {pages} must be served, got {} with {:?}",
                answer.status, answer.body
            );
            let page = answer.sids();
            if let Some(max) = max_results {
                assert!(
                    page.len() <= usize::try_from(max).expect("a positive page size"),
                    "case {case:?}: page {pages} must hold at most {max} statements, got \
                     {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            match answer.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= stored + 2,
                "case {case:?}: paging must terminate, saw {pages} pages for {stored} \
                 statements"
            );
        }

        assert_eq!(
            seen.len(),
            stored,
            "case {case:?}: paging must report each of the {stored} statements exactly \
             once, got {seen:?}"
        );
        let mut sorted_seen = seen.clone();
        sorted_seen.sort();
        let mut sorted_minted = minted.clone();
        sorted_minted.sort();
        assert_eq!(
            sorted_seen, sorted_minted,
            "case {case:?}: paging must report exactly the statements that were put"
        );
        assert_eq!(
            seen,
            minted,
            "case {case:?}: paging must report the statements in mint order, got {seen:?}"
        );

        // A list is a read: a later caller sees the same store.
        let again: Vec<String> = all_statements(&router)
            .await
            .iter()
            .map(|item| item["sid"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            again, minted,
            "case {case:?}: listing must not change what a later caller sees"
        );
    }

    // A token this operation never issued is a bad request, not an empty page. The rows
    // span both ways a token can fail to be one of this operation's: a shape the ids it
    // mints never take, and a value its mint has not reached. The store holds two
    // statements here, so a page IS available — what is refused is the token, not the
    // query.
    let router = emulator();
    for index in 0..2 {
        put_statement(&router, json!({"sourceIp": format!("192.0.2.{index}")})).await;
    }
    for (case, token) in [
        ("not a statement id at all", "not-a-token".to_string()),
        ("one character short", "0".repeat(63)),
        ("one character too long", "0".repeat(65)),
        ("the zero value no mint issues", "0".repeat(64)),
        ("a value far past the ids minted", "f".repeat(64)),
        (
            "a character the mint never produces",
            format!("{}+", "0".repeat(63)),
        ),
        (
            "an upper-case id the mint never produces",
            format!("{}A", "0".repeat(63)),
        ),
    ] {
        let refused = list_statements(&router, json!({"nextToken": token})).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an unissued nextToken must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }

    // And the token this operation DID issue is served.
    let first_page = list_statements(&router, json!({"maxResults": 1})).await;
    let issued = first_page
        .next_token()
        .expect("a page bounded below the store size carries a nextToken");
    let resumed = list_statements(&router, json!({"nextToken": issued.clone()})).await;
    assert_eq!(
        resumed.status, 200,
        "the token {issued:?} this operation issued must be served, got {} with {:?}",
        resumed.status, resumed.body
    );
}

/// A delete removes the statement from every reader, and an id that names no stored
/// statement is refused by kind: a well-formed id nothing holds is a 404, one the model's
/// pattern does not admit is a 400, and a retry under the token that already deleted it is
/// the same success.
#[tokio::test]
async fn a_delete_removes_the_statement_from_every_reader() {
    let router = emulator();
    let mut ids = Vec::new();
    for index in 0..4 {
        ids.push(
            put_statement(
                &router,
                json!({"sourceIp": format!("192.0.2.{index}"), "requestedRegion": "us-east-1"}),
            )
            .await
            .statement_id(),
        );
    }

    // Delete them one at a time, checking both readers after each.
    for (index, id) in ids.clone().into_iter().enumerate() {
        let answer = delete_statement(&router, json!({"statementId": id})).await;
        assert_eq!(
            answer.status, 200,
            "case {index}: a delete of a stored statement must be accepted, got {} with {:?}",
            answer.status, answer.body
        );

        let remaining: Vec<String> = all_statements(&router)
            .await
            .iter()
            .map(|item| item["sid"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = ids[index + 1..].to_vec();
        assert_eq!(
            remaining, expected,
            "case {index}: the list must report exactly the statements still stored"
        );
        assert_eq!(
            get_policy(&router).await.policy_statements().len(),
            expected.len(),
            "case {index}: the policy must carry one statement per statement still stored"
        );

        // A second delete of the same id, with no token to replay, is a 404.
        let again = delete_statement(&router, json!({"statementId": id})).await;
        assert_eq!(
            again.status, 404,
            "case {index}: deleting an already deleted statement must answer 404, got {} \
             with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ResourceNotFoundException",
            "case {index}: expected ResourceNotFoundException, got {:?}",
            again.body
        );
    }

    // An empty store renders a policy with an empty statement list, not a canned one.
    let policy = get_policy(&router).await;
    assert_eq!(
        policy.policy_statements(),
        Vec::<Value>::new(),
        "with nothing stored the policy must carry an empty statement list, got {:?}",
        policy.body
    );

    // A retried delete under the token that removed the statement answers the same
    // success rather than the 404 an unknown id earns.
    let doomed = put_statement(&router, json!({"sourceIp": "192.0.2.9"}))
        .await
        .statement_id();
    for attempt in 0..3 {
        let answer = delete_statement(
            &router,
            json!({"statementId": doomed, "clientToken": "delete-once"}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "attempt {attempt}: a retried delete under one clientToken must answer the \
             same success, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // Refusals by kind. A well-formed id nothing holds is a 404; an id the pattern does
    // not admit is a 400.
    let cases = [
        ("a well-formed id nothing holds", "0".repeat(64), 404, "ResourceNotFoundException"),
        ("another well-formed id nothing holds", "f".repeat(64), 404, "ResourceNotFoundException"),
        ("a padded well-formed id nothing holds", format!("{}=", "A".repeat(64)), 404, "ResourceNotFoundException"),
        ("not a statement id at all", "nope".to_string(), 400, "ValidationException"),
        ("one character short", "a".repeat(63), 400, "ValidationException"),
        ("one character too long", "a".repeat(65), 400, "ValidationException"),
        ("a character outside the alphabet", format!("{}-", "a".repeat(63)), 400, "ValidationException"),
        ("empty", String::new(), 400, "ValidationException"),
    ];
    for (case, statement_id, status, code) in cases {
        let answer = delete_statement(&router, json!({"statementId": statement_id})).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} for {statement_id:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answer.body
        );
    }
}

/// `requestedRegion` documents the one cross-member rule the model cannot express:
/// "Required when sourceVpc or signinSourceVpce/consoleSourceVpce is provided". A put
/// that names a VPC or a VPC endpoint without it is refused and stores nothing; a put
/// that names neither is accepted without it.
#[tokio::test]
async fn a_vpc_member_requires_the_requested_region() {
    let cases = [
        ("a VPC alone", json!({"sourceVpc": "vpc-0123abcd"}), 400),
        (
            "the SignIn endpoint alone",
            json!({"signinSourceVpce": "vpce-0123abcd"}),
            400,
        ),
        (
            "the console endpoint alone",
            json!({"consoleSourceVpce": "vpce-0123abcd"}),
            400,
        ),
        (
            "a VPC and both endpoints, no region",
            json!({
                "sourceVpc": "vpc-0123abcd",
                "signinSourceVpce": "vpce-0123abcd",
                "consoleSourceVpce": "vpce-4567efgh",
            }),
            400,
        ),
        (
            "a VPC with its region",
            json!({"sourceVpc": "vpc-0123abcd", "requestedRegion": "us-east-1"}),
            200,
        ),
        (
            "the console endpoint with its region",
            json!({"consoleSourceVpce": "vpce-0123abcd", "requestedRegion": "sa-east-1"}),
            200,
        ),
        ("an address, no region", json!({"sourceIp": "192.0.2.1"}), 200),
        (
            "a VPC address, no region",
            json!({"vpcSourceIp": "10.1.2.3"}),
            200,
        ),
        (
            "an excluded principal, no region",
            json!({"excludedPrincipal": "arn:aws:iam::123456789012:user/Alice"}),
            200,
        ),
        (
            "a region on its own",
            json!({"requestedRegion": "ap-south-1"}),
            200,
        ),
        ("no member at all", json!({}), 200),
    ];

    let router = emulator();
    let mut accepted = 0usize;
    for (case, request, status) in cases {
        let answer = put_statement(&router, request.clone()).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
        } else {
            accepted += 1;
        }
        assert_eq!(
            all_statements(&router).await.len(),
            accepted,
            "case {case:?}: only an accepted put may store a statement"
        );
    }
}

/// The store is bounded: past its quota a put is refused with the modelled
/// `ServiceQuotaExceededException` instead of growing the state, and a delete frees room
/// for exactly one more.
///
/// The bound is imported because the test has to know where it lies; what is asserted is
/// the behaviour at it, not the number.
#[tokio::test]
async fn the_permission_statement_store_is_bounded_by_its_quota() {
    let router = emulator();
    for index in 0..PERMISSION_STATEMENT_QUOTA {
        let answer = put_statement(&router, json!({})).await;
        assert_eq!(
            answer.status, 200,
            "put {index} of {PERMISSION_STATEMENT_QUOTA} must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let refused = put_statement(&router, json!({})).await;
    assert_eq!(
        refused.status, 402,
        "a put past the quota must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {:?}",
        refused.body
    );

    let first = all_statements(&router)
        .await
        .first()
        .and_then(|item| item["sid"].as_str().map(str::to_string))
        .expect("the store holds statements");
    assert_eq!(
        delete_statement(&router, json!({"statementId": first}))
            .await
            .status,
        200,
        "the delete that frees quota must be accepted"
    );
    assert_eq!(
        put_statement(&router, json!({})).await.status,
        200,
        "a put must be accepted again once a statement has been deleted"
    );
}

/// A console-authorization configuration is whatever the put persisted for that target:
/// the get reports it in a request of its own, the scope follows the form of the
/// `targetId`, a target that was never configured is a 404, and a request naming no
/// target addresses the caller's own account.
#[tokio::test]
async fn a_console_authorization_configuration_round_trips_per_target() {
    let router = emulator();

    // Nothing is configured yet, so every target is missing — never a default.
    for (case, target_id, _) in target_scope_cases() {
        let answer = get_configuration(&router, json!({"targetId": target_id})).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unconfigured target must answer 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    for (case, target_id, scope) in target_scope_cases() {
        let written = put_configuration(&router, json!({"targetId": target_id})).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a put must be accepted, got {} with {:?}",
            written.status, written.body
        );
        assert_eq!(
            written.body["targetId"],
            json!(target_id),
            "case {case:?}: the put must report the target it configured, got {:?}",
            written.body
        );
        assert_eq!(
            written.body["scope"],
            json!(scope),
            "case {case:?}: expected scope {scope:?}, got {:?}",
            written.body
        );
        assert_eq!(
            written.body["consoleAuthorizationEnabled"],
            json!(true),
            "case {case:?}: a put enables console authorization, got {:?}",
            written.body
        );

        // A separate request must observe exactly what was stored.
        let read = get_configuration(&router, json!({"targetId": target_id})).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the configured target must be served, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, written.body,
            "case {case:?}: the get must report what the put persisted"
        );

        // And a repeated put is idempotent: the same three members, one record.
        let again = put_configuration(&router, json!({"targetId": target_id})).await;
        assert_eq!(
            again.body, written.body,
            "case {case:?}: a repeated put must report the same configuration"
        );
    }

    // A target nothing configured is still missing, even with others stored.
    let unconfigured = get_configuration(&router, json!({"targetId": "999999999999"})).await;
    assert_eq!(
        unconfigured.status, 404,
        "a target nothing configured must answer 404, got {} with {:?}",
        unconfigured.status, unconfigured.body
    );

    // Absent a `targetId` the target is the caller's own account.
    let defaulted = put_configuration(&router, json!({})).await;
    assert_eq!(
        defaulted.status, 200,
        "a put naming no target must be accepted, got {} with {:?}",
        defaulted.status, defaulted.body
    );
    assert_eq!(
        defaulted.body["targetId"],
        json!(EMULATOR_ACCOUNT),
        "a put naming no target must configure the caller's own account, got {:?}",
        defaulted.body
    );
    let read_defaulted = get_configuration(&router, json!({})).await;
    assert_eq!(
        read_defaulted.body, defaulted.body,
        "a get naming no target must report the caller's own account's configuration"
    );
    assert_eq!(
        get_configuration(&router, json!({"targetId": EMULATOR_ACCOUNT}))
            .await
            .body,
        defaulted.body,
        "naming the caller's account explicitly must read the same record"
    );
}

/// A `targetId` the model's pattern does not admit is refused, and nothing is stored for
/// it.
#[tokio::test]
async fn a_malformed_target_id_is_refused() {
    // Rows span the ways a value can fall outside
    // `^(\d{12}|o-[a-z0-9]{10}|r-[0-9a-z]{4,32})$` and its `@length(12..32)`.
    let cases = [
        ("eleven digits", "12345678901".to_string()),
        ("thirteen digits", "1234567890123".to_string()),
        ("an organization id one character short", "o-abc123456".to_string()),
        ("an organization id with upper case", "o-ABC1234567".to_string()),
        (
            "a root id one character under the length floor",
            "r-012345678".to_string(),
        ),
        (
            "a root id one character over the length ceiling",
            format!("r-{}", "a".repeat(31)),
        ),
        ("a root id with too few characters after the prefix", "r-abc".to_string()),
        ("a word", "not-a-target".to_string()),
    ];

    let router = emulator();
    for (case, target_id) in cases {
        for (operation, answer) in [
            (
                "put",
                put_configuration(&router, json!({"targetId": target_id.clone()})).await,
            ),
            (
                "get",
                get_configuration(&router, json!({"targetId": target_id.clone()})).await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: {operation} of {target_id:?} must be refused, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: {operation} expected ValidationException, got {:?}",
                answer.body
            );
        }
    }
}

/// The console-authorization store is bounded: once it holds its quota of distinct
/// targets, a put naming a NEW target is refused with a modelled error and stores
/// nothing, while a put naming a target already stored is an update and is still served.
///
/// The bound is imported because the test has to know where it lies; what is asserted is
/// the behaviour at it, not the number.
#[tokio::test]
async fn the_console_authorization_store_is_bounded_by_its_quota() {
    let router = emulator();
    // Rows are distinct 12-digit account ids, the `TargetId` form with the largest key
    // space, so each put takes on one more target.
    for index in 0..CONSOLE_AUTHORIZATION_TARGET_QUOTA {
        let target_id = format!("{:012}", index + 1);
        let answer = put_configuration(&router, json!({"targetId": target_id})).await;
        assert_eq!(
            answer.status, 200,
            "target {target_id} ({index} of {CONSOLE_AUTHORIZATION_TARGET_QUOTA}) must be \
             configured, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // One target too many: refused, and nothing is stored for it.
    let over = format!("{:012}", CONSOLE_AUTHORIZATION_TARGET_QUOTA + 1);
    let refused = put_configuration(&router, json!({"targetId": over})).await;
    assert_eq!(
        refused.status, 429,
        "a put naming a new target past the quota must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "TooManyRequestsError",
        "expected TooManyRequestsError, got {:?}",
        refused.body
    );
    let missing = get_configuration(&router, json!({"targetId": over})).await;
    assert_eq!(
        missing.status, 404,
        "a refused put must store nothing for {over}, got {} with {:?}",
        missing.status, missing.body
    );

    // A target the store already holds is an update: it grows nothing, so it is served.
    for target_id in ["000000000001", "000000000100"] {
        let updated = put_configuration(&router, json!({"targetId": target_id})).await;
        assert_eq!(
            updated.status, 200,
            "updating the already configured {target_id} must be served, got {} with {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            get_configuration(&router, json!({"targetId": target_id}))
                .await
                .body,
            updated.body,
            "the get of {target_id} must report what the update persisted"
        );
    }
}

/// The delete-retry record is bounded AND does not decay: however many deletes the store
/// has seen, a retry of a recent one still answers its original success, while a token
/// evicted by the bound answers the 404 an unknown id earns.
#[tokio::test]
async fn a_delete_retry_survives_the_bound_on_the_retry_record() {
    let router = emulator();
    // More create/delete cycles than the record can hold, so the bound is exercised.
    let cycles = PERMISSION_STATEMENT_QUOTA + 5;
    let mut deleted: Vec<(String, String)> = Vec::with_capacity(cycles);
    for index in 0..cycles {
        let statement_id = put_statement(&router, json!({"sourceIp": "192.0.2.1"}))
            .await
            .statement_id();
        let token = format!("delete-{index}");
        let answer = delete_statement(
            &router,
            json!({"statementId": statement_id, "clientToken": token}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "cycle {index}: the delete must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        deleted.push((token, statement_id));
    }

    // The most recent deletes are still remembered, so their retries replay.
    for (token, statement_id) in deleted.iter().rev().take(3) {
        let retry = delete_statement(
            &router,
            json!({"statementId": statement_id, "clientToken": token}),
        )
        .await;
        assert_eq!(
            retry.status, 200,
            "a retry under the recent token {token:?} must answer the same success, got {} \
             with {:?}",
            retry.status, retry.body
        );
    }

    // The oldest token was evicted to keep the record bounded, so its retry is a 404 —
    // the same answer any id the store does not hold earns.
    let (oldest_token, oldest_id) = &deleted[0];
    let expired = delete_statement(
        &router,
        json!({"statementId": oldest_id, "clientToken": oldest_token}),
    )
    .await;
    assert_eq!(
        expired.status, 404,
        "a retry under the evicted token {oldest_token:?} must answer 404, got {} with {:?}",
        expired.status, expired.body
    );
    assert_eq!(
        expired.error_code(),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {:?}",
        expired.body
    );
}
