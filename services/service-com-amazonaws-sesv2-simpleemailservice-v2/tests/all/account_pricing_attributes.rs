// SPDX-License-Identifier: CC0-1.0

//! `PutAccountPricingAttributes` and the pricing attributes it feeds, end to end over
//! HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the handler and serialization are all exercised,
//! and what a call persisted is read back through `GetAccount`.
//!
//! The rules these cases pin:
//!
//! * a request for a plan ABOVE the one currently active makes that plan active at
//!   once, reported as `PricingAttributes$CurrentPlan`, and any other plan is the
//!   change requested for the start of the next cycle, reported as
//!   `PricingAttributes$NextPlan` with the active plan untouched;
//! * the account carries one outstanding request, the latest: a request that leaves
//!   nothing to activate at the next cycle clears whatever an earlier one staged, so
//!   `NextPlan` is empty exactly when no scheduled change is pending;
//! * a request made while an account details update is under review is refused with
//!   the modelled conflict, and the pricing attributes are left alone;
//! * two requests racing on the same account land at least one winner, and a request
//!   that is refused is refused with that same modelled conflict and no other code.
//!
//! The table spans every ORDERED PAIR of plans — each plan requested from each plan
//! the account can be on — so every value of the plan catalog is covered as both the
//! active and the requested plan.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    MAIL_TYPE_VALUES, PRICING_PLAN_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, PRICING_PLAN_LADDER, PRICING_PLAN_NONE,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The pricing plans the account report carries: the active plan, and the plan a
    /// scheduled change will activate when one is pending.
    fn plans(&self) -> (Option<String>, Option<String>) {
        let attributes = self.body.get("PricingAttributes");
        let member = |name: &str| {
            attributes
                .and_then(|value| value.get(name))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        (member("CurrentPlan"), member("NextPlan"))
    }

    /// This reply's body with `PricingAttributes` dropped, so two reports can be
    /// compared on everything the pricing call does not touch.
    fn body_without_pricing(&self) -> Value {
        let mut body = self.body.clone();
        if let Some(object) = body.as_object_mut() {
            object.remove("PricingAttributes");
        }
        body
    }
}

/// The emulated service behind its router, plus the shared engine so a case can read
/// what a handler kept.
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
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// `PutAccountPricingAttributes` for one plan.
    async fn put_plan(&self, plan: &str) -> Reply {
        self.call(
            "PUT",
            "/v2/email/account/pricing-attributes",
            Some(json!({"Plan": plan})),
        )
        .await
    }

    /// The account report, which is where the applied plan is observable.
    async fn account(&self) -> Reply {
        self.call("GET", "/v2/email/account", None).await
    }

    /// `PutAccountDetails` asking for production access, which puts an account
    /// details update under review.
    async fn ask_for_production_access(&self) -> Reply {
        self.call(
            "POST",
            "/v2/email/account/details",
            Some(json!({
                "MailType": MAIL_TYPE_VALUES[0],
                "WebsiteURL": "https://pricing.example.com",
                "ProductionAccessEnabled": true,
            })),
        )
        .await
    }

    /// Bring the decision on the account details update under review into the past,
    /// so a case does not wait out the span the clock owes it.
    fn bring_the_decision_due(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_millis(1))
            .expect("an instant just before now");
        assert!(
            store.account.production_access_review_due_at.is_some(),
            "case decision-due: an account details update must be under review here"
        );
        store.account.production_access_review_due_at = Some(past);
    }

    /// The plans the handler persisted, as the store holds them.
    fn stored_plans(&self) -> (String, Option<String>) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        (
            store.account.pricing_plan.clone(),
            store.account.next_pricing_plan.clone(),
        )
    }
}

/// Where a plan requested from `active` lands: the active plan afterwards, and the
/// plan staged for the next cycle.
///
/// The rule under test, stated once: a plan further up the ladder than the active one
/// becomes active at once; anything else is what the account asks to be on from the
/// next cycle, which for the plan already active is nothing at all.
fn expected_plans(active: &str, requested: &str) -> (Option<String>, Option<String>) {
    let rank = |plan: &str| {
        PRICING_PLAN_LADDER
            .iter()
            .position(|known| *known == plan)
            .unwrap_or_else(|| panic!("the ladder must rank {plan:?}"))
    };
    if rank(requested) > rank(active) {
        (Some(requested.to_string()), None)
    } else if requested == active {
        (Some(active.to_string()), None)
    } else {
        (Some(active.to_string()), Some(requested.to_string()))
    }
}

