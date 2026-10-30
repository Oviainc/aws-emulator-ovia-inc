// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Shield Advanced protection-group, tagging and
//! account-level surface: CreateProtectionGroup, DescribeProtectionGroup,
//! ListProtectionGroups, ListResourcesInProtectionGroup, TagResource,
//! ListTagsForResource, DescribeEmergencyContactSettings,
//! UpdateEmergencyContactSettings, AssociateDRTRole, DisassociateDRTRole and
//! UpdateSubscription.
//!
//! Every case goes over the wire — an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AWSShield_20160616.<Operation>` through the same
//! `MultiServiceRouter` the production router serves — so routing, deserialization,
//! modelled-constraint validation, the handler and serialization are all exercised, and
//! what a write stored is read back through a SEPARATE request.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * a protection group's `Pattern`, `ResourceType` and `Members` constrain each other
//!   (`Members`: "You must set this when you set Pattern to ARBITRARY and you must not
//!   set it for any other Pattern setting"; `ResourceType`: the same for
//!   `BY_RESOURCE_TYPE`), and each named member must be a resource the account protects:
//!   any other combination is refused as a bad parameter, ahead of the account's
//!   subscription gate, and stores nothing;
//! * an operation whose only addressable resource is the account refuses with the
//!   account-level not-found wording while the account holds no subscription;
//! * an operation that addresses a protection group refuses with the group-level
//!   wording, which shares the error code and must stay distinct;
//! * a listing refuses a `NextToken` it did not issue before it reads the store at all;
//! * a group's membership follows its `Pattern`, and a listing reports it in the
//!   account's protection creation order, `MaxResults` per page;
//! * a tagging operation resolves `ResourceARN` to a Shield resource the account holds
//!   and refuses any other ARN as an invalid resource;
//! * `AutoRenew` is settled only in the last 30 days of the subscription period.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_shield_awsshield_20160616::state::{
    service_state, AUTO_RENEW_ENABLED, MAX_ARBITRARY_PATTERN_MEMBERS,
};
use service_com_amazonaws_shield_awsshield_20160616::EmulatedService;

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSShield_20160616.";

/// The account segment every emulated ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// How AWS words the account-level absence, shared by every operation whose only
/// addressable resource is the account's subscription.
const SUBSCRIPTION_NOT_FOUND: &str = "The subscription does not exist.";

/// How AWS words the absence of an addressed protection group — the same error code as
/// [`SUBSCRIPTION_NOT_FOUND`], deliberately different text.
const GROUP_NOT_FOUND: &str = "The referenced protection-group does not exist.";

/// How AWS words a protection group whose field combination it does not accept.
const GROUP_FIELDS_INCORRECT: &str = "One or more protection group input fields are incorrect.";

/// How AWS words an ARN that names no Shield resource of the account.
const INVALID_RESOURCE: &str = "The referenced resource is invalid.";

/// How AWS words a `NextToken` the listing did not issue.
const INVALID_PAGINATION_TOKEN: &str = "Invalid pagination token.";

/// One answer from the emulator: the HTTP status and the parsed JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type`), or `""` for a success. A `__type` may be
    /// namespaced (`com.amazonaws.shield#Foo`); the code clients branch on is the shape
    /// name.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .map(|t| t.rsplit('#').next().unwrap_or(t))
            .unwrap_or_default()
    }

    /// The error message as the envelope carries it, under either spelling the
    /// `awsJson1_1` error shape may use.
    fn error_message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Assert this reply is the modelled failure `code` with HTTP `status` and exactly
    /// `message`.
    fn expect_error(&self, case: &str, status: u16, code: &str, message: &str) {
        assert_eq!(
            (self.status, self.error_code(), self.error_message()),
            (status, code, message),
            "case {case}: expected HTTP {status} {code} {message:?}, got {self:?}"
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
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's; the
        // routing key still comes from the `Authorization` header below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
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

    /// Protect a resource, asserting the create succeeded.
    async fn protect(&self, case: &str, name: &str, resource_arn: &str) {
        self.call(
            "CreateProtection",
            &json!({ "Name": name, "ResourceArn": resource_arn }),
        )
        .await
        .expect_ok(case);
    }

    /// Create a protection group, asserting the create succeeded.
    async fn create_group(&self, case: &str, request: &Value) {
        self.call("CreateProtectionGroup", request)
            .await
            .expect_ok(case);
    }

    /// The stored `ProtectionGroup` as `DescribeProtectionGroup` reports it.
    async fn describe_group(&self, case: &str, protection_group_id: &str) -> Value {
        let reply = self
            .call(
                "DescribeProtectionGroup",
                &json!({ "ProtectionGroupId": protection_group_id }),
            )
            .await;
        reply
            .expect_ok(case)
            .get("ProtectionGroup")
            .unwrap_or_else(|| panic!("case {case}: no ProtectionGroup in {:?}", reply.body))
            .clone()
    }

    /// Walk every page of a listing, asserting no page exceeds `page_size` and that only
    /// the last page carries no token. Hands back the values of `member` in report order.
    async fn list_all(
        &self,
        case: &str,
        operation: &str,
        request: &Value,
        member: &str,
        page_size: usize,
        extract: impl Fn(&Value) -> String,
    ) -> Vec<String> {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..=MAX_PAGES {
            let mut body = request.clone();
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let reply = self.call(operation, &body).await;
            let answer = reply.expect_ok(case);
            let items = answer
                .get(member)
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: no {member} in {answer}"))
                .clone();
            assert!(
                items.len() <= page_size,
                "case {case}: page {page} carries {} items, more than the page size {page_size}",
                items.len()
            );
            for item in &items {
                seen.push(extract(item));
            }
            match answer.get("NextToken").and_then(Value::as_str) {
                None => return seen,
                Some(next) => {
                    assert!(
                        !items.is_empty(),
                        "case {case}: page {page} carries a NextToken but no items"
                    );
                    token = Some(next.to_string());
                }
            }
        }
        panic!("case {case}: {operation} never stopped issuing NextToken");
    }

    /// Move the stored subscription's end so that it is `remaining` away from now, the
    /// way the passage of time would. Reaches the store directly because no modelled
    /// operation sets the end of a subscription.
    fn set_remaining_commitment(&self, remaining: Duration) {
        let state = service_state(&*self.engine).expect("the shield store");
        let mut guard = state.lock();
        let subscription = guard
            .subscription
            .as_mut()
            .expect("an activated subscription");
        subscription.end_time = SystemTime::now() + remaining;
    }
}

/// The most pages [`Emulator::list_all`] follows before calling the pagination broken.
const MAX_PAGES: usize = 64;

/// A string member of a JSON object, which must be present.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member {name} is absent or not a string in {body}"))
        .to_string()
}

