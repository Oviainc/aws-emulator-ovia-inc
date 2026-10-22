// SPDX-License-Identifier: CC0-1.0

//! `GetBuyerDashboard`, end to end over HTTP.
//!
//! Every case here goes through the router, so it exercises routing, deserialization,
//! the modelled constraints, the handler, the persisted session and serialization — the
//! path a client takes. The rules under test are the ones the operation's own
//! documentation states:
//!
//! * each call GENERATES a URL — a fresh one-shot bearer token per invocation, recorded
//!   as a session whose stored record is what the response is built from;
//! * the token is valid 5 minutes and single-use, and the session it opens lasts one
//!   hour, measured separately from that window;
//! * `embeddingDomains` are fully qualified domains (optionally with `*` standing for
//!   every subdomain), which is narrower than the member's `@pattern`;
//! * the API is available only to an Organizations management account, or to a delegated
//!   administrator registered for procurement insights — so every case that expects a URL
//!   records that grant first ([`Emulator::available_to_default_caller`]).
//!
//! The harness here is shared with `buyer_dashboard_authorization.rs`, which carries the
//! availability gate's own table.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacereporting_awsmarketplacereporting::state::{
    CallerAccess, ComAmazonawsMarketplacereportingAwsmarketplacereportingState as Store, Dashboard,
    Redemption,
};
use service_com_amazonaws_marketplacereporting_awsmarketplacereporting::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the requests below are signed for, and so the region every generated embed
/// URL names.
pub(crate) const REGION: &str = "us-east-1";

/// The SigV4 access key id every case signs with unless it names another — the principal
/// a [`CallerAccess`] record is keyed by.
pub(crate) const DEFAULT_PRINCIPAL: &str = "AKIDEXAMPLE";

/// The lifetimes `GetBuyerDashboard`'s docs state ("valid for 5 minutes after it is
/// generated", "a session lifetime of one hour"), spelled out here rather than imported
/// from the crate so a wrong constant in `src/state.rs` fails this suite.
const TOKEN_VALIDITY: Duration = Duration::from_secs(300);
const SESSION_LIFETIME: Duration = Duration::from_secs(3600);

/// The ARN of the `AgreementSummary_V1` dashboard for `account`. Both tails below are the
/// ones `DashboardIdentifier`'s `@pattern` admits
/// (`smithy_query(query="shape",shape="DashboardIdentifier")`).
pub(crate) fn agreement_summary(account: &str) -> String {
    format!(
        "arn:aws:aws-marketplace::{account}:AWSMarketplace/ReportingData/\
         Agreement_V1/Dashboard/AgreementSummary_V1"
    )
}

/// The ARN of the `CostAnalysis_V1` dashboard for `account` — the dashboard the issue's
/// own reproduction never names.
pub(crate) fn cost_analysis(account: &str) -> String {
    format!(
        "arn:aws:aws-marketplace::{account}:AWSMarketplace/ReportingData/\
         BillingEvent_V1/Dashboard/CostAnalysis_V1"
    )
}

/// One emulator: a router carrying only this service over one shared engine, plus the
/// engine itself so a case can read the session the handler persisted.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The same emulator, with the default caller recorded as the organization's
    /// management account.
    ///
    /// The API is available only to the callers the operation's docs name, and this
    /// service holds that grant only as a record (nothing here enrols an account — see
    /// `CallerAccess::standalone_account`), so a case that expects a URL has to put one
    /// in the store first.
    pub(crate) fn available_to_default_caller() -> Self {
        let emulator = Self::new();
        emulator.store().lock().access.insert(
            DEFAULT_PRINCIPAL.to_string(),
            CallerAccess::management_account(),
        );
        emulator
    }

    /// `POST /getBuyerDashboard` signed by `principal` (the SigV4 access key id, which is
    /// the caller identity a `CallerAccess` record is keyed by).
    pub(crate) async fn get_buyer_dashboard(&self, principal: &str, body: &Value) -> Answer {
        let credential =
            format!("AWS4-HMAC-SHA256 Credential={principal}/20260101/{REGION}/aws-marketplace/aws4_request, SignedHeaders=host, Signature=abc");
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/getBuyerDashboard")
            .header("host", "reporting-marketplace.us-east-1.amazonaws.com")
            .header("authorization", credential)
            .header("content-type", "application/json")
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "the answer is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }

    /// The same call as the default caller.
    pub(crate) async fn call(&self, body: &Value) -> Answer {
        self.get_buyer_dashboard(DEFAULT_PRINCIPAL, body).await
    }

    pub(crate) fn store(&self) -> ::engine::ServiceState<Store> {
        self.engine.state::<Store>()
    }
}

