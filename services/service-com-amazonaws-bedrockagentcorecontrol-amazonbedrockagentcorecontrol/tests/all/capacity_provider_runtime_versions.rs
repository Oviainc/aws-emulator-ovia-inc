// SPDX-License-Identifier: CC0-1.0

//! `ListAgentRuntimeVersionsByCapacityProvider` over HTTP.
//!
//! Two rules are under test:
//!
//! 1. The listing reports exactly the stored agent-runtime VERSIONS associated with the
//!    capacity provider the path names — the association being the
//!    `capacityProviderConfiguration` each version was created or updated with
//!    (`smithy_query shape=CapacityProviderConfiguration`) — paginated on `maxResults`
//!    and the opaque `nextToken` the model declares (`smithy_query
//!    operation=ListAgentRuntimeVersionsByCapacityProvider`, `paginated`).
//! 2. A `capacityProviderId` that names no capacity provider of this account is DENIED:
//!    HTTP 403 `AccessDeniedException`. The operation models `AccessDeniedException`
//!    (403) and NO not-found error, and the recorded service answers 403 for an id that
//!    names nothing — so the decision is an authorization one and is taken BEFORE the
//!    request's input constraints are validated, which is why an id outside the member's
//!    `@pattern` is denied with 403 rather than refused with 400. The denial's MESSAGE is
//!    the one sentence the recorded service answers with, carrying none of the caller's
//!    values.
//!
//! The rows differ along what each rule generalises over: how many versions a provider
//! holds, which provider a version names, whether a version names one at all, the page
//! size, and the SHAPE of the denied id (inside the member's format, outside its pattern,
//! and at each published length bound and past it).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request here is signed for, and the Region its ARNs carry.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// A role ARN inside `RoleArn`'s pattern. IAM is not mounted here, so nothing is known
/// about it and the create is not refused on an assumption (see
/// `control_plane_preconditions.rs`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AgentCoreRole";

/// The settle span `src/state.rs` gives the `CREATING`/`UPDATING` -> `READY` transition,
/// plus slack.
const PAST_SETTLE: Duration = Duration::from_millis(900);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `CapacityProviderArn`'s form:
/// `arn:aws:bedrock-agentcore:{region}:{account}:capacity-provider/{capacityProviderId}`
/// (`smithy_query shape=CapacityProviderConfiguration`).
fn capacity_provider_arn(id: &str) -> String {
    format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:capacity-provider/{id}")
}

/// A `CreateAgentRuntime` body, optionally configured with a capacity provider.
fn runtime_body(name: &str, capacity_provider_id: Option<&str>) -> Value {
    let mut body = json!({
        "agentRuntimeName": name,
        "agentRuntimeArtifact": {"containerConfiguration": {
            "containerUri": "public.ecr.aws/agentcore/demo:v1"
        }},
        "networkConfiguration": {"networkMode": "PUBLIC"},
        "roleArn": ROLE_ARN,
    });
    if let Some(id) = capacity_provider_id {
        body["capacityProviderConfiguration"] =
            json!({"capacityProviderArn": capacity_provider_arn(id)});
    }
    body
}

/// Create a runtime and answer with its `agentRuntimeId` and `agentRuntimeArn`.
async fn create_runtime(
    router: &MultiServiceRouter,
    name: &str,
    capacity_provider_id: Option<&str>,
) -> (String, String) {
    let (status, created) = send(
        router,
        "PUT",
        "/runtimes/",
        Some(&runtime_body(name, capacity_provider_id)),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup {name:?}: CreateAgentRuntime must succeed, got {status} ({created})"
    );
    (
        text(&created, "agentRuntimeId"),
        text(&created, "agentRuntimeArn"),
    )
}

async fn list_by_provider(
    router: &MultiServiceRouter,
    capacity_provider_id: &str,
    query: &str,
) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/capacity-providers/{capacity_provider_id}/runtime-versions{query}"),
        None,
    )
    .await
}

