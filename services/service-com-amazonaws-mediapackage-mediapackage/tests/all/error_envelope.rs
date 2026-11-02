// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the BODY, the STATUS and the ERROR CODE of a MediaPackage
//! refusal, driven over real HTTP through the same `MultiServiceRouter` the emulator
//! boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. ENVELOPE. Every client-visible refusal answers with a modelled error — its own
//!    status, its name in `__type` AND in the `x-amzn-errortype` header — and a body
//!    whose `message` states the failure. A refusal the service reports AGAINST A KEY
//!    carries an `errors` object keyed by the thing that was wrong as well, and the
//!    sentence states the same key and reason, so a client reading either half reads the
//!    same failure. An entry's VALUE says which step refused the request: the request
//!    validator reports the one requirement a member failed, as a bare string;
//!    everything decided against state reports a list of reasons for its key. A refusal
//!    the service words as ONE sentence of its own reports no key, so its body carries
//!    `message` alone.
//! 2. WHICH ERROR. A request ADDRESSING a resource by id that this service does not hold
//!    is `NotFoundException` (404), keyed by the RESOURCE TYPE. A request whose MEMBER
//!    names a resource that cannot be resolved is `UnprocessableEntityException` (422),
//!    keyed by that MEMBER: the request body is what the caller has to fix.
//! 3. CONFLICT. A create on an id already in use, and a delete blocked by children, are
//!    `UnprocessableEntityException` (422) keyed by the resource type, and the reason
//!    names what is in the way — for a blocked delete, how many children hold it and
//!    what they are.
//! 4. INPUT. A value this service will not accept — a page size outside the bound it
//!    enforces, a pagination token it did not issue, a member a declared constraint
//!    refuses, a required member that is absent — is `UnprocessableEntityException`
//!    (422), never the framework's generic 400 `ValidationException`
//!    (a status and a code this service's model does not declare at all).
//!
//! What is NOT asserted is the recorded sentence of a KEYED refusal: the status and the
//! error code are the contract, and that message follows a rule — `<key> <reason>`, built
//! from the request — so it is the rule that is pinned, on every row. A refusal the
//! service words as its own fixed sentence has no such rule to derive it from, so the
//! sentence itself is the requirement and the row states it
//! (`tests/all/refusal_wording.rs` covers those).

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for.
const REGION: &str = "eu-west-1";