/// The string entries of a JSON array member, which must be present.
fn members(body: &Value, name: &str) -> Vec<String> {
    body.get(name)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("member {name} is absent or not an array in {body}"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("member {name} holds a non-string entry in {body}"))
                .to_string()
        })
        .collect()
}

/// Protected resources this suite uses, one per ARN layout
/// `CreateProtectionRequest$ResourceArn` documents that the issue did not cite, each with
/// the `ProtectedResourceType` it names (the values of the model's
/// `ProtectedResourceType` enum).
const APPLICATION_LOAD_BALANCER_ARN: &str =
    "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/app/front-door/0123456789ab";
const CLOUDFRONT_DISTRIBUTION_ARN: &str = "arn:aws:cloudfront::000000000000:distribution/E1XMPL01";
const GLOBAL_ACCELERATOR_ARN: &str =
    "arn:aws:globalaccelerator::000000000000:accelerator/a1b2c3d4-5678-90ab-cdef-EXAMPLE11111";
const ELASTIC_IP_ALLOCATION_ARN: &str =
    "arn:aws:ec2:eu-west-1:000000000000:eip-allocation/eipalloc-0123456789abcdef0";

/// One `CreateProtectionGroup` field combination and what it is refused with.
struct GroupFieldCase {
    case: &'static str,
    aggregation: &'static str,
    pattern: &'static str,
    resource_type: Option<&'static str>,
    group_members: &'static [&'static str],
    /// Whether the combination itself is wrong, so the request could not name a group
    /// under ANY account; otherwise the request is structurally sound and only the
    /// account's subscription is missing.
    fields_rejected: bool,
}