/// The `(agentRuntimeArn, agentRuntimeVersion)` pairs a page reports, in order.
fn listed(body: &Value) -> Vec<(String, String)> {
    body["agentRuntimes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                text(entry, "agentRuntimeArn"),
                text(entry, "agentRuntimeVersion"),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_listing_reports_exactly_the_versions_associated_with_the_provider() {
    let router = router();

    // Two providers, so a version of one is never reported for the other, plus a runtime
    // configured with no provider at all, which belongs to neither.
    let first = "instances1-abcdefghij";
    let second = "instances2-abcdefghij";
    let (first_id, first_arn) = create_runtime(&router, "first_runtime", Some(first)).await;
    let (_, second_arn) = create_runtime(&router, "second_runtime", Some(second)).await;
    create_runtime(&router, "no_provider_runtime", None).await;

    // A second version of the first runtime, still on the first provider: a listing is of
    // VERSIONS, so both must be reported.
    let (status, updated) = send(
        &router,
        "PUT",
        &format!("/runtimes/{first_id}/"),
        Some(&json!({
            "agentRuntimeArtifact": {"containerConfiguration": {
                "containerUri": "public.ecr.aws/agentcore/demo:v2"
            }},
            "roleArn": ROLE_ARN,
            "capacityProviderConfiguration": {
                "capacityProviderArn": capacity_provider_arn(first)
            },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: UpdateAgentRuntime must publish a second version, got {status} ({updated})"
    );

    // (case, provider, expected rows)
    type Row = (&'static str, &'static str, Vec<(String, String)>);
    let rows: [Row; 2] = [
        (
            "a provider two versions of one runtime name",
            first,
            vec![
                (first_arn.clone(), "1".to_string()),
                (first_arn.clone(), "2".to_string()),
            ],
        ),
        (
            "a provider one version of another runtime names",
            second,
            vec![(second_arn.clone(), "1".to_string())],
        ),
    ];

    for (case, provider, expected) in rows {
        let (status, body) = list_by_provider(&router, provider, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} ({body})"
        );
        assert_eq!(
            listed(&body),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {:?} ({body})",
            listed(&body)
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?}: a page that reported everything hands back no token, got {body}"
        );
        for entry in body["agentRuntimes"].as_array().cloned().unwrap_or_default() {
            let status = text(&entry, "status");
            // Each version is still in the transient status its own write started:
            // `CREATING` for a created version, `UPDATING` for one an update published.
            assert!(
                status == "CREATING" || status == "UPDATING",
                "case {case:?}: a version just written is still in its transient status, got \
                 {status:?}"
            );
        }
    }

    // The clock-owed transition is reported by this listing too: the same rows, now READY.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, settled) = list_by_provider(&router, first, "").await;
    assert_eq!(
        status, 200,
        "case \"a settled provider listing\": expected HTTP 200, got {status} ({settled})"
    );
    for entry in settled["agentRuntimes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        assert_eq!(
            text(&entry, "status"),
            "READY",
            "case \"a settled provider listing\": expected READY, got {} ({settled})",
            text(&entry, "status")
        );
    }

    // Pagination: one row per page, resuming strictly after the row the token named.
    let (status, page) = list_by_provider(&router, first, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case \"maxResults=1\": expected HTTP 200, got {status} ({page})"
    );
    assert_eq!(
        listed(&page),
        vec![(first_arn.clone(), "1".to_string())],
        "case \"maxResults=1\": expected the first version only, got {:?} ({page})",
        listed(&page)
    );
    let token = text(&page, "nextToken");
    assert!(
        !token.is_empty(),
        "case \"maxResults=1\": a page with rows left over hands back a token, got {page}"
    );
    let (status, second_page) =
        list_by_provider(&router, first, &format!("?maxResults=1&nextToken={token}")).await;
    assert_eq!(
        status, 200,
        "case \"the second page\": expected HTTP 200, got {status} ({second_page})"
    );
    assert_eq!(
        listed(&second_page),
        vec![(first_arn, "2".to_string())],
        "case \"the second page\": expected the second version only, got {:?} ({second_page})",
        listed(&second_page)
    );
    assert!(
        second_page.get("nextToken").is_none(),
        "case \"the second page\": nothing is left over, so no token, got {second_page}"
    );

    // A token this service never issued is a refusal on that member, not a page.
    let (status, refused) = list_by_provider(&router, first, "?nextToken=notatoken").await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"a nextToken this service never issued\": expected HTTP 400 ValidationException, \
         got {status} {} ({refused})",
        error_code(&refused)
    );
}

/// An id naming no capacity provider of this account is denied, whether or not it is
/// inside the member's modelled pattern: the authorization decision precedes the
/// constraint walk, and the denial is worded the way the recorded service words this
/// operation's denial, with none of the caller's values in it.
#[tokio::test]
async fn an_unknown_capacity_provider_is_denied() {
    let router = router();
    let known = "instances1-abcdefghij";
    create_runtime(&router, "provider_runtime", Some(known)).await;

    /// The sentence the recorded service answers a denied
    /// `ListAgentRuntimeVersionsByCapacityProvider` with.
    const DENIAL: &str = "Request denied: capacityProviderId does not match the required format \
                          with an explicit deny in a resource-based policy";

    // (case, capacityProviderId) — the rows differ along the SHAPE of the id, read off
    // `CapacityProviderId`'s modelled format (`@length` 12..59 and `@pattern`
    // `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$`, `smithy_query
    // operation=ListAgentRuntimeVersionsByCapacityProvider`): inside that format, outside
    // its pattern, shorter than its minimum, at each published length bound, and one
    // character past the upper one. All of them name nothing.
    let rows: [(&str, &str); 6] = [
        (
            "an id inside the member's pattern that names nothing",
            "instances9-abcdefghij",
        ),
        ("an id shorter than the member's minimum length", "ab-cdefghij"),
        (
            "an id at the member's minimum length, naming nothing",
            "a-bcdefghijk",
        ),
        (
            "an id at the member's maximum length, naming nothing",
            "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuv-wxyzabcdef",
        ),
        (
            "an id one character past the member's maximum length",
            "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvw-wxyzabcdef",
        ),
        (
            "an id outside the member's pattern",
            "nonexistentcp0000000000000000000",
        ),
    ];

    for (case, provider) in rows {
        let (status, body) = list_by_provider(&router, provider, "").await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?} ({provider}): an id naming no capacity provider of this account must \
             be denied 403 AccessDeniedException, got {status} {} ({body})",
            error_code(&body)
        );
        assert_eq!(
            text(&body, "message"),
            DENIAL,
            "case {case:?} ({provider}): expected the denial {DENIAL:?}, got {}",
            text(&body, "message")
        );
        assert!(
            !text(&body, "message").contains(provider),
            "case {case:?}: the denial echoes none of the caller's values, got {}",
            text(&body, "message")
        );
    }

    // The provider that IS associated with a stored version is served, so the denial is
    // the id and not the route.
    let (status, body) = list_by_provider(&router, known, "").await;
    assert_eq!(
        status, 200,
        "case \"the provider a stored version names\": expected HTTP 200, got {status} ({body})"
    );
    assert_eq!(
        listed(&body).len(),
        1,
        "case \"the provider a stored version names\": expected one version, got {body}"
    );
}
