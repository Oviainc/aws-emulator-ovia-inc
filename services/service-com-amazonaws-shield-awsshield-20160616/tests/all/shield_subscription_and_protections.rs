// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Shield Advanced subscription + protection lifecycle:
//! CreateSubscription, DescribeSubscription, GetSubscriptionState, CreateProtection,
//! DescribeProtection, ListProtections and DeleteProtection.
//!
//! Every case goes over the wire — an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AWSShield_20160616.<Operation>` through the same
//! `MultiServiceRouter` the production router serves — so routing, deserialization,
//! modelled-constraint validation, the handler and serialization are all exercised, and
//! what a create wrote is read back through a SEPARATE request.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * an account holds at most one subscription, and every protection operation needs
//!   one;
//! * a protection is minted with a fresh 36-character `ProtectionId` and persisted, so
//!   both of `DescribeProtection`'s addresses report the stored record;
//! * one resource carries at most one protection;
//! * `ListProtections` reports every persisted protection exactly once, in creation
//!   order, `MaxResults` per page, and emits a `NextToken` only while more remain;
//! * `DeleteProtection` releases the record and its resource, and cannot be repeated;
//! * the `ProtectedResourceType` of a protection follows the ARN layouts
//!   `CreateProtectionRequest$ResourceArn` documents.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_shield_awsshield_20160616::ops::types::PROTECTED_RESOURCE_TYPE_VALUES;
use service_com_amazonaws_shield_awsshield_20160616::state::{
    protected_resource_type, AUTO_RENEW_ENABLED, DEFAULT_LIST_PROTECTIONS_MAX_RESULTS,
    MAX_PROTECTIONS_PER_ACCOUNT, PROACTIVE_ENGAGEMENT_STATUS_DISABLED, SUBSCRIPTION_STATE_ACTIVE,
    SUBSCRIPTION_STATE_INACTIVE, SUBSCRIPTION_TIME_COMMITMENT,
};
use service_com_amazonaws_shield_awsshield_20160616::EmulatedService;

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSShield_20160616.";

/// The account segment every emulated ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// One answer from the emulator: the HTTP status and the parsed JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type`), or `""` for a success. A `__type` may be
    /// namespaced (`com.amazonaws.shield#Foo`); the code clients branch on is the
    /// shape name.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .map(|t| t.rsplit('#').next().unwrap_or(t))
            .unwrap_or_default()
    }

    /// Assert this reply is the modelled failure `code` with HTTP `status`.
    fn expect_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code()),
            (status, code),
            "case {case}: expected HTTP {status} {code}, got {self:?}"
        );
    }

    /// Assert this reply is a success, and hand back its body.
    fn expect_ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected HTTP 200, got {self:?}"
        );
        &self.body
    }
}

/// The emulator under test: one shared engine, one registered service, requests
/// dispatched through the production router.
struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's; the
        // routing key still comes from the `Authorization` header below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, operation: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/us-east-1/shield/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{operation}: response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Reply { status, body }
    }

    /// Activate Shield Advanced, asserting the activation succeeded.
    async fn subscribe(&self) {
        self.call("CreateSubscription", &json!({}))
            .await
            .expect_ok("CreateSubscription");
    }

    /// Create a protection and hand back its generated `ProtectionId`.
    async fn create_protection(&self, case: &str, name: &str, resource_arn: &str) -> String {
        let reply = self
            .call(
                "CreateProtection",
                &json!({ "Name": name, "ResourceArn": resource_arn }),
            )
            .await;
        let body = reply.expect_ok(case);
        body.get("ProtectionId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: CreateProtection returned no ProtectionId: {body}"))
            .to_string()
    }

    /// Every `Protection` one `ListProtections` page carries, plus the page's token.
    async fn list_page(&self, case: &str, request: &Value) -> (Vec<Value>, Option<String>) {
        let reply = self.call("ListProtections", request).await;
        let body = reply.expect_ok(case);
        let protections = body
            .get("Protections")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: ListProtections returned no Protections: {body}"))
            .clone();
        let next = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        (protections, next)
    }

    /// Walk every page of `ListProtections` for `request`, asserting no page exceeds
    /// `page_size` and that the last page carries no token. Hands back the ids in the
    /// order they were reported.
    async fn list_all_ids(&self, case: &str, request: &Value, page_size: usize) -> Vec<String> {
        let mut ids = Vec::new();
        let mut token: Option<String> = None;
        // One more iteration than there can be pages, so a token that never clears is
        // caught as a failure rather than hanging the suite.
        for page in 0..=MAX_PAGES {
            let mut body = request.clone();
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let (protections, next) = self.list_page(case, &body).await;
            assert!(
                protections.len() <= page_size,
                "case {case}: page {page} carries {} protections, more than the page size {page_size}",
                protections.len()
            );
            for protection in &protections {
                ids.push(member(protection, "Id"));
            }
            match next {
                None => return ids,
                Some(next) => {
                    assert!(
                        !protections.is_empty(),
                        "case {case}: page {page} carries a NextToken but no protections"
                    );
                    token = Some(next);
                }
            }
        }
        panic!("case {case}: ListProtections never stopped issuing NextToken");
    }
}