#[tokio::test]
async fn protection_group_fields_are_judged_ahead_of_the_subscription_gate() {
    let emulator = Emulator::new();

    // One row per way the three members can combine, on an account with no subscription.
    // `Members` entries name resources this unsubscribed account cannot be protecting.
    const ROWS: &[GroupFieldCase] = &[
        GroupFieldCase {
            case: "all",
            aggregation: "SUM",
            pattern: "ALL",
            resource_type: None,
            group_members: &[],
            fields_rejected: false,
        },
        GroupFieldCase {
            case: "all with a resource type",
            aggregation: "SUM",
            pattern: "ALL",
            resource_type: Some("GLOBAL_ACCELERATOR"),
            group_members: &[],
            fields_rejected: true,
        },
        GroupFieldCase {
            case: "all with members",
            aggregation: "MEAN",
            pattern: "ALL",
            resource_type: None,
            group_members: &[GLOBAL_ACCELERATOR_ARN],
            fields_rejected: true,
        },
        GroupFieldCase {
            case: "by resource type with members",
            aggregation: "SUM",
            pattern: "BY_RESOURCE_TYPE",
            resource_type: Some("ELASTIC_IP_ALLOCATION"),
            group_members: &[ELASTIC_IP_ALLOCATION_ARN],
            fields_rejected: true,
        },
        GroupFieldCase {
            case: "by resource type without a resource type",
            aggregation: "MAX",
            pattern: "BY_RESOURCE_TYPE",
            resource_type: None,
            group_members: &[],
            fields_rejected: true,
        },
        GroupFieldCase {
            case: "arbitrary with an unprotected member",
            aggregation: "SUM",
            pattern: "ARBITRARY",
            resource_type: None,
            group_members: &[CLOUDFRONT_DISTRIBUTION_ARN],
            fields_rejected: true,
        },
        GroupFieldCase {
            case: "arbitrary with two unprotected members",
            aggregation: "MEAN",
            pattern: "ARBITRARY",
            resource_type: None,
            group_members: &[CLOUDFRONT_DISTRIBUTION_ARN, GLOBAL_ACCELERATOR_ARN],
            fields_rejected: true,
        },
        GroupFieldCase {
            case: "arbitrary without members",
            aggregation: "MAX",
            pattern: "ARBITRARY",
            resource_type: None,
            group_members: &[],
            fields_rejected: true,
        },
        GroupFieldCase {
            case: "arbitrary with a resource type",
            aggregation: "SUM",
            pattern: "ARBITRARY",
            resource_type: Some("CLOUDFRONT_DISTRIBUTION"),
            group_members: &[CLOUDFRONT_DISTRIBUTION_ARN],
            fields_rejected: true,
        },
    ];

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let group_id = format!("group-{index}");
        let mut request = json!({
            "ProtectionGroupId": group_id,
            "Aggregation": row.aggregation,
            "Pattern": row.pattern,
        });
        if let Some(resource_type) = row.resource_type {
            request["ResourceType"] = json!(resource_type);
        }
        if !row.group_members.is_empty() {
            request["Members"] = json!(row.group_members);
        }

        let reply = emulator.call("CreateProtectionGroup", &request).await;
        if row.fields_rejected {
            reply.expect_error(case, 400, "InvalidParameterException", GROUP_FIELDS_INCORRECT);
        } else {
            reply.expect_error(case, 400, "ResourceNotFoundException", SUBSCRIPTION_NOT_FOUND);
        }

        // Refused either way, so no group was stored: the read reports the group-level
        // absence, not the account-level one.
        emulator
            .call(
                "DescribeProtectionGroup",
                &json!({ "ProtectionGroupId": group_id }),
            )
            .await
            .expect_error(case, 400, "ResourceNotFoundException", GROUP_NOT_FOUND);
    }

    // An explicitly EMPTY `Members` list is not a set one: an ARBITRARY group spells its
    // membership out, and the empty list is accepted for every other pattern because it
    // names nothing.
    emulator
        .call(
            "CreateProtectionGroup",
            &json!({
                "ProtectionGroupId": "group-empty-arbitrary",
                "Aggregation": "SUM",
                "Pattern": "ARBITRARY",
                "Members": [],
            }),
        )
        .await
        .expect_error(
            "arbitrary with an empty member list",
            400,
            "InvalidParameterException",
            GROUP_FIELDS_INCORRECT,
        );
    emulator
        .call(
            "CreateProtectionGroup",
            &json!({
                "ProtectionGroupId": "group-empty-all",
                "Aggregation": "SUM",
                "Pattern": "ALL",
                "Members": [],
            }),
        )
        .await
        .expect_error(
            "all with an empty member list",
            400,
            "ResourceNotFoundException",
            SUBSCRIPTION_NOT_FOUND,
        );
}