/// What the emulator answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    pub(crate) fn embed_url(&self) -> &str {
        self.body["embedUrl"].as_str().unwrap_or_default()
    }

    pub(crate) fn dashboard_identifier(&self) -> &str {
        self.body["dashboardIdentifier"].as_str().unwrap_or_default()
    }

    pub(crate) fn embedding_domains(&self) -> Vec<String> {
        self.body["embeddingDomains"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    pub(crate) fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message the body carries, or the empty string when it carries none.
    pub(crate) fn error_message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// A request body naming `dashboard` and `domains`.
pub(crate) fn request(dashboard: &str, domains: &[&str]) -> Value {
    json!({ "dashboardIdentifier": dashboard, "embeddingDomains": domains })
}

/// The bearer token out of an embed URL, parsed here rather than through the crate's own
/// helper so a wrong helper cannot make this suite pass.
pub(crate) fn token_of(url: &str) -> &str {
    url.split("/embed/")
        .nth(1)
        .unwrap_or_default()
        .split('/')
        .next()
        .unwrap_or_default()
}

#[tokio::test]
async fn every_call_generates_a_fresh_session_the_response_is_built_from() {
    // The class the rule generalises over: either published dashboard, any account the
    // ARN pattern admits, and one or two embedding domains of every documented form —
    // a wildcard subdomain, two plain domains, the localhost development form, and a
    // multi-label domain.
    let cases: [(String, Vec<&str>, &str); 4] = [
        (
            agreement_summary("123456789012"),
            vec!["https://*.amazon.com"],
            "AgreementSummary_V1",
        ),
        (
            cost_analysis("000000000000"),
            vec!["https://a.example.com", "https://b.example.com"],
            "CostAnalysis_V1",
        ),
        (
            agreement_summary("210987654321"),
            vec!["http://localhost:3000"],
            "AgreementSummary_V1",
        ),
        (
            cost_analysis("999999999999"),
            vec!["https://sub.example.co.uk", "https://*.example.io"],
            "CostAnalysis_V1",
        ),
    ];

    let emulator = Emulator::available_to_default_caller();
    // Every token this suite sees, so no two calls — in one case or across them — ever
    // mint the same one.
    let mut tokens: HashSet<String> = HashSet::new();

    for (dashboard, domains, dashboard_name) in &cases {
        let case = (dashboard, domains);
        let body = request(dashboard, domains);

        // Two successive calls for the same dashboard: the URL carries a one-shot token,
        // so each call must generate its own.
        let mut urls: Vec<String> = Vec::new();
        for attempt in 1..=2 {
            let before = SystemTime::now();
            let answer = emulator.call(&body).await;
            let after = SystemTime::now();
            assert_eq!(
                (answer.status, answer.error_code()),
                (200, ""),
                "case {case:?} attempt {attempt}: expected 200, got {} {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.dashboard_identifier(),
                dashboard.as_str(),
                "case {case:?}: dashboardIdentifier must come back as asked, got {:?}",
                answer.dashboard_identifier()
            );
            assert_eq!(
                answer.embedding_domains(),
                domains
                    .iter()
                    .map(|domain| (*domain).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: embeddingDomains must come back as asked, got {:?}",
                answer.embedding_domains()
            );

            let url = answer.embed_url().to_string();
            let token = token_of(&url).to_string();
            assert!(
                !token.is_empty() && token.len() >= 32 && token.chars().all(|c| c.is_ascii_hexdigit()),
                "case {case:?}: the embed URL must carry an opaque token, got {url:?}"
            );
            assert_eq!(
                url,
                format!(
                    "https://{REGION}.quicksight.aws.amazon.com/embed/{token}/dashboards/{dashboard_name}"
                ),
                "case {case:?}: the embed URL must name the signed region and the dashboard asked for, got {url:?}"
            );
            assert!(
                tokens.insert(token.clone()),
                "case {case:?} attempt {attempt}: token {token:?} was already minted — \
                 each call carries its own single-use token"
            );

            // The response must be the record that was persisted, not the request echoed.
            let store = emulator.store();
            let held = store.lock();
            let session = held
                .embed_sessions
                .get(&token)
                .unwrap_or_else(|| panic!("case {case:?}: no session was persisted for {token:?}"));
            assert_eq!(
                (
                    session.embed_url.as_str(),
                    session.principal.as_str(),
                    session.dashboard_identifier.as_str(),
                    session.embedding_domains.clone(),
                    session.redeemed_at
                ),
                (
                    url.as_str(),
                    "AKIDEXAMPLE",
                    dashboard.as_str(),
                    domains
                        .iter()
                        .map(|domain| (*domain).to_string())
                        .collect::<Vec<String>>(),
                    None
                ),
                "case {case:?}: the stored session must agree with the answer, got {session:?}"
            );
            assert!(
                session.created_at >= before && session.created_at <= after,
                "case {case:?}: the session must be stamped with the call's instant, got {:?}",
                session.created_at
            );
            assert_eq!(
                (
                    session
                        .token_expires_at
                        .duration_since(session.created_at)
                        .ok(),
                    session
                        .session_expires_at
                        .duration_since(session.created_at)
                        .ok()
                ),
                (Some(TOKEN_VALIDITY), Some(SESSION_LIFETIME)),
                "case {case:?}: the documented lifetimes must run from generation, got {session:?}"
            );
            assert!(
                session.token_is_valid(session.created_at + Duration::from_secs(1)),
                "case {case:?}: a fresh token must be redeemable, got {session:?}"
            );

            // The dashboard the ARN names decides the URL's dashboard segment, which the
            // URL assertion above pins; the account the ARN carries is the one it names.
            assert_eq!(
                Dashboard::parse(dashboard).map(|held| (
                    held.kind.dashboard_name(),
                    held.account_id
                )),
                Some((
                    *dashboard_name,
                    dashboard.split(':').nth(4).unwrap_or_default().to_string()
                )),
                "case {case:?}: the ARN must resolve to the dashboard it names"
            );

            urls.push(url);
        }
        assert_ne!(
            urls[0], urls[1],
            "case {case:?}: two calls must generate two different embed URLs, got {urls:?}"
        );
    }
}

#[tokio::test]
async fn an_embedding_domain_that_is_not_fully_qualified_is_refused() {
    // Each value satisfies `EmbeddingDomain`'s `@pattern` — the boundary validator admits
    // it — and is still not the "fully qualified domain" the member documents, so the
    // handler answers the modelled `BadRequestException`.
    let cases: [(&str, &str); 5] = [
        ("https://*.com", "the wildcard stands for the registrable label"),
        ("https://a.*.example.com", "the wildcard is not the leftmost label"),
        ("https://*.*.amazon.com", "two wildcard labels"),
        ("https://ama*zon.com", "a wildcard inside a label"),
        ("https://example..com", "an empty label"),
    ];

    let emulator = Emulator::available_to_default_caller();
    let dashboard = agreement_summary("123456789012");

    for (domain, why) in cases {
        let answer = emulator.call(&request(&dashboard, &[domain])).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "BadRequestException"),
            "case {domain:?} ({why}): expected 400 BadRequestException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            emulator.store().lock().embed_sessions.is_empty(),
            "case {domain:?}: a refused request must generate no session"
        );
    }

    // And the forms the documentation does name are served, including the two the issue
    // never cites.
    for domain in [
        "https://*.amazon.com",
        "https://aws.amazon.com",
        "http://localhost",
        "http://localhost:65535",
        "https://example.io",
    ] {
        let answer = emulator.call(&request(&dashboard, &[domain])).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (200, ""),
            "case {domain:?}: a documented embedding domain must be served, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

#[tokio::test]
async fn the_models_own_constraints_are_refused_at_the_boundary() {
    // `embeddingDomains` is `@length(min: 1, max: 2)` and `dashboardIdentifier` carries a
    // `@pattern`, so these never reach the handler: they are refused as a
    // `ValidationException` before it runs — and never as a 501.
    let dashboard = agreement_summary("123456789012");
    let cases: [(&str, Value); 5] = [
        (
            "three embedding domains",
            request(
                &dashboard,
                &[
                    "https://a.example.com",
                    "https://b.example.com",
                    "https://c.example.com",
                ],
            ),
        ),
        ("no embedding domain", request(&dashboard, &[])),
        (
            "a dashboard outside the ARN pattern",
            request("arn:aws:aws-marketplace::123456789012:AWSMarketplace/ReportingData/Nope_V1/Dashboard/Nope_V1", &["https://a.example.com"]),
        ),
        (
            "an account id that is not twelve digits",
            request(&agreement_summary("12345678901"), &["https://a.example.com"]),
        ),
        (
            "a domain outside the domain pattern",
            request(&dashboard, &["not-a-url"]),
        ),
    ];

    let emulator = Emulator::available_to_default_caller();
    for (case, body) in cases {
        let answer = emulator.call(&body).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException"),
            "case {case:?}: expected the boundary's 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

#[tokio::test]
async fn the_api_is_available_only_to_the_callers_its_docs_name() {
    // The documented rule: "available only to Amazon Web Services Organization
    // management accounts or delegated administrators registered for the procurement
    // insights … feature". One row per combination of the three flags that decides it,
    // plus the caller the store holds NO record for — which is every caller until
    // something records a grant, since no operation of this service enrols an account.
    let cases: [(&str, Option<CallerAccess>, u16, &str); 7] = [
        ("no record at all", None, 401, "UnauthorizedException"),
        (
            "management account",
            Some(CallerAccess {
                is_org_management_account: true,
                delegated_admin: false,
                procurement_insights_registered: true,
            }),
            200,
            "",
        ),
        (
            "management account, feature not registered",
            Some(CallerAccess {
                is_org_management_account: true,
                delegated_admin: false,
                procurement_insights_registered: false,
            }),
            200,
            "",
        ),
        (
            "registered delegated administrator",
            Some(CallerAccess {
                is_org_management_account: false,
                delegated_admin: true,
                procurement_insights_registered: true,
            }),
            200,
            "",
        ),
        (
            "delegated administrator not registered for procurement insights",
            Some(CallerAccess {
                is_org_management_account: false,
                delegated_admin: true,
                procurement_insights_registered: false,
            }),
            403,
            "AccessDeniedException",
        ),
        (
            "neither, registered for procurement insights",
            Some(CallerAccess {
                is_org_management_account: false,
                delegated_admin: false,
                procurement_insights_registered: true,
            }),
            401,
            "UnauthorizedException",
        ),
        (
            "a member account",
            Some(CallerAccess {
                is_org_management_account: false,
                delegated_admin: false,
                procurement_insights_registered: false,
            }),
            401,
            "UnauthorizedException",
        ),
    ];

    let emulator = Emulator::new();
    let body = request(&agreement_summary("123456789012"), &["https://*.amazon.com"]);
    // Only an available caller may generate a session, so this is what the store must
    // hold after each row.
    let mut generated = 0usize;

    for (index, (case, access, status, code)) in cases.into_iter().enumerate() {
        let principal = format!("AKIDCALLER{index}");
        if let Some(access) = access {
            emulator
                .store()
                .lock()
                .access
                .insert(principal.clone(), access);
        }
        let answer = emulator.get_buyer_dashboard(&principal, &body).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (status, code),
            "case {case:?}: expected {status} {code:?}, got {} {:?}",
            answer.status,
            answer.body
        );
        if status == 200 {
            generated += 1;
            assert!(
                answer.embed_url().contains("/embed/"),
                "case {case:?}: an allowed caller must get an embed URL, got {:?}",
                answer.embed_url()
            );
        } else {
            assert!(
                answer.embed_url().is_empty(),
                "case {case:?}: a refused caller must get no embed URL, got {:?}",
                answer.embed_url()
            );
        }
        let sessions = emulator.store().lock().embed_sessions.len();
        assert_eq!(
            sessions, generated,
            "case {case:?}: only an allowed call may generate a session, store holds {sessions}"
        );
    }
}

#[tokio::test]
async fn a_bearer_token_is_redeemable_once_inside_its_five_minute_window() {
    let emulator = Emulator::available_to_default_caller();
    let body = request(&cost_analysis("123456789012"), &["https://*.amazon.com"]);

    // The instants a redemption is attempted at, and what each must come to. The token's
    // window and the session's hour run separately, so a redemption at 4 minutes opens a
    // session that lasts until an hour after GENERATION.
    let attempts: [(&str, Duration, bool); 4] = [
        ("the instant it was generated", Duration::ZERO, true),
        ("one second inside the window", TOKEN_VALIDITY - Duration::from_secs(1), true),
        ("one second past the window", TOKEN_VALIDITY + Duration::from_secs(1), false),
        ("an hour later", SESSION_LIFETIME, false),
    ];

    for (case, elapsed, grants) in attempts {
        let answer = emulator.call(&body).await;
        assert_eq!(answer.status, 200, "case {case:?}: {:?}", answer.body);
        let token = token_of(answer.embed_url()).to_string();

        let store = emulator.store();
        let created_at = {
            let held = store.lock();
            held.embed_sessions
                .get(&token)
                .unwrap_or_else(|| panic!("case {case:?}: the session was not persisted"))
                .created_at
        };
        let at = created_at + elapsed;

        // A token is the requester's: another principal holding it redeems nothing, and
        // the session is left for its own requester.
        let other = store.lock().redeem(&token, "AKIDSOMEONEELSE", at);
        assert_eq!(
            other,
            Redemption::NotTheRequester,
            "case {case:?}: another principal must not redeem this token, got {other:?}"
        );

        let first = store.lock().redeem(&token, "AKIDEXAMPLE", at);
        if grants {
            assert_eq!(
                first,
                Redemption::Granted {
                    session_expires_at: created_at + SESSION_LIFETIME
                },
                "case {case:?}: a token inside its window must be redeemable, got {first:?}"
            );
            // Single-use: the second redemption of the same token is refused, even at the
            // same instant.
            let second = store.lock().redeem(&token, "AKIDEXAMPLE", at);
            assert_eq!(
                second,
                Redemption::AlreadyRedeemed,
                "case {case:?}: a redeemed token must not be re-usable, got {second:?}"
            );
        } else {
            assert_eq!(
                first,
                Redemption::Expired,
                "case {case:?}: a token past its 5-minute window must be refused, got {first:?}"
            );
        }
    }

    // A token no session was generated with is nobody's.
    let unknown = emulator
        .store()
        .lock()
        .redeem("not-a-token", "AKIDEXAMPLE", SystemTime::now());
    assert_eq!(
        unknown,
        Redemption::Unknown,
        "an unknown token must not be redeemable, got {unknown:?}"
    );
}

#[tokio::test]
async fn a_session_whose_hour_has_run_out_is_released() {
    let emulator = Emulator::available_to_default_caller();
    let body = request(&agreement_summary("123456789012"), &["https://*.amazon.com"]);
    let answer = emulator.call(&body).await;
    assert_eq!(answer.status, 200, "{:?}", answer.body);
    let token = token_of(answer.embed_url()).to_string();

    let store = emulator.store();
    let created_at = store
        .lock()
        .embed_sessions
        .get(&token)
        .map(|session| session.created_at)
        .unwrap_or_else(|| panic!("the session was not persisted"));

    // One instant before the hour is up the session is still there; at the hour the clock
    // owes its release, whether or not anybody called.
    for (case, elapsed, held) in [
        ("a minute short of the hour", SESSION_LIFETIME - Duration::from_secs(60), true),
        ("the hour itself", SESSION_LIFETIME, false),
    ] {
        store.lock().forget_lapsed_sessions(created_at + elapsed);
        assert_eq!(
            store.lock().embed_sessions.contains_key(&token),
            held,
            "case {case:?}: expected the session to be held = {held}"
        );
    }
}
