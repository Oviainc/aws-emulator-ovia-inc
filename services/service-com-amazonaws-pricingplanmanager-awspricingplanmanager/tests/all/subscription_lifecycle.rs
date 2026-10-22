// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the subscription lifecycle: `CreateSubscription`,
//! `GetSubscription`, `UpdateSubscription`, `ListSubscriptions`, `CancelSubscription`
//! and `ApprovePaidSubscription`.
//!
//! Every assertion here goes over the wire through the same `MultiServiceRouter` the
//! emulator boots, so what is proven is the HTTP behaviour: the status, the `ETag`
//! header, the `@httpPayload` body and the persisted state behind them.
//!
//! The tests are tables: each row is one input in the class the rule covers (tier,
//! usage level, resource count, approval mode, page size, operation), so a rule is
//! asserted over a set rather than over the single case the issue cited.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::state::{
    format_date_time, parse_date_time,
    ComAmazonawsPricingplanmanagerAwspricingplanmanagerState as ServiceState, LIST_PAGE_SIZE,
    MAX_SUBSCRIPTIONS, PLAN_TIERS_ASCENDING,
};
use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::EmulatedService;

/// A SigV4 `Authorization` header scoped to this service in `us-west-2`. The router
/// routes on the credential scope's service name, and the handler reads the region out
/// of the same scope, so the minted ARNs must carry `us-west-2`.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/pricingplanmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The region in [`AUTHORIZATION`], which every minted ARN must name.
const REGION: &str = "us-west-2";

/// The canonical account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One HTTP exchange: status, parsed body, `ETag` header, error code.
struct Answer {
    status: u16,
    body: Value,
    etag: Option<String>,
    error_code: Option<String>,
}

impl Answer {
    /// The body member `name`, as a string. Panics with the whole body when absent, so
    /// a failure names the actual response rather than `None`.
    fn string(&self, name: &str) -> String {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("member {name} missing from body {:?}", self.body))
            .to_string()
    }

    /// The body member `name`, when present.
    fn opt_string(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `ETag` response header; panics when the operation did not send one.
    fn require_etag(&self, case: &str) -> String {
        self.etag
            .clone()
            .unwrap_or_else(|| panic!("case {case}: no ETag response header on {:?}", self.body))
    }
}

/// A router with this one service mounted on a single shared engine, with request
/// authentication off so a test does not have to produce real signatures.
fn router() -> MultiServiceRouter {
    router_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The same router, over an engine the caller keeps a handle to — so a test can read or
/// backdate the stored state the handlers see.
fn router_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// POST `/v1/<operation>` with `body`, optionally under `If-Match`.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    if_match: Option<&str>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://pricingplanmanager.{REGION}.amazonaws.com/v1/{operation}"
        ))
        .header("host", format!("pricingplanmanager.{REGION}.amazonaws.com"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    if let Some(tag) = if_match {
        builder = builder.header("if-match", tag);
    }
    let request = builder
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split('/')
                .next()
                .unwrap_or(value)
                .split('#')
                .next_back()
                .unwrap_or(value)
                .to_string()
        });
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer {
        status,
        body: parsed,
        etag,
        error_code,
    }
}

/// Create one subscription and return its answer, asserting only that it succeeded.
async fn create(router: &MultiServiceRouter, body: Value) -> Answer {
    let answer = call(router, "CreateSubscription", body.clone(), None).await;
    assert_eq!(
        answer.status, 200,
        "CreateSubscription {body:?} must succeed, got {} {:?}",
        answer.status, answer.body
    );
    answer
}

/// A minimal valid CreateSubscription request for `tier`.
fn create_body(family: &str, tier: &str, resources: usize, approval: Option<&str>) -> Value {
    let resource_arns: Vec<String> = (0..resources)
        .map(|index| format!("arn:aws:cloudfront::{ACCOUNT}:distribution/E{index}"))
        .collect();
    let mut body = json!({
        "planFamily": family,
        "planTier": tier,
        "resourceArns": resource_arns,
    });
    if let Some(mode) = approval {
        body["approvalMode"] = json!(mode);
    }
    body
}

// ---------------------------------------------------------------------------
// Create -> Get round trip
// ---------------------------------------------------------------------------