#[tokio::test]
async fn protection_groups_round_trip_and_report_their_members() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    // The account's protections, in creation order — the order every membership listing
    // reports.
    emulator
        .protect("seed", "front-door", APPLICATION_LOAD_BALANCER_ARN)
        .await;
    emulator
        .protect("seed", "edge", CLOUDFRONT_DISTRIBUTION_ARN)
        .await;
    emulator
        .protect("seed", "anycast", GLOBAL_ACCELERATOR_ARN)
        .await;

    // One row per `Pattern`, each with the membership that pattern implies.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "all",
            json!({
                "ProtectionGroupId": "every-resource",
                "Aggregation": "SUM",
                "Pattern": "ALL",
            }),
            vec![
                APPLICATION_LOAD_BALANCER_ARN.to_string(),
                CLOUDFRONT_DISTRIBUTION_ARN.to_string(),
                GLOBAL_ACCELERATOR_ARN.to_string(),
            ],
        ),
        (
            "by resource type",
            json!({
                "ProtectionGroupId": "accelerators",
                "Aggregation": "MEAN",
                "Pattern": "BY_RESOURCE_TYPE",
                "ResourceType": "GLOBAL_ACCELERATOR",
            }),
            vec![GLOBAL_ACCELERATOR_ARN.to_string()],
        ),
        (
            "arbitrary",
            json!({
                "ProtectionGroupId": "hand-picked",
                "Aggregation": "MAX",
                "Pattern": "ARBITRARY",
                "Members": [CLOUDFRONT_DISTRIBUTION_ARN, APPLICATION_LOAD_BALANCER_ARN],
            }),
            vec![
                CLOUDFRONT_DISTRIBUTION_ARN.to_string(),
                APPLICATION_LOAD_BALANCER_ARN.to_string(),
            ],
        ),
    ];

    for (case, request, expected_resources) in &rows {
        emulator.create_group(case, request).await;
        let group_id = member(request, "ProtectionGroupId");

        // The stored record answers the read, member for member.
        let stored = emulator.describe_group(case, &group_id).await;
        assert_eq!(
            member(&stored, "ProtectionGroupId"),
            group_id,
            "case {case}: expected the group's own id, got {stored}"
        );
        assert_eq!(
            member(&stored, "Aggregation"),
            member(request, "Aggregation"),
            "case {case}: expected the submitted Aggregation, got {stored}"
        );
        assert_eq!(
            member(&stored, "Pattern"),
            member(request, "Pattern"),
            "case {case}: expected the submitted Pattern, got {stored}"
        );
        assert_eq!(
            stored.get("ResourceType").and_then(Value::as_str),
            request.get("ResourceType").and_then(Value::as_str),
            "case {case}: expected the submitted ResourceType, got {stored}"
        );
        assert_eq!(
            members(&stored, "Members"),
            request
                .get("Members")
                .map(|_| members(request, "Members"))
                .unwrap_or_default(),
            "case {case}: expected the submitted Members, got {stored}"
        );
        assert_eq!(
            member(&stored, "ProtectionGroupArn"),
            format!("arn:aws:shield::{ACCOUNT_ID}:protection-group/{group_id}"),
            "case {case}: expected the group's own ARN, got {stored}"
        );

        // The membership the pattern implies, in the account's protection creation order.
        let reported = emulator
            .list_all(
                case,
                "ListResourcesInProtectionGroup",
                &json!({ "ProtectionGroupId": group_id }),
                "ResourceArns",
                20,
                |item| {
                    item.as_str()
                        .expect("a ResourceArns entry is a string")
                        .to_string()
                },
            )
            .await;
        assert_eq!(
            &reported, expected_resources,
            "case {case}: unexpected membership for group {group_id}"
        );

        // The same membership one entry at a time: paging does not change the set or its
        // order.
        let paged = emulator
            .list_all(
                case,
                "ListResourcesInProtectionGroup",
                &json!({ "ProtectionGroupId": group_id, "MaxResults": 1 }),
                "ResourceArns",
                1,
                |item| {
                    item.as_str()
                        .expect("a ResourceArns entry is a string")
                        .to_string()
                },
            )
            .await;
        assert_eq!(
            &paged, expected_resources,
            "case {case}: one-per-page listing diverged for group {group_id}"
        );
    }

    // A second group under a name the account already holds is a conflict.
    emulator
        .call("CreateProtectionGroup", &rows[0].1)
        .await
        .expect_error(
            "a repeated group id",
            400,
            "ResourceAlreadyExistsException",
            "The protection group every-resource already exists.",
        );

    // Every stored group, in creation order, whole and one page at a time.
    let expected_ids: Vec<String> = rows
        .iter()
        .map(|(_, request, _)| member(request, "ProtectionGroupId"))
        .collect();
    for page_size in [20usize, 1, 2] {
        let mut request = json!({});
        if page_size != 20 {
            request["MaxResults"] = json!(page_size);
        }
        let listed = emulator
            .list_all(
                &format!("list with page size {page_size}"),
                "ListProtectionGroups",
                &request,
                "ProtectionGroups",
                page_size,
                |item| member(item, "ProtectionGroupId"),
            )
            .await;
        assert_eq!(
            listed, expected_ids,
            "case list with page size {page_size}: expected every group in creation order"
        );
    }

    // One row per filter type `InclusionProtectionGroupFilters` declares: a present type
    // must hold, an absent one constrains nothing.
    let filter_rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "by id",
            json!({ "ProtectionGroupIds": ["accelerators"] }),
            vec!["accelerators".to_string()],
        ),
        (
            "by pattern",
            json!({ "Patterns": ["ARBITRARY"] }),
            vec!["hand-picked".to_string()],
        ),
        (
            "by aggregation",
            json!({ "Aggregations": ["SUM"] }),
            vec!["every-resource".to_string()],
        ),
        (
            "by resource type",
            json!({ "ResourceTypes": ["GLOBAL_ACCELERATOR"] }),
            vec!["accelerators".to_string()],
        ),
        (
            "by a resource type no group carries",
            json!({ "ResourceTypes": ["CLASSIC_LOAD_BALANCER"] }),
            Vec::new(),
        ),
        (
            "by two criteria that cannot both hold",
            json!({ "Patterns": ["ALL"], "Aggregations": ["MAX"] }),
            Vec::new(),
        ),
    ];
    for (case, filters, expected) in &filter_rows {
        let listed = emulator
            .list_all(
                case,
                "ListProtectionGroups",
                &json!({ "InclusionFilters": filters }),
                "ProtectionGroups",
                20,
                |item| member(item, "ProtectionGroupId"),
            )
            .await;
        assert_eq!(
            &listed, expected,
            "case {case}: unexpected groups for filters {filters}"
        );
    }
}

#[tokio::test]
async fn an_arbitrary_group_may_name_no_more_members_than_the_subscription_grants() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    // One protection per member the subscription's
    // `ProtectionGroupArbitraryPatternLimits$MaxMembers` grants, plus one: every member
    // must be a protected resource, so the account protects them all first.
    let limit = MAX_ARBITRARY_PATTERN_MEMBERS as usize;
    let mut protected = Vec::new();
    for index in 0..=limit {
        let resource_arn = format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/E{index:013}");
        emulator
            .protect("fill the membership", &format!("member-{index}"), &resource_arn)
            .await;
        protected.push(resource_arn);
    }

    // Exactly the greatest membership the subscription grants is accepted and stored.
    emulator
        .create_group(
            "the greatest membership granted",
            &json!({
                "ProtectionGroupId": "at-the-limit",
                "Aggregation": "SUM",
                "Pattern": "ARBITRARY",
                "Members": protected[..limit],
            }),
        )
        .await;
    let stored = emulator
        .describe_group("the greatest membership granted", "at-the-limit")
        .await;
    assert_eq!(
        members(&stored, "Members").len(),
        limit,
        "case the greatest membership granted: expected {limit} members, got {stored}"
    );

    // One member past it is refused with the limit it would exceed, and nothing is
    // stored.
    emulator
        .call(
            "CreateProtectionGroup",
            &json!({
                "ProtectionGroupId": "past-the-limit",
                "Aggregation": "SUM",
                "Pattern": "ARBITRARY",
                "Members": protected,
            }),
        )
        .await
        .expect_error(
            "one member past the limit",
            400,
            "LimitsExceededException",
            &format!("A protection group cannot name more than {limit} members."),
        );
    emulator
        .call(
            "DescribeProtectionGroup",
            &json!({ "ProtectionGroupId": "past-the-limit" }),
        )
        .await
        .expect_error(
            "one member past the limit",
            400,
            "ResourceNotFoundException",
            GROUP_NOT_FOUND,
        );
}

