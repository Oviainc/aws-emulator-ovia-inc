// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the whole subscription-scoped surface answers while the
//! account holds no ACTIVE Shield Advanced subscription.
//!
//! Every case goes over the wire — an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AWSShield_20160616.<Operation>` through the same
//! `MultiServiceRouter` the production router serves — so routing, deserialization,
//! modelled-constraint validation, the handler and serialization are all exercised.
//!
//! The rule under test: an operation whose addressable resource is the account's
//! subscription — or anything the subscription grants (the Shield Response Team's
//! access, the emergency contacts, the protections and protection groups) — refuses,
//! while the account holds no active subscription, with the `ResourceNotFoundException`
//! the model declares for it (HTTP 400) and the ONE sentence this service words that
//! account-level absence with, `The subscription does not exist.`; the whole family
//! agrees on that status, code and message. `GetSubscriptionState` is the single
//! exception, because its own contract is to REPORT the account's state ("Returns the
//! SubscriptionState, either Active or Inactive"), so it answers `INACTIVE` rather than
//! refusing.
//!
//! The table spans the surface in both directions: one row per operation (reads and
//! writes, empty-input and populated-input) crossed with every way an account can be
//! without a live subscription (never subscribed, just lapsed, long lapsed), and the
//! same operations again on a subscribed account, where none of them may report the
//! account-level absence.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_shield_awsshield_20160616::state::service_state;
use service_com_amazonaws_shield_awsshield_20160616::EmulatedService;

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSShield_20160616.";

/// How AWS words the absence of the account's Shield Advanced subscription, spelled out
/// here rather than read from the service's own constant so the wording itself is pinned.
const SUBSCRIPTION_NOT_FOUND_MESSAGE: &str = "The subscription does not exist.";

/// The `SubscriptionState` enum value for an account that holds no live subscription
/// (the model's `SubscriptionState` values are `ACTIVE` and `INACTIVE`).
const SUBSCRIPTION_STATE_INACTIVE: &str = "INACTIVE";

/// A `RoleArn` admitted by the pattern the model puts on `AssociateDRTRoleRequest$RoleArn`
/// (`^arn:aws:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
const DRT_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSShieldDRTAccessRole";

/// A protectable resource ARN, in one of the layouts `Protection$ResourceArn` documents.
const PROTECTED_RESOURCE_ARN: &str =
    "arn:aws:cloudfront::000000000000:distribution/E1XMPLDISTRIBUTION";

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
        let reply = self.call("CreateSubscription", &json!({})).await;
        assert_eq!(
            reply.status, 200,
            "case CreateSubscription: expected HTTP 200, got {reply:?}"
        );
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
}

