// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDRTAccess`, the account-level read of the Shield
//! Response Team's access to the account, together with the writes that settle what it
//! reports (`AssociateDRTRole`, `DisassociateDRTRole`).
//!
//! Every case goes over the wire — an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AWSShield_20160616.<Operation>` through the same
//! `MultiServiceRouter` the production router serves — so routing, deserialization,
//! modelled-constraint validation, the handler and serialization are all exercised, and
//! what a write stored is read back through a SEPARATE request.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * the access this read reports is granted on the account's subscription, so while the
//!   account holds no active subscription the read refuses with the account-level
//!   not-found wording — whatever else the store holds;
//! * with an active subscription the read reports the store: the role the account last
//!   associated (`AssociateDRTRole`: "You can associate only one RoleArn with your
//!   subscription … the new RoleArn will replace the existing RoleArn"), absent again
//!   once `DisassociateDRTRole` withdraws it, and the authorized Amazon S3 log buckets in
//!   the order they were authorized;
//! * the read is a read: repeating it answers the same, and leaves the role there to be
//!   withdrawn afterwards.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_shield_awsshield_20160616::state::{service_state, MAX_DRT_LOG_BUCKETS};
use service_com_amazonaws_shield_awsshield_20160616::EmulatedService;

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSShield_20160616.";

/// How AWS words the account-level absence, shared by every operation whose only
/// addressable resource is the account's subscription.
const SUBSCRIPTION_NOT_FOUND: &str = "The subscription does not exist.";

/// Every `RoleArn` in this suite's table, each admitted by the pattern the model puts on
/// `AssociateDRTRoleRequest$RoleArn` and `DescribeDRTAccessResponse$RoleArn`
/// (`^arn:aws:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`): a plain role, a role with an
/// IAM path, a name using the punctuation the pattern admits, and the no-slash form the
/// pattern's optional `/` allows.
const ROLE_ARNS: &[&str] = &[
    "arn:aws:iam::000000000000:role/AWSShieldDRTAccessRole",
    "arn:aws:iam::123456789012:role/service-role/SRTAccess",
    "arn:aws:iam::123456789012:role/Shield+DRT=Access,srt.team@example-_1",
    "arn:aws:iam::210987654321:roleShieldResponseTeam",
];

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

    /// Authorize the Shield Response Team through `role_arn`, asserting it succeeded.
    async fn associate_role(&self, case: &str, role_arn: &str) {
        self.call("AssociateDRTRole", &json!({ "RoleArn": role_arn }))
            .await
            .expect_ok(case);
    }

    /// `DescribeDRTAccess`, whatever it answers.
    async fn describe_drt_access(&self) -> Reply {
        self.call("DescribeDRTAccess", &json!({})).await
    }

    /// Move the stored subscription's end to `end_time`, the way the passage of time
    /// would. Reaches the store directly because no modelled operation sets the end of a
    /// subscription.
    fn set_subscription_end(&self, end_time: SystemTime) {
        let state = service_state(&*self.engine).expect("the shield store");
        let mut guard = state.lock();
        let subscription = guard
            .subscription
            .as_mut()
            .expect("an activated subscription");
        subscription.end_time = end_time;
    }

    /// Authorize `buckets` for the Shield Response Team by writing the store directly:
    /// the modelled writes of that list (`AssociateDRTLogBucket`,
    /// `DisassociateDRTLogBucket`) are still scaffolds, so this suite seeds what they
    /// will store and pins that the read derives its answer from it.
    fn set_log_buckets(&self, buckets: &[String]) {
        let state = service_state(&*self.engine).expect("the shield store");
        state.lock().drt_log_buckets = buckets.to_vec();
    }
}

/// The `RoleArn` a `DescribeDRTAccess` answer carries, or `None` when it carries none.
fn reported_role(body: &Value) -> Option<String> {
    body.get("RoleArn")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `LogBucketList` a `DescribeDRTAccess` answer carries, which must be a list of
/// strings.
fn reported_log_buckets(case: &str, body: &Value) -> Vec<String> {
    body.get("LogBucketList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: no LogBucketList in {body}"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: LogBucketList holds a non-string: {body}"))
                .to_string()
        })
        .collect()
}

/// `count` distinct Amazon S3 bucket names, as a caller would name log buckets.
fn log_bucket_names(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("shield-srt-access-logs-{index}"))
        .collect()
}