#[tokio::test]
async fn a_token_a_listing_did_not_issue_is_refused_before_the_store_is_read() {
    let emulator = Emulator::new();

    // Tokens no listing of this service minted: a forged one, an empty-ish one, a token
    // shaped like another listing's, and (below) one this listing DID mint but for a
    // different listing.
    const FORGED: &[(&str, &str)] = &[
        ("free text", "not-a-real-continuation-token-xxxxxxxxxxxx"),
        ("a single character", "x"),
        ("hex that decodes to nothing known", "6162636465"),
        ("a plausible base64 token", "eyJvZmZzZXQiOiAxfQ=="),
    ];

    // The account holds no subscription, so a token-less call is refused for the account
    // — which is exactly what proves the token is inspected FIRST.
    for (case, token) in FORGED {
        for operation in ["ListProtectionGroups", "ListProtections"] {
            emulator
                .call(operation, &json!({ "NextToken": token }))
                .await
                .expect_error(
                    &format!("{operation} with {case}"),
                    400,
                    "InvalidPaginationTokenException",
                    INVALID_PAGINATION_TOKEN,
                );
        }
        emulator
            .call(
                "ListResourcesInProtectionGroup",
                &json!({ "ProtectionGroupId": "never-created", "NextToken": token }),
            )
            .await
            .expect_error(
                &format!("ListResourcesInProtectionGroup with {case}"),
                400,
                "InvalidPaginationTokenException",
                INVALID_PAGINATION_TOKEN,
            );
    }

    // With a subscription and more groups than fit one page, the token this listing
    // issues is accepted — and the one it issued is not understood by another listing.
    emulator.subscribe().await;
    for index in 0..2 {
        emulator
            .create_group(
                "seed",
                &json!({
                    "ProtectionGroupId": format!("group-{index}"),
                    "Aggregation": "SUM",
                    "Pattern": "ALL",
                }),
            )
            .await;
    }
    let first = emulator
        .call("ListProtectionGroups", &json!({ "MaxResults": 1 }))
        .await;
    let token = member(first.expect_ok("first page"), "NextToken");
    emulator
        .call(
            "ListProtectionGroups",
            &json!({ "MaxResults": 1, "NextToken": token }),
        )
        .await
        .expect_ok("the page the token names");
    emulator
        .call("ListProtections", &json!({ "NextToken": token }))
        .await
        .expect_error(
            "another listing's token",
            400,
            "InvalidPaginationTokenException",
            INVALID_PAGINATION_TOKEN,
        );

    // A token this listing WOULD issue but for a position past the end of the membership
    // is understood, and names the last (empty) page: a position is not an error, and no
    // position — however large — may take the handler out of the collection it indexes.
    emulator
        .create_group(
            "a group to page over",
            &json!({
                "ProtectionGroupId": "paged",
                "Aggregation": "SUM",
                "Pattern": "ALL",
            }),
        )
        .await;
    let past_the_end = ::hex::encode(format!(
        "protection-group-resources/v1:{}",
        u64::MAX - 1
    ));
    let reply = emulator
        .call(
            "ListResourcesInProtectionGroup",
            &json!({ "ProtectionGroupId": "paged", "NextToken": past_the_end }),
        )
        .await;
    let body = reply.expect_ok("a position past the end of the membership");
    assert_eq!(
        body.get("ResourceArns").and_then(Value::as_array),
        Some(&vec![]),
        "case a position past the end of the membership: expected an empty last page, got {body}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "case a position past the end of the membership: the last page carries no token, got {body}"
    );
}

#[tokio::test]
async fn account_level_operations_name_the_missing_subscription() {
    let emulator = Emulator::new();

    // One row per operation whose only addressable resource is the account itself: with
    // no subscription, each reports the account-level absence.
    const ACCOUNT_SCOPED: &[(&str, &str)] = &[
        ("describe emergency contacts", "DescribeEmergencyContactSettings"),
        ("update emergency contacts", "UpdateEmergencyContactSettings"),
        ("associate the response team's role", "AssociateDRTRole"),
        ("disassociate the response team's role", "DisassociateDRTRole"),
        ("list protection groups", "ListProtectionGroups"),
        ("update the subscription", "UpdateSubscription"),
    ];
    for (case, operation) in ACCOUNT_SCOPED {
        // Only the members the operation requires; everything else is optional.
        let request = match *operation {
            "AssociateDRTRole" => {
                json!({ "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/ShieldResponseTeam") })
            }
            "UpdateSubscription" => json!({ "AutoRenew": "DISABLED" }),
            _ => json!({}),
        };
        emulator
            .call(operation, &request)
            .await
            .expect_error(case, 400, "ResourceNotFoundException", SUBSCRIPTION_NOT_FOUND);
    }

    // An operation that addresses a GROUP reports the group-level absence instead, even
    // on the same unsubscribed account: the two share the error code and nothing else.
    for (case, operation, request) in [
        (
            "describe a group that was never created",
            "DescribeProtectionGroup",
            json!({ "ProtectionGroupId": "never-created" }),
        ),
        (
            "list the resources of a group that was never created",
            "ListResourcesInProtectionGroup",
            json!({ "ProtectionGroupId": "never-created" }),
        ),
        (
            "list the resources of a group named with the maximum id length",
            "ListResourcesInProtectionGroup",
            json!({ "ProtectionGroupId": "a".repeat(36) }),
        ),
    ] {
        emulator
            .call(operation, &request)
            .await
            .expect_error(case, 400, "ResourceNotFoundException", GROUP_NOT_FOUND);
    }
}