/// The emulator's canonical placeholder account id (`emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// `arn:aws:mediapackage:<region>:<account>:<type>/<id>`, as this service mints it.
fn resource_arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// The URI the tagging operations bind. `ResourceArn` is a NON-greedy `@httpLabel`
/// (`smithy_query(query="operation",operation="TagResource")`), so an SDK percent-encodes
/// the ARN's reserved characters into one path segment and its `:` and `/` never split the
/// path.
fn tags_path(arn: &str) -> String {
    let label: String = arn
        .chars()
        .map(|character| match character {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect();
    format!("/tags/{label}")
}

/// The page-size bound this service enforces, and the parameter it reads it from —
/// `crate::state::MIN_PAGE_SIZE`..=`MAX_PAGE_SIZE` on `maxResults`, which `MaxResults`
/// binds as `@httpQuery("maxResults")` on every `List*` operation.
const PAGE_SIZE_BOUND: (usize, usize) = (1, 50);

/// The reason every refused page size states.
fn page_size_reason() -> String {
    let (low, high) = PAGE_SIZE_BOUND;
    format!("must be an integer between {low} and {high}")
}

/// A fresh emulator holding only this service, with request authentication off (this
/// suite exercises refusal bodies, not signing).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One round trip: the HTTP status, the `x-amzn-errortype` header, and the parsed JSON
/// body (`Null` when there is none).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://mediapackage.{REGION}.amazonaws.com{uri}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/mediapackage/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, error_type, parsed)
}

/// The wire error code restJson1 carries in `__type`, stripped of any namespace prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Which step of the service refused the request, and so how its `errors` entry reports
/// the reason: the request VALIDATOR reports the one requirement a member failed as a
/// bare string; anything decided against STATE reports a list of reasons for its key; and
/// a class the service words as ONE sentence of its own reports no entry at all.
#[derive(Clone, Copy)]
enum Reported {
    Requirement,
    Reasons,
    Sentence,
}

/// One refusal to exercise: the request, and the envelope it must answer with.
struct Refusal {
    /// What this row is, for the assertion messages.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The modelled status.
    status: u16,
    /// The modelled error shape name.
    code: &'static str,
    /// The `errors` key this refusal is about: a resource type or a request member.
    /// Ignored by a [`Reported::Sentence`] row, which reports no key.
    key: &'static str,
    /// How that key reports its reason.
    reported: Reported,
    /// The reason, which is the same text whichever way it is reported — or, for a
    /// [`Reported::Sentence`] row, the whole sentence the refusal states.
    reason: String,
}

/// Assert `row`'s whole envelope: status, both places the code appears, the `errors`
/// entry, and the sentence built from it.
async fn assert_envelope(router: &MultiServiceRouter, row: &Refusal) {
    let case = row.case;
    let (status, error_type, body) = call(router, row.method, &row.uri, row.body.as_ref()).await;
    assert_eq!(
        status, row.status,
        "case {case:?}: expected HTTP {}, got {status} {body:?}",
        row.status
    );
    assert_eq!(
        error_code(&body),
        row.code,
        "case {case:?}: expected __type {:?}, got {body:?}",
        row.code
    );
    assert_eq!(
        error_type, row.code,
        "case {case:?}: expected x-amzn-errortype {:?}, got {error_type:?}",
        row.code
    );
    // The structured half: an object keyed by the thing that was wrong, reporting that
    // key's reasons the way the refusing step reports them — and nothing at all when the
    // refusal is the service's own sentence, which is about no key.
    let expected_errors = match row.reported {
        Reported::Requirement => json!({ row.key: row.reason.clone() }),
        Reported::Reasons => json!({ row.key: [row.reason.clone()] }),
        Reported::Sentence => Value::Null,
    };
    let errors = body.get("errors").unwrap_or(&Value::Null).clone();
    assert_eq!(
        errors, expected_errors,
        "case {case:?}: expected errors {expected_errors}, got {body:?}"
    );
    // The sentence: the same key and reason, so the two halves cannot disagree.
    let expected_message = match row.reported {
        Reported::Sentence => row.reason.clone(),
        _ => format!("{} {}", row.key, row.reason),
    };
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(expected_message.as_str()),
        "case {case:?}: expected message {expected_message:?}, got {body:?}"
    );
}

/// Create a channel, asserting it was stored and reads back — the refusals under test are
/// only meaningful against a store the valid requests reached.
async fn seed_channel(router: &MultiServiceRouter, id: &str) {
    let (status, _, created) = call(
        router,
        "POST",
        "/channels",
        Some(&json!({ "id": id })),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding channel {id:?}: expected 200, got {status} {created:?}"
    );
    let (status, _, described) = call(router, "GET", &format!("/channels/{id}"), None).await;
    assert_eq!(
        status, 200,
        "seeding channel {id:?}: the read back expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described.get("id").and_then(Value::as_str),
        Some(id),
        "seeding channel {id:?}: the read back must return the stored record, got {described:?}"
    );
}

/// Create an origin endpoint on `channel`, asserting it was stored and reads back.
async fn seed_endpoint(router: &MultiServiceRouter, channel: &str, id: &str) {
    let (status, _, created) = call(
        router,
        "POST",
        "/origin_endpoints",
        Some(&json!({ "channelId": channel, "id": id, "hlsPackage": {} })),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding endpoint {id:?}: expected 200, got {status} {created:?}"
    );
    let (status, _, described) =
        call(router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 200,
        "seeding endpoint {id:?}: the read back expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described.get("channelId").and_then(Value::as_str),
        Some(channel),
        "seeding endpoint {id:?}: the read back must return the stored record, got {described:?}"
    );
}

