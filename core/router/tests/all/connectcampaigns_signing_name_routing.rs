// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `connect-campaigns` SigV4 signing name.
//!
//! ConnectCampaigns (2021-01-30) and ConnectCampaignsV2 (2024-04-23) are two API
//! versions of one AWS service, and they SHARE everything the router used to resolve a
//! service by: the `aws.auth#sigv4` name `connect-campaigns`, the `arnNamespace`
//! `connect-campaigns`, and the endpoint host family
//! `connect-campaigns.{Region}.{dnsSuffix}`. A service that claims every request
//! carrying its signing name therefore claims the other version's traffic too: the
//! router finds two claimants and refuses the request before dispatch with a code no
//! AWS model declares, which made BOTH versions unreachable.
//!
//! On AWS the two are disambiguated by their restJson1 routes, which are disjoint:
//! every V2 route is `/v2/`-prefixed, every v1 route is unprefixed. Each crate's
//! `can_handle` therefore adds the framework's path-segment conjunct (the rule
//! `emulator_server::can_handle_based_on_route_key_and_path` implements) over its own
//! generated, model-derived `ROUTE_PATH_SEGMENTS`.
//!
//! This suite pins the split with both versions mounted together: every route family of
//! each version reaches exactly that version — answered from its own store when the
//! operation is implemented, or with its own scaffold `NotImplemented` when it is still
//! a stub — and a path neither version models is refused only because nobody claims it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// API versions share. The signature is never checked: the router here runs with
/// request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/connect-campaigns/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A well-formed ARN in this service's namespace, percent-encoded so it occupies the
/// single `{arn}` path label the tagging routes bind.
macro_rules! encoded_arn {
    () => {
        "arn%3Aaws%3Aconnect-campaigns%3Aus-east-1%3A000000000000%3Acampaign%2F000000000000"
    };
}

/// A V2 `CreateCampaign` body: `channelSubtypeConfig` is a member only the V2 model
/// has, and v1's own create requires `dialerConfig`/`outboundCallConfig` instead.
const V2_CREATE: &str = r#"{"name":"v2-campaign","connectInstanceId":"instance-1",
    "channelSubtypeConfig":{"telephony":{"outboundMode":{"agentless":{}},
    "defaultOutboundConfig":{"connectContactFlowId":"flow-1"}}}}"#;

/// A v1 `CreateCampaign` body, with the two members v1 requires.
const V1_CREATE: &str = r#"{"name":"v1-campaign","connectInstanceId":"instance-1",
    "dialerConfig":{"progressiveDialerConfig":{"bandwidthAllocation":0.5}},
    "outboundCallConfig":{"connectContactFlowId":"flow-1"}}"#;

fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &str,
) -> (u16, Option<String>, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://connect-campaigns.us-east-1.amazonaws.com{uri}"))
        .header("host", "connect-campaigns.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_owned()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
    // The error code, read from the body's `__type` discriminator: a status alone
    // cannot tell the router's own refusals — `UnknownService` (nobody claimed the
    // request) and `AmbiguousService` (several did) — from a service's own modeled
    // error.
    let error_code = payload
        .get("__type")
        .or_else(|| payload.get("Code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, error_code, payload)
}

/// What a row expects of the version that claimed it: either an error code only that
/// version's own answer carries, or a member only its output has.
#[derive(Debug)]
enum Expect {
    Error(&'static str),
    Member(&'static str),
}

/// Neither routing refusal may ever be the answer to a legitimately signed request.
const ROUTER_REFUSALS: [&str; 2] = ["AmbiguousService", "UnknownService"];

#[tokio::test]
async fn each_connect_campaigns_api_version_serves_its_own_route_space() {
    let router = build_router();

    // One row per modelled route family of each version (sources: the `smithy_query`
    // digests of the two models — 22 v1 operations over `/campaigns`,
    // `/campaigns-state`, `/campaigns-summary`, `/connect-instance/…`, `/tags/{arn}`;
    // 37 V2 operations over the same families under `/v2/`). Asserting WHAT the
    // response carries, not just its status, is what proves exactly one version
    // claimed it: an implemented operation answers from its own store, a stub answers
    // its own `NotImplemented`, and neither router refusal can produce either.
    let rows: [(&str, &str, &str, &str, u16, Expect); 14] = [
        // ---- V2: `/v2/`-prefixed ----
        (
            "V2 CreateCampaign",
            "PUT",
            "/v2/campaigns",
            V2_CREATE,
            200,
            Expect::Member("id"),
        ),
        (
            "V2 ListCampaigns",
            "POST",
            "/v2/campaigns-summary",
            "{}",
            200,
            Expect::Member("campaignSummaryList"),
        ),
        (
            "V2 DescribeCampaign for an id no store holds",
            "GET",
            "/v2/campaigns/nosuchcampaign",
            "",
            404,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            "V2 GetCampaignStateBatch (still a stub)",
            "POST",
            "/v2/campaigns-state",
            r#"{"campaignIds":["000000000000"]}"#,
            501,
            Expect::Error("NotImplemented"),
        ),
        (
            "V2 StartInstanceOnboardingJob (still a stub)",
            "PUT",
            "/v2/connect-instance/instance-1/onboarding",
            r#"{"encryptionConfig":{"enabled":false}}"#,
            501,
            Expect::Error("NotImplemented"),
        ),
        (
            "V2 PutOutboundRequestBatch (still a stub)",
            "PUT",
            "/v2/campaigns/000000000000/outbound-requests",
            r#"{"outboundRequests":[{"clientToken":"token-1",
                "expirationTime":1767225600,"channelSubtypeParameters":{"telephony":{
                    "destinationPhoneNumber":"+15550100","attributes":{}}}}]}"#,
            501,
            Expect::Error("NotImplemented"),
        ),
        (
            "V2 ListTagsForResource (still a stub)",
            "GET",
            concat!("/v2/tags/", encoded_arn!()),
            "",
            501,
            Expect::Error("NotImplemented"),
        ),
        // ---- v1: unprefixed ----
        (
            "v1 CreateCampaign",
            "PUT",
            "/campaigns",
            V1_CREATE,
            200,
            Expect::Member("id"),
        ),
        (
            "v1 ListCampaigns",
            "POST",
            "/campaigns-summary",
            "{}",
            200,
            Expect::Member("campaignSummaryList"),
        ),
        (
            "v1 DescribeCampaign for an id no store holds",
            "GET",
            "/campaigns/nosuchcampaign",
            "",
            404,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            "v1 GetCampaignStateBatch (still a stub)",
            "POST",
            "/campaigns-state",
            r#"{"campaignIds":["000000000000"]}"#,
            501,
            Expect::Error("NotImplemented"),
        ),
        (
            "v1 StartInstanceOnboardingJob (still a stub)",
            "PUT",
            "/connect-instance/instance-1/onboarding",
            r#"{"encryptionConfig":{"enabled":false}}"#,
            501,
            Expect::Error("NotImplemented"),
        ),
        (
            "v1 PutDialRequestBatch (still a stub)",
            "PUT",
            "/campaigns/000000000000/dial-requests",
            r#"{"dialRequests":[{"clientToken":"token-1","phoneNumber":"+15550100",
                "expirationTime":1767225600,"attributes":{}}]}"#,
            501,
            Expect::Error("NotImplemented"),
        ),
        (
            "v1 ListTagsForResource (still a stub)",
            "GET",
            concat!("/tags/", encoded_arn!()),
            "",
            501,
            Expect::Error("NotImplemented"),
        ),
    ];

    for (case, method, uri, body, expected_status, expected) in rows {
        let (status, error_code, payload) = dispatch(&router, method, uri, body).await;
        for refusal in ROUTER_REFUSALS {
            assert_ne!(
                error_code.as_deref(),
                Some(refusal),
                "case {case:?}: {uri} must be claimed by exactly one API version, got \
                 the router's {refusal}: {status} {payload}"
            );
        }
        match expected {
            Expect::Error(expected_code) => assert_eq!(
                (status, error_code.as_deref()),
                (expected_status, Some(expected_code)),
                "case {case:?}: expected HTTP {expected_status} {expected_code} from the \
                 one version that owns {uri}, got {status} {error_code:?}: {payload}"
            ),
            Expect::Member(member) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected HTTP {expected_status} from the one version \
                     that owns {uri}, got {status} {payload}"
                );
                assert!(
                    payload.get(member).is_some(),
                    "case {case:?}: expected the claiming version's {member} in the \
                     response, got {payload}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_campaign_created_through_one_version_belongs_to_that_version_alone() {
    let router = build_router();

    // Create through V2, with a member only the V2 model carries.
    let (status, error_code, payload) = dispatch(&router, "PUT", "/v2/campaigns", V2_CREATE).await;
    assert_eq!(
        status, 200,
        "V2 CreateCampaign: expected 200, got {status} {error_code:?}: {payload}"
    );
    let v2_id = payload
        .get("id")
        .and_then(Value::as_str)
        .expect("V2 CreateCampaign returns the minted id")
        .to_string();

    // The V2 reader serves it, and reports the V2-only member back.
    let (status, error_code, payload) =
        dispatch(&router, "GET", &format!("/v2/campaigns/{v2_id}"), "").await;
    assert_eq!(
        (status, payload.pointer("/campaign/channelSubtypeConfig").is_some()),
        (200, true),
        "case {:?}: the V2 version must serve its own campaign, got {status} \
         {error_code:?}: {payload}",
        "V2 DescribeCampaign of a V2 campaign"
    );

    // The same id on v1's route reaches v1, whose own store has no such campaign:
    // v1's modeled 404, not a router refusal and not V2's answer.
    let (status, error_code, payload) =
        dispatch(&router, "GET", &format!("/campaigns/{v2_id}"), "").await;
    assert_eq!(
        (status, error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {:?}: expected v1's own 404 for a campaign it never created, got {status} \
         {error_code:?}: {payload}",
        "v1 DescribeCampaign of a V2 campaign id"
    );

    // And symmetrically: a v1 campaign is unknown to V2.
    let (status, error_code, payload) = dispatch(&router, "PUT", "/campaigns", V1_CREATE).await;
    assert_eq!(
        status, 200,
        "v1 CreateCampaign: expected 200, got {status} {error_code:?}: {payload}"
    );
    let v1_id = payload
        .get("id")
        .and_then(Value::as_str)
        .expect("v1 CreateCampaign returns the minted id")
        .to_string();
    let (status, error_code, payload) =
        dispatch(&router, "GET", &format!("/campaigns/{v1_id}"), "").await;
    assert_eq!(
        (status, payload.pointer("/campaign/dialerConfig").is_some()),
        (200, true),
        "case {:?}: the v1 version must serve its own campaign, got {status} \
         {error_code:?}: {payload}",
        "v1 DescribeCampaign of a v1 campaign"
    );
    let (status, error_code, payload) =
        dispatch(&router, "GET", &format!("/v2/campaigns/{v1_id}"), "").await;
    assert_eq!(
        (status, error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {:?}: expected V2's own 404 for a campaign it never created, got {status} \
         {error_code:?}: {payload}",
        "V2 DescribeCampaign of a v1 campaign id"
    );
}

#[tokio::test]
async fn a_path_neither_version_models_is_refused_only_for_want_of_a_claimant() {
    let router = build_router();

    // The claim is exact segment equality, so a near miss of either version's space is
    // claimed by neither — and THAT is the only reason routing may fail.
    for (case, uri) in [
        ("a third API version's prefix", "/v3/campaigns"),
        ("a longer segment the V2 prefix starts", "/v22/campaigns"),
        ("a segment that merely contains a modelled one", "/my-campaigns"),
    ] {
        let (status, error_code, payload) = dispatch(&router, "PUT", uri, V2_CREATE).await;
        assert_ne!(
            error_code.as_deref(),
            Some("AmbiguousService"),
            "case {case:?}: an unmodelled path must not be reported as contended, got \
             {status} {payload}"
        );
        assert_eq!(
            (status, error_code.as_deref()),
            (404, Some("UnknownService")),
            "case {case:?}: expected the router's no-claimant refusal for a path neither \
             version models, got {status} {error_code:?}: {payload}"
        );
    }
}

#[test]
fn the_two_path_spaces_the_claim_rests_on_are_disjoint() {
    // The premise of both `can_handle` seams, read from the generated, model-derived
    // constants: each version publishes the first segments its restJson1 operations
    // bind, and the two sets share nothing — which is what lets the path conjunct
    // separate two services that share a signing name.
    let v1 = service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::ROUTE_PATH_SEGMENTS;
    let v2 = service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ROUTE_PATH_SEGMENTS;
    assert_eq!(
        service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::ROUTE_KEY,
        service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ROUTE_KEY,
        "the two API versions are expected to SHARE one signing name; that is why the \
         path conjunct exists"
    );
    assert!(
        !v1.is_empty() && !v2.is_empty(),
        "each version must publish the first segments its routes bind: v1 {v1:?}, V2 {v2:?}"
    );
    for segment in v2 {
        assert!(
            !v1.contains(segment),
            "the two versions' path spaces must be disjoint, but both claim {segment:?}: \
             v1 {v1:?}, V2 {v2:?}"
        );
    }
    assert!(
        !v1.contains(&emulator_server::ROOT_PATH_SEGMENT)
            && !v2.contains(&emulator_server::ROOT_PATH_SEGMENT),
        "neither restJson1 version binds the root path, so neither may claim it: \
         v1 {v1:?}, V2 {v2:?}"
    );
}
