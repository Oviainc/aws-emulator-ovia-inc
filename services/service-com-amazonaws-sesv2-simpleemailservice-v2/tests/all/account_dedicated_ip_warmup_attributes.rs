// SPDX-License-Identifier: CC0-1.0

//! `PutAccountDedicatedIpWarmupAttributes`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the handler and serialization are all
//! exercised, and what the call persisted is read back through `GetAccount`, which
//! reports it as `DedicatedIpAutoWarmupEnabled`.
//!
//! The rule these cases pin: the call stores the account-level automatic warm-up
//! switch the request carries — the whole setting, so the switch afterwards is
//! exactly the value supplied, whatever it was before and however often the same
//! value is written, and a request that omits the member stores the member's
//! modelled default — every later read reports it, and nothing else about the
//! account moves. The tables below span what the rule generalises over: both
//! settings, the member supplied and left out, and the orderings and repetitions of
//! writes.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::DashboardAttributes;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    AccountState, ComAmazonawsSesv2SimpleemailserviceV2State, FEATURE_ENABLED,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled default of `PutAccountDedicatedIpWarmupAttributesRequest$AutoWarmupEnabled`:
/// the member targets `Enabled`, a boolean with a default of `false`
/// (`smithy_query(query="operation",operation="PutAccountDedicatedIpWarmupAttributes")`),
/// so a request that omits it asks for the feature to be disabled.
const AUTO_WARMUP_ENABLED_DEFAULT: bool = false;

/// One answered request: the status and the parsed body.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// A boolean member of the body, absent meaning the modelled default `false`.
    fn flag(&self, member: &str) -> bool {
        self.body
            .get(member)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// This reply's body with `member` dropped, so two reports can be compared on
    /// everything except the one member a case changed.
    fn body_without(&self, member: &str) -> Value {
        let mut body = self.body.clone();
        if let Some(object) = body.as_object_mut() {
            object.remove(member);
        }
        body
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// persist the account state it is about and read the switch the handler kept.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses`
            // signing name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// `PutAccountDedicatedIpWarmupAttributes` with whatever body the case supplies,
    /// so a case can leave the optional member out entirely.
    async fn put_warmup(&self, body: Value) -> Reply {
        self.call("PUT", "/v2/email/account/dedicated-ips/warmup", Some(body))
            .await
    }

    /// The account report.
    async fn account(&self) -> Reply {
        self.call("GET", "/v2/email/account", None).await
    }

    /// Persist the account state a case is about.
    fn persist(&self, account: &AccountState) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.account = account.clone();
    }

    /// The warm-up switch the handler persisted.
    fn stored_auto_warmup(&self) -> bool {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.account.dedicated_ip_auto_warmup_enabled
    }
}

/// One write of the switch: the request body, and the setting it asks for.
struct SwitchCase {
    name: &'static str,
    body: Value,
    expected: bool,
}

/// The writes the rule has to hold for, in an order that covers the transitions and
/// the repetitions: each setting written from both of its predecessors, each written
/// twice in a row, and the member left out of the request.
fn switch_cases() -> Vec<SwitchCase> {
    vec![
        SwitchCase {
            name: "disable-from-the-default",
            body: json!({ "AutoWarmupEnabled": false }),
            expected: false,
        },
        SwitchCase {
            name: "disable-again",
            body: json!({ "AutoWarmupEnabled": false }),
            expected: false,
        },
        SwitchCase {
            name: "enable",
            body: json!({ "AutoWarmupEnabled": true }),
            expected: true,
        },
        SwitchCase {
            name: "enable-again",
            body: json!({ "AutoWarmupEnabled": true }),
            expected: true,
        },
        SwitchCase {
            name: "member-omitted",
            body: json!({}),
            expected: AUTO_WARMUP_ENABLED_DEFAULT,
        },
        SwitchCase {
            name: "enable-after-the-omitted-member",
            body: json!({ "AutoWarmupEnabled": true }),
            expected: true,
        },
    ]
}