/// A created subscription is stored, and a later `GetSubscription` reports exactly what
/// was stored: the same ARN, plan family, tier, usage level, resources, status,
/// timestamps and `ETag`.
///
/// The rows differ along everything the rule generalises over — plan family, tier,
/// usage level present or absent, resource count across the modelled 1..10 bound, and
/// the approval mode — so nothing here is keyed to one recorded request's shape.
#[tokio::test]
async fn a_created_subscription_reads_back_unchanged() {
    struct Case {
        family: &'static str,
        tier: &'static str,
        usage_level: Option<&'static str>,
        resources: usize,
        approval: Option<&'static str>,
        expected_status: &'static str,
    }

    // `approvalMode` unset or IMMEDIATE activates immediately; MANUAL on a paid tier
    // holds the subscription for approval; the FREE tier is always activated
    // immediately "regardless of approval mode".
    let cases = [
        Case {
            family: "CloudFront",
            tier: "PRO",
            usage_level: None,
            resources: 1,
            approval: Some("IMMEDIATE"),
            expected_status: "ACTIVE",
        },
        Case {
            family: "CloudFront",
            tier: "BUSINESS",
            usage_level: Some("DEFAULT"),
            resources: 3,
            approval: None,
            expected_status: "ACTIVE",
        },
        Case {
            family: "CloudFront",
            tier: "PREMIUM",
            usage_level: Some("LEVEL_2"),
            resources: 10,
            approval: Some("MANUAL"),
            expected_status: "PENDING_APPROVAL",
        },
        Case {
            family: "Route53",
            tier: "FREE",
            usage_level: None,
            resources: 2,
            approval: Some("MANUAL"),
            expected_status: "ACTIVE",
        },
        Case {
            family: "Route53",
            tier: "FREE",
            usage_level: None,
            resources: 1,
            approval: None,
            expected_status: "ACTIVE",
        },
    ];

    let router = router();
    for case in &cases {
        let label = format!(
            "{}/{}/{:?}/{} resources/{:?}",
            case.family, case.tier, case.usage_level, case.resources, case.approval
        );
        let mut body = create_body(case.family, case.tier, case.resources, case.approval);
        if let Some(level) = case.usage_level {
            body["usageLevel"] = json!(level);
        }
        let created = create(&router, body.clone()).await;
        let created_etag = created.require_etag(&label);

        let arn = created.string("arn");
        assert!(
            arn.starts_with(&format!(
                "arn:aws:pricingplanmanager:{REGION}:{ACCOUNT}:subscription/"
            )),
            "case {label}: minted ARN must name this service, region and account, got {arn:?}"
        );
        assert_eq!(
            created.string("status"),
            case.expected_status,
            "case {label}: unexpected status in {:?}",
            created.body
        );

        let fetched = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {label}: GetSubscription must find the subscription, got {} {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.body, created.body,
            "case {label}: GetSubscription must report exactly what CreateSubscription stored"
        );
        assert_eq!(
            fetched.require_etag(&label),
            created_etag,
            "case {label}: an unchanged subscription keeps its ETag"
        );
        assert_eq!(
            fetched.string("planFamily"),
            case.family,
            "case {label}: planFamily must round trip"
        );
        assert_eq!(
            fetched.string("planTier"),
            case.tier,
            "case {label}: planTier must round trip"
        );
        assert_eq!(
            fetched.opt_string("usageLevel").as_deref(),
            case.usage_level,
            "case {label}: usageLevel must round trip"
        );
        assert_eq!(
            fetched.body["resourceArns"],
            body["resourceArns"],
            "case {label}: resourceArns must round trip"
        );
        let created_at = epoch_seconds(&fetched.body["createdAt"], &label);
        let updated_at = epoch_seconds(&fetched.body["updatedAt"], &label);
        assert!(
            created_at > 1_600_000_000.0,
            "case {label}: createdAt must be a real instant, got {created_at}"
        );
        assert_eq!(
            created_at, updated_at,
            "case {label}: a freshly created subscription has not been modified since"
        );
        assert!(
            fetched.body.get("scheduledChange").is_none(),
            "case {label}: a new subscription has no pending change, got {:?}",
            fetched.body
        );
    }
}

/// A modelled `date-time` timestamp, read as seconds since the epoch.
///
/// Read format-agnostically on purpose. The model marks these members
/// `@timestampFormat("date-time")`, but this framework's JSON codec does not carry that
/// trait into the member schema, so a body timestamp goes out in restJson1's default
/// encoding (epoch seconds). Accepting either keeps every assertion below about the
/// VALUE the service stored, which is what this service owns.
fn epoch_seconds(value: &Value, case: &str) -> f64 {
    if let Some(number) = value.as_f64() {
        return number;
    }
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: {value:?} is not a timestamp"));
    parse_date_time(text)
        .unwrap_or_else(|| panic!("case {case}: {text:?} is not a date-time"))
        .secs() as f64
}

/// Assert `effective` is the end of the billing period that contains `after`: the first
/// instant of the next calendar month, UTC.
///
/// Checked by rendering the instant through the same date type the wire serializer uses
/// (day 1, midnight, within one month of `after`) rather than by recomputing the
/// production expression, so the assertion is independent of how the handler derived it.
fn assert_end_of_billing_period(effective: f64, after: f64, case: &str) {
    assert!(
        effective > after,
        "case {case}: a scheduled change takes effect in the future, got {effective} <= {after}"
    );
    assert!(
        effective - after <= 31.0 * 86_400.0,
        "case {case}: the end of the current billing period is at most a month away, got \
         {effective} vs {after}"
    );
    let rendered = format_date_time(aws_smithy_types::DateTime::from_secs(effective as i64))
        .unwrap_or_else(|| panic!("case {case}: {effective} is not a representable date-time"));
    assert!(
        rendered.ends_with("-01T00:00:00Z"),
        "case {case}: a calendar-month boundary is the 1st at midnight UTC, got {rendered}"
    );
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// Repeating a `clientToken` returns the subscription the first call minted and creates
/// nothing new, for any token value and any request shape.
#[tokio::test]
async fn a_repeated_client_token_returns_the_original_subscription() {
    let tokens = ["tok-1", "a", "0123456789", "Mixed-Case_Token.42"];

    let router = router();
    let mut minted = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let mut body = create_body("CloudFront", "PRO", index + 1, Some("IMMEDIATE"));
        body["clientToken"] = json!(token);

        let first = create(&router, body.clone()).await;
        let replay = create(&router, body.clone()).await;
        assert_eq!(
            first.body, replay.body,
            "case {token}: a replayed clientToken must return the original subscription"
        );
        assert_eq!(
            first.etag, replay.etag,
            "case {token}: a replayed clientToken must return the original ETag"
        );
        minted.push(first.string("arn"));
    }

    let arns = all_arns(&router).await;
    assert_eq!(
        arns,
        minted.iter().cloned().collect::<BTreeSet<_>>(),
        "one subscription per distinct clientToken must exist, got {arns:?}"
    );
}