#[tokio::test]
async fn tagging_resolves_only_the_shield_resources_the_account_holds() {
    let emulator = Emulator::new();

    // One row per ARN that names no Shield resource of this account: a Shield ARN whose
    // resource was never created, a Shield ARN of another account, a Shield resource that
    // carries no tags, and ARNs of services Shield does not manage tags on. Every row
    // satisfies `ResourceArn`'s own `^arn:aws` pattern, so the handler — not the
    // modelled-constraint layer — is what refuses it.
    let unknown_arns: Vec<(&str, String)> = vec![
        (
            "a protection that was never created",
            format!("arn:aws:shield::{ACCOUNT_ID}:protection/00000000-0000-0000-0000-000000000000"),
        ),
        (
            "a protection group that was never created",
            format!("arn:aws:shield::{ACCOUNT_ID}:protection-group/never-created"),
        ),
        (
            "another account's protection",
            "arn:aws:shield::111122223333:protection/11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
        (
            "the account's subscription",
            format!("arn:aws:shield::{ACCOUNT_ID}:subscription/00000000-0000-0000-0000-000000000000"),
        ),
        ("an S3 bucket", "arn:aws:s3:::shield-emulator-bucket".to_string()),
        (
            "an IAM role",
            format!("arn:aws:iam::{ACCOUNT_ID}:role/ShieldResponseTeam"),
        ),
        (
            "a protected resource rather than its protection",
            CLOUDFRONT_DISTRIBUTION_ARN.to_string(),
        ),
    ];

    for (case, resource_arn) in &unknown_arns {
        emulator
            .call(
                "TagResource",
                &json!({ "ResourceARN": resource_arn, "Tags": [{ "Key": "team", "Value": "edge" }] }),
            )
            .await
            .expect_error(
                &format!("TagResource on {case}"),
                400,
                "InvalidResourceException",
                INVALID_RESOURCE,
            );
        emulator
            .call("ListTagsForResource", &json!({ "ResourceARN": resource_arn }))
            .await
            .expect_error(
                &format!("ListTagsForResource on {case}"),
                400,
                "InvalidResourceException",
                INVALID_RESOURCE,
            );
    }

    // The resources Shield DOES hold tags on: a protection and a protection group.
    emulator.subscribe().await;
    emulator
        .protect("seed", "edge", CLOUDFRONT_DISTRIBUTION_ARN)
        .await;
    let protection_id = {
        let state = service_state(&*emulator.engine).expect("the shield store");
        let guard = state.lock();
        guard
            .protection_by_resource_arn(CLOUDFRONT_DISTRIBUTION_ARN)
            .expect("the protection just created")
            .id
            .clone()
    };
    emulator
        .create_group(
            "a group created with tags",
            &json!({
                "ProtectionGroupId": "tagged-group",
                "Aggregation": "SUM",
                "Pattern": "ALL",
                "Tags": [{ "Key": "owner", "Value": "platform" }],
            }),
        )
        .await;

    let protection_arn = format!("arn:aws:shield::{ACCOUNT_ID}:protection/{protection_id}");
    let group_arn = format!("arn:aws:shield::{ACCOUNT_ID}:protection-group/tagged-group");

    // A protection is created with no tags; the group was created with one.
    for (case, resource_arn, expected) in [
        ("an untagged protection", &protection_arn, Vec::new()),
        (
            "a group created with tags",
            &group_arn,
            vec![("owner".to_string(), "platform".to_string())],
        ),
    ] {
        let reply = emulator
            .call("ListTagsForResource", &json!({ "ResourceARN": resource_arn }))
            .await;
        assert_eq!(
            tag_pairs(reply.expect_ok(case)),
            expected,
            "case {case}: unexpected stored tags for {resource_arn}"
        );
    }

    // One row per way a tagging call meets the stored tags: a new key is added, a key
    // already held takes the new value, and a key the call does not name is untouched.
    let tag_rows: Vec<TagRow> = vec![
        (
            "the first tag of a protection",
            &protection_arn,
            json!([{ "Key": "team", "Value": "edge" }]),
            vec![("team".to_string(), "edge".to_string())],
        ),
        (
            "a second key beside it",
            &protection_arn,
            json!([{ "Key": "tier", "Value": "gold" }]),
            vec![
                ("team".to_string(), "edge".to_string()),
                ("tier".to_string(), "gold".to_string()),
            ],
        ),
        (
            "a new value for a key already held",
            &protection_arn,
            json!([{ "Key": "team", "Value": "core" }]),
            vec![
                ("team".to_string(), "core".to_string()),
                ("tier".to_string(), "gold".to_string()),
            ],
        ),
        (
            "no tags at all",
            &protection_arn,
            json!([]),
            vec![
                ("team".to_string(), "core".to_string()),
                ("tier".to_string(), "gold".to_string()),
            ],
        ),
        (
            "a group's tags beside the ones it was created with",
            &group_arn,
            json!([{ "Key": "owner", "Value": "network" }, { "Key": "cost-centre", "Value": "4711" }]),
            vec![
                ("owner".to_string(), "network".to_string()),
                ("cost-centre".to_string(), "4711".to_string()),
            ],
        ),
    ];
    for (case, resource_arn, tags, expected) in &tag_rows {
        emulator
            .call(
                "TagResource",
                &json!({ "ResourceARN": resource_arn, "Tags": tags }),
            )
            .await
            .expect_ok(case);
        let reply = emulator
            .call("ListTagsForResource", &json!({ "ResourceARN": resource_arn }))
            .await;
        assert_eq!(
            &tag_pairs(reply.expect_ok(case)),
            expected,
            "case {case}: unexpected tags after tagging {resource_arn}"
        );
    }

    // The resource's tag ceiling: the greatest number of tags a read can report is the
    // modelled `TagList` bound, and a call that would take the resource past it changes
    // nothing.
    let reply = emulator
        .call("ListTagsForResource", &json!({ "ResourceARN": group_arn }))
        .await;
    let already_held = tag_pairs(reply.expect_ok("the tags held before filling up")).len();
    let to_the_ceiling: Vec<Value> = (already_held..200)
        .map(|index| json!({ "Key": format!("key-{index}"), "Value": format!("value-{index}") }))
        .collect();
    emulator
        .call(
            "TagResource",
            &json!({ "ResourceARN": group_arn, "Tags": to_the_ceiling }),
        )
        .await
        .expect_ok("tags up to the ceiling");
    let reply = emulator
        .call("ListTagsForResource", &json!({ "ResourceARN": group_arn }))
        .await;
    let at_the_ceiling = tag_pairs(reply.expect_ok("tags up to the ceiling"));
    assert_eq!(
        at_the_ceiling.len(),
        200,
        "case tags up to the ceiling: expected the group to carry 200 tags, got {}",
        at_the_ceiling.len()
    );
    emulator
        .call(
            "TagResource",
            &json!({ "ResourceARN": group_arn, "Tags": [{ "Key": "one-too-many", "Value": "x" }] }),
        )
        .await
        .expect_error(
            "one tag past the ceiling",
            400,
            "InvalidParameterException",
            "The resource cannot carry more than 200 tags.",
        );
    let reply = emulator
        .call("ListTagsForResource", &json!({ "ResourceARN": group_arn }))
        .await;
    assert_eq!(
        tag_pairs(reply.expect_ok("one tag past the ceiling")),
        at_the_ceiling,
        "case one tag past the ceiling: the refused call must leave the tags alone"
    );

    // A protection that is released takes its tags with it, so its ARN names no Shield
    // resource any more.
    emulator
        .call("DeleteProtection", &json!({ "ProtectionId": protection_id }))
        .await
        .expect_ok("release the protection");
    emulator
        .call("ListTagsForResource", &json!({ "ResourceARN": protection_arn }))
        .await
        .expect_error(
            "a released protection",
            400,
            "InvalidResourceException",
            INVALID_RESOURCE,
        );
}

/// One tagging call and the tags the resource carries afterwards: the case name, the
/// `ResourceARN`, the `Tags` submitted and the key/value pairs a read must then report.
type TagRow<'a> = (&'a str, &'a String, Value, Vec<(String, String)>);

/// The `Tags` of a tagging answer as key/value pairs, in report order.
fn tag_pairs(body: &Value) -> Vec<(String, String)> {
    body.get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no Tags in {body}"))
        .iter()
        .map(|tag| (member(tag, "Key"), member(tag, "Value")))
        .collect()
}

#[tokio::test]
async fn emergency_contacts_round_trip_under_the_subscription() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    // A fresh subscription carries no contacts.
    let reply = emulator
        .call("DescribeEmergencyContactSettings", &json!({}))
        .await;
    assert_eq!(
        contact_emails(reply.expect_ok("a fresh subscription")),
        Vec::<String>::new(),
        "case a fresh subscription: expected no emergency contacts, got {:?}",
        reply.body
    );

    // One row per shape of list the member admits (`len 0..10`), each read back whole.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "one contact with only the required email",
            json!([{ "EmailAddress": "oncall@example.com" }]),
            vec!["oncall@example.com".to_string()],
        ),
        (
            "two contacts, one with a phone number and notes",
            json!([
                { "EmailAddress": "primary@example.com", "PhoneNumber": "+15551234567", "ContactNotes": "office hours" },
                { "EmailAddress": "secondary@example.com" },
            ]),
            vec![
                "primary@example.com".to_string(),
                "secondary@example.com".to_string(),
            ],
        ),
        (
            "the greatest number of contacts the list admits",
            json!((0..10)
                .map(|index| json!({ "EmailAddress": format!("contact{index}@example.com") }))
                .collect::<Vec<Value>>()),
            (0..10)
                .map(|index| format!("contact{index}@example.com"))
                .collect(),
        ),
        ("no contacts at all", json!([]), Vec::new()),
    ];

    for (case, contacts, expected) in &rows {
        emulator
            .call(
                "UpdateEmergencyContactSettings",
                &json!({ "EmergencyContactList": contacts }),
            )
            .await
            .expect_ok(case);
        let reply = emulator
            .call("DescribeEmergencyContactSettings", &json!({}))
            .await;
        assert_eq!(
            &contact_emails(reply.expect_ok(case)),
            expected,
            "case {case}: the read did not report what the update stored"
        );
    }

    // A whole contact is stored, not just its email.
    emulator
        .call(
            "UpdateEmergencyContactSettings",
            &json!({ "EmergencyContactList": [
                { "EmailAddress": "primary@example.com", "PhoneNumber": "+15551234567", "ContactNotes": "office hours" },
            ] }),
        )
        .await
        .expect_ok("a contact with every member");
    let reply = emulator
        .call("DescribeEmergencyContactSettings", &json!({}))
        .await;
    let stored = reply.expect_ok("a contact with every member");
    let contact = stored
        .get("EmergencyContactList")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .unwrap_or_else(|| panic!("no emergency contact in {stored}"));
    assert_eq!(
        (
            member(contact, "EmailAddress"),
            member(contact, "PhoneNumber"),
            member(contact, "ContactNotes"),
        ),
        (
            "primary@example.com".to_string(),
            "+15551234567".to_string(),
            "office hours".to_string(),
        ),
        "case a contact with every member: expected every member stored, got {contact}"
    );
}

