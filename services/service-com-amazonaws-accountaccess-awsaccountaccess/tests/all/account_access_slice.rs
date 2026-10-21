// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account-access slice: create -> use -> read -> list ->
//! delete, driven over HTTP through the same router the emulator serves with.
//!
//! Every assertion here reads the response the service computed from its store; no
//! test reaches into the state struct, so an echoed or canned answer fails.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_accountaccess_awsaccountaccess::state::{
    APPLICATION_ACTIVATION_MILLIS, DEFAULT_MAX_RESULTS, MAX_APPLICATIONS,
    MAX_ENTITLEMENTS_PER_APPLICATION, STATUS_ACTIVE, STATUS_CREATE_IN_PROGRESS,
};
use service_com_amazonaws_accountaccess_awsaccountaccess::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every test signs for, so the ARNs the service mints must carry it.
const REGION: &str = "us-west-2";

/// The account segment every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// One emulator: a router with only this service mounted, over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here; routing, handlers and state are.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch a request and hand back the status and the raw body.
    async fn raw(&self, method: &str, uri: &str, body: &str) -> (u16, bytes::Bytes) {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20240101/{REGION}/account-access/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }

    /// Dispatch a request and parse the body as JSON (`Value::Null` when empty).
    async fn json(&self, method: &str, uri: &str, body: &Value) -> (u16, Value) {
        let payload = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        let (status, raw) = self.raw(method, uri, &payload).await;
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// Percent-encode a value for one URI path segment or query value, the way an AWS SDK
/// encodes an `@httpLabel` / `@httpQuery` member (an ARN's `:` and `/` included).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Sleep past the instant an application's creation falls due.
fn wait_for_activation() {
    let millis = u64::try_from(APPLICATION_ACTIVATION_MILLIS).unwrap_or(500);
    std::thread::sleep(Duration::from_millis(millis + 100));
}

async fn create_application(emulator: &Emulator, instance_arn: &str, tags: &Value) -> (u16, Value) {
    emulator
        .json(
            "POST",
            "/applications",
            &json!({
                "identitySource": { "identityCenter": { "instanceArn": instance_arn } },
                "tags": tags,
            }),
        )
        .await
}

async fn get_application(emulator: &Emulator, application_arn: &str) -> (u16, Value) {
    emulator
        .json(
            "GET",
            &format!("/applications/{}", encode(application_arn)),
            &Value::Null,
        )
        .await
}

async fn create_entitlement(
    emulator: &Emulator,
    application_arn: &str,
    principal: &Value,
    role_arn: &str,
) -> (u16, Value) {
    emulator
        .json(
            "POST",
            "/entitlements",
            &json!({
                "applicationArn": application_arn,
                "entitlement": {
                    "principalRole": { "principal": principal, "roleArn": role_arn },
                },
            }),
        )
        .await
}

async fn get_entitlement(
    emulator: &Emulator,
    application_arn: &str,
    entitlement_id: &str,
) -> (u16, Value) {
    emulator
        .json(
            "GET",
            &format!(
                "/entitlements/{}?applicationArn={}",
                encode(entitlement_id),
                encode(application_arn)
            ),
            &Value::Null,
        )
        .await
}

async fn list_entitlements(
    emulator: &Emulator,
    application_arn: &str,
    filter: &Value,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "applicationArn": application_arn, "filter": filter });
    if let Some(max_results) = max_results {
        body["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["nextToken"] = json!(next_token);
    }
    emulator.json("POST", "/entitlements-list", &body).await
}