#[tokio::test]
async fn the_switch_a_request_carries_is_what_every_later_read_reports() {
    let ses = Ses::new();
    for case in switch_cases() {
        let name = case.name;
        let written = ses.put_warmup(case.body.clone()).await;
        assert_eq!(
            written.status, 200,
            "case {name:?}: expected 200 from the write, got {} {:?}",
            written.status, written.body
        );
        assert!(
            written.body.is_null() || written.body == json!({}),
            "case {name:?}: expected an empty response body, got {:?}",
            written.body
        );
        assert_eq!(
            ses.stored_auto_warmup(),
            case.expected,
            "case {name:?}: expected the stored switch to be {}, got {:?}",
            case.expected,
            ses.stored_auto_warmup()
        );

        // Every later caller sees it, and a read does not move it: two reports in a
        // row agree with the write.
        for read in ["first", "second"] {
            let reported = ses.account().await;
            assert_eq!(
                reported.status, 200,
                "case {name:?} ({read} read): expected 200 from the account report, got {} {:?}",
                reported.status, reported.body
            );
            assert_eq!(
                reported.flag("DedicatedIpAutoWarmupEnabled"),
                case.expected,
                "case {name:?} ({read} read): expected DedicatedIpAutoWarmupEnabled {}, got {:?}",
                case.expected,
                reported.body
            );
        }
    }
}

#[tokio::test]
async fn the_write_leaves_every_other_account_attribute_alone() {
    // An account whose other attributes are all away from their defaults, so a write
    // that reached any of them would show.
    let account = AccountState {
        sending_enabled: false,
        dedicated_ip_auto_warmup_enabled: false,
        production_access_enabled: true,
        vdm_enabled: FEATURE_ENABLED.to_string(),
        vdm_dashboard_attributes: Some(DashboardAttributes {
            engagement_metrics: Some(FEATURE_ENABLED.to_string()),
        }),
        pricing_plan: "ESSENTIALS".to_string(),
        next_pricing_plan: Some("PRO".to_string()),
        suppressed_reasons: Vec::new(),
        ..AccountState::default()
    };

    for (name, asked) in [("to-true", true), ("back-to-false", false)] {
        let ses = Ses::new();
        ses.persist(&account);
        let before = ses.account().await;
        let written = ses.put_warmup(json!({ "AutoWarmupEnabled": asked })).await;
        assert_eq!(
            written.status, 200,
            "case {name:?}: expected 200 from the write, got {} {:?}",
            written.status, written.body
        );

        let after = ses.account().await;
        assert_eq!(
            after.flag("DedicatedIpAutoWarmupEnabled"),
            asked,
            "case {name:?}: expected DedicatedIpAutoWarmupEnabled {asked}, got {:?}",
            after.body
        );
        assert_eq!(
            after.body_without("DedicatedIpAutoWarmupEnabled"),
            before.body_without("DedicatedIpAutoWarmupEnabled"),
            "case {name:?}: expected every other reported attribute to be untouched, got {:?}",
            after.body
        );
    }
}

#[tokio::test]
async fn a_fresh_account_reports_its_default_until_a_request_flips_it() {
    // The switch a new account holds is the one `GetAccount` already reports, so the
    // first write is what changes it, not the read before it — and the write that
    // flips it is reachable: the flipped value is reported by the very next read.
    let ses = Ses::new();
    let default = AccountState::default().dedicated_ip_auto_warmup_enabled;
    assert_eq!(
        ses.stored_auto_warmup(),
        default,
        "case fresh: expected a new account to hold the default warm-up switch"
    );
    let reported = ses.account().await;
    assert_eq!(
        reported.flag("DedicatedIpAutoWarmupEnabled"),
        default,
        "case fresh: expected the report to agree with the stored switch, got {:?}",
        reported.body
    );

    let written = ses.put_warmup(json!({ "AutoWarmupEnabled": !default })).await;
    assert_eq!(
        written.status, 200,
        "case flipped: expected 200 from the write, got {} {:?}",
        written.status, written.body
    );
    let reported = ses.account().await;
    assert_eq!(
        reported.flag("DedicatedIpAutoWarmupEnabled"),
        !default,
        "case flipped: expected the next read to report {}, got {:?}",
        !default,
        reported.body
    );
}