/// The email address of every contact a `DescribeEmergencyContactSettings` answer
/// carries, in report order.
fn contact_emails(body: &Value) -> Vec<String> {
    body.get("EmergencyContactList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no EmergencyContactList in {body}"))
        .iter()
        .map(|contact| member(contact, "EmailAddress"))
        .collect()
}

#[tokio::test]
async fn the_response_team_role_is_associated_and_withdrawn_once() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    let role_arn = format!("arn:aws:iam::{ACCOUNT_ID}:role/ShieldResponseTeam");
    let other_role_arn = format!("arn:aws:iam::{ACCOUNT_ID}:role/AWSShieldDRTAccess");

    // Nothing is associated yet, so withdrawing would change nothing.
    emulator
        .call("DisassociateDRTRole", &json!({}))
        .await
        .expect_error(
            "withdraw with no role associated",
            400,
            "InvalidOperationException",
            "The operation would not cause any change to occur.",
        );

    // "You can associate only one RoleArn with your subscription": a second association
    // replaces the first, and one withdrawal clears it.
    for (case, arn) in [
        ("the first role", &role_arn),
        ("a replacement role", &other_role_arn),
    ] {
        emulator
            .call("AssociateDRTRole", &json!({ "RoleArn": arn }))
            .await
            .expect_ok(case);
    }
    emulator
        .call("DisassociateDRTRole", &json!({}))
        .await
        .expect_ok("withdraw the associated role");
    emulator
        .call("DisassociateDRTRole", &json!({}))
        .await
        .expect_error(
            "withdraw twice",
            400,
            "InvalidOperationException",
            "The operation would not cause any change to occur.",
        );
}