/// RULES 1 and 2: a request that addresses a resource by id this service does not hold is
/// a 404 keyed by the resource type; the rows differ along the operation (read, delete,
/// update, configure, tag, untag), the resource type, and the shape of the absent id.
#[tokio::test]
async fn addressing_a_resource_the_service_does_not_hold_is_a_keyed_404() {
    let router = emulator();
    seed_channel(&router, "live-channel").await;
    seed_endpoint(&router, "live-channel", "live-endpoint").await;

    let mut rows: Vec<Refusal> = Vec::new();
    // Ids differing in form and length, so what is asserted is the rule and not one id.
    for id in ["absent", "live-channe", "LIVE-CHANNEL", "x", "a-much-longer-id-0123456789"] {
        let missing_channel = format!("with id={id} not found");
        for (case, method) in [
            ("DescribeChannel", "GET"),
            ("DeleteChannel", "DELETE"),
        ] {
            rows.push(Refusal {
                case,
                method,
                uri: format!("/channels/{id}"),
                body: None,
                status: 404,
                code: "NotFoundException",
                key: "channel",
                reported: Reported::Reasons,
                reason: missing_channel.clone(),
            });
        }
        // The mutating operations resolve their target the same way, before they judge
        // the request.
        rows.push(Refusal {
            case: "UpdateChannel",
            method: "PUT",
            uri: format!("/channels/{id}"),
            body: Some(json!({ "description": "a new description" })),
            status: 404,
            code: "NotFoundException",
            key: "channel",
            reported: Reported::Reasons,
            reason: missing_channel.clone(),
        });
        rows.push(Refusal {
            case: "ConfigureLogs",
            method: "PUT",
            uri: format!("/channels/{id}/configure_logs"),
            body: Some(json!({ "egressAccessLogs": { "logGroupName": "/aws/egress" } })),
            status: 404,
            code: "NotFoundException",
            key: "channel",
            reported: Reported::Reasons,
            reason: missing_channel.clone(),
        });
        rows.push(Refusal {
            case: "TagResource on a channel ARN",
            method: "POST",
            uri: tags_path(&resource_arn("channels", id)),
            body: Some(json!({ "tags": { "team": "video" } })),
            status: 404,
            code: "NotFoundException",
            key: "channel",
            reported: Reported::Reasons,
            reason: missing_channel.clone(),
        });

        let missing_endpoint = format!("with id={id} not found");
        for (case, method) in [
            ("DescribeOriginEndpoint", "GET"),
            ("DeleteOriginEndpoint", "DELETE"),
        ] {
            rows.push(Refusal {
                case,
                method,
                uri: format!("/origin_endpoints/{id}"),
                body: None,
                status: 404,
                code: "NotFoundException",
                key: "originEndpoint",
                reported: Reported::Reasons,
                reason: missing_endpoint.clone(),
            });
        }
        rows.push(Refusal {
            case: "UpdateOriginEndpoint",
            method: "PUT",
            uri: format!("/origin_endpoints/{id}"),
            body: Some(json!({ "description": "a new description" })),
            status: 404,
            code: "NotFoundException",
            key: "originEndpoint",
            reported: Reported::Reasons,
            reason: missing_endpoint.clone(),
        });
        rows.push(Refusal {
            case: "UntagResource on an origin endpoint ARN",
            method: "DELETE",
            uri: format!(
                "{}?tagKeys=team",
                tags_path(&resource_arn("origin_endpoints", id))
            ),
            body: None,
            status: 404,
            code: "NotFoundException",
            key: "originEndpoint",
            reported: Reported::Reasons,
            reason: missing_endpoint.clone(),
        });
    }
    for row in &rows {
        assert_envelope(&router, row).await;
    }

    // And the resources that ARE held keep answering, so the rule refuses only a miss.
    for uri in ["/channels/live-channel", "/origin_endpoints/live-endpoint"] {
        let (status, _, body) = call(&router, "GET", uri, None).await;
        assert_eq!(
            status, 200,
            "case {uri:?}: a stored resource must still read back, got {status} {body:?}"
        );
        assert_eq!(
            body.get("errors"),
            None,
            "case {uri:?}: a success carries no errors member, got {body:?}"
        );
    }
}

