// SPDX-License-Identifier: CC0-1.0

//! The campaign slice of AmazonConnectCampaignService, end to end over HTTP.
//!
//! Every assertion here drives the real restJson1 wire path: a request is dispatched
//! through a [`MultiServiceRouter`] holding this service, exactly as the production
//! router serves it, so routing, deserialization, the handler, and serialization are all
//! exercised. Nothing reaches into the engine's state directly — a campaign is only ever
//! observed through DescribeCampaign / GetCampaignState / ListCampaigns.
//!
//! The rules under test:
//!
//! * CreateCampaign mints an id and the ARN naming it, and stores every member it was
//!   given, so a read-back returns what was created (whatever dialer config variant,
//!   outbound call config, tag set, name or instance id it was given).
//! * The run state is a state machine: start moves a campaign that has never run to
//!   Running, pause moves a Running one to Paused, stop moves a Running or Paused one to
//!   Stopped, and a request asking a campaign to enter the state it already holds is an
//!   idempotent success that changes nothing; every other request is refused with
//!   `ConflictException` (409) naming the state the campaign is in, and changes nothing.
//! * Every id-taking operation answers `ResourceNotFoundException` (404) for an id the
//!   store does not hold, including after a delete.
//! * ListCampaigns enumerates the stored campaigns in creation order, filtered by
//!   `filters.instanceIdFilter` and cut into `maxResults`-sized pages joined by
//!   `nextToken`.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::EmulatedService;

/// The region every request below is signed for; it is the region the service must
/// stamp into the ARNs it mints.
const REGION: &str = "us-west-2";

/// The canonical account id `engine::emulator_arn` puts in every ARN.
const ACCOUNT: &str = "000000000000";

/// A sigv4 `Authorization` header naming this service's signing name
/// (`connect-campaigns`, see `OPERATIONS.md`) and the region the request is signed for,
/// which is how the router decides this service claims the request and how the service
/// reads the region for the ARNs it mints. Signature verification is off in these tests,
/// so the signature bytes are immaterial.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAEMULATORTEST/20260101/{region}/connect-campaigns/\
aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// One HTTP exchange with the emulator.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The parsed JSON body, or `Value::Null` for an empty one.
    body: Value,
    /// The wire error code (`x-amzn-errortype`), when the answer is an error.
    error_code: Option<String>,
}

impl Answer {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name)
    }

    fn string_member(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }
}

/// Every Amazon Connect instance the cases below name in a `connectInstanceId`.
///
/// `CreateCampaign` mints a campaign only for an instance the account holds, so the
/// router these cases drive starts with each of them in Amazon Connect's own instance
/// registry; a case that needs an id the account does NOT hold names one of its own.
/// The last entry is `InstanceId`'s 256-character maximum, built by [`long_instance`].
const INSTANCES: &[&str] = &[
    "inst-1",
    "inst-2",
    "inst-a",
    "inst-b",
    "inst-full",
    "inst-spare",
    "a.b_c-D9",
];

/// An instance id of exactly 256 characters: `InstanceId`'s length maximum (len 0..256,
/// pattern `^[a-zA-Z0-9_\-.]*$`).
fn long_instance() -> String {
    "i".repeat(256)
}

fn service_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    for instance in INSTANCES {
        super::common::register_connect_instance(&engine, REGION, instance);
    }
    super::common::register_connect_instance(&engine, REGION, &long_instance());
    let mut router = MultiServiceRouter::new();
    // These tests send unsigned-but-scoped requests; the signature itself is the
    // transport's business, not this service's.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Answer {
    call_signed_for(router, Some(REGION), method, path, body).await
}

/// One HTTP exchange, signed for `region` — or unsigned (`None`), which the router then
/// claims by the host-style endpoint instead.
async fn call_signed_for(
    router: &MultiServiceRouter,
    region: Option<&str>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Answer {
    let bytes = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the body")),
        None => bytes::Bytes::new(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://connect-campaigns.{REGION}.amazonaws.com{path}"
        ))
        .header("host", format!("connect-campaigns.{REGION}.amazonaws.com"))
        .header("content-type", "application/json");
    if let Some(region) = region {
        builder = builder.header("authorization", authorization(region));
    }
    let request = builder.body(bytes).expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    Answer {
        status,
        body,
        error_code,
    }
}