#[tokio::test]
async fn auto_renew_is_settled_only_in_the_last_thirty_days() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    // One row per span left on the commitment, with whether `AutoRenew` may be changed
    // there: the change window is the last 30 days of the subscription period.
    const DAY: u64 = 24 * 60 * 60;
    const ROWS: &[(&str, u64, bool)] = &[
        ("a year left", 365 * DAY, false),
        ("31 days left", 31 * DAY, false),
        ("exactly 30 days left", 30 * DAY, true),
        ("29 days left", 29 * DAY, true),
        ("an hour left", 60 * 60, true),
    ];

    for (case, remaining, changeable) in ROWS {
        // Back to the value a subscription is created with, so each row starts level.
        emulator.set_remaining_commitment(Duration::from_secs(30 * DAY));
        emulator
            .call("UpdateSubscription", &json!({ "AutoRenew": AUTO_RENEW_ENABLED }))
            .await
            .expect_ok(case);

        emulator.set_remaining_commitment(Duration::from_secs(*remaining));
        let reply = emulator
            .call("UpdateSubscription", &json!({ "AutoRenew": "DISABLED" }))
            .await;
        if *changeable {
            reply.expect_ok(case);
        } else {
            reply.expect_error(
                case,
                400,
                "LockedSubscriptionException",
                "You can change the AutoRenew parameter during the last 30 days of your subscription.",
            );
        }

        // Whatever the verdict, the subscription reports what it now holds.
        let describe = emulator.call("DescribeSubscription", &json!({})).await;
        let body = describe.expect_ok(case);
        let subscription = body
            .get("Subscription")
            .unwrap_or_else(|| panic!("case {case}: no Subscription in {body}"));
        let expected = if *changeable { "DISABLED" } else { AUTO_RENEW_ENABLED };
        assert_eq!(
            member(subscription, "AutoRenew"),
            expected,
            "case {case}: unexpected AutoRenew after the update"
        );
    }

    // "Only enter values for parameters you want to change. Empty parameters are not
    // updated": a call naming nothing changes nothing, wherever the period stands.
    emulator.set_remaining_commitment(Duration::from_secs(365 * DAY));
    emulator
        .call("UpdateSubscription", &json!({}))
        .await
        .expect_ok("a call that names no AutoRenew");
}