/// RULE 2, the other half: a MEMBER of the request naming a resource that cannot be
/// resolved is a 422 keyed by that member — the caller has to fix the request, not go
/// looking for a resource that is gone.
#[tokio::test]
async fn a_request_member_naming_an_unresolvable_resource_is_a_keyed_422() {
    let router = emulator();
    seed_channel(&router, "live-channel").await;

    // Rows differ along the absent channelId's form, and along the packaging the create
    // carries: the refusal is about the reference, not about the rest of the request.
    let mut rows: Vec<Refusal> = Vec::new();
    for (index, channel_id) in ["absent", "live-channe", "LIVE-CHANNEL", "z"]
        .into_iter()
        .enumerate()
    {
        for package in [
            json!({ "hlsPackage": {} }),
            json!({ "dashPackage": { "segmentDurationSeconds": 2 } }),
            json!({ "cmafPackage": { "hlsManifests": [{ "id": "primary" }] } }),
        ] {
            let mut body = json!({ "channelId": channel_id, "id": format!("new-{index}") });
            for (key, value) in package.as_object().expect("a package object") {
                body[key] = value.clone();
            }
            rows.push(Refusal {
                case: "CreateOriginEndpoint with an unknown channelId",
                method: "POST",
                uri: "/origin_endpoints".to_string(),
                body: Some(body),
                status: 422,
                code: "UnprocessableEntityException",
                key: "channelId",
                reported: Reported::Reasons,
                reason: "not found".to_string(),
            });
        }
    }
    for row in &rows {
        assert_envelope(&router, row).await;
    }

    // Nothing was stored by any refused create.
    let (status, _, listed) = call(&router, "GET", "/origin_endpoints", None).await;
    assert_eq!(status, 200, "listing endpoints expected 200, got {listed:?}");
    assert_eq!(
        listed.get("originEndpoints").and_then(Value::as_array),
        Some(&vec![]),
        "a refused create must store nothing, got {listed:?}"
    );
}

/// RULE 3: a create on an id already in use names the resource type that is in the way.
#[tokio::test]
async fn a_create_on_an_id_already_in_use_names_the_resource_in_the_way() {
    let router = emulator();

    // Rows differ along the resource type and the id's form.
    for id in ["taken", "T", "taken-0123456789-longer"] {
        seed_channel(&router, id).await;
        assert_envelope(
            &router,
            &Refusal {
                case: "CreateChannel on an id in use",
                method: "POST",
                uri: "/channels".to_string(),
                body: Some(json!({ "id": id })),
                status: 422,
                code: "UnprocessableEntityException",
                key: "channel",
                reported: Reported::Reasons,
                reason: format!("with id={id} already exists"),
            },
        )
        .await;

        seed_endpoint(&router, id, &format!("{id}-endpoint")).await;
        assert_envelope(
            &router,
            &Refusal {
                case: "CreateOriginEndpoint on an id in use",
                method: "POST",
                uri: "/origin_endpoints".to_string(),
                body: Some(json!({
                    "channelId": id,
                    "id": format!("{id}-endpoint"),
                    "hlsPackage": {}
                })),
                status: 422,
                code: "UnprocessableEntityException",
                key: "originEndpoint",
                reported: Reported::Reasons,
                reason: format!("with id={id}-endpoint already exists"),
            },
        )
        .await;
    }
}

