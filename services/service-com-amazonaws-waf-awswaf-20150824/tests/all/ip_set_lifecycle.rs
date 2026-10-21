// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic `IPSet` lifecycle, end to end over the wire.
//!
//! Every assertion here goes through the real front door: a `MultiServiceRouter` with
//! this service mounted, `POST /` with `X-Amz-Target: AWSWAF_20150824.<Operation>` and an
//! `awsJson1_1` body — the same path the emulator serves a client script on. Nothing
//! reaches into the state struct, so a response that is not computed from persisted state
//! fails these tests.
//!
//! The tables span the classes the rules generalise over (address version, CIDR prefix
//! length, page size, update count, operation), not just the one case the bootstrap issue
//! wrote down.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_waf_awswaf_20150824::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// One emulator with this service mounted, driven over HTTP.
struct Waf {
    router: MultiServiceRouter,
}

impl Waf {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject here; routing is driven by the credential scope
        // in the `Authorization` header below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` with `body` and return the HTTP status and the parsed JSON body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AWSWAF_20150824.{operation}"))
            // The routing signal every signed AWS SDK request carries: the SigV4
            // credential scope, whose service field is this service's signing name.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/waf/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=abc",
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "{operation}: response body is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, parsed)
    }

    /// A fresh change token from `GetChangeToken`.
    async fn change_token(&self) -> String {
        let (status, body) = self.call("GetChangeToken", json!({})).await;
        assert_eq!(status, 200, "GetChangeToken: expected 200, got {status} with {body}");
        body["ChangeToken"]
            .as_str()
            .unwrap_or_else(|| panic!("GetChangeToken: no ChangeToken in {body}"))
            .to_string()
    }

    /// Create an `IPSet` named `name` and return its generated `IPSetId`.
    async fn create_ip_set(&self, name: &str) -> String {
        let token = self.change_token().await;
        let (status, body) = self
            .call("CreateIPSet", json!({"Name": name, "ChangeToken": token}))
            .await;
        assert_eq!(
            status, 200,
            "CreateIPSet {name:?}: expected 200, got {status} with {body}"
        );
        body["IPSet"]["IPSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateIPSet {name:?}: no IPSet.IPSetId in {body}"))
            .to_string()
    }

    /// One single-update `UpdateIPSet` call, with a freshly minted change token.
    async fn update_one(&self, ip_set_id: &str, action: &str, kind: &str, value: &str) -> (u16, Value) {
        let token = self.change_token().await;
        self.call(
            "UpdateIPSet",
            json!({
                "IPSetId": ip_set_id,
                "ChangeToken": token,
                "Updates": [{"Action": action, "IPSetDescriptor": {"Type": kind, "Value": value}}],
            }),
        )
        .await
    }

    /// The descriptors `GetIPSet` reports for `ip_set_id`, as (Type, Value) pairs.
    async fn descriptors(&self, ip_set_id: &str) -> Vec<(String, String)> {
        let (status, body) = self.call("GetIPSet", json!({"IPSetId": ip_set_id})).await;
        assert_eq!(
            status, 200,
            "GetIPSet {ip_set_id}: expected 200, got {status} with {body}"
        );
        body["IPSet"]["IPSetDescriptors"]
            .as_array()
            .map(|descriptors| {
                descriptors
                    .iter()
                    .map(|descriptor| {
                        (
                            descriptor["Type"].as_str().unwrap_or_default().to_string(),
                            descriptor["Value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The wire error code of an error response: the `__type` discriminator, without the
/// namespace some protocols prefix it with.
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or_default();
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// The whole happy path, plus the two refusals that guard it: create, insert, read back,
/// list, refuse deleting a non-empty set, empty it, delete, and observe it gone.
#[tokio::test]
async fn ip_set_lifecycle_is_served_from_persisted_state() {
    let waf = Waf::new();

    let ip_set_id = waf.create_ip_set("bootstrap-ipset").await;
    assert!(
        !ip_set_id.is_empty(),
        "CreateIPSet must generate an IPSetId, got {ip_set_id:?}"
    );
    assert_ne!(
        ip_set_id, "bootstrap-ipset",
        "CreateIPSet must generate an IPSetId, not echo the Name: got {ip_set_id:?}"
    );
    assert_eq!(
        waf.descriptors(&ip_set_id).await,
        Vec::<(String, String)>::new(),
        "a created IPSet starts empty"
    );

    // Inserted descriptors are read back by GetIPSet, in insertion order. The rows differ
    // in address version and prefix length, so this is the class and not one example.
    let inserts = [
        ("IPV4", "10.0.0.0/8"),
        ("IPV4", "192.0.2.44/32"),
        ("IPV6", "2001:db8::/32"),
        ("IPV6", "2001:db8:abcd:1200::/56"),
    ];
    for (kind, value) in inserts {
        let (status, body) = waf.update_one(&ip_set_id, "INSERT", kind, value).await;
        assert_eq!(
            status, 200,
            "INSERT {kind} {value}: expected 200, got {status} with {body}"
        );
    }
    let held = waf.descriptors(&ip_set_id).await;
    let expected: Vec<(String, String)> = inserts
        .iter()
        .map(|(kind, value)| ((*kind).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        held, expected,
        "GetIPSet must report every inserted descriptor, in order: got {held:?}"
    );

    // ListIPSets enumerates live state.
    let (status, body) = waf.call("ListIPSets", json!({"Limit": 10})).await;
    assert_eq!(status, 200, "ListIPSets: expected 200, got {status} with {body}");
    let summaries = body["IPSets"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "ListIPSets must report the one live IPSet, got {summaries:?}"
    );
    assert_eq!(
        summaries[0]["IPSetId"].as_str(),
        Some(ip_set_id.as_str()),
        "ListIPSets must report the created IPSetId, got {summaries:?}"
    );
    assert_eq!(
        summaries[0]["Name"].as_str(),
        Some("bootstrap-ipset"),
        "ListIPSets must report the created Name, got {summaries:?}"
    );

    // A non-empty IPSet cannot be deleted.
    let token = waf.change_token().await;
    let (status, body) = waf
        .call(
            "DeleteIPSet",
            json!({"IPSetId": ip_set_id, "ChangeToken": token}),
        )
        .await;
    assert_eq!(
        status, 400,
        "DeleteIPSet on a non-empty IPSet: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "WAFNonEmptyEntityException",
        "DeleteIPSet on a non-empty IPSet: got {body}"
    );
    assert_eq!(
        waf.descriptors(&ip_set_id).await,
        expected,
        "a refused DeleteIPSet must leave the IPSet untouched"
    );

    // Emptying it makes the delete succeed.
    for (kind, value) in inserts {
        let (status, body) = waf.update_one(&ip_set_id, "DELETE", kind, value).await;
        assert_eq!(
            status, 200,
            "DELETE {kind} {value}: expected 200, got {status} with {body}"
        );
    }
    assert_eq!(
        waf.descriptors(&ip_set_id).await,
        Vec::<(String, String)>::new(),
        "every deleted descriptor must be gone"
    );

    let token = waf.change_token().await;
    let (status, body) = waf
        .call(
            "DeleteIPSet",
            json!({"IPSetId": ip_set_id, "ChangeToken": token}),
        )
        .await;
    assert_eq!(
        status, 200,
        "DeleteIPSet on an empty IPSet: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["ChangeToken"].as_str(),
        Some(token.as_str()),
        "DeleteIPSet returns the ChangeToken it was given, got {body}"
    );

    // And it is gone from both readers.
    let (status, body) = waf.call("GetIPSet", json!({"IPSetId": ip_set_id})).await;
    assert_eq!(
        status, 400,
        "GetIPSet after delete: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "WAFNonexistentItemException",
        "GetIPSet after delete: got {body}"
    );
    let (status, body) = waf.call("ListIPSets", json!({})).await;
    assert_eq!(status, 200, "ListIPSets: expected 200, got {status} with {body}");
    assert_eq!(
        body["IPSets"].as_array().map(Vec::len).unwrap_or_default(),
        0,
        "ListIPSets must not report a deleted IPSet, got {body}"
    );
}

/// `UpdateIPSet` accepts exactly the CIDR ranges AWS WAF documents, for both address
/// versions, and an accepted range is the one that shows up in `GetIPSet`.
///
/// The catalog comes from the `UpdateIPSet` documentation (smithy_query doc,
/// shape="UpdateIPSet"): "AWS WAF supports IPv4 address ranges: /8 and any range between
/// /16 through /32. AWS WAF supports IPv6 address ranges: /24, /32, /48, /56, /64, and
/// /128." Each published bound is covered, together with one step past it.
#[tokio::test]
async fn update_ip_set_accepts_the_documented_cidr_ranges_only() {
    let waf = Waf::new();
    let ip_set_id = waf.create_ip_set("cidr-table").await;

    // (Type, Value, accepted)
    let cases = [
        // IPv4: /8 and /16../32.
        ("IPV4", "10.0.0.0/8", true),
        ("IPV4", "172.16.0.0/16", true),
        ("IPV4", "192.0.2.0/24", true),
        ("IPV4", "192.0.2.44/32", true),
        ("IPV4", "10.0.0.0/7", false),
        ("IPV4", "10.0.0.0/9", false),
        ("IPV4", "172.16.0.0/15", false),
        ("IPV4", "192.0.2.44/33", false),
        ("IPV4", "192.0.2.44", false),
        ("IPV4", "not-an-address/24", false),
        ("IPV4", "2001:db8::/32", false),
        // IPv6: /24, /32, /48, /56, /64, /128.
        ("IPV6", "2400::/24", true),
        ("IPV6", "2001:db8::/32", true),
        ("IPV6", "2001:db8:1234::/48", true),
        ("IPV6", "2001:db8:abcd:1200::/56", true),
        ("IPV6", "2001:db8:abcd:1200::/64", true),
        ("IPV6", "2001:db8::1/128", true),
        ("IPV6", "2001:db8::/16", false),
        ("IPV6", "2001:db8::/25", false),
        ("IPV6", "2001:db8::/129", false),
        ("IPV6", "192.0.2.0/24", false),
    ];

    for (kind, value, accepted) in cases {
        let case = format!("{kind} {value}");
        let (status, body) = waf.update_one(&ip_set_id, "INSERT", kind, value).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            assert!(
                waf.descriptors(&ip_set_id)
                    .await
                    .contains(&(kind.to_string(), value.to_string())),
                "case {case:?}: an accepted descriptor must be readable back, got {:?}",
                waf.descriptors(&ip_set_id).await
            );
            // Leave the set as we found it, so each row is independent.
            let (status, body) = waf.update_one(&ip_set_id, "DELETE", kind, value).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DELETE of the inserted descriptor: got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "WAFInvalidParameterException",
                "case {case:?}: got {body}"
            );
            assert_eq!(
                waf.descriptors(&ip_set_id).await,
                Vec::<(String, String)>::new(),
                "case {case:?}: a refused update must store nothing"
            );
        }
    }
}

/// An `UpdateIPSet` request is atomic: one refused update leaves the whole request
/// without effect, however many valid updates precede it.
#[tokio::test]
async fn a_refused_update_in_a_batch_changes_nothing() {
    let waf = Waf::new();
    let ip_set_id = waf.create_ip_set("atomic-updates").await;

    // Rows differ in where the refused update sits and in why it is refused.
    let batches: [(&str, Value); 3] = [
        (
            "bad CIDR last",
            json!([
                {"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV4", "Value": "10.0.0.0/8"}},
                {"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV4", "Value": "10.0.0.0/12"}},
            ]),
        ),
        (
            "delete of an absent descriptor last",
            json!([
                {"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV6", "Value": "2001:db8::/32"}},
                {"Action": "DELETE", "IPSetDescriptor": {"Type": "IPV4", "Value": "192.0.2.0/24"}},
            ]),
        ),
        (
            "duplicate insert within one request",
            json!([
                {"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV4", "Value": "192.0.2.0/24"}},
                {"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV4", "Value": "192.0.2.0/24"}},
            ]),
        ),
    ];

    for (case, updates) in batches {
        let token = waf.change_token().await;
        let (status, body) = waf
            .call(
                "UpdateIPSet",
                json!({"IPSetId": ip_set_id, "ChangeToken": token, "Updates": updates}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            waf.descriptors(&ip_set_id).await,
            Vec::<(String, String)>::new(),
            "case {case:?}: a refused request must leave the IPSet empty"
        );
        // The token the refused request carried is still usable.
        let (status, body) = waf
            .call(
                "UpdateIPSet",
                json!({
                    "IPSetId": ip_set_id,
                    "ChangeToken": token,
                    "Updates": [{"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV4", "Value": "198.51.100.0/24"}}],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the token of a refused request stays usable, got {status} with {body}"
        );
        let (status, _) = waf
            .update_one(&ip_set_id, "DELETE", "IPV4", "198.51.100.0/24")
            .await;
        assert_eq!(status, 200, "case {case:?}: cleanup DELETE must succeed");
    }
}

/// `Limit` bounds the page, and a returned `NextMarker` resumes immediately after the
/// last item of the page — for every page size in the modelled range.
#[tokio::test]
async fn list_ip_sets_pages_by_limit_and_resumes_after_the_marker() {
    let waf = Waf::new();

    const SETS: usize = 7;
    let mut created = Vec::new();
    for index in 0..SETS {
        created.push(waf.create_ip_set(&format!("paged-{index}")).await);
    }

    // `Limit`'s modelled range is 0..100 (smithy_query, ListIPSetsRequest$Limit): 0 (its
    // declared default) asks for no particular page size, 1 is the smallest real page,
    // and 100 is the published maximum.
    for limit in [0_usize, 1, 2, 3, 7, 8, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"Limit": limit});
            if let Some(marker) = &marker {
                request["NextMarker"] = json!(marker);
            }
            let (status, body) = waf.call("ListIPSets", request).await;
            assert_eq!(
                status, 200,
                "limit {limit}: expected 200, got {status} with {body}"
            );
            let page = body["IPSets"].as_array().cloned().unwrap_or_default();
            let expected_page = if limit == 0 {
                SETS - seen.len()
            } else {
                limit.min(SETS - seen.len())
            };
            assert_eq!(
                page.len(),
                expected_page,
                "limit {limit}, page {pages}: expected {expected_page} items, got {page:?}"
            );
            for summary in &page {
                seen.push(
                    summary["IPSetId"]
                        .as_str()
                        .unwrap_or_else(|| panic!("limit {limit}: no IPSetId in {summary}"))
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= SETS + 1,
                "limit {limit}: pagination did not terminate after {pages} pages"
            );
            match body["NextMarker"].as_str() {
                Some(next) => {
                    assert_eq!(
                        Some(next.to_string()),
                        seen.last().cloned(),
                        "limit {limit}: NextMarker must be the last item of the page"
                    );
                    marker = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "limit {limit}: paging must yield every IPSet once, in creation order"
        );
    }

    // One past the published maximum is refused by the modelled range.
    let (status, body) = waf.call("ListIPSets", json!({"Limit": 101})).await;
    assert_eq!(
        status, 400,
        "Limit 101 is outside the modelled range 0..100: got {status} with {body}"
    );

    // A marker that names no live IPSet is refused rather than silently restarting.
    let (status, body) = waf
        .call("ListIPSets", json!({"NextMarker": "not-a-marker"}))
        .await;
    assert_eq!(
        status, 400,
        "an unknown NextMarker must be refused, got {status} with {body}"
    );
}

/// Change tokens: reissued unchanged while unused, consumed exactly once by a mutating
/// call, and reported by `GetChangeTokenStatus` through the documented
/// PROVISIONED -> PENDING -> INSYNC progression.
///
/// The reissue rule is the `GetChangeToken` documentation (smithy_query doc,
/// shape="GetChangeToken"): "If your application submits a GetChangeToken request and then
/// submits a second GetChangeToken request before submitting a create, update, or delete
/// request, the second GetChangeToken request returns the same value as the first
/// GetChangeToken request." The bootstrap issue's "a NEW unique token per call" contradicts
/// it, so the model's own documentation is what is asserted here.
#[tokio::test]
async fn change_tokens_are_reissued_until_used_then_single_use() {
    let waf = Waf::new();

    // The same value, over a run of calls, while nothing has used it.
    let mut tokens = Vec::new();
    for _ in 0..8 {
        tokens.push(waf.change_token().await);
    }
    for (call, token) in tokens.iter().enumerate() {
        assert_eq!(
            token, &tokens[0],
            "call {call}: GetChangeToken must reissue the unused token, got {token:?}"
        );
        assert!(
            (1..=128).contains(&token.len()) && !token.contains(char::is_whitespace),
            "call {call}: ChangeToken must satisfy len 1..128 and pattern \\S, got {token:?}"
        );
    }

    // An unused token reports PROVISIONED, however often it is polled.
    let token = tokens[0].clone();
    for poll in 0..3 {
        let (status, body) = waf
            .call("GetChangeTokenStatus", json!({"ChangeToken": token}))
            .await;
        assert_eq!(
            status, 200,
            "poll {poll}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["ChangeTokenStatus"].as_str(),
            Some("PROVISIONED"),
            "poll {poll}: an unused token stays PROVISIONED, got {body}"
        );
    }

    // Using it moves it to PENDING, and the clock moves it to INSYNC on its own.
    let (status, body) = waf
        .call("CreateIPSet", json!({"Name": "token-states", "ChangeToken": token}))
        .await;
    assert_eq!(status, 200, "CreateIPSet: expected 200, got {status} with {body}");
    let ip_set_id = body["IPSet"]["IPSetId"].as_str().unwrap_or_default().to_string();
    let (_, body) = waf
        .call("GetChangeTokenStatus", json!({"ChangeToken": token}))
        .await;
    assert_eq!(
        body["ChangeTokenStatus"].as_str(),
        Some("PENDING"),
        "a just-used token is PENDING while the change propagates, got {body}"
    );
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let (_, body) = waf
        .call("GetChangeTokenStatus", json!({"ChangeToken": token}))
        .await;
    assert_eq!(
        body["ChangeTokenStatus"].as_str(),
        Some("INSYNC"),
        "propagation completes on the clock, got {body}"
    );

    // A token is single-use: every mutating operation refuses a spent or unknown one.
    let spent_or_unknown = [
        ("a spent token", token.clone()),
        ("a token this service never minted", "00000000-0000-0000-0000-000000000000".to_string()),
    ];
    for (case, candidate) in spent_or_unknown {
        let calls: [(&str, Value); 3] = [
            (
                "CreateIPSet",
                json!({"Name": "refused", "ChangeToken": candidate}),
            ),
            (
                "UpdateIPSet",
                json!({
                    "IPSetId": ip_set_id,
                    "ChangeToken": candidate,
                    "Updates": [{"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV4", "Value": "192.0.2.0/24"}}],
                }),
            ),
            (
                "DeleteIPSet",
                json!({"IPSetId": ip_set_id, "ChangeToken": candidate}),
            ),
        ];
        for (operation, request) in calls {
            let (status, body) = waf.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "WAFStaleDataException",
                "case {case:?} on {operation}: got {body}"
            );
        }
    }

    // None of those refusals changed anything: the IPSet is still there and still empty.
    assert_eq!(
        waf.descriptors(&ip_set_id).await,
        Vec::<(String, String)>::new(),
        "a refused mutating call must not change the IPSet"
    );

    // And an unknown token has no status to report.
    let (status, body) = waf
        .call(
            "GetChangeTokenStatus",
            json!({"ChangeToken": "11111111-1111-1111-1111-111111111111"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "GetChangeTokenStatus on an unknown token: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "WAFNonexistentItemException",
        "GetChangeTokenStatus on an unknown token: got {body}"
    );
}

/// Every operation that names a resource refuses an id no `CreateIPSet` minted, with the
/// error its own model declares for that case.
#[tokio::test]
async fn operations_refuse_an_unknown_ip_set_id() {
    let waf = Waf::new();
    // One live IPSet, so the refusals below are about the id and not about empty state.
    let live = waf.create_ip_set("live").await;
    let token = waf.change_token().await;

    let unknown = "99999999-9999-9999-9999-999999999999";
    let cases: [(&str, Value, &str); 3] = [
        (
            "GetIPSet",
            json!({"IPSetId": unknown}),
            "WAFNonexistentItemException",
        ),
        (
            "DeleteIPSet",
            json!({"IPSetId": unknown, "ChangeToken": token}),
            "WAFNonexistentItemException",
        ),
        (
            // The IPSet is the CONTAINER the addresses go into, which is the case
            // WAFNonexistentContainerException is modelled for.
            "UpdateIPSet",
            json!({
                "IPSetId": unknown,
                "ChangeToken": token,
                "Updates": [{"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV4", "Value": "192.0.2.0/24"}}],
            }),
            "WAFNonexistentContainerException",
        ),
    ];

    for (operation, request, expected) in cases {
        let (status, body) = waf.call(operation, request).await;
        assert_eq!(
            status, 400,
            "{operation} on an unknown IPSetId: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "{operation} on an unknown IPSetId: got {body}"
        );
    }

    // The live IPSet is untouched by all of that.
    let (status, body) = waf.call("GetIPSet", json!({"IPSetId": live})).await;
    assert_eq!(status, 200, "GetIPSet on the live IPSet: got {status} with {body}");
    assert_eq!(
        body["IPSet"]["Name"].as_str(),
        Some("live"),
        "GetIPSet must report the stored Name, got {body}"
    );
}

/// Two `IPSet`s may share a `Name`; they are distinguished by their generated ids, and
/// each one's descriptors are its own.
#[tokio::test]
async fn ip_sets_are_keyed_by_generated_id_not_by_name() {
    let waf = Waf::new();

    let first = waf.create_ip_set("same-name").await;
    let second = waf.create_ip_set("same-name").await;
    assert_ne!(
        first, second,
        "two CreateIPSet calls must mint distinct IPSetIds, got {first} and {second}"
    );

    let (status, body) = waf.update_one(&first, "INSERT", "IPV4", "203.0.113.0/24").await;
    assert_eq!(status, 200, "INSERT into the first IPSet: got {status} with {body}");

    assert_eq!(
        waf.descriptors(&first).await,
        vec![("IPV4".to_string(), "203.0.113.0/24".to_string())],
        "the first IPSet holds its own descriptor"
    );
    assert_eq!(
        waf.descriptors(&second).await,
        Vec::<(String, String)>::new(),
        "the second IPSet is unaffected by an update to the first"
    );
}

/// The two quotas that bound this service's stores are enforced at their published value
/// and one past it, with the modelled `WAFLimitsExceededException`, and a refused request
/// leaves both the store and the caller's change token as they were.
///
/// The values are written here as the literals AWS publishes, and checked against the
/// service's own constants, so the test fails if the bound the emulator enforces and the
/// published quota ever disagree.
#[tokio::test]
async fn the_published_quotas_bound_the_stores() {
    /// AWS WAF Classic's default per-account quota for conditions of one type.
    const MAX_IP_SETS_PER_ACCOUNT: usize = 100;
    assert_eq!(
        MAX_IP_SETS_PER_ACCOUNT,
        service_com_amazonaws_waf_awswaf_20150824::state::MAX_IP_SETS_PER_ACCOUNT,
        "the service's IPSet-per-account bound must be the published quota"
    );

    let waf = Waf::new();

    // At the bound: every one of the quota's worth of IPSets is created.
    let mut created = Vec::new();
    for index in 0..MAX_IP_SETS_PER_ACCOUNT {
        created.push(waf.create_ip_set(&format!("quota-{index}")).await);
    }

    // One past it: refused, with the token left usable and nothing stored.
    let token = waf.change_token().await;
    let (status, body) = waf
        .call(
            "CreateIPSet",
            json!({"Name": "one-too-many", "ChangeToken": token}),
        )
        .await;
    assert_eq!(
        status, 400,
        "CreateIPSet past the account quota: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "WAFLimitsExceededException",
        "CreateIPSet past the account quota: got {body}"
    );

    // Paging the whole list back yields exactly the quota's worth, and no more.
    let mut listed = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(marker) = &marker {
            request["NextMarker"] = json!(marker);
        }
        let (status, body) = waf.call("ListIPSets", request).await;
        assert_eq!(status, 200, "ListIPSets: got {status} with {body}");
        for summary in body["IPSets"].as_array().cloned().unwrap_or_default() {
            listed.push(summary["IPSetId"].as_str().unwrap_or_default().to_string());
        }
        match body["NextMarker"].as_str() {
            Some(next) => marker = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        listed, created,
        "the store must hold exactly the quota's worth of IPSets, in creation order"
    );

    // The token the refused CreateIPSet carried is still usable — it is what frees a slot.
    let (status, body) = waf
        .call(
            "DeleteIPSet",
            json!({"IPSetId": created[0], "ChangeToken": token}),
        )
        .await;
    assert_eq!(
        status, 200,
        "the token of the refused CreateIPSet must still be usable: got {status} with {body}"
    );

    // And freeing one makes room again.
    let refill_token = waf.change_token().await;
    assert_ne!(
        refill_token, token,
        "a spent token must not be handed out again, got {refill_token}"
    );
    let (status, body) = waf
        .call(
            "CreateIPSet",
            json!({"Name": "refill", "ChangeToken": refill_token}),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateIPSet after a slot was freed: got {status} with {body}"
    );
}

/// The per-IPSet descriptor quota and the per-request update quota, each at its published
/// value and one past it.
#[tokio::test]
async fn the_descriptor_quotas_bound_one_ip_set() {
    /// AWS WAF Classic's documented quota of IP address ranges in one IP match condition.
    const MAX_DESCRIPTORS_PER_IP_SET: usize = 10_000;
    /// The documented per-request maximum: "You can insert a maximum of 1000 addresses in
    /// a single request" (smithy_query doc, shape="UpdateIPSet").
    const MAX_UPDATES_PER_REQUEST: usize = 1000;

    /// A distinct, valid IPv4 /32 descriptor for every index below 2^24.
    fn descriptor(index: usize) -> Value {
        let value = format!(
            "10.{}.{}.{}/32",
            (index >> 16) & 0xff,
            (index >> 8) & 0xff,
            index & 0xff
        );
        json!({"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV4", "Value": value}})
    }

    let waf = Waf::new();

    assert_eq!(
        MAX_DESCRIPTORS_PER_IP_SET,
        service_com_amazonaws_waf_awswaf_20150824::state::MAX_DESCRIPTORS_PER_IP_SET,
        "the service's descriptor-per-IPSet bound must be the published quota"
    );

    // The per-request bound: 1000 updates are accepted, 1001 are refused.
    let batched = waf.create_ip_set("per-request-quota").await;
    let token = waf.change_token().await;
    let too_many: Vec<Value> = (0..=MAX_UPDATES_PER_REQUEST).map(descriptor).collect();
    let (status, body) = waf
        .call(
            "UpdateIPSet",
            json!({"IPSetId": batched, "ChangeToken": token, "Updates": too_many}),
        )
        .await;
    assert_eq!(
        status, 400,
        "{} updates in one request: expected 400, got {status} with {body}",
        MAX_UPDATES_PER_REQUEST + 1
    );
    assert_eq!(
        error_code(&body),
        "WAFLimitsExceededException",
        "one update past the per-request maximum: got {body}"
    );
    assert_eq!(
        waf.descriptors(&batched).await.len(),
        0,
        "a refused oversized request must store nothing"
    );

    // Filling the IPSet to its quota, in requests of the per-request maximum.
    let full = waf.create_ip_set("descriptor-quota").await;
    for batch in 0..(MAX_DESCRIPTORS_PER_IP_SET / MAX_UPDATES_PER_REQUEST) {
        let first = batch * MAX_UPDATES_PER_REQUEST;
        let updates: Vec<Value> = (first..first + MAX_UPDATES_PER_REQUEST)
            .map(descriptor)
            .collect();
        let token = waf.change_token().await;
        let (status, body) = waf
            .call(
                "UpdateIPSet",
                json!({"IPSetId": full, "ChangeToken": token, "Updates": updates}),
            )
            .await;
        assert_eq!(
            status, 200,
            "batch {batch} up to the descriptor quota: expected 200, got {status} with {body}"
        );
    }
    assert_eq!(
        waf.descriptors(&full).await.len(),
        MAX_DESCRIPTORS_PER_IP_SET,
        "the IPSet must hold exactly its quota of descriptors"
    );

    // One past the quota is refused, and the IPSet keeps exactly its quota.
    let token = waf.change_token().await;
    let (status, body) = waf
        .call(
            "UpdateIPSet",
            json!({
                "IPSetId": full,
                "ChangeToken": token,
                "Updates": [descriptor(MAX_DESCRIPTORS_PER_IP_SET)],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "one descriptor past the quota: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "WAFLimitsExceededException",
        "one descriptor past the quota: got {body}"
    );
    assert_eq!(
        waf.descriptors(&full).await.len(),
        MAX_DESCRIPTORS_PER_IP_SET,
        "a refused update must not grow the IPSet past its quota"
    );
}

/// Which call ends the reissue of a change token: every operation the `GetChangeToken`
/// documentation calls a "create, update, or delete request" does, and nothing else.
///
/// The rows are the three mutating operations of this slice, so the rule is asserted over
/// the class and not over the one operation the issue wrote down; the control rows are a
/// reporting call and a refused mutating call, neither of which spends the token.
#[tokio::test]
async fn only_a_successful_mutating_call_spends_a_change_token() {
    let waf = Waf::new();
    // One live, empty IPSet for the DELETE row, and one the UPDATE row writes into.
    let updated = waf.create_ip_set("token-spender-update").await;
    let deleted = waf.create_ip_set("token-spender-delete").await;

    // (case, operation, request built from the token, spends the token)
    type Request = fn(&str, &str, &str) -> Value;
    let rows: [(&str, &str, Request, bool); 5] = [
        (
            "a reporting call",
            "GetChangeTokenStatus",
            |token, _updated, _deleted| json!({"ChangeToken": token}),
            false,
        ),
        (
            "a refused mutating call",
            "UpdateIPSet",
            |token, _updated, _deleted| {
                json!({
                    "IPSetId": "99999999-9999-9999-9999-999999999999",
                    "ChangeToken": token,
                    "Updates": [{"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV4", "Value": "192.0.2.0/24"}}],
                })
            },
            false,
        ),
        (
            "a create request",
            "CreateIPSet",
            |token, _updated, _deleted| json!({"Name": "token-spender-create", "ChangeToken": token}),
            true,
        ),
        (
            "an update request",
            "UpdateIPSet",
            |token, updated, _deleted| {
                json!({
                    "IPSetId": updated,
                    "ChangeToken": token,
                    "Updates": [{"Action": "INSERT", "IPSetDescriptor": {"Type": "IPV6", "Value": "2001:db8::/32"}}],
                })
            },
            true,
        ),
        (
            "a delete request",
            "DeleteIPSet",
            |token, _updated, deleted| json!({"IPSetId": deleted, "ChangeToken": token}),
            true,
        ),
    ];

    for (case, operation, request, spends) in rows {
        let before = waf.change_token().await;
        let (status, body) = waf
            .call(operation, request(&before, &updated, &deleted))
            .await;
        let after = waf.change_token().await;
        if spends {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the mutating call to succeed, got {status} with {body}"
            );
            assert_ne!(
                after, before,
                "case {case:?}: a spent token must be replaced, got {after} twice"
            );
        } else {
            assert_eq!(
                after, before,
                "case {case:?}: an unspent token must be reissued, got {after} after {before}"
            );
        }
        let (_, status_body) = waf
            .call("GetChangeTokenStatus", json!({"ChangeToken": after}))
            .await;
        assert_eq!(
            status_body["ChangeTokenStatus"].as_str(),
            Some("PROVISIONED"),
            "case {case:?}: the reissued token must be PROVISIONED, got {status_body}"
        );
    }
}