/// The wire error code of an answer: the `x-amzn-errortype` header, else the body's
/// `__type` discriminator.
fn error_code(answer: &Answer) -> String {
    answer
        .error_code
        .clone()
        .or_else(|| answer.string_member("__type").map(str::to_string))
        .unwrap_or_default()
}

/// JSON with every number widened to `f64`, so a comparison does not fail only because
/// one side carries `1` where the other carries `1.0`.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Number(number) => Value::from(number.as_f64().unwrap_or_default()),
        Value::Array(items) => Value::Array(items.iter().map(normalized).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, member)| (key.clone(), normalized(member)))
                .collect(),
        ),
        other => other.clone(),
    }
}

async fn create_campaign(router: &MultiServiceRouter, request: &Value) -> Answer {
    call(router, "PUT", "/campaigns", Some(request)).await
}

/// Create a campaign and return its id, failing the test if the create did not succeed.
async fn created_id(router: &MultiServiceRouter, request: &Value) -> String {
    let answer = create_campaign(router, request).await;
    assert_eq!(
        answer.status, 200,
        "create {request}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer
        .string_member("id")
        .unwrap_or_else(|| panic!("create {request}: no id in {:?}", answer.body))
        .to_string()
}

async fn campaign_state(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/campaigns/{id}/state"), None).await
}

async fn lifecycle_call(router: &MultiServiceRouter, id: &str, action: &str) -> Answer {
    call(router, "POST", &format!("/campaigns/{id}/{action}"), None).await
}

/// A minimal valid CreateCampaign request body for `instance`.
fn simple_request(name: &str, instance: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": instance,
        "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.5}},
        "outboundCallConfig": {"connectContactFlowId": "flow-1"},
    })
}

/// The CreateCampaign bodies the round-trip test spans: one row per shape of input the
/// rule generalises over — each dialer-config variant of the modelled `DialerConfig`
/// union, an outbound call config with and without its optional members, a tagged and an
/// untagged campaign, and the `name`/`connectInstanceId` length bounds the model
/// declares (`name` 1..127, `connectInstanceId` 0..256). Every instance named here is
/// one the router's account holds (see [`INSTANCES`]).
fn round_trip_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "progressive dialer, minimal outbound call config, no tags",
            json!({
                "name": "a",
                "connectInstanceId": "inst-1",
                "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.5}},
                "outboundCallConfig": {"connectContactFlowId": "flow-1"},
            }),
        ),
        (
            "predictive dialer, full outbound call config, two tags",
            json!({
                "name": "demo-campaign",
                "connectInstanceId": "a.b_c-D9",
                "dialerConfig": {
                    "predictiveDialerConfig": {
                        "bandwidthAllocation": 0.75,
                        "dialingCapacity": 0.5,
                    }
                },
                "outboundCallConfig": {
                    "connectContactFlowId": "flow-2",
                    "connectSourcePhoneNumber": "+15550100",
                    "connectQueueId": "queue-7",
                    "answerMachineDetectionConfig": {
                        "enableAnswerMachineDetection": true,
                        "awaitAnswerMachinePrompt": false,
                    },
                },
                "tags": {"owner": "ops", "tier": "gold"},
            }),
        ),
        (
            "agentless dialer with no members, one tag",
            json!({
                "name": "agentless",
                "connectInstanceId": "inst-2",
                "dialerConfig": {"agentlessDialerConfig": {}},
                "outboundCallConfig": {"connectContactFlowId": "flow-3"},
                "tags": {"owner": "ops"},
            }),
        ),
        (
            "name at its 127-character maximum, connectInstanceId at its 256-character \
             maximum",
            json!({
                "name": "n".repeat(127),
                "connectInstanceId": long_instance(),
                "dialerConfig": {
                    "agentlessDialerConfig": {"dialingCapacity": 0.25}
                },
                "outboundCallConfig": {"connectContactFlowId": "flow-4"},
            }),
        ),
    ]
}