/// Put the account on `plan` from the plan a fresh account carries, which is one move
/// up the ladder for every plan above it and nothing at all for that plan itself.
async fn put_the_account_on(ses: &Ses, plan: &str, case: &str) {
    if plan == PRICING_PLAN_NONE {
        return;
    }
    let reply = ses.put_plan(plan).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 putting the account on {plan:?}, got {} {:?}",
        reply.status, reply.body
    );
    let report = ses.account().await;
    assert_eq!(
        report.plans(),
        (Some(plan.to_string()), None),
        "case {case:?}: expected the account to be on {plan:?} with no change pending, got {:?}",
        report.body
    );
}

#[tokio::test]
async fn a_requested_plan_either_activates_at_once_or_is_staged_for_the_next_cycle() {
    // Every plan the wire accepts is on the ladder the rows are built from, so no
    // value of the catalog goes untested as the active or as the requested plan.
    for plan in PRICING_PLAN_VALUES {
        assert!(
            PRICING_PLAN_LADDER.contains(plan),
            "case catalog: the ladder must rank the plan {plan:?}, got {PRICING_PLAN_LADDER:?}"
        );
    }

    for active in PRICING_PLAN_LADDER {
        for requested in PRICING_PLAN_LADDER {
            let case = format!("{active} then {requested}");
            // A fresh emulator per row: the pricing attributes are one record per
            // account and Region, so the rows must not see each other's writes.
            let ses = Ses::new();
            let fresh = ses.account().await;
            assert_eq!(
                fresh.plans(),
                (Some(PRICING_PLAN_NONE.to_string()), None),
                "case {case:?}: expected a new account on no paid plan, got {:?}",
                fresh.body
            );
            put_the_account_on(&ses, active, &case).await;
            let before = ses.account().await;

            let written = ses.put_plan(requested).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected 200 from the request, got {} {:?}",
                written.status, written.body
            );
            assert!(
                written.body.is_null() || written.body == json!({}),
                "case {case:?}: expected an empty response body, got {:?}",
                written.body
            );

            let expected = expected_plans(active, requested);
            // Every later caller sees the request, and a read does not move it: two
            // reports in a row agree with it.
            for read in ["first", "second"] {
                let after = ses.account().await;
                assert_eq!(
                    after.status, 200,
                    "case {case:?} ({read} read): expected 200 from the account report, got {} {:?}",
                    after.status, after.body
                );
                assert_eq!(
                    after.plans(),
                    expected,
                    "case {case:?} ({read} read): expected the plans {expected:?}, got {:?}",
                    after.body
                );
                assert_eq!(
                    after.body_without_pricing(),
                    before.body_without_pricing(),
                    "case {case:?} ({read} read): expected every other reported attribute to be \
                     untouched, got {:?}",
                    after.body
                );
            }
            assert_eq!(
                ses.stored_plans(),
                (
                    expected.0.clone().unwrap_or_default(),
                    expected.1.clone()
                ),
                "case {case:?}: expected the store to hold the plans the report named"
            );
        }
    }
}

#[tokio::test]
async fn the_account_carries_only_the_latest_request() {
    // Each request replaces the one before it, so what is pending at any moment is
    // the latest request alone: a staged change is dropped as soon as a request
    // leaves nothing to activate at the next cycle.
    let ses = Ses::new();
    for (case, plan, expected) in [
        // Up the ladder from a fresh account: active at once.
        ("upgrade-to-pro", "PRO", (Some("PRO"), None)),
        // Down the ladder: staged, with the active plan untouched.
        ("stage-essentials", "ESSENTIALS", (Some("PRO"), Some("ESSENTIALS"))),
        // A lower plan again: the staged change is the later request's.
        ("restage-none", "NONE", (Some("PRO"), Some("NONE"))),
        // The plan already active: nothing is left to activate next cycle, so the
        // staged change is gone.
        ("request-the-active-plan", "PRO", (Some("PRO"), None)),
        // Down again, then up: the move up activates and clears the staging.
        ("stage-none-again", "NONE", (Some("PRO"), Some("NONE"))),
        ("upgrade-to-enterprise", "ENTERPRISE", (Some("ENTERPRISE"), None)),
    ] {
        let written = ses.put_plan(plan).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected 200 requesting {plan:?}, got {} {:?}",
            written.status, written.body
        );
        let after = ses.account().await;
        let expected = (
            expected.0.map(str::to_string),
            expected.1.map(str::to_string),
        );
        assert_eq!(
            after.plans(),
            expected,
            "case {case:?}: expected the plans {expected:?}, got {:?}",
            after.body
        );
    }
}