// ---------------------------------------------------------------------------
// If-Match
// ---------------------------------------------------------------------------

/// Every mutating operation refuses a stale `If-Match` with `ConflictException` (409)
/// and leaves the subscription untouched, then accepts the current one.
#[tokio::test]
async fn if_match_is_enforced_by_every_mutating_operation() {
    struct Case {
        operation: &'static str,
        /// The approval mode the subject is created with, so the operation applies.
        approval: Option<&'static str>,
        /// Extra body members beyond `arn`.
        extra: fn() -> Value,
    }

    let cases = [
        Case {
            operation: "UpdateSubscription",
            approval: Some("IMMEDIATE"),
            extra: || json!({"planTier": "PREMIUM"}),
        },
        Case {
            operation: "CancelSubscription",
            approval: Some("IMMEDIATE"),
            extra: || json!({}),
        },
        Case {
            operation: "ApprovePaidSubscription",
            approval: Some("MANUAL"),
            extra: || json!({}),
        },
    ];

    // Each stale value is a token the service never issued for this record.
    let stale_tags = ["", "0", "deadbeefdeadbeefdeadbeefdeadbeef"];

    let router = router();
    for case in &cases {
        for stale in &stale_tags {
            let created = create(
                &router,
                create_body("CloudFront", "BUSINESS", 1, case.approval),
            )
            .await;
            let arn = created.string("arn");
            let current = created.require_etag(case.operation);
            assert_ne!(
                current, *stale,
                "case {}: the stale tag must differ from the real one",
                case.operation
            );

            let mut body = (case.extra)();
            body["arn"] = json!(arn.clone());

            let refused = call(&router, case.operation, body.clone(), Some(stale)).await;
            assert_eq!(
                refused.status, 409,
                "case {}/{stale:?}: a stale If-Match must be a conflict, got {} {:?}",
                case.operation, refused.status, refused.body
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("ConflictException"),
                "case {}/{stale:?}: unexpected error code for {:?}",
                case.operation,
                refused.body
            );

            let unchanged = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
            assert_eq!(
                unchanged.body, created.body,
                "case {}/{stale:?}: a refused mutation must not change the subscription",
                case.operation
            );
            assert_eq!(
                unchanged.require_etag(case.operation),
                current,
                "case {}/{stale:?}: a refused mutation must not mint a new ETag",
                case.operation
            );

            let accepted = call(&router, case.operation, body, Some(&current)).await;
            assert_eq!(
                accepted.status, 200,
                "case {}/{stale:?}: the current If-Match must be accepted, got {} {:?}",
                case.operation, accepted.status, accepted.body
            );
            assert_ne!(
                accepted.require_etag(case.operation),
                current,
                "case {}/{stale:?}: a mutation must mint a new ETag",
                case.operation
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Update: upgrade now, downgrade scheduled
// ---------------------------------------------------------------------------

/// `UpdateSubscription` replaces the tier immediately when the new tier ranks at or
/// above the current one, and records a pending `DOWNGRADE` (leaving the current tier in
/// place) when it ranks below. Asserted across the published tier ladder, not only the
/// one pair the issue cited.
#[tokio::test]
async fn update_applies_an_upgrade_now_and_schedules_a_downgrade() {
    struct Case {
        from: &'static str,
        to: &'static str,
        usage_level: Option<&'static str>,
        downgrade: bool,
    }

    let cases = [
        // Upgrades, including both published bounds of the tier ladder.
        Case { from: "FREE", to: "PRO", usage_level: None, downgrade: false },
        Case { from: "PRO", to: "BUSINESS", usage_level: Some("DEFAULT"), downgrade: false },
        Case { from: "BUSINESS", to: "PREMIUM", usage_level: None, downgrade: false },
        Case { from: "FREE", to: "PREMIUM", usage_level: Some("LEVEL_3"), downgrade: false },
        // Same tier: no rank change, so it is applied immediately.
        Case { from: "PRO", to: "PRO", usage_level: Some("LEVEL_2"), downgrade: false },
        // A tier outside the published ladder cannot be ranked, so it is applied now.
        Case { from: "PRO", to: "CUSTOM", usage_level: None, downgrade: false },
        // Downgrades, including the step down to the bottom of the ladder.
        Case { from: "PREMIUM", to: "BUSINESS", usage_level: None, downgrade: true },
        Case { from: "BUSINESS", to: "PRO", usage_level: Some("DEFAULT"), downgrade: true },
        Case { from: "PRO", to: "FREE", usage_level: None, downgrade: true },
        Case { from: "PREMIUM", to: "FREE", usage_level: Some("DEFAULT"), downgrade: true },
    ];

    let router = router();
    for case in &cases {
        let label = format!("{} -> {}", case.from, case.to);
        let mut body = create_body("CloudFront", case.from, 1, Some("IMMEDIATE"));
        body["usageLevel"] = json!("INITIAL");
        let created = create(&router, body).await;
        let arn = created.string("arn");
        let etag = created.require_etag(&label);

        let mut update = json!({"arn": arn.clone(), "planTier": case.to});
        if let Some(level) = case.usage_level {
            update["usageLevel"] = json!(level);
        }
        let updated = call(&router, "UpdateSubscription", update, Some(&etag)).await;
        assert_eq!(
            updated.status, 200,
            "case {label}: UpdateSubscription must succeed, got {} {:?}",
            updated.status, updated.body
        );

        // The read-back is what proves the change was persisted, not just reported.
        let fetched = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
        assert_eq!(
            fetched.body, updated.body,
            "case {label}: GetSubscription must agree with UpdateSubscription"
        );
        assert_eq!(
            fetched.require_etag(&label),
            updated.require_etag(&label),
            "case {label}: the new ETag must be the one a later read reports"
        );
        assert_ne!(
            updated.require_etag(&label),
            etag,
            "case {label}: a mutation must mint a new ETag"
        );

        if case.downgrade {
            assert_eq!(
                fetched.string("planTier"),
                case.from,
                "case {label}: a downgrade leaves the current tier in place until the \
                 billing cycle ends, got {:?}",
                fetched.body
            );
            let change = fetched
                .body
                .get("scheduledChange")
                .cloned()
                .unwrap_or_else(|| panic!("case {label}: no scheduledChange in {:?}", fetched.body));
            assert_eq!(
                change["changeType"], json!("DOWNGRADE"),
                "case {label}: unexpected changeType in {change:?}"
            );
            assert_eq!(
                change["planTier"], json!(case.to),
                "case {label}: the pending change must carry the target tier, got {change:?}"
            );
            assert_eq!(
                change.get("usageLevel").and_then(Value::as_str),
                case.usage_level,
                "case {label}: the pending change must carry the target usage level, got {change:?}"
            );
            let effective = change
                .get("effectiveDate")
                .unwrap_or_else(|| panic!("case {label}: no effectiveDate in {change:?}"));
            assert_end_of_billing_period(
                epoch_seconds(effective, &label),
                epoch_seconds(&fetched.body["updatedAt"], &label),
                &label,
            );
        } else {
            assert_eq!(
                fetched.string("planTier"),
                case.to,
                "case {label}: an upgrade takes effect immediately, got {:?}",
                fetched.body
            );
            assert_eq!(
                fetched.opt_string("usageLevel").as_deref(),
                case.usage_level,
                "case {label}: usageLevel is replaced, and reset to the default when omitted"
            );
            assert!(
                fetched.body.get("scheduledChange").is_none(),
                "case {label}: an immediate change schedules nothing, got {:?}",
                fetched.body
            );
        }
    }
}

/// A pending change blocks another update: "You cannot update a subscription while a
/// scheduled change is pending."
#[tokio::test]
async fn an_update_is_refused_while_a_change_is_pending() {
    let router = router();
    for pending in ["UpdateSubscription", "CancelSubscription"] {
        let created = create(
            &router,
            create_body("CloudFront", "PREMIUM", 1, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag = created.require_etag(pending);

        // Both of these leave a pending ScheduledChange behind: a downgrade, and a
        // cancellation of an active subscription.
        let body = if pending == "UpdateSubscription" {
            json!({"arn": arn.clone(), "planTier": "FREE"})
        } else {
            json!({"arn": arn.clone()})
        };
        let scheduled = call(&router, pending, body, Some(&etag)).await;
        assert_eq!(
            scheduled.status, 200,
            "case {pending}: must succeed, got {} {:?}",
            scheduled.status, scheduled.body
        );
        let etag = scheduled.require_etag(pending);

        let refused = call(
            &router,
            "UpdateSubscription",
            json!({"arn": arn.clone(), "planTier": "BUSINESS"}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {pending}: an update with a change pending must conflict, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {pending}: unexpected error code for {:?}",
            refused.body
        );
    }
}

// ---------------------------------------------------------------------------
// Cancel
// ---------------------------------------------------------------------------

/// Cancelling an active subscription schedules a `CANCELLATION` and keeps the record
/// readable; cancelling one still awaiting approval deletes it outright, so it is gone
/// from both `GetSubscription` and `ListSubscriptions`.
#[tokio::test]
async fn cancel_schedules_for_an_active_subscription_and_deletes_a_pending_one() {
    struct Case {
        approval: Option<&'static str>,
        tier: &'static str,
        deleted: bool,
    }

    let cases = [
        Case { approval: Some("IMMEDIATE"), tier: "PRO", deleted: false },
        Case { approval: None, tier: "BUSINESS", deleted: false },
        Case { approval: Some("MANUAL"), tier: "FREE", deleted: false },
        Case { approval: Some("MANUAL"), tier: "PRO", deleted: true },
        Case { approval: Some("MANUAL"), tier: "PREMIUM", deleted: true },
    ];

    let router = router();
    for case in &cases {
        let label = format!("{:?}/{}", case.approval, case.tier);
        let created = create(&router, create_body("CloudFront", case.tier, 1, case.approval)).await;
        let arn = created.string("arn");
        let etag = created.require_etag(&label);

        let cancelled = call(
            &router,
            "CancelSubscription",
            json!({"arn": arn.clone()}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            cancelled.status, 200,
            "case {label}: CancelSubscription must succeed, got {} {:?}",
            cancelled.status, cancelled.body
        );

        let fetched = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        let listed = all_arns(&router).await;
        if case.deleted {
            assert_eq!(
                fetched.status, 404,
                "case {label}: a deleted subscription must be gone, got {} {:?}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {label}: unexpected error code for {:?}",
                fetched.body
            );
            assert!(
                !listed.contains(&arn),
                "case {label}: a deleted subscription must not be listed, got {listed:?}"
            );
        } else {
            assert_eq!(
                fetched.status, 200,
                "case {label}: an active subscription stays readable, got {} {:?}",
                fetched.status, fetched.body
            );
            assert!(
                listed.contains(&arn),
                "case {label}: a scheduled cancellation stays listed, got {listed:?}"
            );
            let change = fetched
                .body
                .get("scheduledChange")
                .cloned()
                .unwrap_or_else(|| panic!("case {label}: no scheduledChange in {:?}", fetched.body));
            assert_eq!(
                change["changeType"], json!("CANCELLATION"),
                "case {label}: unexpected changeType in {change:?}"
            );
            assert!(
                change.get("planTier").is_none(),
                "case {label}: planTier is not present for cancellations, got {change:?}"
            );
            let effective = change
                .get("effectiveDate")
                .unwrap_or_else(|| panic!("case {label}: no effectiveDate in {change:?}"));
            assert_end_of_billing_period(
                epoch_seconds(effective, &label),
                epoch_seconds(&fetched.body["updatedAt"], &label),
                &label,
            );
            assert_eq!(
                fetched.string("status"),
                created.string("status"),
                "case {label}: the subscription remains in its current status until the \
                 cancellation takes effect"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Approve
// ---------------------------------------------------------------------------

/// `ApprovePaidSubscription` moves a `PENDING_APPROVAL` subscription to `ACTIVE`, and a
/// subscription in any other status conflicts — including a second approval of the one
/// it just activated.
#[tokio::test]
async fn approve_activates_a_pending_subscription_once() {
    let router = router();
    for tier in ["PRO", "BUSINESS", "PREMIUM"] {
        let created = create(&router, create_body("CloudFront", tier, 1, Some("MANUAL"))).await;
        let arn = created.string("arn");
        let etag = created.require_etag(tier);
        assert_eq!(
            created.string("status"),
            "PENDING_APPROVAL",
            "case {tier}: a paid tier under MANUAL waits for approval, got {:?}",
            created.body
        );

        let approved = call(
            &router,
            "ApprovePaidSubscription",
            json!({"arn": arn.clone()}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            approved.status, 200,
            "case {tier}: approval must succeed, got {} {:?}",
            approved.status, approved.body
        );
        assert_eq!(
            approved.string("status"),
            "ACTIVE",
            "case {tier}: approval activates the subscription, got {:?}",
            approved.body
        );
        let new_etag = approved.require_etag(tier);

        let fetched = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        assert_eq!(
            fetched.string("status"),
            "ACTIVE",
            "case {tier}: the activation must be persisted, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.require_etag(tier),
            new_etag,
            "case {tier}: the ETag a read reports is the one approval minted"
        );

        let again = call(
            &router,
            "ApprovePaidSubscription",
            json!({"arn": arn}),
            Some(&new_etag),
        )
        .await;
        assert_eq!(
            again.status, 409,
            "case {tier}: approving an already-active subscription must conflict, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code.as_deref(),
            Some("ConflictException"),
            "case {tier}: unexpected error code for {:?}",
            again.body
        );
    }
}

/// An ACTIVE subscription created under IMMEDIATE has nothing to approve, for every
/// tier the service publishes.
#[tokio::test]
async fn approving_an_active_subscription_conflicts() {
    let router = router();
    // The published tier ladder, read from the service's own catalogue rather than
    // repeated here.
    for tier in PLAN_TIERS_ASCENDING {
        let created = create(&router, create_body("CloudFront", tier, 1, Some("IMMEDIATE"))).await;
        let arn = created.string("arn");
        let etag = created.require_etag(tier);
        let answer = call(
            &router,
            "ApprovePaidSubscription",
            json!({"arn": arn}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            answer.status, 409,
            "case {tier}: an ACTIVE subscription cannot be approved, got {} {:?}",
            answer.status, answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// List + pagination
// ---------------------------------------------------------------------------

/// Every ARN in the store, collected by paging `ListSubscriptions` to exhaustion.
async fn all_arns(router: &MultiServiceRouter) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(value) => json!({"nextToken": value}),
            None => json!({}),
        };
        let answer = call(router, "ListSubscriptions", body, None).await;
        assert_eq!(
            answer.status, 200,
            "ListSubscriptions must succeed, got {} {:?}",
            answer.status, answer.body
        );
        let page = answer.body["subscriptionSummaries"]
            .as_array()
            .expect("subscriptionSummaries is a required list")
            .clone();
        for summary in &page {
            let arn = summary["arn"].as_str().expect("every summary has an arn");
            assert!(
                seen.insert(arn.to_string()),
                "paging returned {arn} twice"
            );
        }
        token = answer.opt_string("nextToken");
        if token.is_none() {
            break;
        }
    }
    seen
}

/// Paging is real: pages are bounded, resume where the previous one stopped, never
/// repeat or skip a record, and the token is omitted on the last page. Asserted at
/// several store sizes around the page boundary.
#[tokio::test]
async fn list_subscriptions_pages_the_whole_store_exactly_once() {
    for total in [0_usize, 1, LIST_PAGE_SIZE - 1, LIST_PAGE_SIZE, LIST_PAGE_SIZE + 1, LIST_PAGE_SIZE * 2 + 3] {
        let router = router();
        let mut expected = BTreeSet::new();
        for index in 0..total {
            let mut body = create_body("CloudFront", "PRO", 1, Some("IMMEDIATE"));
            body["clientToken"] = json!(format!("token-{index}"));
            expected.insert(create(&router, body).await.string("arn"));
        }

        // Walk the pages by hand so the page bound and the final token can be checked.
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let body = match &token {
                Some(value) => json!({"nextToken": value}),
                None => json!({}),
            };
            let answer = call(&router, "ListSubscriptions", body, None).await;
            assert_eq!(
                answer.status, 200,
                "case {total}: ListSubscriptions must succeed, got {} {:?}",
                answer.status, answer.body
            );
            let page = answer.body["subscriptionSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {total}: no subscriptionSummaries in {:?}", answer.body))
                .clone();
            assert!(
                page.len() <= LIST_PAGE_SIZE,
                "case {total}: a page must be bounded by {LIST_PAGE_SIZE}, got {}",
                page.len()
            );
            for summary in &page {
                let arn = summary["arn"].as_str().expect("every summary has an arn");
                seen.push(arn.to_string());
                // `eTag` is a BODY member of SubscriptionSummary, and it must be the
                // same token the single-resource read reports in its ETag header.
                let fetched = call(&router, "GetSubscription", json!({"arn": arn}), None).await;
                assert_eq!(
                    summary["eTag"].as_str().map(str::to_string),
                    fetched.etag,
                    "case {total}: the summary's eTag must equal the record's current ETag"
                );
                assert_eq!(
                    summary["planFamily"], fetched.body["planFamily"],
                    "case {total}: a summary must report the stored record"
                );
                assert_eq!(
                    summary["status"], fetched.body["status"],
                    "case {total}: a summary must report the stored record"
                );
            }
            pages += 1;
            token = answer.opt_string("nextToken");
            if token.is_none() {
                assert!(
                    page.len() <= LIST_PAGE_SIZE,
                    "case {total}: unexpected page size on the last page"
                );
                break;
            }
            assert_eq!(
                page.len(),
                LIST_PAGE_SIZE,
                "case {total}: a token is only sent when the page was full"
            );
            assert!(
                pages <= total + 2,
                "case {total}: paging did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen.len(),
            expected.len(),
            "case {total}: paging must return every subscription exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "case {total}: paging must return exactly the stored subscriptions"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {total}: pages must be returned in the store's stable order"
        );
    }
}

/// `ListSubscriptions` is a read: it never changes what a later caller sees.
#[tokio::test]
async fn listing_does_not_change_the_store() {
    let router = router();
    for index in 0..3 {
        let mut body = create_body("CloudFront", "PRO", 1, Some("IMMEDIATE"));
        body["clientToken"] = json!(format!("token-{index}"));
        create(&router, body).await;
    }
    let first = call(&router, "ListSubscriptions", json!({}), None).await;
    let second = call(&router, "ListSubscriptions", json!({}), None).await;
    assert_eq!(
        first.body, second.body,
        "two identical ListSubscriptions calls must answer identically"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The refusal table: a request outside a modelled constraint is a 400
/// `ValidationException`, an ARN the store does not hold is a 404
/// `ResourceNotFoundException`.
#[tokio::test]
async fn bad_requests_are_refused_with_the_modelled_error() {
    struct Case {
        label: &'static str,
        operation: &'static str,
        body: Value,
        if_match: Option<&'static str>,
        status: u16,
        code: &'static str,
    }

    let long_arn = format!("arn:aws:pricingplanmanager:{REGION}:{ACCOUNT}:subscription/{}", "x".repeat(2048));
    let max_arn = format!(
        "arn:{}",
        "y".repeat(2048 - "arn:".len())
    );
    let eleven: Vec<String> = (0..11)
        .map(|index| format!("arn:aws:cloudfront::{ACCOUNT}:distribution/E{index}"))
        .collect();

    let cases = vec![
        Case {
            label: "resourceArns below the modelled minimum of 1",
            operation: "CreateSubscription",
            body: json!({"planFamily": "CloudFront", "planTier": "PRO", "resourceArns": []}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "resourceArns above the modelled maximum of 10",
            operation: "CreateSubscription",
            body: json!({"planFamily": "CloudFront", "planTier": "PRO", "resourceArns": eleven}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "planFamily absent",
            operation: "CreateSubscription",
            body: json!({"planTier": "PRO", "resourceArns": ["arn:aws:cloudfront::1:distribution/E"]}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "planTier absent",
            operation: "CreateSubscription",
            body: json!({"planFamily": "CloudFront", "resourceArns": ["arn:aws:cloudfront::1:distribution/E"]}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "arn below the modelled minimum length of 1",
            operation: "GetSubscription",
            body: json!({"arn": ""}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "arn above the modelled maximum length of 2048",
            operation: "GetSubscription",
            body: json!({"arn": long_arn}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            label: "an arn exactly at the modelled maximum length of 2048 is well-formed",
            operation: "GetSubscription",
            body: json!({"arn": max_arn}),
            if_match: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Case {
            label: "an arn the store does not hold",
            operation: "GetSubscription",
            body: json!({"arn": format!("arn:aws:pricingplanmanager:{REGION}:{ACCOUNT}:subscription/does-not-exist")}),
            if_match: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Case {
            label: "an arn the store does not hold, on a mutating operation",
            operation: "CancelSubscription",
            body: json!({"arn": format!("arn:aws:pricingplanmanager:{REGION}:{ACCOUNT}:subscription/does-not-exist")}),
            if_match: Some("deadbeefdeadbeefdeadbeefdeadbeef"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Case {
            label: "a nextToken this service never issued",
            operation: "ListSubscriptions",
            body: json!({"nextToken": "not-a-token-this-service-issued"}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            // Hex of "zzz": decodes cleanly, but is not an ARN this service mints.
            label: "a nextToken that decodes but carries no subscription ARN",
            operation: "ListSubscriptions",
            body: json!({"nextToken": "7a7a7a"}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
        Case {
            // Hex of "arn:aws:s3:::bucket/key": a well-formed ARN, but not one of ours.
            label: "a nextToken carrying another service's ARN",
            operation: "ListSubscriptions",
            body: json!({"nextToken": "61726e3a6177733a73333a3a3a6275636b65742f6b6579"}),
            if_match: None,
            status: 400,
            code: "ValidationException",
        },
    ];

    let router = router();
    for case in &cases {
        let answer = call(&router, case.operation, case.body.clone(), case.if_match).await;
        assert_eq!(
            answer.status, case.status,
            "case {}: expected HTTP {}, got {} {:?}",
            case.label, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some(case.code),
            "case {}: expected {}, got {:?} / {:?}",
            case.label,
            case.code,
            answer.error_code,
            answer.body
        );
    }
}

/// A scheduled change that has become due is materialised before the next answer: the
/// change happens at its effective instant whether or not anybody calls.
///
/// The effective instant is a month out, so the test backdates the one the handler
/// stored — the store the handlers read is the engine's, which this test holds — and
/// then asks over HTTP.
#[tokio::test]
async fn a_change_whose_effective_date_has_passed_is_applied_before_the_next_answer() {
    struct Case {
        /// How the pending change is created.
        operation: &'static str,
        body: fn(&str) -> Value,
        /// The tier the subscription must report once the change has taken effect, or
        /// `None` when the subscription is gone.
        settled_tier: Option<&'static str>,
    }

    let cases = [
        Case {
            operation: "UpdateSubscription",
            body: |arn| json!({"arn": arn, "planTier": "PRO"}),
            settled_tier: Some("PRO"),
        },
        Case {
            operation: "UpdateSubscription",
            body: |arn| json!({"arn": arn, "planTier": "FREE"}),
            settled_tier: Some("FREE"),
        },
        Case {
            operation: "CancelSubscription",
            body: |arn| json!({"arn": arn}),
            settled_tier: None,
        },
    ];

    for case in &cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(Arc::clone(&engine));

        let created = create(
            &router,
            create_body("CloudFront", "PREMIUM", 1, Some("IMMEDIATE")),
        )
        .await;
        let arn = created.string("arn");
        let etag = created.require_etag(case.operation);

        let scheduled = call(&router, case.operation, (case.body)(&arn), Some(&etag)).await;
        assert_eq!(
            scheduled.status, 200,
            "case {}: must succeed, got {} {:?}",
            case.operation, scheduled.status, scheduled.body
        );
        assert!(
            scheduled.body.get("scheduledChange").is_some(),
            "case {}: expected a pending change in {:?}",
            case.operation,
            scheduled.body
        );

        // Move the stored effective instant into the past: the clock now owes the change.
        {
            let state = engine.state::<ServiceState>();
            let mut store = state.lock();
            let record = store
                .subscriptions
                .get_mut(&arn)
                .expect("the subscription is stored");
            let change = record
                .scheduled_change
                .as_mut()
                .expect("the pending change is stored");
            change.effective_date = Some(aws_smithy_types::DateTime::from_secs(1_600_000_000));
        }

        let fetched = call(&router, "GetSubscription", json!({"arn": arn.clone()}), None).await;
        match case.settled_tier {
            Some(tier) => {
                assert_eq!(
                    fetched.status, 200,
                    "case {}: the subscription survives a due downgrade, got {} {:?}",
                    case.operation, fetched.status, fetched.body
                );
                assert_eq!(
                    fetched.string("planTier"),
                    tier,
                    "case {}: a due downgrade moves the tier, got {:?}",
                    case.operation,
                    fetched.body
                );
                assert!(
                    fetched.body.get("scheduledChange").is_none(),
                    "case {}: a change that has taken effect is no longer pending, got {:?}",
                    case.operation,
                    fetched.body
                );
            }
            None => {
                assert_eq!(
                    fetched.status, 404,
                    "case {}: a due cancellation removes the subscription, got {} {:?}",
                    case.operation, fetched.status, fetched.body
                );
                assert!(
                    !all_arns(&router).await.contains(&arn),
                    "case {}: a cancelled subscription must not be listed",
                    case.operation
                );
            }
        }
    }
}

/// The account holds a bounded number of subscriptions. At the bound,
/// `CreateSubscription` answers the modelled `ServiceQuotaExceededException` (402)
/// rather than letting the store grow without limit — while an idempotent replay of a
/// token already stored still succeeds, because it creates nothing.
#[tokio::test]
async fn creating_past_the_subscription_quota_is_refused() {
    let router = router();
    for index in 0..MAX_SUBSCRIPTIONS {
        let mut body = create_body("CloudFront", "PRO", 1, Some("IMMEDIATE"));
        body["clientToken"] = json!(format!("token-{index}"));
        let answer = call(&router, "CreateSubscription", body, None).await;
        assert_eq!(
            answer.status, 200,
            "case {index}: a create below the quota of {MAX_SUBSCRIPTIONS} must succeed, \
             got {} {:?}",
            answer.status, answer.body
        );
    }

    // One past the bound.
    let refused = call(
        &router,
        "CreateSubscription",
        create_body("CloudFront", "PRO", 1, Some("IMMEDIATE")),
        None,
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "case over-quota: expected HTTP 402, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case over-quota: unexpected error code for {:?}",
        refused.body
    );

    // A replay at the bound is not a new subscription, so it is still served.
    let mut replay = create_body("CloudFront", "PRO", 1, Some("IMMEDIATE"));
    replay["clientToken"] = json!("token-0");
    let replayed = call(&router, "CreateSubscription", replay, None).await;
    assert_eq!(
        replayed.status, 200,
        "case replay-at-quota: an idempotent replay creates nothing and must be served, \
         got {} {:?}",
        replayed.status, replayed.body
    );
}

/// Deleting a subscription drops its `clientToken` with it, so the token is free again
/// and a later call with it mints a NEW subscription rather than resurrecting the old
/// ARN. This is what keeps the idempotency map from outliving the records it indexes.
#[tokio::test]
async fn a_deleted_subscription_releases_its_client_token() {
    let router = router();
    for token in ["tok-a", "tok-b"] {
        let mut body = create_body("CloudFront", "PRO", 1, Some("MANUAL"));
        body["clientToken"] = json!(token);

        let created = create(&router, body.clone()).await;
        let first_arn = created.string("arn");
        let etag = created.require_etag(token);

        // A PENDING_APPROVAL subscription is deleted outright by CancelSubscription.
        let cancelled = call(
            &router,
            "CancelSubscription",
            json!({"arn": first_arn.clone()}),
            Some(&etag),
        )
        .await;
        assert_eq!(
            cancelled.status, 200,
            "case {token}: CancelSubscription must succeed, got {} {:?}",
            cancelled.status, cancelled.body
        );

        let again = create(&router, body).await;
        let second_arn = again.string("arn");
        assert_ne!(
            second_arn, first_arn,
            "case {token}: the token of a deleted subscription must not resurrect it"
        );
        let listed = all_arns(&router).await;
        assert!(
            !listed.contains(&first_arn) && listed.contains(&second_arn),
            "case {token}: only the new subscription must be stored, got {listed:?}"
        );
    }
}

/// A page token keeps working when the subscription it resumes after is deleted between
/// pages: the token is a cursor into the ordered store, not a handle on a record.
#[tokio::test]
async fn a_page_token_survives_the_deletion_of_the_record_it_resumes_after() {
    let router = router();
    // One full page plus one, created PENDING_APPROVAL so they can be deleted outright.
    let total = LIST_PAGE_SIZE + 1;
    for index in 0..total {
        let mut body = create_body("CloudFront", "PRO", 1, Some("MANUAL"));
        body["clientToken"] = json!(format!("token-{index}"));
        create(&router, body).await;
    }

    let first = call(&router, "ListSubscriptions", json!({}), None).await;
    let page = first.body["subscriptionSummaries"]
        .as_array()
        .expect("subscriptionSummaries is a required list")
        .clone();
    assert_eq!(
        page.len(),
        LIST_PAGE_SIZE,
        "case first-page: expected a full page, got {}",
        page.len()
    );
    let token = first
        .opt_string("nextToken")
        .expect("a further page remains, so a token must be sent");

    // The token resumes after the LAST summary of this page; delete exactly that record.
    let cursor = page
        .last()
        .expect("a full page has a last summary")
        .clone();
    let cursor_arn = cursor["arn"].as_str().expect("every summary has an arn");
    let cursor_etag = cursor["eTag"].as_str().expect("every summary has an eTag");
    let deleted = call(
        &router,
        "CancelSubscription",
        json!({"arn": cursor_arn}),
        Some(cursor_etag),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case delete-cursor: CancelSubscription must succeed, got {} {:?}",
        deleted.status, deleted.body
    );

    let second = call(&router, "ListSubscriptions", json!({"nextToken": token}), None).await;
    assert_eq!(
        second.status, 200,
        "case resume-after-delete: the token must still be accepted, got {} {:?}",
        second.status, second.body
    );
    let rest = second.body["subscriptionSummaries"]
        .as_array()
        .expect("subscriptionSummaries is a required list")
        .clone();
    assert_eq!(
        rest.len(),
        total - LIST_PAGE_SIZE,
        "case resume-after-delete: the remaining records must still be returned, got {rest:?}"
    );
    assert!(
        rest.iter()
            .all(|summary| summary["arn"].as_str() != Some(cursor_arn)),
        "case resume-after-delete: the deleted record must not reappear, got {rest:?}"
    );
}

/// The operations this slice does not implement still answer 501, so a client can tell
/// "not written yet" from an empty success.
#[tokio::test]
async fn the_operations_outside_this_slice_still_answer_501() {
    let router = router();
    let created = create(&router, create_body("CloudFront", "PRO", 1, Some("IMMEDIATE"))).await;
    let arn = created.string("arn");
    let etag = created.require_etag("stubs");

    let cases = [
        (
            "AssociateResourcesToSubscription",
            json!({"arn": arn.clone(), "resourceArns": [format!("arn:aws:cloudfront::{ACCOUNT}:distribution/EX")]}),
        ),
        (
            "DisassociateResourcesFromSubscription",
            json!({"arn": arn.clone(), "resourceArns": [format!("arn:aws:cloudfront::{ACCOUNT}:distribution/EX")]}),
        ),
        ("CancelSubscriptionChange", json!({"arn": arn.clone()})),
    ];

    for (operation, body) in &cases {
        let answer = call(&router, operation, body.clone(), Some(&etag)).await;
        assert_eq!(
            answer.status, 501,
            "case {operation}: an unimplemented operation answers 501, got {} {:?}",
            answer.status, answer.body
        );
    }
}
