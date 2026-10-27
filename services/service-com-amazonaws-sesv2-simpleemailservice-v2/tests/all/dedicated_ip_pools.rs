// SPDX-License-Identifier: CC0-1.0

//! The dedicated IP pool surface and the account's dedicated-IP auto warm-up switch, end
//! to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rules these cases pin, each over a SET of inputs rather than one recorded request:
//!
//! 1. `CreateDedicatedIpPool` PERSISTS the pool the request names, with the scaling mode
//!    it carried (`STANDARD` when it carried none), and the pool's own read
//!    (`GetDedicatedIpPool`) reports exactly what was stored; a second create of a name
//!    the account already holds is refused `AlreadyExistsException` and changes nothing;
//! 2. `ListDedicatedIpPools` reports the names the account holds in one stable order, and
//!    a walk with ANY `PageSize` visits every pool exactly once — no duplicate and no
//!    drop — and terminates;
//! 3. a `NextToken` the caller did not get from this listing is REFUSED
//!    (`BadRequestException`) rather than read as the start of the listing;
//! 4. `PutAccountDedicatedIpWarmupAttributes` writes the account's auto warm-up switch,
//!    which `GetAccount` then reports — in both directions, and idempotently.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::MAX_DEDICATED_IP_POOLS;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Every `ScalingMode` the model declares, in the order it declares them
/// (`smithy_query(query="shape",shape="ScalingMode")`).
const SCALING_MODES: &[&str] = &["STANDARD", "MANAGED"];

/// The mode a pool created without one carries: `STANDARD`, the mode
/// `PutDedicatedIpPoolScalingAttributes` converts a pool OUT of ("`MANAGED` pools cannot
/// be converted to `STANDARD` scaling mode").
const DEFAULT_SCALING_MODE: &str = "STANDARD";

/// One answered request: the status, the error shape when there was one, and the body.
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

    /// A member reached by its path through the nested structures, or `Null`.
    fn at(&self, path: &[&str]) -> Value {
        let mut here = &self.body;
        for step in path {
            match here.get(step) {
                Some(next) => here = next,
                None => return Value::Null,
            }
        }
        here.clone()
    }

    /// A string at `path`, or `""` when the member is absent.
    fn text_at(&self, path: &[&str]) -> String {
        self.at(path).as_str().unwrap_or_default().to_string()
    }

    /// The strings of the list member at `path`; an absent list reads as empty.
    fn strings_at(&self, path: &[&str]) -> Vec<String> {
        self.at(path)
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| row.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
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
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
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

    async fn create_pool(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/dedicated-ip-pools", Some(body))
            .await
    }

    async fn get_pool(&self, pool_name: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/dedicated-ip-pools/{pool_name}"),
            None,
        )
        .await
    }

    async fn list_pools(&self, query: &str) -> Reply {
        self.call("GET", &format!("/v2/email/dedicated-ip-pools{query}"), None)
            .await
    }

    async fn account(&self) -> Reply {
        self.call("GET", "/v2/email/account", None).await
    }

    async fn put_auto_warmup(&self, enabled: bool) -> Reply {
        self.call(
            "PUT",
            "/v2/email/account/dedicated-ips/warmup",
            Some(json!({"AutoWarmupEnabled": enabled})),
        )
        .await
    }
}

/// One create: the body sent and the scaling mode the stored pool must report.
struct Row {
    case: &'static str,
    pool_name: &'static str,
    body: Value,
    scaling_mode: &'static str,
}