/// RULE 3, the dependency half: a delete blocked by children says how many hold it and
/// what they are, so a client learns the blocker programmatically. The rows differ along
/// the number of children attached, which is read from state.
#[tokio::test]
async fn a_delete_blocked_by_children_reports_how_many_and_of_what() {
    let router = emulator();
    seed_channel(&router, "live-channel").await;

    let endpoints = ["ep-1", "ep-2", "ep-3"];
    for (index, endpoint) in endpoints.iter().enumerate() {
        seed_endpoint(&router, "live-channel", endpoint).await;
        let attached = index + 1;
        let children = if attached == 1 {
            "1 OriginEndpoint is".to_string()
        } else {
            format!("{attached} OriginEndpoints are")
        };
        assert_envelope(
            &router,
            &Refusal {
                case: "DeleteChannel with endpoints attached",
                method: "DELETE",
                uri: "/channels/live-channel".to_string(),
                body: None,
                status: 422,
                code: "UnprocessableEntityException",
                key: "channel",
                reported: Reported::Reasons,
                reason: format!("cannot be deleted because {children} attached to it"),
            },
        )
        .await;
    }

    // With every child gone the delete goes through, so the refusal tracked state.
    for endpoint in endpoints {
        let (status, _, body) =
            call(&router, "DELETE", &format!("/origin_endpoints/{endpoint}"), None).await;
        assert_eq!(
            status, 202,
            "deleting {endpoint:?} expected 202, got {status} {body:?}"
        );
    }
    let (status, _, body) = call(&router, "DELETE", "/channels/live-channel", None).await;
    assert_eq!(
        status, 202,
        "with no children attached the delete must succeed, got {status} {body:?}"
    );
}

/// RULE 4: the page size this service enforces. Every paginated operation refuses a
/// `maxResults` outside 1..=50 — including values the MODELLED `@range` (1..1000) would
/// admit, and values that are not integers at all — with one 422 keyed by the member, and
/// accepts every value inside it.
#[tokio::test]
async fn a_page_size_outside_the_enforced_bound_is_a_keyed_422() {
    let router = emulator();
    seed_channel(&router, "live-channel").await;
    seed_endpoint(&router, "live-channel", "live-endpoint").await;

    let (low, high) = PAGE_SIZE_BOUND;
    // The paginated operations, read off the model: `MaxResults` is `@httpQuery` on
    // ListChannels and ListOriginEndpoints (ListHarvestJobs binds it too, and answers
    // the scaffold's 501 once accepted, so its refusal is covered by the gate alone).
    let paths = ["/channels", "/origin_endpoints"];
    // Rows: one past each bound, a value the modelled range admits but the service does
    // not, the recorded out-of-range value, and values that are not integers.
    let refused = [
        (low - 1).to_string(),
        (high + 1).to_string(),
        "1000".to_string(),
        "10000".to_string(),
        "-5".to_string(),
        "abc".to_string(),
        "1.5".to_string(),
        // Form encoding's space: the decoded value is " 5", which is not an integer, so
        // it fails the same requirement rather than the framework's generic 400.
        "+5".to_string(),
        String::new(),
    ];
    for path in paths {
        for value in &refused {
            assert_envelope(
                &router,
                &Refusal {
                    case: "a page size the service does not accept",
                    method: "GET",
                    uri: format!("{path}?maxResults={value}"),
                    body: None,
                    status: 422,
                    code: "UnprocessableEntityException",
                    key: "maxResults",
                    reported: Reported::Requirement,
                    reason: page_size_reason(),
                },
            )
            .await;
        }
        // The parameter sent with no value at all — the HTTP binding reads that as the
        // empty value, which fails the same requirement.
        assert_envelope(
            &router,
            &Refusal {
                case: "a page size sent with no value",
                method: "GET",
                uri: format!("{path}?maxResults"),
                body: None,
                status: 422,
                code: "UnprocessableEntityException",
                key: "maxResults",
                reported: Reported::Requirement,
                reason: page_size_reason(),
            },
        )
        .await;
        // Sent twice, the FIRST value is the one a non-list member is read from, so that
        // is the one the bound is applied to — whichever way round the two are sent.
        assert_envelope(
            &router,
            &Refusal {
                case: "a page size sent twice, the first outside the bound",
                method: "GET",
                uri: format!("{path}?maxResults={}&maxResults=10", high + 1),
                body: None,
                status: 422,
                code: "UnprocessableEntityException",
                key: "maxResults",
                reported: Reported::Requirement,
                reason: page_size_reason(),
            },
        )
        .await;
        let uri = format!("{path}?maxResults=10&maxResults={}", high + 1);
        let (status, _, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {uri:?}: the first value is inside the bound, so the request must be \
             accepted, got {status} {body:?}"
        );
        // Each bound itself, and a value between them, are accepted — sent plainly and
        // percent-encoded, because the bound applies to the value and not to its
        // encoding (`%31%30` is `10`).
        for value in [
            low.to_string(),
            ((low + high) / 2).to_string(),
            high.to_string(),
            "%31%30".to_string(),
        ] {
            let uri = format!("{path}?maxResults={value}");
            let (status, _, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {uri:?}: a page size inside the bound must be accepted, got \
                 {status} {body:?}"
            );
        }
        // And an absent page size is not a refusal at all.
        let (status, _, body) = call(&router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path:?}: an absent maxResults must be accepted, got {status} {body:?}"
        );
    }
}