#[tokio::test]
async fn a_created_campaign_reads_back_exactly_as_created() {
    let router = service_router();

    for (case, request) in round_trip_rows() {
        let created = create_campaign(&router, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateCampaign to answer 200, got {} {:?}",
            created.status, created.body
        );

        let id = created
            .string_member("id")
            .unwrap_or_else(|| panic!("case {case:?}: no id in {:?}", created.body))
            .to_string();
        assert!(
            !id.is_empty() && !id.chars().any(char::is_whitespace),
            "case {case:?}: expected a non-empty id with no whitespace (CampaignId pattern \
             ^[\\S]*$), got {id:?}"
        );

        let arn = created
            .string_member("arn")
            .unwrap_or_else(|| panic!("case {case:?}: no arn in {:?}", created.body))
            .to_string();
        assert_eq!(
            arn,
            format!("arn:aws:connect-campaigns:{REGION}:{ACCOUNT}:campaign/{id}"),
            "case {case:?}: expected the ARN to name this campaign in the signed region, \
             got {arn:?}"
        );
        assert!(
            (20..=500).contains(&arn.len()),
            "case {case:?}: CampaignArn is 20..500 characters, got {} in {arn:?}",
            arn.len()
        );

        // The tags CreateCampaign answers with are the ones it stored.
        let requested_tags = request.get("tags").cloned();
        assert_eq!(
            created.member("tags").cloned(),
            requested_tags.clone(),
            "case {case:?}: expected CreateCampaign to answer the tags it stored"
        );

        let described = call(&router, "GET", &format!("/campaigns/{id}"), None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeCampaign to answer 200, got {} {:?}",
            described.status, described.body
        );
        let campaign = described
            .member("campaign")
            .unwrap_or_else(|| panic!("case {case:?}: no campaign in {:?}", described.body));

        assert_eq!(
            campaign.get("id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: expected the described campaign to carry its own id, got {campaign:?}"
        );
        assert_eq!(
            campaign.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the described campaign to carry the minted ARN, \
             got {campaign:?}"
        );
        for member in ["name", "connectInstanceId"] {
            assert_eq!(
                campaign.get(member),
                request.get(member),
                "case {case:?}: expected {member} to read back as created, got {campaign:?}"
            );
        }
        for member in ["dialerConfig", "outboundCallConfig"] {
            assert_eq!(
                campaign.get(member).map(normalized),
                request.get(member).map(normalized),
                "case {case:?}: expected {member} to read back as created, got {campaign:?}"
            );
        }
        assert_eq!(
            campaign.get("tags").cloned(),
            requested_tags,
            "case {case:?}: expected the tags to read back as created, got {campaign:?}"
        );

        let state = campaign_state(&router, &id).await;
        assert_eq!(
            state.status, 200,
            "case {case:?}: expected GetCampaignState to answer 200, got {} {:?}",
            state.status, state.body
        );
        assert_eq!(
            state.string_member("state"),
            Some("Initialized"),
            "case {case:?}: expected a campaign nobody has started to be Initialized, \
             got {:?}",
            state.body
        );
    }
}

#[tokio::test]
async fn each_create_mints_its_own_id_and_arn() {
    let router = service_router();
    let request = simple_request("twice", "inst-1");

    let mut ids = BTreeSet::new();
    let mut arns = BTreeSet::new();
    for attempt in 0..3 {
        let created = create_campaign(&router, &request).await;
        assert_eq!(
            created.status, 200,
            "attempt {attempt}: expected 200, got {} {:?}",
            created.status, created.body
        );
        ids.insert(created.string_member("id").unwrap_or_default().to_string());
        arns.insert(created.string_member("arn").unwrap_or_default().to_string());
    }

    assert_eq!(
        ids.len(),
        3,
        "expected three distinct campaign ids for three creates, got {ids:?}"
    );
    assert_eq!(
        arns.len(),
        3,
        "expected three distinct campaign ARNs for three creates, got {arns:?}"
    );
}

/// Drive a fresh campaign into `target` through the lifecycle operations, and return its
/// id. Only the paths the state machine allows are used, so the setup itself asserts
/// the allowed transitions.
async fn campaign_in_state(router: &MultiServiceRouter, target: &str) -> String {
    let id = created_id(router, &simple_request("transition", "inst-1")).await;
    let path: &[&str] = match target {
        "Initialized" => &[],
        "Running" => &["start"],
        "Paused" => &["start", "pause"],
        "Stopped" => &["start", "stop"],
        other => panic!("no lifecycle path to {other}"),
    };

    for action in path {
        let answer = lifecycle_call(router, &id, action).await;
        assert_eq!(
            answer.status, 200,
            "driving to {target}: expected {action} to answer 200, got {} {:?}",
            answer.status, answer.body
        );
    }
    let state = campaign_state(router, &id).await;
    assert_eq!(
        state.string_member("state"),
        Some(target),
        "driving to {target}: campaign ended in {:?}",
        state.body
    );
    id
}

#[tokio::test]
async fn the_run_state_machine_allows_only_its_own_transitions() {
    // Every (state, operation) pair of the run-state machine: the state the campaign is
    // in, the lifecycle operation called on it, and the state it must be in afterwards —
    // `None` meaning the request is refused with ConflictException and the campaign does
    // not move. A pair whose operation asks for the state the campaign already holds is a
    // success that leaves it there, which is why `Running`/start, `Paused`/pause and
    // `Stopped`/stop name their own state rather than `None`.
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("Initialized", "start", Some("Running")),
        ("Initialized", "pause", None),
        ("Initialized", "stop", None),
        ("Running", "start", Some("Running")),
        ("Running", "pause", Some("Paused")),
        ("Running", "stop", Some("Stopped")),
        ("Paused", "start", None),
        ("Paused", "pause", Some("Paused")),
        ("Paused", "stop", Some("Stopped")),
        ("Stopped", "start", None),
        ("Stopped", "pause", None),
        ("Stopped", "stop", Some("Stopped")),
    ];

    let router = service_router();

    for (from, action, expected) in rows {
        let case = format!("{action} a {from} campaign");
        let id = campaign_in_state(&router, from).await;
        let answer = lifecycle_call(&router, &id, action).await;

        match expected {
            Some(to) => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected 200, got {} {:?}",
                    answer.status, answer.body
                );
                let state = campaign_state(&router, &id).await;
                assert_eq!(
                    state.string_member("state"),
                    Some(*to),
                    "case {case:?}: expected the campaign to be {to}, got {:?}",
                    state.body
                );
            }
            None => {
                assert_eq!(
                    answer.status, 409,
                    "case {case:?}: expected 409, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    error_code(&answer),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {:?}",
                    answer.body
                );
                let expected_message =
                    format!("Campaign: {id} is in invalid state: {}", from.to_uppercase());
                assert_eq!(
                    answer.string_member("message"),
                    Some(expected_message.as_str()),
                    "case {case:?}: expected the refusal to name the campaign and the \
                     upper-case state it is in, got {:?}",
                    answer.body
                );
                let state = campaign_state(&router, &id).await;
                assert_eq!(
                    state.string_member("state"),
                    Some(*from),
                    "case {case:?}: a refused transition must not move the campaign, \
                     got {:?}",
                    state.body
                );
            }
        }
    }
}