#[tokio::test]
async fn a_created_pool_is_persisted_and_read_back() {
    let ses = Ses::new();
    let rows = vec![
        Row {
            case: "a pool with no ScalingMode, which starts STANDARD",
            pool_name: "plain-pool",
            body: json!({"PoolName": "plain-pool"}),
            scaling_mode: DEFAULT_SCALING_MODE,
        },
        Row {
            case: "a pool naming STANDARD explicitly",
            pool_name: "standard-pool",
            body: json!({"PoolName": "standard-pool", "ScalingMode": "STANDARD"}),
            scaling_mode: "STANDARD",
        },
        Row {
            case: "a pool naming the other mode the enum declares",
            pool_name: "managed-pool",
            body: json!({"PoolName": "managed-pool", "ScalingMode": "MANAGED"}),
            scaling_mode: "MANAGED",
        },
        Row {
            case: "a pool carrying tags, which the pool's own read does not report",
            pool_name: "tagged-pool",
            body: json!({
                "PoolName": "tagged-pool",
                "Tags": [{"Key": "team", "Value": "growth"}, {"Key": "tier", "Value": "gold"}],
            }),
            scaling_mode: DEFAULT_SCALING_MODE,
        },
        Row {
            case: "a one-character pool name",
            pool_name: "p",
            body: json!({"PoolName": "p", "ScalingMode": "MANAGED"}),
            scaling_mode: "MANAGED",
        },
        Row {
            case: "a long pool name carrying both documented separators",
            pool_name: "marketing_pool-for-eu-campaigns-2026",
            body: json!({"PoolName": "marketing_pool-for-eu-campaigns-2026"}),
            scaling_mode: DEFAULT_SCALING_MODE,
        },
    ];

    let mut created: Vec<&str> = Vec::new();
    for row in &rows {
        let case = row.case;
        let reply = ses.create_pool(row.body.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (200, ""),
            "case {case:?}: expected 200 from the create, got {} {:?}",
            reply.status,
            reply.body
        );

        // The round trip: the pool's own read reports what was stored, not what the
        // create echoed.
        let read = ses.get_pool(row.pool_name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from the pool's read, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            (
                read.text_at(&["DedicatedIpPool", "PoolName"]).as_str(),
                read.text_at(&["DedicatedIpPool", "ScalingMode"]).as_str(),
            ),
            (row.pool_name, row.scaling_mode),
            "case {case:?}: expected the stored pool {:?}/{:?}, got {:?}",
            row.pool_name,
            row.scaling_mode,
            read.body
        );
        created.push(row.pool_name);

        // Creating the same name again is refused and leaves the stored pool alone.
        let again = ses
            .create_pool(json!({"PoolName": row.pool_name, "ScalingMode": "MANAGED"}))
            .await;
        assert_eq!(
            (again.status, again.error_code().as_str()),
            (400, "AlreadyExistsException"),
            "case {case:?}: expected 400 AlreadyExistsException on the second create, got {} {:?}",
            again.status,
            again.body
        );
        let read = ses.get_pool(row.pool_name).await;
        assert_eq!(
            read.text_at(&["DedicatedIpPool", "ScalingMode"]),
            row.scaling_mode,
            "case {case:?}: the refused create must change nothing, got {:?}",
            read.body
        );
    }

    // Every pool created above is listed, and nothing else is.
    let listed: BTreeSet<String> = ses
        .list_pools("")
        .await
        .strings_at(&["DedicatedIpPools"])
        .into_iter()
        .collect();
    let expected: BTreeSet<String> = created.iter().map(|name| (*name).to_string()).collect();
    assert_eq!(
        listed, expected,
        "case the whole listing: expected exactly the pools created, got {listed:?}"
    );
}