#[tokio::test]
async fn a_request_made_while_an_account_update_is_under_review_is_refused() {
    let ses = Ses::new();
    let staged = ses.put_plan("ENTERPRISE").await;
    assert_eq!(
        staged.status, 200,
        "case setup: expected 200 putting the account on a plan, got {} {:?}",
        staged.status, staged.body
    );

    let asked = ses.ask_for_production_access().await;
    assert_eq!(
        asked.status, 200,
        "case setup: expected 200 from the production-access request, got {} {:?}",
        asked.status, asked.body
    );

    // "If there is already an ongoing account details update under review": the
    // request is refused, and nothing it asked for is persisted.
    let before = ses.stored_plans();
    let conflicted = ses.put_plan("ESSENTIALS").await;
    assert_eq!(
        (conflicted.status, conflicted.error_code().as_str()),
        (409, "ConflictException"),
        "case conflict: expected a 409 ConflictException, got {} {:?}",
        conflicted.status,
        conflicted.body
    );
    assert_eq!(
        conflicted.message(),
        "There is already an ongoing account details update under review.",
        "case conflict: expected the ongoing-review message, got {:?}",
        conflicted.message()
    );
    assert_eq!(
        ses.stored_plans(),
        before,
        "case conflict: expected the refused request to leave the plans alone"
    );

    // Once the decision the clock owed that update falls due, nothing is under
    // review any more and the same request is served.
    ses.bring_the_decision_due();
    let served = ses.put_plan("ESSENTIALS").await;
    assert_eq!(
        served.status, 200,
        "case after-review: expected 200 once nothing is under review, got {} {:?}",
        served.status, served.body
    );
    let after = ses.account().await;
    assert_eq!(
        after.plans(),
        (Some("ENTERPRISE".to_string()), Some("ESSENTIALS".to_string())),
        "case after-review: expected the lower plan staged for the next cycle, got {:?}",
        after.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_racing_requests_land_a_winner_and_refuse_a_loser_with_the_modelled_conflict() {
    // Rows differing along what the racers ask for: two moves up the ladder, a move
    // up against a staged move down, and both racers asking for the same plan.
    for (case, first, second) in [
        ("essentials-against-pro", "ESSENTIALS", "PRO"),
        ("enterprise-against-none", "ENTERPRISE", "NONE"),
        ("the-same-plan-twice", "PRO", "PRO"),
    ] {
        let ses = Arc::new(Ses::new());
        let racers = [first, second].map(|plan| {
            let ses = Arc::clone(&ses);
            tokio::spawn(async move { (plan, ses.put_plan(plan).await) })
        });
        let mut replies = Vec::new();
        for racer in racers {
            replies.push(racer.await.expect("a racer must not panic"));
        }

        // At least one racer lands, and a racer that does not is refused with the
        // operation's one modelled conflict — never another code.
        assert!(
            replies.iter().any(|(_, reply)| reply.status == 200),
            "case {case:?}: expected at least one racer to land, got {:?}",
            replies
                .iter()
                .map(|(plan, reply)| (*plan, reply.status, reply.error_code()))
                .collect::<Vec<_>>()
        );
        for (plan, reply) in &replies {
            assert!(
                reply.status == 200
                    || (reply.status == 409 && reply.error_code() == "ConflictException"),
                "case {case:?}: expected the racer asking for {plan:?} to land or be refused \
                 409 ConflictException, got {} {:?}",
                reply.status,
                reply.body
            );
        }

        // The account is left on a plan a winning racer asked for: active when that
        // racer moved up the ladder, staged for the next cycle otherwise.
        let winners: Vec<&str> = replies
            .iter()
            .filter(|(_, reply)| reply.status == 200)
            .map(|(plan, _)| *plan)
            .collect();
        let after = ses.account().await;
        let (current, next) = after.plans();
        let named: Vec<String> = [current.clone(), next.clone()].into_iter().flatten().collect();
        assert!(
            winners
                .iter()
                .any(|plan| named.iter().any(|named| named == plan)),
            "case {case:?}: expected the account to report a plan a winner asked for \
             (winners {winners:?}), got {:?}",
            after.body
        );
    }
}