#[tokio::test]
async fn an_unknown_campaign_id_is_a_404_on_every_operation_that_takes_one() {
    // Every operation of this slice that names a campaign by id, as `method path`.
    let rows: &[(&str, &str)] = &[
        ("GET", "/campaigns/{id}"),
        ("GET", "/campaigns/{id}/state"),
        ("POST", "/campaigns/{id}/start"),
        ("POST", "/campaigns/{id}/pause"),
        ("POST", "/campaigns/{id}/stop"),
        ("DELETE", "/campaigns/{id}"),
    ];
    let router = service_router();
    // A well-formed id (CampaignId pattern ^[\S]*$) that was never created.
    let unknown = "11111111-2222-4333-8444-555555555555";

    for (method, template) in rows {
        let path = template.replace("{id}", unknown);
        let case = format!("{method} {path}");
        let answer = call(&router, method, &path, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_deleted_campaign_is_gone_from_every_reader() {
    let router = service_router();
    let kept = created_id(&router, &simple_request("kept", "inst-1")).await;
    let doomed = created_id(&router, &simple_request("doomed", "inst-1")).await;

    let deleted = call(&router, "DELETE", &format!("/campaigns/{doomed}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteCampaign to answer 200, got {} {:?}",
        deleted.status, deleted.body
    );

    for (case, method, path) in [
        ("describe", "GET", format!("/campaigns/{doomed}")),
        ("get state", "GET", format!("/campaigns/{doomed}/state")),
        ("delete again", "DELETE", format!("/campaigns/{doomed}")),
    ] {
        let answer = call(&router, method, &path, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 after the delete, got {} {:?}",
            answer.status, answer.body
        );
    }

    let listed = list_ids(&router, None, None, None).await;
    assert_eq!(
        listed,
        vec![kept.clone()],
        "expected ListCampaigns to list only the campaign that was not deleted"
    );
}

/// The ids ListCampaigns reports, walking every page.
async fn list_ids(
    router: &MultiServiceRouter,
    instance: Option<&str>,
    max_results: Option<i64>,
    expected_pages: Option<usize>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;

    loop {
        let mut request = serde_json::Map::new();
        if let Some(instance) = instance {
            request.insert(
                "filters".to_string(),
                json!({"instanceIdFilter": {"value": instance, "operator": "Eq"}}),
            );
        }
        if let Some(max_results) = max_results {
            request.insert("maxResults".to_string(), json!(max_results));
        }
        if let Some(token) = &next_token {
            request.insert("nextToken".to_string(), json!(token));
        }

        let answer = call(
            router,
            "POST",
            "/campaigns-summary",
            Some(&Value::Object(request.clone())),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "list {request:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        pages += 1;

        let page = answer
            .member("campaignSummaryList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("list {request:?}: no campaignSummaryList in {:?}", answer.body))
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                "list {request:?}: a page may not exceed maxResults={max_results}, got {}",
                page.len()
            );
        }
        for summary in &page {
            // Every CampaignSummary member the model marks required must be present.
            for member in ["id", "arn", "name", "connectInstanceId"] {
                assert!(
                    summary.get(member).and_then(Value::as_str).is_some(),
                    "list {request:?}: CampaignSummary.{member} is required, got {summary:?}"
                );
            }
            ids.push(
                summary
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }

        next_token = answer.string_member("nextToken").map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            pages < 200,
            "list {request:?}: pagination did not terminate after {pages} pages"
        );
    }

    if let Some(expected_pages) = expected_pages {
        assert_eq!(
            pages, expected_pages,
            "expected ListCampaigns to answer in {expected_pages} pages, got {pages}"
        );
    }
    ids
}

#[tokio::test]
async fn list_campaigns_filters_by_instance_and_pages_in_creation_order() {
    let router = service_router();

    // Three campaigns on one instance and two on another, created in this order.
    let mut created_in_order = Vec::new();
    let mut on_instance_a = Vec::new();
    let mut on_instance_b = Vec::new();
    for (index, instance) in ["inst-a", "inst-b", "inst-a", "inst-a", "inst-b"]
        .into_iter()
        .enumerate()
    {
        let id = created_id(&router, &simple_request(&format!("c{index}"), instance)).await;
        created_in_order.push(id.clone());
        if instance == "inst-a" {
            on_instance_a.push(id);
        } else {
            on_instance_b.push(id);
        }
    }

    // Page size, and the number of pages five campaigns take at that size.
    let page_rows: &[(Option<i64>, usize)] = &[
        (None, 1),    // no maxResults: one page
        (Some(1), 5), // the smallest page the model allows
        (Some(2), 3),
        (Some(4), 2),
        (Some(5), 1),
        (Some(50), 1), // the largest page the model allows
    ];
    for (max_results, expected_pages) in page_rows {
        let ids = list_ids(&router, None, *max_results, Some(*expected_pages)).await;
        assert_eq!(
            ids, created_in_order,
            "case maxResults={max_results:?}: expected every campaign exactly once in \
             creation order, got {ids:?}"
        );
    }

    // `instanceIdFilter` keeps exactly the campaigns of the named instance.
    let filter_rows: &[(&str, &[String])] = &[
        ("inst-a", &on_instance_a),
        ("inst-b", &on_instance_b),
        ("inst-none", &[]),
    ];
    for (instance, expected) in filter_rows {
        let ids = list_ids(&router, Some(instance), Some(2), None).await;
        assert_eq!(
            ids,
            expected.to_vec(),
            "case instanceIdFilter={instance:?}: expected the campaigns of that instance \
             in creation order, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn list_campaigns_refuses_a_page_request_it_cannot_answer() {
    let router = service_router();
    created_id(&router, &simple_request("only", "inst-1")).await;

    // One row per way a page request can be out of contract: `maxResults` is modelled
    // 1..50, so 0 and 51 are refused at the boundary; a `nextToken` naming no campaign
    // cannot be resumed from; `instanceIdFilter.operator` is an enum whose only value is
    // `Eq`.
    let rows: &[(&str, Value)] = &[
        ("maxResults below the 1 minimum", json!({"maxResults": 0})),
        ("maxResults above the 50 maximum", json!({"maxResults": 51})),
        (
            "a nextToken naming no campaign",
            json!({"nextToken": "not-a-campaign-id"}),
        ),
        (
            "an unmodelled instanceIdFilter operator",
            json!({"filters": {"instanceIdFilter": {"value": "inst-1", "operator": "Ne"}}}),
        ),
    ];

    for (case, request) in rows {
        let answer = call(&router, "POST", "/campaigns-summary", Some(request)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    // Both ends of the modelled range are accepted.
    for max_results in [1, 50] {
        let answer = call(
            &router,
            "POST",
            "/campaigns-summary",
            Some(&json!({"maxResults": max_results})),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={max_results}: expected the modelled bound to be accepted, \
             got {} {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_minted_arn_names_the_region_the_request_was_signed_for() {
    let router = service_router();

    // One row per region a caller can sign for, plus an unsigned request — which carries
    // no credential scope, so the service falls back to its default region.
    let rows: &[(Option<&str>, &str)] = &[
        (Some("us-west-2"), "us-west-2"),
        (Some("eu-central-1"), "eu-central-1"),
        (Some("ap-southeast-2"), "ap-southeast-2"),
        (None, "us-east-1"),
    ];

    for (signed_for, expected_region) in rows {
        let case = format!("signed for {signed_for:?}");
        let answer = call_signed_for(
            &router,
            *signed_for,
            "PUT",
            "/campaigns",
            Some(&simple_request("regional", "inst-1")),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let id = answer.string_member("id").unwrap_or_default().to_string();
        assert_eq!(
            answer.string_member("arn"),
            Some(
                format!("arn:aws:connect-campaigns:{expected_region}:{ACCOUNT}:campaign/{id}")
                    .as_str()
            ),
            "case {case:?}: expected the ARN to name {expected_region}, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn creating_past_the_per_instance_quota_is_refused() {
    let router = service_router();
    let quota = service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::CAMPAIGNS_PER_INSTANCE_QUOTA;

    for index in 0..quota {
        let answer = create_campaign(&router, &simple_request(&format!("c{index}"), "inst-full")).await;
        assert_eq!(
            answer.status, 200,
            "campaign {index} of the quota: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    let over = create_campaign(&router, &simple_request("one-too-many", "inst-full")).await;
    assert_eq!(
        over.status, 402,
        "expected the campaign past the quota to be refused with 402, got {} {:?}",
        over.status, over.body
    );
    assert_eq!(
        error_code(&over),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {:?}",
        over.body
    );

    // The quota is per Amazon Connect instance, so another instance is unaffected.
    let other = create_campaign(&router, &simple_request("elsewhere", "inst-spare")).await;
    assert_eq!(
        other.status, 200,
        "expected another instance to still accept a campaign, got {} {:?}",
        other.status, other.body
    );

    // And releasing one makes room again.
    let listed = list_ids(&router, Some("inst-full"), Some(50), None).await;
    let released = listed.first().expect("the full instance holds campaigns");
    let deleted = call(&router, "DELETE", &format!("/campaigns/{released}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "expected the delete to answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let again = create_campaign(&router, &simple_request("room-again", "inst-full")).await;
    assert_eq!(
        again.status, 200,
        "expected a create to succeed after a delete freed quota, got {} {:?}",
        again.status, again.body
    );
}

#[tokio::test]
async fn every_run_state_renders_as_a_value_the_model_declares() {
    use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::ops::types::CAMPAIGN_STATE_VALUES;
    use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::{
        CampaignState, ALL_CAMPAIGN_STATES,
    };

    // The catalog's source is the model's own `CampaignState` enum, rendered into
    // `CAMPAIGN_STATE_VALUES` by the scaffold (src/ops/types/shared_types.rs).
    for state in ALL_CAMPAIGN_STATES {
        assert!(
            CAMPAIGN_STATE_VALUES.contains(&state.as_wire()),
            "case {state:?}: {:?} is not one of the modelled CampaignState values {:?}",
            state.as_wire(),
            CAMPAIGN_STATE_VALUES
        );
    }
    assert_eq!(
        ALL_CAMPAIGN_STATES.len(),
        CAMPAIGN_STATE_VALUES.len(),
        "expected one stored state per modelled CampaignState value, got {:?} against {:?}",
        ALL_CAMPAIGN_STATES,
        CAMPAIGN_STATE_VALUES
    );
    // `Failed` is the one modelled state this slice never enters: nothing in the
    // campaign lifecycle fails a campaign, and no operation here reports it.
    assert!(
        ALL_CAMPAIGN_STATES.contains(&CampaignState::Failed),
        "expected the stored state set to still cover the modelled Failed state"
    );
}

#[tokio::test]
async fn operations_outside_this_slice_still_answer_501() {
    // The campaign slice is implemented; the rest of the service is still scaffolded, and
    // a scaffold must answer 501 rather than an empty success. These are each their own
    // gap issue.
    let router = service_router();
    let id = created_id(&router, &simple_request("not-yet", "inst-1")).await;

    // `GetCampaignStateBatch` used to sit here; it is implemented now, and the batch
    // state read has its own suite (`campaign_state_batch.rs`).
    let rows: &[(&str, String, Option<Value>)] = &[
        ("ResumeCampaign", format!("/campaigns/{id}/resume"), None),
    ];

    for (case, path, body) in rows {
        let answer = call(&router, "POST", path, body.as_ref()).await;
        assert_eq!(
            answer.status, 501,
            "case {case:?}: expected 501 from an unimplemented operation, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// One page of ListCampaigns: the ids it reports, and the `nextToken` it answers with.
async fn list_page(
    router: &MultiServiceRouter,
    max_results: i64,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut request = serde_json::Map::new();
    request.insert("maxResults".to_string(), json!(max_results));
    if let Some(token) = next_token {
        request.insert("nextToken".to_string(), json!(token));
    }
    let answer = call(
        router,
        "POST",
        "/campaigns-summary",
        Some(&Value::Object(request.clone())),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "list {request:?}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let ids = answer
        .member("campaignSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("list {request:?}: no campaignSummaryList in {:?}", answer.body))
        .iter()
        .map(|summary| {
            summary
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    (ids, answer.string_member("nextToken").map(str::to_string))
}

#[tokio::test]
async fn a_page_token_names_a_position_not_a_campaign() {
    // A `nextToken` names where the walk resumes, so whichever campaign a caller deletes
    // between two pages, the walk still reports every campaign that is still stored,
    // exactly once, in creation order — and the token itself carries no campaign id.
    //
    // One row per campaign the caller may delete after the first page of two: the one the
    // next page was going to start at, one later in the walk, one already reported, and
    // none at all.
    let rows: &[(&str, Option<usize>)] = &[
        ("no campaign deleted between the pages", None),
        ("the campaign the next page starts at is deleted", Some(2)),
        ("a campaign later in the walk is deleted", Some(3)),
        ("a campaign already reported on page one is deleted", Some(0)),
    ];

    for (case, deleted_index) in rows.iter().copied() {
        let router = service_router();
        let mut created = Vec::new();
        for index in 0..4 {
            created
                .push(created_id(&router, &simple_request(&format!("c{index}"), "inst-1")).await);
        }

        let (first_page, token) = list_page(&router, 2, None).await;
        assert_eq!(
            first_page,
            created[..2].to_vec(),
            "case {case:?}: expected the first page to be the two oldest campaigns, got \
             {first_page:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: expected a nextToken after a page of 2 of 4 campaigns")
        });
        assert!(
            !created.iter().any(|id| token.contains(id)),
            "case {case:?}: a page token must not carry a campaign id, got {token:?}"
        );

        if let Some(index) = deleted_index {
            let deleted = call(
                &router,
                "DELETE",
                &format!("/campaigns/{}", created[index]),
                None,
            )
            .await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected DeleteCampaign to answer 200, got {} {:?}",
                deleted.status, deleted.body
            );
        }

        // Resume from the token issued before the delete, to exhaustion.
        let mut walked = first_page;
        let mut next = Some(token);
        let mut pages = 0;
        while let Some(token) = next {
            let (page, token) = list_page(&router, 2, Some(&token)).await;
            walked.extend(page);
            next = token;
            pages += 1;
            assert!(
                pages < 20,
                "case {case:?}: the walk did not terminate after {pages} pages"
            );
        }

        // A campaign deleted before it was reported drops out of the walk; one deleted
        // after it was reported stays in what the walk already said.
        let expected: Vec<String> = created
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                deleted_index.is_none_or(|deleted| *index < 2 || *index != deleted)
            })
            .map(|(_, id)| id.clone())
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the resumed walk to report every stored campaign once \
             in creation order, got {walked:?}"
        );
    }
}