/// RULE 4, the rest of the input surface: a token this service did not issue, and a
/// required member the request leaves out, are the same modelled 422 — never a generic
/// 400 `ValidationException`, which this service's model does not declare.
#[tokio::test]
async fn an_input_the_service_will_not_accept_is_a_modelled_422() {
    let router = emulator();
    seed_channel(&router, "live-channel").await;

    let mut rows: Vec<Refusal> = Vec::new();
    // Tokens differing in how they are wrong: not hex, odd-length hex, non-hex letters,
    // and percent noise. The service words this class as one sentence of its own, so the
    // row states that sentence rather than a per-member reason.
    for token in ["not-a-token", "abc", "zz", "%%%"] {
        for path in ["/channels", "/origin_endpoints"] {
            rows.push(Refusal {
                case: "a pagination token this service did not issue",
                method: "GET",
                uri: format!("{path}?nextToken={token}"),
                body: None,
                status: 422,
                code: "UnprocessableEntityException",
                key: "",
                reported: Reported::Sentence,
                reason: "Invalid NextToken".to_string(),
            });
        }
    }
    // A required member absent from the body. Source for which members are required:
    // `smithy_query(query="operation",operation="CreateChannel,CreateOriginEndpoint",…)`
    // — `CreateChannelRequest.Id` and `CreateOriginEndpointRequest.ChannelId`/`Id`.
    rows.push(Refusal {
        case: "CreateChannel with no id",
        method: "POST",
        uri: "/channels".to_string(),
        body: Some(json!({ "description": "no id at all" })),
        status: 422,
        code: "UnprocessableEntityException",
        key: "id",
        reported: Reported::Requirement,
        reason: "is required".to_string(),
    });
    rows.push(Refusal {
        case: "CreateOriginEndpoint with no channelId",
        method: "POST",
        uri: "/origin_endpoints".to_string(),
        body: Some(json!({ "id": "no-channel", "hlsPackage": {} })),
        status: 422,
        code: "UnprocessableEntityException",
        key: "channelId",
        reported: Reported::Requirement,
        reason: "is required".to_string(),
    });
    // A caller-sized member past the bound this service stores: the same envelope, keyed
    // by the member, stating the bound it broke.
    rows.push(Refusal {
        case: "CreateChannel with an over-long description",
        method: "POST",
        uri: "/channels".to_string(),
        body: Some(json!({ "id": "over-long", "description": "d".repeat(1_025) })),
        status: 422,
        code: "UnprocessableEntityException",
        key: "description",
        reported: Reported::Requirement,
        reason: "may be at most 1024 characters, got 1025".to_string(),
    });
    // And an identifier the service cannot read as one of its own ARNs: a value in the
    // request, so it is keyed by the member rather than by a resource type. The rows
    // differ along how the ARN is wrong.
    for arn in [
        "not-an-arn",
        "arn:aws:mediapackage:eu-west-1:000000000000:widgets/x",
        "arn:aws:s3:::a-bucket",
    ] {
        rows.push(Refusal {
            case: "TagResource with an ARN this service cannot read",
            method: "POST",
            uri: tags_path(arn),
            body: Some(json!({ "tags": { "team": "video" } })),
            status: 422,
            code: "UnprocessableEntityException",
            key: "resourceArn",
            reported: Reported::Requirement,
            reason: "must be a MediaPackage channel or origin endpoint ARN".to_string(),
        });
    }

    for row in &rows {
        assert_envelope(&router, row).await;
    }
}