/// Create an application and let the creation the clock owes fall due, so it is
/// `ACTIVE` and can carry entitlements.
async fn active_application(emulator: &Emulator, instance_arn: &str) -> String {
    let (status, body) = create_application(emulator, instance_arn, &json!({})).await;
    assert_eq!(status, 200, "create {instance_arn}: got {status}, body {body}");
    let application_arn = body["applicationArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_for_activation();
    let (status, body) = get_application(emulator, &application_arn).await;
    assert_eq!(
        (status, &body["status"]),
        (200, &json!(STATUS_ACTIVE)),
        "case {instance_arn}: expected an ACTIVE application, got {status} / {body}"
    );
    application_arn
}

fn user(user_id: &str) -> Value {
    json!({ "identityCenter": { "userId": user_id } })
}

fn group(group_id: &str) -> Value {
    json!({ "identityCenter": { "groupId": group_id } })
}

/// Whether a value is a wire timestamp. restJson1 renders a `DateTime` as epoch
/// seconds, so accept a number of seconds; a `@timestampFormat("date-time")` rendering
/// is an ISO-8601 string. Either way the member must carry an instant, not nothing.
fn is_timestamp(value: &Value) -> bool {
    value.as_f64().is_some_and(|seconds| seconds > 1_600_000_000.0)
        || value.as_str().is_some_and(|text| text.contains('T'))
}

/// The entitlement ids a list page reports, in page order.
fn page_ids(body: &Value) -> Vec<String> {
    body["entitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|member| member["entitlementId"].as_str().unwrap_or("").to_string())
        .collect()
}

/// Every identity source in this table is a different, well-formed IAM Identity
/// Center instance ARN (`(sso)?ins-` + 16 of `[a-zA-Z0-9-.]`, per
/// `smithy_query(query="shape", shape="IdentityCenterDetails")`), paired with a tag
/// map of a different size. The rule: a created application reports back the identity
/// source and tags it was created with, under an ARN this service minted.
#[tokio::test]
async fn an_application_reports_the_identity_source_and_tags_it_was_created_with() {
    let cases: [(&str, Value); 4] = [
        ("arn:aws:sso:::instance/ssoins-1234567890abcdef", json!({})),
        (
            "arn:aws:sso:::instance/ins-abcdef0123456789",
            json!({ "env": "dev" }),
        ),
        (
            "arn:aws:sso:::instance/ssoins-a.b-c.d-01234567",
            json!({ "env": "prod", "team": "platform", "cost-centre": "42" }),
        ),
        (
            "arn:aws-us-gov:sso:::instance/ins-ABCDEFGHIJ01234z",
            json!({ "owner": "ops" }),
        ),
    ];

    // One emulator for the whole table, so the applications must not collide.
    let emulator = Emulator::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut tenants: BTreeSet<String> = BTreeSet::new();

    for (instance_arn, tags) in &cases {
        let (status, body) = create_application(&emulator, instance_arn, tags).await;
        assert_eq!(
            status, 200,
            "case {instance_arn:?}: expected 200 from CreateApplication, got {status} / {body}"
        );
        let application_arn = body["applicationArn"].as_str().unwrap_or_default();
        let prefix = format!("arn:aws:account-access:{REGION}:{ACCOUNT_ID}:application/");
        assert!(
            application_arn.starts_with(&prefix),
            "case {instance_arn:?}: expected an ARN under {prefix:?}, got {application_arn:?}"
        );
        assert!(
            application_arn.len() >= 49,
            "case {instance_arn:?}: ApplicationArn is len 49..2048, got {} in {application_arn:?}",
            application_arn.len()
        );
        let resource_id = application_arn.split('/').next_back().unwrap_or_default();
        assert!(
            !resource_id.is_empty()
                && resource_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {instance_arn:?}: the ARN's id must match [a-zA-Z0-9-]+, got {resource_id:?}"
        );
        assert!(
            minted.insert(application_arn.to_string()),
            "case {instance_arn:?}: {application_arn:?} was already minted for another \
             identity source"
        );

        // Before the creation falls due the application is in the transient status the
        // model declares, and both timestamps are the instant of the create.
        let (status, body) = get_application(&emulator, application_arn).await;
        assert_eq!(
            (status, &body["status"]),
            (200, &json!(STATUS_CREATE_IN_PROGRESS)),
            "case {instance_arn:?}: a just-created application is {STATUS_CREATE_IN_PROGRESS}, \
             got {status} / {body}"
        );
        assert_eq!(
            body["createdAt"], body["updatedAt"],
            "case {instance_arn:?}: nothing has updated the application yet, got {body}"
        );
    }

    wait_for_activation();

    for (instance_arn, tags) in &cases {
        let application_arn = {
            let (_, body) = create_application(&emulator, instance_arn, tags).await;
            body["applicationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        let (status, body) = get_application(&emulator, &application_arn).await;
        assert_eq!(
            status, 200,
            "case {instance_arn:?}: expected 200 from GetApplication, got {status} / {body}"
        );
        assert_eq!(
            body["identitySource"]["identityCenter"]["instanceArn"],
            json!(instance_arn),
            "case {instance_arn:?}: GetApplication must report the stored identity source, \
             got {body}"
        );
        assert_eq!(
            body["status"],
            json!(STATUS_ACTIVE),
            "case {instance_arn:?}: the creation has fallen due, got {body}"
        );
        if tags.as_object().is_some_and(|tags| tags.is_empty()) {
            assert!(
                body["tags"].is_null(),
                "case {instance_arn:?}: an application created without tags reports none, \
                 got {body}"
            );
        } else {
            assert_eq!(
                body["tags"], *tags,
                "case {instance_arn:?}: GetApplication must report the stored tags, got {body}"
            );
        }
        assert!(
            body["tenantId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {instance_arn:?}: the application carries a tenant id, got {body}"
        );
        tenants.insert(body["tenantId"].as_str().unwrap_or_default().to_string());
        for member in ["createdAt", "updatedAt"] {
            assert!(
                is_timestamp(&body[member]),
                "case {instance_arn:?}: {member} is a required timestamp, got {body}"
            );
        }

        // The emulator-minted IAM Identity Center application ARN:
        // `arn:aws:sso::<account>:application/<instance id>/apl-<16 alphanumerics>`.
        let instance_id = instance_arn.split('/').next_back().unwrap_or_default();
        let identity_center_application_arn = body["identitySource"]["identityCenter"]
            ["applicationArn"]
            .as_str()
            .unwrap_or_default();
        let expected_prefix =
            format!("arn:aws:sso::{ACCOUNT_ID}:application/{instance_id}/apl-");
        assert!(
            identity_center_application_arn.starts_with(&expected_prefix),
            "case {instance_arn:?}: expected an Identity Center application ARN under \
             {expected_prefix:?}, got {identity_center_application_arn:?}"
        );
        let application_id = identity_center_application_arn
            .split('/')
            .next_back()
            .unwrap_or_default();
        assert!(
            application_id.len() == 20
                && application_id[4..].chars().all(char::is_alphanumeric),
            "case {instance_arn:?}: the Identity Center application id is apl- plus 16 \
             alphanumerics, got {application_id:?}"
        );
    }

    // One emulator presents ONE account, so every application reports the same tenant:
    // the store has no tenant dimension to partition on or to check.
    assert_eq!(
        tenants.len(),
        1,
        "one emulated account must report one tenantId, got {tenants:?}"
    );
}

/// `CreateApplication` and `CreateEntitlement` are `@idempotent`: repeating the same
/// request must not create a second resource. A different request for the same
/// identity source is `AlreadyCreatedException`, not a silent second application.
#[tokio::test]
async fn repeating_a_create_returns_the_resource_already_stored() {
    let emulator = Emulator::new();
    let instance_arn = "arn:aws:sso:::instance/ssoins-0123456789abcdef";
    let tags = json!({ "env": "dev" });

    let (status, first) = create_application(&emulator, instance_arn, &tags).await;
    assert_eq!(status, 200, "first create: got {status} / {first}");
    let (status, second) = create_application(&emulator, instance_arn, &tags).await;
    assert_eq!(status, 200, "repeated create: got {status} / {second}");
    assert_eq!(
        first["applicationArn"], second["applicationArn"],
        "case repeated identical create: expected the stored ARN, got {first} then {second}"
    );

    // Same identity source, different tags: a different request for a resource that
    // already exists.
    let (status, body) =
        create_application(&emulator, instance_arn, &json!({ "env": "prod" })).await;
    assert_eq!(
        status, 400,
        "case same identity source with different tags: expected 400 \
         AlreadyCreatedException, got {status} / {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .contains("AlreadyCreatedException"),
        "case same identity source with different tags: got {body}"
    );

    let application_arn = first["applicationArn"].as_str().unwrap_or_default();
    wait_for_activation();
    let role_arn = "arn:aws:iam::123456789012:role/Readonly";
    let (status, first) =
        create_entitlement(&emulator, application_arn, &user("user-1"), role_arn).await;
    assert_eq!(status, 200, "first entitlement: got {status} / {first}");
    let (status, second) =
        create_entitlement(&emulator, application_arn, &user("user-1"), role_arn).await;
    assert_eq!(status, 200, "repeated entitlement: got {status} / {second}");
    assert_eq!(
        first["entitlementId"], second["entitlementId"],
        "case repeated identical CreateEntitlement: expected the stored id, got {first} \
         then {second}"
    );

    let (status, body) = list_entitlements(&emulator, application_arn, &json!({}), None, None).await;
    assert_eq!(
        (status, page_ids(&body).len()),
        (200, 1),
        "case repeated identical CreateEntitlement: the application holds ONE entitlement, \
         got {status} / {body}"
    );
}

/// Creation is asynchronous: the model gives an application a non-terminal
/// `CREATE_IN_PROGRESS` status, so a caller acting straight after
/// `CreateApplication` still observes it — and the state error it implies — until the
/// clock makes the transition due.
#[tokio::test]
async fn an_application_becomes_active_when_the_clock_makes_its_creation_due() {
    let emulator = Emulator::new();
    let instance_arn = "arn:aws:sso:::instance/ssoins-fedcba9876543210";

    let (status, body) = create_application(&emulator, instance_arn, &json!({})).await;
    assert_eq!(status, 200, "create: got {status} / {body}");
    let application_arn = body["applicationArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, before) = get_application(&emulator, &application_arn).await;
    assert_eq!(
        (status, &before["status"]),
        (200, &json!(STATUS_CREATE_IN_PROGRESS)),
        "case straight after create: got {status} / {before}"
    );
    let (status, body) = create_entitlement(
        &emulator,
        &application_arn,
        &user("user-1"),
        "arn:aws:iam::123456789012:role/Readonly",
    )
    .await;
    assert_eq!(
        status, 409,
        "case entitlement on a {STATUS_CREATE_IN_PROGRESS} application: expected 409 \
         ConflictException, got {status} / {body}"
    );

    wait_for_activation();

    let (status, after) = get_application(&emulator, &application_arn).await;
    assert_eq!(
        (status, &after["status"]),
        (200, &json!(STATUS_ACTIVE)),
        "case once the creation is due: got {status} / {after}"
    );
    assert_eq!(
        after["createdAt"], before["createdAt"],
        "case once the creation is due: createdAt never moves, got {before} then {after}"
    );
    assert_ne!(
        after["updatedAt"], after["createdAt"],
        "case once the creation is due: the transition updated the application, got {after}"
    );
    // A read must not move the clock on: the next read reports the same instants.
    let (_, again) = get_application(&emulator, &application_arn).await;
    assert_eq!(
        again, after,
        "case a second read: a report must not change what a later caller sees, got {after} \
         then {again}"
    );
}

/// The rule: an entitlement reports the principal and role it was created with, plus
/// the 12-digit account PARSED from that role's ARN. Rows differ along the principal
/// kind (`userId` / `groupId`), the role's account, the role name's shape (a path, the
/// `+=,.@_-` characters `RoleArn` allows) and the partition.
#[tokio::test]
async fn an_entitlement_reports_its_principal_role_and_the_account_of_its_role() {
    let cases: [(&str, Value, &str, &str); 4] = [
        (
            "an Identity Center user",
            user("user-1"),
            "arn:aws:iam::123456789012:role/Readonly",
            "123456789012",
        ),
        (
            "an Identity Center group",
            group("group-7"),
            "arn:aws:iam::210987654321:role/Admin",
            "210987654321",
        ),
        (
            "a role behind a path",
            user("user-2"),
            "arn:aws:iam::111122223333:role/engineering/platform/Deploy",
            "111122223333",
        ),
        (
            "a role in another partition with punctuation in its name",
            group("group-8"),
            "arn:aws-us-gov:iam::444455556666:role/audit.read+write@corp",
            "444455556666",
        ),
    ];

    let emulator = Emulator::new();
    let application_arn =
        active_application(&emulator, "arn:aws:sso:::instance/ssoins-aaaabbbbccccdddd").await;
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for (case, principal, role_arn, expected_account) in &cases {
        let (status, body) =
            create_entitlement(&emulator, &application_arn, principal, role_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateEntitlement, got {status} / {body}"
        );
        let entitlement_id = body["entitlementId"].as_str().unwrap_or_default().to_string();
        assert!(
            !entitlement_id.is_empty(),
            "case {case:?}: entitlementId is required, got {body}"
        );
        assert!(
            minted.insert(entitlement_id.clone()),
            "case {case:?}: {entitlement_id:?} was already minted for another entitlement"
        );

        let (status, body) = get_entitlement(&emulator, &application_arn, &entitlement_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetEntitlement, got {status} / {body}"
        );
        assert_eq!(
            body["entitlementId"],
            json!(entitlement_id),
            "case {case:?}: got {body}"
        );
        assert_eq!(
            body["applicationArn"],
            json!(application_arn),
            "case {case:?}: got {body}"
        );
        let stored = &body["entitlement"]["principalRole"];
        assert_eq!(
            stored["principal"], *principal,
            "case {case:?}: GetEntitlement must report the stored principal, got {body}"
        );
        assert_eq!(
            stored["roleArn"],
            json!(role_arn),
            "case {case:?}: GetEntitlement must report the stored roleArn, got {body}"
        );
        assert_eq!(
            stored["account"],
            json!(expected_account),
            "case {case:?}: account is the 12-digit account of {role_arn:?}, got {body}"
        );
        assert!(
            stored["accountName"].is_null(),
            "case {case:?}: the emulator holds no account directory, so accountName is \
             absent, got {body}"
        );
        assert!(
            is_timestamp(&body["createdAt"]),
            "case {case:?}: createdAt is a required timestamp, got {body}"
        );
    }
}

/// `PrincipalRoleEntitlementFilter`: "All specified criteria must match for an
/// entitlement to be returned." Rows differ along which criteria the filter sets,
/// including a combination that matches nothing — an empty list, not an error.
#[tokio::test]
async fn list_entitlements_returns_exactly_the_entitlements_every_criterion_matches() {
    let emulator = Emulator::new();
    let application_arn =
        active_application(&emulator, "arn:aws:sso:::instance/ssoins-1111222233334444").await;

    let fixture: [(Value, &str); 4] = [
        (user("user-1"), "arn:aws:iam::111111111111:role/Readonly"),
        (group("group-1"), "arn:aws:iam::111111111111:role/Admin"),
        (user("user-1"), "arn:aws:iam::222222222222:role/Readonly"),
        (user("user-2"), "arn:aws:iam::222222222222:role/Admin"),
    ];
    let mut ids: Vec<String> = Vec::new();
    for (principal, role_arn) in &fixture {
        let (status, body) =
            create_entitlement(&emulator, &application_arn, principal, role_arn).await;
        assert_eq!(status, 200, "fixture {role_arn}: got {status} / {body}");
        ids.push(body["entitlementId"].as_str().unwrap_or_default().to_string());
    }

    let cases: [(&str, Value, Vec<usize>); 7] = [
        ("no criteria", json!({}), vec![0, 1, 2, 3]),
        (
            "principal only",
            json!({ "principalRole": { "principal": user("user-1") } }),
            vec![0, 2],
        ),
        (
            "a group principal",
            json!({ "principalRole": { "principal": group("group-1") } }),
            vec![1],
        ),
        (
            "roleArn only",
            json!({ "principalRole": { "roleArn": "arn:aws:iam::111111111111:role/Readonly" } }),
            vec![0],
        ),
        (
            "account only",
            json!({ "principalRole": { "account": "222222222222" } }),
            vec![2, 3],
        ),
        (
            "principal and account together",
            json!({
                "principalRole": {
                    "principal": user("user-1"),
                    "account": "222222222222",
                },
            }),
            vec![2],
        ),
        (
            "criteria no entitlement satisfies together",
            json!({
                "principalRole": {
                    "principal": user("user-2"),
                    "account": "111111111111",
                },
            }),
            vec![],
        ),
    ];

    for (case, filter, expected) in &cases {
        let (status, body) = list_entitlements(&emulator, &application_arn, filter, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListEntitlements, got {status} / {body}"
        );
        let expected_ids: Vec<String> = expected.iter().map(|index| ids[*index].clone()).collect();
        assert_eq!(
            page_ids(&body),
            expected_ids,
            "case {case:?}: expected {expected_ids:?} in creation order, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: one unbounded page holds everything, got {body}"
        );
    }
}

/// `maxResults` truncates a page and `nextToken` resumes exactly where it stopped:
/// paging through with any page size must visit every entitlement once, in creation
/// order, and the final page must carry no token. Rows differ along the page size,
/// including one equal to and one above the number stored.
#[tokio::test]
async fn paging_with_any_max_results_visits_every_entitlement_once_in_order() {
    let emulator = Emulator::new();
    let application_arn =
        active_application(&emulator, "arn:aws:sso:::instance/ssoins-5555666677778888").await;

    let roles = [
        "arn:aws:iam::123456789012:role/One",
        "arn:aws:iam::123456789012:role/Two",
        "arn:aws:iam::123456789012:role/Three",
        "arn:aws:iam::123456789012:role/Four",
        "arn:aws:iam::123456789012:role/Five",
    ];
    let mut ids: Vec<String> = Vec::new();
    for role_arn in &roles {
        let (status, body) =
            create_entitlement(&emulator, &application_arn, &user("user-1"), role_arn).await;
        assert_eq!(status, 200, "fixture {role_arn}: got {status} / {body}");
        ids.push(body["entitlementId"].as_str().unwrap_or_default().to_string());
    }

    for max_results in [1_i64, 2, 3, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, body) = list_entitlements(
                &emulator,
                &application_arn,
                &json!({}),
                Some(max_results),
                token.as_deref(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: expected 200, got {status} / {body}"
            );
            let page = page_ids(&body);
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                "case maxResults={max_results}: a page may not exceed it, got {body}"
            );
            seen.extend(page);
            token = body["nextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= roles.len() + 1,
                "case maxResults={max_results}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() < roles.len(),
                "case maxResults={max_results}: a token was issued with nothing left, \
                 after {seen:?}"
            );
        }
        assert_eq!(
            seen, ids,
            "case maxResults={max_results}: paging must visit every entitlement once in \
             creation order"
        );
    }

    // A token this service never issued is a refused request, not a silent first page.
    let (status, body) = list_entitlements(
        &emulator,
        &application_arn,
        &json!({}),
        None,
        Some("not-a-token"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a token the service never issued: expected 400 ValidationException, got \
         {status} / {body}"
    );
}

/// Deleting an application removes it and cascades to its entitlements, leaving every
/// other application untouched; deleting it again is still a success.
#[tokio::test]
async fn deleting_an_application_removes_it_and_its_entitlements() {
    let emulator = Emulator::new();
    let doomed =
        active_application(&emulator, "arn:aws:sso:::instance/ssoins-9999aaaabbbbcccc").await;
    let kept = active_application(&emulator, "arn:aws:sso:::instance/ins-ddddeeeeffff0000").await;

    let mut doomed_ids: Vec<String> = Vec::new();
    for role_arn in [
        "arn:aws:iam::123456789012:role/One",
        "arn:aws:iam::123456789012:role/Two",
    ] {
        let (_, body) = create_entitlement(&emulator, &doomed, &user("user-1"), role_arn).await;
        doomed_ids.push(body["entitlementId"].as_str().unwrap_or_default().to_string());
    }
    let (_, body) = create_entitlement(
        &emulator,
        &kept,
        &user("user-9"),
        "arn:aws:iam::123456789012:role/Three",
    )
    .await;
    let kept_id = body["entitlementId"].as_str().unwrap_or_default().to_string();

    let (status, raw) = emulator
        .raw("DELETE", &format!("/applications/{}", encode(&doomed)), "")
        .await;
    assert_eq!(
        status, 204,
        "case DeleteApplication: expected 204, got {status} / {:?}",
        String::from_utf8_lossy(&raw)
    );
    assert!(
        raw.is_empty() || raw.as_ref() == b"{}",
        "case DeleteApplication: 204 carries no members, got {:?}",
        String::from_utf8_lossy(&raw)
    );

    let (status, body) = get_application(&emulator, &doomed).await;
    assert_eq!(
        status, 404,
        "case GetApplication after delete: expected 404 ResourceNotFoundException, got \
         {status} / {body}"
    );
    let (status, body) = list_entitlements(&emulator, &doomed, &json!({}), None, None).await;
    assert_eq!(
        status, 404,
        "case ListEntitlements after delete: expected 404, got {status} / {body}"
    );
    for entitlement_id in &doomed_ids {
        let (status, body) = get_entitlement(&emulator, &doomed, entitlement_id).await;
        assert_eq!(
            status, 404,
            "case GetEntitlement {entitlement_id} after delete: expected 404, got \
             {status} / {body}"
        );
    }

    // Deleting an application that is already gone is documented as not an error.
    let (status, _) = emulator
        .raw("DELETE", &format!("/applications/{}", encode(&doomed)), "")
        .await;
    assert_eq!(
        status, 204,
        "case repeated DeleteApplication: the operation is idempotent, got {status}"
    );

    // The other application and its entitlement are untouched.
    let (status, body) = get_application(&emulator, &kept).await;
    assert_eq!(
        status, 200,
        "case the application that was not deleted: got {status} / {body}"
    );
    let (status, body) = get_entitlement(&emulator, &kept, &kept_id).await;
    assert_eq!(
        status, 200,
        "case the entitlement of the application that was not deleted: got {status} / {body}"
    );
}

/// Every lookup is of a resource the store actually holds: an id that names nothing,
/// or that belongs to another application, is `ResourceNotFoundException`, and an ARN
/// the model's pattern refuses is a `ValidationException` — never a fabricated answer.
#[tokio::test]
async fn unknown_and_malformed_identifiers_are_refused() {
    let emulator = Emulator::new();
    let application_arn =
        active_application(&emulator, "arn:aws:sso:::instance/ssoins-0000111122223333").await;
    let other =
        active_application(&emulator, "arn:aws:sso:::instance/ssoins-4444555566667777").await;
    let (_, body) = create_entitlement(
        &emulator,
        &application_arn,
        &user("user-1"),
        "arn:aws:iam::123456789012:role/Readonly",
    )
    .await;
    let entitlement_id = body["entitlementId"].as_str().unwrap_or_default().to_string();

    // Well-formed but never created: 404 on every operation that resolves it.
    let unknown =
        format!("arn:aws:account-access:{REGION}:{ACCOUNT_ID}:application/aam-ffffffffffffffff");
    let (status, body) = get_application(&emulator, &unknown).await;
    assert_eq!(
        status, 404,
        "case GetApplication on an uncreated ARN: got {status} / {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .contains("ResourceNotFoundException"),
        "case GetApplication on an uncreated ARN: got {body}"
    );
    let (status, body) = list_entitlements(&emulator, &unknown, &json!({}), None, None).await;
    assert_eq!(
        status, 404,
        "case ListEntitlements on an uncreated ARN: got {status} / {body}"
    );
    let (status, body) = create_entitlement(
        &emulator,
        &unknown,
        &user("user-1"),
        "arn:aws:iam::123456789012:role/Readonly",
    )
    .await;
    assert_eq!(
        status, 404,
        "case CreateEntitlement on an uncreated ARN: got {status} / {body}"
    );

    // An entitlement is a child of ONE application.
    let (status, body) = get_entitlement(&emulator, &other, &entitlement_id).await;
    assert_eq!(
        status, 404,
        "case GetEntitlement with the wrong applicationArn: got {status} / {body}"
    );
    let (status, body) = get_entitlement(&emulator, &application_arn, "ent-nope").await;
    assert_eq!(
        status, 404,
        "case GetEntitlement on an unknown id: got {status} / {body}"
    );

    // Outside the modeled `ApplicationArn` pattern or length: a refused request.
    for malformed in [
        "not-an-arn",
        "arn:aws:account-access:us-west-2:000000000000:workspace/aam-1",
        "arn:aws:account-access:us-west-2:0:application/aam-1",
    ] {
        let (status, body) = get_application(&emulator, malformed).await;
        assert_eq!(
            status, 400,
            "case GetApplication on {malformed:?}: expected 400 ValidationException, got \
             {status} / {body}"
        );
    }
}

/// A listing is bounded whether or not the caller bounds it: with `maxResults` absent
/// the page holds at most `DEFAULT_MAX_RESULTS` entitlements and hands back a token to
/// continue, rather than collecting every stored entitlement into one response.
#[tokio::test]
async fn an_absent_max_results_pages_at_the_services_own_default() {
    let emulator = Emulator::new();
    let application_arn =
        active_application(&emulator, "arn:aws:sso:::instance/ssoins-8888999900001111").await;

    let stored = DEFAULT_MAX_RESULTS + 1;
    for index in 0..stored {
        let (status, body) = create_entitlement(
            &emulator,
            &application_arn,
            &user("user-1"),
            &format!("arn:aws:iam::123456789012:role/Role{index}"),
        )
        .await;
        assert_eq!(status, 200, "fixture {index}: got {status} / {body}");
    }

    let (status, body) = list_entitlements(&emulator, &application_arn, &json!({}), None, None).await;
    assert_eq!(status, 200, "case maxResults absent: got {status} / {body}");
    assert_eq!(
        page_ids(&body).len(),
        DEFAULT_MAX_RESULTS,
        "case maxResults absent: a page holds at most {DEFAULT_MAX_RESULTS}, got {}",
        page_ids(&body).len()
    );
    let token = body["nextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !token.is_empty(),
        "case maxResults absent: entitlements remain, so a token is issued, got {body}"
    );

    let (status, body) =
        list_entitlements(&emulator, &application_arn, &json!({}), None, Some(&token)).await;
    assert_eq!(
        (status, page_ids(&body).len()),
        (200, stored - DEFAULT_MAX_RESULTS),
        "case the resumed page: expected the remaining {} entitlements, got {status} / {body}",
        stored - DEFAULT_MAX_RESULTS
    );
    assert!(
        body["nextToken"].is_null(),
        "case the resumed page: nothing remains after it, got {body}"
    );
}

/// The store is bounded in both dimensions: an application holds at most
/// `MAX_ENTITLEMENTS_PER_APPLICATION` entitlements, refused beyond that with the
/// modeled `ServiceQuotaExceededException` (402), and the quota never breaks the
/// idempotent repeat of an entitlement already stored.
#[tokio::test]
async fn the_entitlement_quota_bounds_one_application() {
    let emulator = Emulator::new();
    let application_arn =
        active_application(&emulator, "arn:aws:sso:::instance/ssoins-2222333344445555").await;

    let mut first_role = String::new();
    for index in 0..MAX_ENTITLEMENTS_PER_APPLICATION {
        let role_arn = format!("arn:aws:iam::123456789012:role/Role{index}");
        if index == 0 {
            first_role = role_arn.clone();
        }
        let (status, body) =
            create_entitlement(&emulator, &application_arn, &user("user-1"), &role_arn).await;
        assert_eq!(status, 200, "fixture {index}: got {status} / {body}");
    }

    let (status, body) = create_entitlement(
        &emulator,
        &application_arn,
        &user("user-1"),
        "arn:aws:iam::123456789012:role/OneTooMany",
    )
    .await;
    assert_eq!(
        status, 402,
        "case one entitlement past the quota: expected 402 \
         ServiceQuotaExceededException, got {status} / {body}"
    );

    let (status, body) =
        create_entitlement(&emulator, &application_arn, &user("user-1"), &first_role).await;
    assert_eq!(
        status, 200,
        "case repeating an entitlement already stored at the quota: an idempotent \
         repeat creates nothing, got {status} / {body}"
    );
}

/// And in the application dimension: the store holds at most `MAX_APPLICATIONS`, past
/// which a create is refused with the modeled `ConflictException` (409) — while a
/// repeat of an application already stored still answers its ARN.
#[tokio::test]
async fn the_application_store_is_bounded() {
    let emulator = Emulator::new();
    let instance_arn = |index: usize| format!("arn:aws:sso:::instance/ssoins-{index:016x}");

    let mut first = String::new();
    for index in 0..MAX_APPLICATIONS {
        let (status, body) = create_application(&emulator, &instance_arn(index), &json!({})).await;
        assert_eq!(status, 200, "fixture {index}: got {status} / {body}");
        if index == 0 {
            first = body["applicationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
    }

    let (status, body) =
        create_application(&emulator, &instance_arn(MAX_APPLICATIONS), &json!({})).await;
    assert_eq!(
        status, 409,
        "case one application past the maximum: expected 409 ConflictException, got \
         {status} / {body}"
    );

    let (status, body) = create_application(&emulator, &instance_arn(0), &json!({})).await;
    assert_eq!(
        (status, body["applicationArn"].as_str()),
        (200, Some(first.as_str())),
        "case repeating an application already stored at the maximum: expected its ARN, \
         got {status} / {body}"
    );
}