/// The most pages [`Emulator::list_all_ids`] follows before calling the pagination
/// broken.
const MAX_PAGES: usize = 64;

/// A string member of a JSON object, which must be present.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member {name} is absent or not a string in {body}"))
        .to_string()
}

/// A timestamp member as the wire carries it, in seconds.
///
/// The `awsJson1_1` default rendering for a timestamp is epoch seconds; a model that
/// asked for `date-time` would render an RFC 3339 string instead, so both are accepted
/// and only the VALUE is asserted on.
fn seconds(body: &Value, name: &str) -> f64 {
    let value = body
        .get(name)
        .unwrap_or_else(|| panic!("member {name} is absent in {body}"));
    match value {
        Value::Number(number) => number.as_f64().expect("a JSON number is an f64"),
        Value::String(rendered) => {
            let parsed = ::aws_smithy_types::DateTime::from_str(
                rendered,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .unwrap_or_else(|e| panic!("member {name} is not an RFC 3339 instant ({e}): {rendered}"));
            parsed.as_secs_f64()
        }
        other => panic!("member {name} is neither a number nor a string: {other}"),
    }
}

/// The ARN layouts `CreateProtectionRequest$ResourceArn` documents, one row per
/// documented format, each with the `ProtectedResourceType` it names. The last row is
/// an ARN in NO documented layout (a Network Load Balancer, which Shield Advanced
/// protects only through an Elastic IP): it is protectable but carries no resource
/// type.
const RESOURCE_ARNS: &[(&str, &str, Option<&str>)] = &[
    (
        "application load balancer",
        "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/app/my-alb/0123456789abcdef",
        Some("APPLICATION_LOAD_BALANCER"),
    ),
    (
        "classic load balancer",
        "arn:aws:elasticloadbalancing:eu-west-1:000000000000:loadbalancer/my-classic-lb",
        Some("CLASSIC_LOAD_BALANCER"),
    ),
    (
        "cloudfront distribution",
        "arn:aws:cloudfront::000000000000:distribution/E1XMPLDISTRIBUTION",
        Some("CLOUDFRONT_DISTRIBUTION"),
    ),
    (
        "global accelerator",
        "arn:aws:globalaccelerator::000000000000:accelerator/1de2e4b1-0000-4000-8000-0123456789ab",
        Some("GLOBAL_ACCELERATOR"),
    ),
    (
        "route 53 hosted zone",
        "arn:aws:route53:::hostedzone/Z1XMPLHOSTEDZONE",
        Some("ROUTE_53_HOSTED_ZONE"),
    ),
    (
        "elastic ip allocation",
        "arn:aws:ec2:ap-southeast-2:000000000000:eip-allocation/eipalloc-0123456789abcdef0",
        Some("ELASTIC_IP_ALLOCATION"),
    ),
    (
        "network load balancer",
        "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/net/my-nlb/0123456789abcdef",
        None,
    ),
];

#[tokio::test]
async fn an_account_holds_at_most_one_subscription_and_reports_the_stored_one() {
    let emulator = Emulator::new();

    // Before activation there is no subscription to report, and the account is
    // INACTIVE.
    emulator
        .call("DescribeSubscription", &json!({}))
        .await
        .expect_error("describe before subscribe", 400, "ResourceNotFoundException");
    let state = emulator.call("GetSubscriptionState", &json!({})).await;
    assert_eq!(
        member(state.expect_ok("state before subscribe"), "SubscriptionState"),
        SUBSCRIPTION_STATE_INACTIVE,
        "case state before subscribe: expected {SUBSCRIPTION_STATE_INACTIVE}, got {state:?}"
    );

    emulator.subscribe().await;

    // A second activation is a conflict, not a second subscription.
    emulator
        .call("CreateSubscription", &json!({}))
        .await
        .expect_error("subscribe twice", 400, "ResourceAlreadyExistsException");

    let state = emulator.call("GetSubscriptionState", &json!({})).await;
    assert_eq!(
        member(state.expect_ok("state after subscribe"), "SubscriptionState"),
        SUBSCRIPTION_STATE_ACTIVE,
        "case state after subscribe: expected {SUBSCRIPTION_STATE_ACTIVE}, got {state:?}"
    );

    let first = emulator.call("DescribeSubscription", &json!({})).await;
    let first = first.expect_ok("describe subscription").clone();
    let subscription = first
        .get("Subscription")
        .unwrap_or_else(|| panic!("case describe subscription: no Subscription in {first}"));

    assert_eq!(
        member(subscription, "AutoRenew"),
        AUTO_RENEW_ENABLED,
        "case describe subscription: a new subscription renews automatically, got {subscription}"
    );
    assert_eq!(
        member(subscription, "ProactiveEngagementStatus"),
        PROACTIVE_ENGAGEMENT_STATUS_DISABLED,
        "case describe subscription: proactive engagement is off until asked for, got {subscription}"
    );
    let commitment = subscription
        .get("TimeCommitmentInSeconds")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case describe subscription: no TimeCommitmentInSeconds in {subscription}"));
    assert_eq!(
        commitment,
        SUBSCRIPTION_TIME_COMMITMENT.as_secs() as i64,
        "case describe subscription: expected the subscription's commitment, got {commitment}"
    );
    let start = seconds(subscription, "StartTime");
    let end = seconds(subscription, "EndTime");
    assert!(
        start > 1_700_000_000.0,
        "case describe subscription: StartTime must be a real clock reading, got {start}"
    );
    assert!(
        (end - start - commitment as f64).abs() < 1.0,
        "case describe subscription: EndTime must be StartTime plus the commitment, got {start} and {end}"
    );
    let subscription_arn = member(subscription, "SubscriptionArn");
    assert!(
        subscription_arn.starts_with(&format!("arn:aws:shield::{ACCOUNT_ID}:subscription/")),
        "case describe subscription: unexpected SubscriptionArn {subscription_arn}"
    );

    // Every protected resource type the model declares carries a limit, and the limits
    // the subscription grants match the protection cap this emulator enforces.
    let limits = subscription
        .get("SubscriptionLimits")
        .and_then(|l| l.get("ProtectionLimits"))
        .and_then(|l| l.get("ProtectedResourceTypeLimits"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case describe subscription: no ProtectedResourceTypeLimits in {subscription}"));
    let limited: BTreeSet<String> = limits.iter().map(|limit| member(limit, "Type")).collect();
    let modelled: BTreeSet<String> = PROTECTED_RESOURCE_TYPE_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    assert_eq!(
        limited, modelled,
        "case describe subscription: every modelled ProtectedResourceType must carry a limit, got {limits:?}"
    );
    for limit in limits {
        let max = limit.get("Max").and_then(Value::as_i64);
        assert_eq!(
            max,
            Some(MAX_PROTECTIONS_PER_ACCOUNT),
            "case describe subscription: limit {limit} must report the enforced maximum"
        );
    }

    // A read does not change what a later caller sees.
    let again = emulator.call("DescribeSubscription", &json!({})).await;
    assert_eq!(
        again.expect_ok("describe subscription again"),
        &first,
        "case describe subscription again: a read must not change the subscription"
    );
}

#[tokio::test]
async fn every_documented_resource_arn_layout_round_trips_through_both_addresses() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (case, resource_arn, resource_type) in RESOURCE_ARNS {
        let name = format!("protect-{case}").replace(' ', "-");
        let id = emulator.create_protection(case, &name, resource_arn).await;

        assert_eq!(
            id.len(),
            36,
            "case {case}: a ProtectionId is 36 characters, got {id:?}"
        );
        assert!(
            id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {case}: a ProtectionId matches ^[a-zA-Z0-9\\-]*$, got {id:?}"
        );
        // The canonical hyphenated version-4 UUID form: 8-4-4-4-12 lowercase hex with
        // the version nibble at index 14 and the variant nibble at index 19.
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "case {case}: a ProtectionId is a hyphenated UUID, got {id:?}"
        );
        assert!(
            groups
                .iter()
                .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())),
            "case {case}: a ProtectionId's groups are lowercase hex, got {id:?}"
        );
        assert_eq!(
            id.as_bytes()[14] as char,
            '4',
            "case {case}: a ProtectionId carries the version-4 nibble, got {id:?}"
        );
        assert!(
            matches!(id.as_bytes()[19] as char, '8' | '9' | 'a' | 'b'),
            "case {case}: a ProtectionId carries the RFC 4122 variant nibble, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case}: ProtectionId {id} was already issued to another protection"
        );

        // Both of the addresses `DescribeProtection` accepts report the SAME stored
        // record — the values the create persisted, not the request's own.
        let by_id = emulator
            .call("DescribeProtection", &json!({ "ProtectionId": id }))
            .await;
        let by_id = by_id.expect_ok(case).clone();
        let by_arn = emulator
            .call("DescribeProtection", &json!({ "ResourceArn": resource_arn }))
            .await;
        assert_eq!(
            by_arn.expect_ok(case),
            &by_id,
            "case {case}: describing by ResourceArn must report the same protection as by id"
        );

        let protection = by_id
            .get("Protection")
            .unwrap_or_else(|| panic!("case {case}: no Protection in {by_id}"));
        assert_eq!(
            member(protection, "Id"),
            id,
            "case {case}: expected the created id, got {protection}"
        );
        assert_eq!(
            member(protection, "Name"),
            name,
            "case {case}: expected the created name, got {protection}"
        );
        assert_eq!(
            member(protection, "ResourceArn"),
            *resource_arn,
            "case {case}: expected the protected resource's ARN, got {protection}"
        );
        assert_eq!(
            member(protection, "ProtectionArn"),
            format!("arn:aws:shield::{ACCOUNT_ID}:protection/{id}"),
            "case {case}: expected the protection's own ARN, got {protection}"
        );
        assert_eq!(
            protection.get("HealthCheckIds").and_then(Value::as_array),
            Some(&vec![]),
            "case {case}: a new protection has no health check associated, got {protection}"
        );

        // The resource type follows the ARN's documented layout, and every type this
        // service derives is one the model declares.
        assert_eq!(
            protected_resource_type(resource_arn),
            *resource_type,
            "case {case}: unexpected ProtectedResourceType for {resource_arn}"
        );
        if let Some(resource_type) = resource_type {
            assert!(
                PROTECTED_RESOURCE_TYPE_VALUES.contains(resource_type),
                "case {case}: {resource_type} is not a modelled ProtectedResourceType"
            );
        }
    }

    // `InclusionFilters` narrows the listed set, exactly as the stored records say:
    // one filter per type, and a protection must match every type given.
    for (case, resource_arn, resource_type) in RESOURCE_ARNS {
        let Some(resource_type) = resource_type else {
            continue;
        };
        let by_type = emulator
            .list_all_ids(
                case,
                &json!({ "InclusionFilters": { "ResourceTypes": [resource_type] } }),
                DEFAULT_LIST_PROTECTIONS_MAX_RESULTS as usize,
            )
            .await;
        let by_arn = emulator
            .list_all_ids(
                case,
                &json!({ "InclusionFilters": { "ResourceArns": [resource_arn] } }),
                DEFAULT_LIST_PROTECTIONS_MAX_RESULTS as usize,
            )
            .await;
        assert_eq!(
            by_type, by_arn,
            "case {case}: exactly one protection has resource type {resource_type}, so filtering \
             by type and by its ARN must report the same one"
        );
        assert_eq!(
            by_type.len(),
            1,
            "case {case}: expected one protection of type {resource_type}, got {by_type:?}"
        );
    }

    // A filter naming a value no protection carries matches nothing — not everything.
    let none = emulator
        .list_all_ids(
            "unmatched name filter",
            &json!({ "InclusionFilters": { "ProtectionNames": ["no-such-protection"] } }),
            DEFAULT_LIST_PROTECTIONS_MAX_RESULTS as usize,
        )
        .await;
    assert!(
        none.is_empty(),
        "case unmatched name filter: expected no protections, got {none:?}"
    );
}