#[tokio::test]
async fn a_pool_the_account_does_not_hold_is_not_found() {
    let ses = Ses::new();
    ses.create_pool(json!({"PoolName": "kept"})).await;
    for case in ["never-created", "KEPT", "kept-2"] {
        let reply = ses.get_pool(case).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

#[tokio::test]
async fn a_scaling_mode_outside_the_enum_is_refused_and_stores_nothing() {
    let ses = Ses::new();
    for case in ["standard", "AUTOMATIC", ""] {
        let reply = ses
            .create_pool(json!({"PoolName": "rejected-pool", "ScalingMode": case}))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        let read = ses.get_pool("rejected-pool").await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must store nothing, got {} {:?}",
            read.status, read.body
        );
    }
    // Each value the enum DOES declare is accepted, so the refusal is about the set and
    // not about the member.
    for mode in SCALING_MODES {
        let reply = ses
            .create_pool(json!({"PoolName": format!("pool-{mode}"), "ScalingMode": mode}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {mode:?}: a declared ScalingMode is accepted, got {} {:?}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn a_paginated_walk_visits_every_pool_exactly_once() {
    let ses = Ses::new();
    // Names that do NOT sort the way they were created, so a page boundary cannot be
    // passing by accident of insertion order.
    let names: Vec<String> = ["m", "c", "z", "a", "q", "b", "y"]
        .iter()
        .map(|prefix| format!("{prefix}-pool"))
        .collect();
    for name in &names {
        let reply = ses.create_pool(json!({"PoolName": name})).await;
        assert_eq!(
            reply.status, 200,
            "case seed {name:?}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
    }
    let mut sorted = names.clone();
    sorted.sort();

    // Every page size from one per page to one past the whole listing, plus the sizes a
    // caller may send that do not truncate at all.
    for page_size in 1..=names.len() + 1 {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= names.len() + 2,
                "case PageSize={page_size}: the walk must terminate, got {pages} pages"
            );
            let query = match &token {
                Some(token) => format!("?PageSize={page_size}&NextToken={token}"),
                None => format!("?PageSize={page_size}"),
            };
            let reply = ses.list_pools(&query).await;
            assert_eq!(
                reply.status, 200,
                "case PageSize={page_size} page {pages}: expected 200, got {} {:?}",
                reply.status, reply.body
            );
            let page = reply.strings_at(&["DedicatedIpPools"]);
            assert!(
                page.len() <= page_size,
                "case PageSize={page_size} page {pages}: a page may not exceed the size \
                 asked for, got {page:?}"
            );
            walked.extend(page);
            match reply.at(&["NextToken"]).as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, sorted,
            "case PageSize={page_size}: the walk must visit every pool exactly once, in the \
             listing's order, got {walked:?}"
        );
    }

    // A size that truncates nothing reports the whole listing and no token.
    for query in ["", "?PageSize=0", "?PageSize=-1"] {
        let reply = ses.list_pools(query).await;
        assert_eq!(
            (reply.status, reply.strings_at(&["DedicatedIpPools"])),
            (200, sorted.clone()),
            "case {query:?}: expected the whole listing, got {} {:?}",
            reply.status,
            reply.body
        );
        assert!(
            reply.at(&["NextToken"]).is_null(),
            "case {query:?}: a page holding the whole listing carries no token, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_token_this_listing_did_not_mint_is_refused() {
    let ses = Ses::new();
    for name in ["a-pool", "b-pool", "c-pool"] {
        ses.create_pool(json!({"PoolName": name})).await;
    }
    // A token the caller forged, one belonging to ANOTHER listing of this service, and
    // one that is not even base64 — none of them is a position in this listing.
    let other_listing = aws_smithy_types::base64::encode("sesv2:tenants:a-pool");
    let not_base64 = "!!!not-a-token!!!".to_string();
    let plain_name = aws_smithy_types::base64::encode("a-pool");
    for token in [other_listing, not_base64, plain_name] {
        let reply = ses.list_pools(&format!("?NextToken={token}")).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {token:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
    }
    // The token this listing DID mint resumes it.
    let first = ses.list_pools("?PageSize=1").await;
    let token = first
        .at(&["NextToken"])
        .as_str()
        .map(str::to_string)
        .expect("a listing with pools behind the first page mints a token");
    let second = ses
        .list_pools(&format!("?PageSize=1&NextToken={token}"))
        .await;
    assert_eq!(
        (second.status, second.strings_at(&["DedicatedIpPools"])),
        (200, vec!["b-pool".to_string()]),
        "case the listing's own token: expected the second pool, got {} {:?}",
        second.status,
        second.body
    );
}

#[tokio::test]
async fn the_pool_ceiling_is_enforced_and_nothing_past_it_is_stored() {
    let ses = Ses::new();
    for index in 0..MAX_DEDICATED_IP_POOLS {
        let reply = ses
            .create_pool(json!({"PoolName": format!("pool-{index:03}")}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case pool {index} of {MAX_DEDICATED_IP_POOLS}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
    }
    let reply = ses.create_pool(json!({"PoolName": "one-too-many"})).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "LimitExceededException"),
        "case one pool past the ceiling: expected 400 LimitExceededException, got {} {:?}",
        reply.status,
        reply.body
    );
    let read = ses.get_pool("one-too-many").await;
    assert_eq!(
        read.status, 404,
        "case one pool past the ceiling: nothing is stored, got {} {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn the_account_auto_warmup_switch_is_written_and_reported() {
    let ses = Ses::new();
    // Both directions, each twice, so the switch is shown to be the stored state rather
    // than a toggle: a second write of the same value reports the same answer.
    for (case, enabled) in [
        ("disabled", false),
        ("disabled again", false),
        ("enabled", true),
        ("enabled again", true),
        ("disabled once more", false),
    ] {
        let reply = ses.put_auto_warmup(enabled).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (200, ""),
            "case {case:?}: expected 200 from the warm-up write, got {} {:?}",
            reply.status,
            reply.body
        );
        let account = ses.account().await;
        assert_eq!(
            account.status, 200,
            "case {case:?}: expected 200 from the account report, got {} {:?}",
            account.status, account.body
        );
        assert_eq!(
            account
                .at(&["DedicatedIpAutoWarmupEnabled"])
                .as_bool()
                .unwrap_or(false),
            enabled,
            "case {case:?}: expected the account to report {enabled}, got {:?}",
            account.body
        );
    }
}