#[tokio::test]
async fn drt_access_is_absent_while_the_account_holds_no_active_subscription() {
    // Each row is an account state in which the subscription the access hangs off is not
    // live: never taken on, just lapsed, lapsed long ago — and lapsed while a role was
    // associated, so the gate is the subscription and not the stored role.
    let rows: &[(&str, bool, Option<Duration>, bool)] = &[
        ("no subscription was ever created", false, None, false),
        (
            "subscription lapsed a second ago",
            true,
            Some(Duration::from_secs(1)),
            false,
        ),
        (
            "subscription lapsed a year ago",
            true,
            Some(Duration::from_secs(365 * 24 * 60 * 60)),
            false,
        ),
        (
            "subscription lapsed while a role stayed associated",
            true,
            Some(Duration::from_secs(60)),
            true,
        ),
    ];

    for (case, subscribe, lapsed_by, with_role) in rows {
        let emulator = Emulator::new();
        if *subscribe {
            emulator.subscribe().await;
        }
        if *with_role {
            emulator.associate_role(case, ROLE_ARNS[0]).await;
        }
        if let Some(lapsed_by) = lapsed_by {
            emulator.set_subscription_end(SystemTime::now() - *lapsed_by);
        }

        let reply = emulator.describe_drt_access().await;
        reply.expect_error(case, 400, "ResourceNotFoundException", SUBSCRIPTION_NOT_FOUND);
        assert!(
            reply.body.get("RoleArn").is_none() && reply.body.get("LogBucketList").is_none(),
            "case {case}: a refusal must report no access configuration, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn drt_access_reports_the_role_the_account_last_associated() {
    let emulator = Emulator::new();
    emulator.subscribe().await;

    // A subscribed account that has authorized nobody: no role, and no log buckets.
    let reply = emulator.describe_drt_access().await;
    let body = reply.expect_ok("no association yet").clone();
    assert!(
        reported_role(&body).is_none(),
        "case no association yet: expected no RoleArn, got {body}"
    );
    assert!(
        reported_log_buckets("no association yet", &body).is_empty(),
        "case no association yet: expected an empty LogBucketList, got {body}"
    );

    // Each association replaces the previous one, so after every row the read reports
    // exactly that row's role.
    for role_arn in ROLE_ARNS {
        emulator.associate_role(role_arn, role_arn).await;

        let reply = emulator.describe_drt_access().await;
        let body = reply.expect_ok(role_arn).clone();
        assert_eq!(
            reported_role(&body).as_deref(),
            Some(*role_arn),
            "case {role_arn}: expected the role just associated, got {body}"
        );
    }

    // Withdrawing the authorization makes the role absent again, not empty.
    emulator
        .call("DisassociateDRTRole", &json!({}))
        .await
        .expect_ok("after disassociate");
    let reply = emulator.describe_drt_access().await;
    let body = reply.expect_ok("after disassociate").clone();
    assert!(
        reported_role(&body).is_none(),
        "case after disassociate: expected no RoleArn, got {body}"
    );
}

#[tokio::test]
async fn drt_access_reports_the_authorized_log_buckets_in_order() {
    // The counts span the modelled `LogBucketList` length bound (0..10): none, one, a
    // few, and the whole allowance.
    let counts = [0usize, 1, 3, MAX_DRT_LOG_BUCKETS];

    for count in counts {
        let case = format!("{count} authorized log bucket(s)");
        let emulator = Emulator::new();
        emulator.subscribe().await;
        let buckets = log_bucket_names(count);
        emulator.set_log_buckets(&buckets);

        let reply = emulator.describe_drt_access().await;
        let body = reply.expect_ok(&case).clone();
        assert_eq!(
            reported_log_buckets(&case, &body),
            buckets,
            "case {case}: expected the authorized buckets in order, got {body}"
        );
        assert!(
            reported_role(&body).is_none(),
            "case {case}: no role was associated, so none may be reported: {body}"
        );
    }

    // Order is the authorization order, not a sorted one: a list stored out of
    // lexicographic order is reported as stored.
    let emulator = Emulator::new();
    emulator.subscribe().await;
    let unsorted = vec![
        "srt-logs-zulu".to_string(),
        "srt-logs-alpha".to_string(),
        "srt-logs-mike".to_string(),
    ];
    emulator.set_log_buckets(&unsorted);
    let reply = emulator.describe_drt_access().await;
    let body = reply.expect_ok("unsorted buckets").clone();
    assert_eq!(
        reported_log_buckets("unsorted buckets", &body),
        unsorted,
        "case unsorted buckets: expected the stored order, got {body}"
    );
}

#[tokio::test]
async fn describing_drt_access_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    emulator.subscribe().await;
    emulator.associate_role("repeat read", ROLE_ARNS[0]).await;
    let buckets = log_bucket_names(2);
    emulator.set_log_buckets(&buckets);

    let first = emulator.describe_drt_access().await;
    let first_body = first.expect_ok("repeat read").clone();
    let second = emulator.describe_drt_access().await;
    let second_body = second.expect_ok("repeat read").clone();
    assert_eq!(
        first_body, second_body,
        "case repeat read: a read must answer the same twice, got {first_body} then {second_body}"
    );

    // The role is still there to withdraw, which is what proves the reads consumed
    // nothing: `DisassociateDRTRole` refuses with `InvalidOperationException` when no
    // role is associated.
    emulator
        .call("DisassociateDRTRole", &json!({}))
        .await
        .expect_ok("withdraw after reading");
    let after = emulator.describe_drt_access().await;
    let after_body = after.expect_ok("withdraw after reading").clone();
    assert!(
        reported_role(&after_body).is_none(),
        "case withdraw after reading: expected no RoleArn, got {after_body}"
    );
    assert_eq!(
        reported_log_buckets("withdraw after reading", &after_body),
        buckets,
        "case withdraw after reading: withdrawing the role must leave the buckets, got {after_body}"
    );
}