#[tokio::test]
async fn describe_protection_needs_exactly_one_address() {
    let emulator = Emulator::new();
    emulator.subscribe().await;
    let arn = RESOURCE_ARNS[0].1;
    let id = emulator.create_protection("seed", "seed", arn).await;

    // Both addresses, or neither, is refused — the model documents "either ... but not
    // both".
    emulator
        .call(
            "DescribeProtection",
            &json!({ "ProtectionId": id, "ResourceArn": arn }),
        )
        .await
        .expect_error("both addresses", 400, "InvalidParameterException");
    emulator
        .call("DescribeProtection", &json!({}))
        .await
        .expect_error("neither address", 400, "InvalidParameterException");

    // An address that resolves to nothing is a missing resource, whichever form it
    // took. The unknown id is a well-formed `ProtectionId` (36 characters), so it is
    // the LOOKUP that fails, not the constraint validator.
    for (case, request) in [
        (
            "unknown id",
            json!({ "ProtectionId": "00000000-0000-4000-8000-000000000000" }),
        ),
        (
            "unknown arn",
            json!({ "ResourceArn": "arn:aws:cloudfront::000000000000:distribution/ENOTPROTECTED" }),
        ),
    ] {
        emulator
            .call("DescribeProtection", &request)
            .await
            .expect_error(case, 400, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn one_resource_carries_at_most_one_protection() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    for (case, resource_arn, _) in RESOURCE_ARNS {
        let id = emulator.create_protection(case, "first", resource_arn).await;

        // A second protection for the same resource is refused however it is named.
        emulator
            .call(
                "CreateProtection",
                &json!({ "Name": "second", "ResourceArn": resource_arn }),
            )
            .await
            .expect_error(case, 400, "ResourceAlreadyExistsException");

        // Releasing the protection releases the resource: it can be protected again,
        // under a new id.
        emulator
            .call("DeleteProtection", &json!({ "ProtectionId": id }))
            .await
            .expect_ok(case);
        let again = emulator
            .create_protection(case, "third", resource_arn)
            .await;
        assert_ne!(
            again, id,
            "case {case}: re-protecting a resource mints a new ProtectionId"
        );
    }
}

#[tokio::test]
async fn list_protections_reports_every_protection_once_in_creation_order() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    // Seven protections, so the page sizes below divide the set evenly, unevenly, and
    // not at all.
    let mut created = Vec::new();
    for index in 0..7 {
        created.push(
            emulator
                .create_protection(
                    &format!("seed {index}"),
                    &format!("protection-{index}"),
                    &format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/E{index:013}"),
                )
                .await,
        );
    }

    // Every page size reports the same protections, in creation order, each exactly
    // once — including a size larger than the set and the model's default (no
    // `MaxResults` at all).
    for page_size in [1_usize, 2, 3, 6, 7, 8, 10_000] {
        let ids = emulator
            .list_all_ids(
                &format!("page size {page_size}"),
                &json!({ "MaxResults": page_size }),
                page_size,
            )
            .await;
        assert_eq!(
            ids, created,
            "case page size {page_size}: expected every protection once in creation order"
        );
    }
    let ids = emulator
        .list_all_ids(
            "default page size",
            &json!({}),
            DEFAULT_LIST_PROTECTIONS_MAX_RESULTS as usize,
        )
        .await;
    assert_eq!(
        ids, created,
        "case default page size: expected every protection once in creation order"
    );

    // The first page of a two-at-a-time walk carries a token, and the page is exactly
    // the first two created.
    let (page, token) = emulator
        .list_page("first page of two", &json!({ "MaxResults": 2 }))
        .await;
    let page_ids: Vec<String> = page.iter().map(|p| member(p, "Id")).collect();
    assert_eq!(
        page_ids,
        created[..2].to_vec(),
        "case first page of two: expected the first two protections created"
    );
    let token = token.expect("case first page of two: four protections remain, so a NextToken is due");

    // A token keeps meaning the same place after an earlier protection is deleted: the
    // page that follows it never repeats what was already reported.
    emulator
        .call("DeleteProtection", &json!({ "ProtectionId": created[0] }))
        .await
        .expect_ok("delete the first protection");
    let (page, _) = emulator
        .list_page(
            "resume after a delete",
            &json!({ "MaxResults": 2, "NextToken": token }),
        )
        .await;
    let page_ids: Vec<String> = page.iter().map(|p| member(p, "Id")).collect();
    assert_eq!(
        page_ids,
        created[2..4].to_vec(),
        "case resume after a delete: expected the protections after the token's place"
    );

    // A token this service did not issue is refused rather than half-understood.
    for (case, token) in [
        ("not hex", "not-a-token"),
        ("hex of foreign text", "6e6f742d612d746f6b656e"),
        ("hex of a bad position", &::hex::encode("protections/v1:nope")),
        ("empty-ish", "00"),
    ] {
        emulator
            .call("ListProtections", &json!({ "NextToken": token }))
            .await
            .expect_error(case, 400, "InvalidPaginationTokenException");
    }
}

#[tokio::test]
async fn deleting_a_protection_removes_it_and_cannot_be_repeated() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    let mut ids = Vec::new();
    for (case, resource_arn, _) in RESOURCE_ARNS {
        ids.push(
            emulator
                .create_protection(case, "to-be-deleted", resource_arn)
                .await,
        );
    }

    for (index, id) in ids.iter().enumerate() {
        let case = format!("delete {index}");
        emulator
            .call("DeleteProtection", &json!({ "ProtectionId": id }))
            .await
            .expect_ok(&case);

        // Gone from both readers...
        emulator
            .call("DescribeProtection", &json!({ "ProtectionId": id }))
            .await
            .expect_error(&case, 400, "ResourceNotFoundException");
        let listed = emulator
            .list_all_ids(&case, &json!({ "MaxResults": 2 }), 2)
            .await;
        assert_eq!(
            listed,
            ids[index + 1..].to_vec(),
            "case {case}: expected only the protections not yet deleted"
        );

        // ...and a repeat delete finds nothing.
        emulator
            .call("DeleteProtection", &json!({ "ProtectionId": id }))
            .await
            .expect_error(&format!("{case} again"), 400, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn the_account_protection_limit_refuses_one_create_too_many() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    // Fill the account to its limit. Each ARN is distinct, so nothing is refused as a
    // duplicate on the way up.
    let mut ids = Vec::new();
    for index in 0..MAX_PROTECTIONS_PER_ACCOUNT {
        ids.push(
            emulator
                .create_protection(
                    "filling the account",
                    &format!("protection-{index}"),
                    &format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/EFILL{index:011}"),
                )
                .await,
        );
    }

    // One more would exceed the limit, and the refusal names the limit it would
    // exceed.
    let over = emulator
        .call(
            "CreateProtection",
            &json!({
                "Name": "one-too-many",
                "ResourceArn": format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/EOVERTHELIMIT"),
            }),
        )
        .await;
    over.expect_error("one past the limit", 400, "LimitsExceededException");
    assert_eq!(
        over.body.get("Limit").and_then(Value::as_i64),
        Some(MAX_PROTECTIONS_PER_ACCOUNT),
        "case one past the limit: the refusal must carry the threshold, got {over:?}"
    );
    assert_eq!(
        member(&over.body, "Type"),
        "protection",
        "case one past the limit: the refusal must name the limit's type, got {over:?}"
    );

    // Releasing a protection makes room for exactly one more.
    emulator
        .call("DeleteProtection", &json!({ "ProtectionId": ids[0] }))
        .await
        .expect_ok("free one slot");
    emulator
        .create_protection(
            "after freeing a slot",
            "room-for-one-more",
            &format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/EOVERTHELIMIT"),
        )
        .await;
    emulator
        .call(
            "CreateProtection",
            &json!({
                "Name": "one-too-many-again",
                "ResourceArn": format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/EONEMORE"),
            }),
        )
        .await
        .expect_error("full again", 400, "LimitsExceededException");
}

#[tokio::test]
async fn protection_operations_need_a_subscription() {
    let emulator = Emulator::new();

    // Without a subscription there is nothing to add a protection to, and no set to
    // list.
    emulator
        .call(
            "CreateProtection",
            &json!({ "Name": "unsubscribed", "ResourceArn": RESOURCE_ARNS[0].1 }),
        )
        .await
        .expect_error("create without a subscription", 400, "ResourceNotFoundException");
    emulator
        .call("ListProtections", &json!({}))
        .await
        .expect_error("list without a subscription", 400, "ResourceNotFoundException");

    // And once the account subscribes, the very same calls work.
    emulator.subscribe().await;
    let id = emulator
        .create_protection("create after subscribing", "subscribed", RESOURCE_ARNS[0].1)
        .await;
    let listed = emulator
        .list_all_ids(
            "list after subscribing",
            &json!({}),
            DEFAULT_LIST_PROTECTIONS_MAX_RESULTS as usize,
        )
        .await;
    assert_eq!(
        listed,
        vec![id],
        "case list after subscribing: expected the protection just created"
    );
}