/// One subscription-scoped operation and a request for it that is well formed under the
/// model, so nothing but the missing subscription can decide the answer.
struct Call {
    operation: &'static str,
    request: fn() -> Value,
    /// What this request is answered with on a freshly subscribed account: the HTTP
    /// status, and the modelled error code, or `""` where it succeeds.
    subscribed: (u16, &'static str),
}

/// Every operation of this service that addresses the account's subscription, or
/// something the subscription grants, and that is implemented today: the DRT/SRT access
/// read and its two writes, the subscription read and write, the emergency-contact read
/// and write, and the protection and protection-group reads and writes.
const SUBSCRIPTION_SCOPED: &[Call] = &[
    Call {
        operation: "DescribeDRTAccess",
        request: || json!({}),
        subscribed: (200, ""),
    },
    Call {
        operation: "AssociateDRTRole",
        request: || json!({ "RoleArn": DRT_ROLE_ARN }),
        subscribed: (200, ""),
    },
    Call {
        // On a subscribed account that has associated no role there is nothing to
        // withdraw, which is the role-level refusal rather than the account-level one.
        operation: "DisassociateDRTRole",
        request: || json!({}),
        subscribed: (400, "InvalidOperationException"),
    },
    Call {
        operation: "DescribeSubscription",
        request: || json!({}),
        subscribed: (200, ""),
    },
    Call {
        operation: "UpdateSubscription",
        request: || json!({}),
        subscribed: (200, ""),
    },
    Call {
        operation: "DescribeEmergencyContactSettings",
        request: || json!({}),
        subscribed: (200, ""),
    },
    Call {
        operation: "UpdateEmergencyContactSettings",
        request: || {
            json!({ "EmergencyContactList": [{ "EmailAddress": "oncall@example.com" }] })
        },
        subscribed: (200, ""),
    },
    Call {
        operation: "ListProtections",
        request: || json!({}),
        subscribed: (200, ""),
    },
    Call {
        operation: "CreateProtection",
        request: || json!({ "Name": "gate", "ResourceArn": PROTECTED_RESOURCE_ARN }),
        subscribed: (200, ""),
    },
    Call {
        operation: "ListProtectionGroups",
        request: || json!({}),
        subscribed: (200, ""),
    },
    Call {
        operation: "CreateProtectionGroup",
        request: || {
            json!({
                "ProtectionGroupId": "subscription-gate",
                "Aggregation": "SUM",
                "Pattern": "ALL",
            })
        },
        subscribed: (200, ""),
    },
];

/// Each way an account can hold no active subscription: it never took one on, its
/// subscription has just run past its end, or it lapsed long ago. `None` is the account
/// that never subscribed; `Some(d)` is a subscription whose end is `d` in the past.
const UNSUBSCRIBED_STATES: &[(&str, Option<Duration>)] = &[
    ("never subscribed", None),
    ("lapsed a second ago", Some(Duration::from_secs(1))),
    ("lapsed an hour ago", Some(Duration::from_secs(60 * 60))),
    (
        "lapsed a year ago",
        Some(Duration::from_secs(365 * 24 * 60 * 60)),
    ),
];

/// Build an emulator in one of the unsubscribed account states.
async fn unsubscribed(lapsed_by: Option<Duration>) -> Emulator {
    let emulator = Emulator::new();
    if let Some(lapsed_by) = lapsed_by {
        emulator.subscribe().await;
        emulator.set_subscription_end(SystemTime::now() - lapsed_by);
    }
    emulator
}

#[tokio::test]
async fn every_subscription_scoped_operation_reports_the_same_account_level_absence() {
    for (state_case, lapsed_by) in UNSUBSCRIBED_STATES {
        for call in SUBSCRIPTION_SCOPED {
            let case = format!("{} on an account {state_case}", call.operation);
            let emulator = unsubscribed(*lapsed_by).await;

            let reply = emulator.call(call.operation, &(call.request)()).await;
            assert_eq!(
                (
                    reply.status,
                    reply.error_code(),
                    reply.error_message()
                ),
                (400, "ResourceNotFoundException", SUBSCRIPTION_NOT_FOUND_MESSAGE),
                "case {case}: expected HTTP 400 ResourceNotFoundException \
                 {SUBSCRIPTION_NOT_FOUND_MESSAGE:?}, got {reply:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_absence_is_reported_rather_than_refused_by_the_subscription_state_read() {
    for (state_case, lapsed_by) in UNSUBSCRIBED_STATES {
        let emulator = unsubscribed(*lapsed_by).await;

        let reply = emulator.call("GetSubscriptionState", &json!({})).await;
        assert_eq!(
            (reply.status, reply.body.get("SubscriptionState")),
            (200, Some(&json!(SUBSCRIPTION_STATE_INACTIVE))),
            "case GetSubscriptionState on an account {state_case}: expected HTTP 200 \
             {SUBSCRIPTION_STATE_INACTIVE}, got {reply:?}"
        );
    }
}

#[tokio::test]
async fn a_refusal_carries_no_part_of_the_answer_it_withheld() {
    // The absence is an absence: the refusal body is the error envelope and nothing
    // else, so no output member of the operation rides along with it.
    let withheld: &[(&str, &[&str])] = &[
        ("DescribeDRTAccess", &["RoleArn", "LogBucketList"]),
        ("DescribeSubscription", &["Subscription"]),
        (
            "DescribeEmergencyContactSettings",
            &["EmergencyContactList"],
        ),
        ("ListProtections", &["Protections", "NextToken"]),
        ("ListProtectionGroups", &["ProtectionGroups", "NextToken"]),
    ];

    for (operation, members) in withheld {
        let emulator = unsubscribed(None).await;
        let reply = emulator.call(operation, &json!({})).await;
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {operation}: expected the account-level absence, got {reply:?}"
        );
        for member in *members {
            assert!(
                reply.body.get(member).is_none(),
                "case {operation}: a refusal must carry no {member}, got {:?}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn no_subscription_scoped_operation_reports_the_absence_once_the_account_subscribes() {
    // The same table again, each row on its own freshly subscribed account: whatever
    // each operation then answers, none of them may still report the account-level
    // absence — which is what proves the refusal above is read off the stored
    // subscription rather than wired into the handler.
    for call in SUBSCRIPTION_SCOPED {
        let case = format!("{} on a subscribed account", call.operation);
        let emulator = Emulator::new();
        emulator.subscribe().await;

        let reply = emulator.call(call.operation, &(call.request)()).await;
        assert_ne!(
            reply.error_message(),
            SUBSCRIPTION_NOT_FOUND_MESSAGE,
            "case {case}: a subscribed account holds a subscription, got {reply:?}"
        );
        assert_eq!(
            (reply.status, reply.error_code()),
            call.subscribed,
            "case {case}: expected {:?}, got {reply:?}",
            call.subscribed
        );
    }
}
