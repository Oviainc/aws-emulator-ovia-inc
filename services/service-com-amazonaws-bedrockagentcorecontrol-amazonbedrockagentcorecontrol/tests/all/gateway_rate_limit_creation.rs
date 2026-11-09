// SPDX-License-Identifier: CC0-1.0

//! `CreateGatewayRateLimit`, and the read of the rate limit it wrote, end to end over
//! HTTP.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised against the one shared engine.
//!
//! The rule under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="CreateGatewayRateLimit,\
//! GetGatewayRateLimit")`): the create ADDS one rate limit to the gateway the path names,
//! under the `rateLimitId` the caller supplied or one the service generates ("An optional
//! customer-defined identifier for the rate limit. If not provided, the system generates
//! one"), stores exactly the description, dimension keys and rule entries submitted, and
//! reports it `CREATING` until the clock owes it `ACTIVE` — so the limit reads back through
//! `GetGatewayRateLimit` exactly as the create reported it. A limit the gateway cannot hold
//! beside the ones it already has is refused and nothing is written: an id the gateway
//! already holds, or dimension keys one of its limits already covers, is the modelled
//! conflict ("Must be unique per gateway—no two rate limits can share the same dimension
//! keys"), and a rule entry naming a dimension the limit does not declare is the modelled
//! validation failure ("Keys must match the parent rate limit's dimension keys").
//!
//! The rows differ along what the rule generalises over: whether the caller names the id,
//! which dimension keys scope the limit and how many, how many rule entries it holds,
//! which of the three rate metrics each configures, over which period, and at which rate.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

/// The settle span `src/state.rs` gives the gateway family's clock-owed transitions, plus
/// slack, so a row can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A `clientToken` inside `ClientToken`'s 33..256 length and its pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`.
const CLIENT_TOKEN: &str = "createratelimittokenaaaaaaaaaaaaaa";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The error code `restJson1` puts in the body's `__type`.
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

async fn create_rate_limit(router: &MultiServiceRouter, gateway: &str, body: &Value) -> Answer {
    send(
        router,
        "POST",
        &format!("/gateways/{gateway}/rate-limits"),
        Some(body),
    )
    .await
}

async fn get_rate_limit(router: &MultiServiceRouter, gateway: &str, rate_limit: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/gateways/{gateway}/rate-limits/{rate_limit}"),
        None,
    )
    .await
}

/// Create a gateway and hand back its `gatewayId`, waited out to `READY` — a gateway
/// enforces a rate limit only from the terminal status it settles into.
async fn settled_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let created = send(
        router,
        "POST",
        "/gateways/",
        Some(&json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "AWS_IAM"})),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup {name:?}: expected HTTP 202 from CreateGateway, got {} ({})",
        created.status, created.body
    );
    let gateway_id = text(&created.body, "gatewayId");
    tokio::time::sleep(PAST_SETTLE).await;
    gateway_id
}

/// A rate configuration: `RateConfig` needs a rate in 0..10000000 and a `Period`, whose
/// values are `second | minute` (`smithy_query(query="shape",shape="Period")`).
fn rate(value: f64, period: &str) -> Value {
    json!([{"rate": value, "period": period}])
}

/// One submitted rate limit.
struct Submitted {
    /// The `rateLimitId` the request carries, or `None` to have one generated.
    rate_limit_id: Option<&'static str>,
    description: Option<&'static str>,
    dimension_keys: Vec<&'static str>,
    entries: Vec<Value>,
}

impl Submitted {
    fn body(&self) -> Value {
        let mut request = json!({
            "dimensionKeys": self.dimension_keys,
            "entries": self.entries,
        });
        if let Some(rate_limit_id) = self.rate_limit_id {
            request["rateLimitId"] = json!(rate_limit_id);
        }
        if let Some(description) = self.description {
            request["description"] = json!(description);
        }
        request
    }
}

/// One row of the create table.
struct Row {
    case: &'static str,
    /// The gateway name the row creates (`GatewayName` is `^([0-9a-zA-Z][-]?){1,48}$`).
    gateway: &'static str,
    submitted: Submitted,
}

/// The rows. Every `dimensionKeys` value below is from `DimensionKey`'s own published
/// catalogue — `targetName | toolName | qualifiedModelId | $.context.iam.principal |
/// $.context.iam.sourceIdentity | $.context.jwt.<claim> |
/// $.context.request.header.<header> | $.context.request.body.<path>`
/// (`smithy_query(query="shape",shape="DimensionKeys")`) — including entries no recorded
/// request used. The rates include both published bounds of `RateConfig`'s `rate`
/// (`@range 0..10000000`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a caller-named id, one dimension key, one request rate per second",
            gateway: "createlimitnamed",
            submitted: Submitted {
                rate_limit_id: Some("per-target"),
                description: Some("one request rate"),
                dimension_keys: vec!["targetName"],
                entries: vec![json!({
                    "dimensions": {"targetName": "orders"},
                    "requests": rate(25.0, "second"),
                })],
            },
        },
        Row {
            case: "an id generated by the service, no description, tokens per minute",
            gateway: "createlimitgenerated",
            submitted: Submitted {
                rate_limit_id: None,
                description: None,
                dimension_keys: vec!["toolName"],
                entries: vec![json!({
                    "dimensions": {"toolName": "search"},
                    "tokens": rate(100_000.0, "minute"),
                })],
            },
        },
        Row {
            case: "two dimension keys, three rule entries, all three rate metrics",
            gateway: "createlimitthreerules",
            submitted: Submitted {
                rate_limit_id: None,
                description: Some(""),
                dimension_keys: vec!["targetName", "toolName"],
                entries: vec![
                    json!({
                        "dimensions": {"targetName": "orders", "toolName": "create"},
                        "requests": rate(5.0, "second"),
                    }),
                    json!({
                        "dimensions": {"targetName": "orders", "toolName": "read"},
                        "tokens": rate(50.0, "minute"),
                        "connections": rate(10.0, "second"),
                    }),
                    json!({
                        "dimensions": {"targetName": "*"},
                        "requests": rate(1.0, "minute"),
                    }),
                ],
            },
        },
        Row {
            case: "scoped by a JWT claim, the rate at the published lower bound",
            gateway: "createlimitjwtclaim",
            submitted: Submitted {
                rate_limit_id: Some("jwt_sub.limit"),
                description: Some("per caller"),
                dimension_keys: vec!["$.context.jwt.sub"],
                entries: vec![json!({
                    "dimensions": {"$.context.jwt.sub": "caller-1"},
                    "connections": rate(0.0, "second"),
                })],
            },
        },
        Row {
            case: "scoped by an IAM source identity, the rate at the published upper bound",
            gateway: "createlimitsourceidentity",
            submitted: Submitted {
                rate_limit_id: Some("source.identity-limit_1"),
                description: Some("the busiest callers"),
                dimension_keys: vec!["$.context.iam.sourceIdentity"],
                entries: vec![json!({
                    "dimensions": {"$.context.iam.sourceIdentity": "operator"},
                    "requests": rate(10_000_000.0, "minute"),
                })],
            },
        },
        Row {
            case: "scoped by a request header and a body path, two dimension keys",
            gateway: "createlimitrequestscoped",
            submitted: Submitted {
                rate_limit_id: None,
                description: Some("per tenant and per account in the body"),
                dimension_keys: vec![
                    "$.context.request.header.x-tenant",
                    "$.context.request.body.account_id",
                ],
                entries: vec![json!({
                    "dimensions": {
                        "$.context.request.header.x-tenant": "acme",
                        "$.context.request.body.account_id": "a-1",
                    },
                    "requests": rate(3.0, "second"),
                })],
            },
        },
    ]
}

/// A `rateLimitId` is well formed: `GatewayRateLimitId` is 2..64 characters matching
/// `^[a-zA-Z0-9][a-zA-Z0-9\-_\.]{0,62}[a-zA-Z0-9]$`.
fn well_formed_id(rate_limit_id: &str) -> bool {
    let characters: Vec<char> = rate_limit_id.chars().collect();
    (2..=64).contains(&characters.len())
        && characters
            .first()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters
            .last()
            .is_some_and(|last| last.is_ascii_alphanumeric())
        && characters
            .iter()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(*character))
}

/// A create stores exactly the submitted rate limit, under the id the caller named or one
/// the service generated, reports it `CREATING`, and it reads back unchanged — settling to
/// `ACTIVE` only once the span the clock owes passes.
#[tokio::test]
async fn a_create_stores_exactly_the_submitted_rate_limit() {
    for Row {
        case,
        gateway,
        submitted,
    } in rows()
    {
        let router = router();
        let gateway_id = settled_gateway(&router, gateway).await;

        let created = create_rate_limit(&router, &gateway_id, &submitted.body()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateGatewayRateLimit, got {} ({})",
            created.status, created.body
        );
        let rate_limit_id = text(&created.body, "rateLimitId");
        match submitted.rate_limit_id {
            Some(named) => assert_eq!(
                rate_limit_id, named,
                "case {case:?}: the limit keeps the id the request named, got {rate_limit_id:?}"
            ),
            None => assert!(
                well_formed_id(&rate_limit_id),
                "case {case:?}: the service generates a well-formed id, got {rate_limit_id:?}"
            ),
        }
        assert_eq!(
            text(&created.body, "gatewayIdentifier"),
            gateway_id,
            "case {case:?}: the limit names the gateway it was created on, got {}",
            created.body
        );
        assert_eq!(
            text(&created.body, "status"),
            "CREATING",
            "case {case:?}: a new limit is CREATING until the clock owes it ACTIVE, got {}",
            created.body
        );
        assert_eq!(
            created.body.get("dimensionKeys"),
            Some(&json!(submitted.dimension_keys)),
            "case {case:?}: the limit reports the dimension keys submitted, got {}",
            created.body
        );
        assert_eq!(
            created.body.get("entries"),
            Some(&json!(submitted.entries)),
            "case {case:?}: the limit reports the rule entries submitted, got {}",
            created.body
        );
        assert_eq!(
            created.body.get("description").cloned(),
            submitted.description.map(|description| json!(description)),
            "case {case:?}: the limit reports the description submitted, got {}",
            created.body
        );
        assert_eq!(
            created.body.get("createdAt"),
            created.body.get("updatedAt"),
            "case {case:?}: nothing has updated the limit since it was created, got {}",
            created.body
        );

        // The round trip: the stored limit reads back through the single-limit read.
        let read = get_rate_limit(&router, &gateway_id, &rate_limit_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created limit must read back, got {} ({})",
            read.status, read.body
        );
        for member in [
            "rateLimitId",
            "gatewayIdentifier",
            "description",
            "dimensionKeys",
            "entries",
            "status",
            "createdAt",
            "updatedAt",
        ] {
            assert_eq!(
                read.body.get(member),
                created.body.get(member),
                "case {case:?}: the read reports the {member} the create reported, got {}",
                read.body
            );
        }

        // Nothing but the clock moves the limit to its terminal status.
        tokio::time::sleep(PAST_SETTLE).await;
        let settled = get_rate_limit(&router, &gateway_id, &rate_limit_id).await;
        assert_eq!(
            (settled.status, text(&settled.body, "status").as_str()),
            (200, "ACTIVE"),
            "case {case:?}: once the span passes the limit is ACTIVE, got {} {} ({})",
            settled.status,
            text(&settled.body, "status"),
            settled.body
        );
    }
}

/// A create ADDS to the gateway's set: a second limit over different dimension keys
/// leaves the first exactly as it was, under its own id.
#[tokio::test]
async fn a_second_create_adds_beside_the_first() {
    let router = router();
    let gateway_id = settled_gateway(&router, "createlimitsecond").await;

    let first = Submitted {
        rate_limit_id: Some("by-target"),
        description: Some("the first limit"),
        dimension_keys: vec!["targetName"],
        entries: vec![json!({
            "dimensions": {"targetName": "orders"},
            "requests": rate(5.0, "second"),
        })],
    };
    let second = Submitted {
        rate_limit_id: None,
        description: Some("the second limit"),
        dimension_keys: vec!["qualifiedModelId"],
        entries: vec![json!({
            "dimensions": {"qualifiedModelId": "anthropic.claude"},
            "tokens": rate(1_000.0, "minute"),
        })],
    };

    let created_first = create_rate_limit(&router, &gateway_id, &first.body()).await;
    assert_eq!(
        created_first.status, 201,
        "setup: the first limit must be created, got {} ({})",
        created_first.status, created_first.body
    );
    let created_second = create_rate_limit(&router, &gateway_id, &second.body()).await;
    assert_eq!(
        created_second.status, 201,
        "a limit over other dimension keys is created beside the first, got {} ({})",
        created_second.status, created_second.body
    );
    assert_ne!(
        text(&created_second.body, "rateLimitId"),
        text(&created_first.body, "rateLimitId"),
        "the two limits are held under different ids, got {}",
        created_second.body
    );

    let held = get_rate_limit(&router, &gateway_id, "by-target").await;
    assert_eq!(
        held.status, 200,
        "the first limit is still held, got {} ({})",
        held.status, held.body
    );
    assert_eq!(
        held.body.get("entries"),
        Some(&json!(first.entries)),
        "the second create leaves the first limit's rule entries alone, got {}",
        held.body
    );
    let other = get_rate_limit(
        &router,
        &gateway_id,
        &text(&created_second.body, "rateLimitId"),
    )
    .await;
    assert_eq!(
        other.body.get("entries"),
        Some(&json!(second.entries)),
        "the second limit holds its own rule entries, got {}",
        other.body
    );
}

/// A replayed request carrying the same `clientToken` answers with the limit that token
/// first created, rather than creating a second one — "if this token matches a previous
/// request, the service ignores the request, but doesn't return an error".
#[tokio::test]
async fn a_replayed_client_token_creates_one_rate_limit() {
    let router = router();
    let gateway_id = settled_gateway(&router, "createlimitreplay").await;

    let mut body = Submitted {
        rate_limit_id: None,
        description: Some("the first submission"),
        dimension_keys: vec!["toolName"],
        entries: vec![json!({
            "dimensions": {"toolName": "search"},
            "requests": rate(7.0, "second"),
        })],
    }
    .body();
    body["clientToken"] = json!(CLIENT_TOKEN);

    let first = create_rate_limit(&router, &gateway_id, &body).await;
    assert_eq!(
        first.status, 201,
        "setup: the first submission must be created, got {} ({})",
        first.status, first.body
    );

    // The replay differs in every member a second create would store, so an answer
    // carrying the FIRST submission's values can only come from the stored limit.
    let mut replayed = body.clone();
    replayed["description"] = json!("the replayed submission");
    replayed["entries"] = json!([{
        "dimensions": {"toolName": "summarize"},
        "requests": rate(9.0, "minute"),
    }]);
    let second = create_rate_limit(&router, &gateway_id, &replayed).await;
    assert_eq!(
        second.status, 201,
        "a replayed clientToken is not an error, got {} ({})",
        second.status, second.body
    );
    for member in [
        "rateLimitId",
        "description",
        "dimensionKeys",
        "entries",
        "createdAt",
    ] {
        assert_eq!(
            second.body.get(member),
            first.body.get(member),
            "a replayed clientToken answers with the {member} the token first produced, got {}",
            second.body
        );
    }

    let stored = get_rate_limit(
        &router,
        &gateway_id,
        &text(&first.body, "rateLimitId"),
    )
    .await;
    assert_eq!(
        stored.body.get("entries"),
        first.body.get("entries"),
        "the replay wrote nothing over the limit the token first created, got {}",
        stored.body
    );
}

/// One row of the refusal table: a request the gateway cannot hold as a new rate limit.
struct Refused {
    case: &'static str,
    /// The request body, built over the limit the gateway already holds.
    request: Value,
    status: u16,
    code: &'static str,
}

/// The id and dimension keys the refusal table's gateway already holds.
const HELD_ID: &str = "already-held";
const HELD_KEYS: [&str; 2] = ["targetName", "toolName"];

fn refusals() -> Vec<Refused> {
    vec![
        Refused {
            case: "a rateLimitId the gateway already holds",
            request: json!({
                "rateLimitId": HELD_ID,
                "dimensionKeys": ["qualifiedModelId"],
                "entries": [{
                    "dimensions": {"qualifiedModelId": "anthropic.claude"},
                    "requests": rate(5.0, "second"),
                }],
            }),
            status: 409,
            code: "ConflictException",
        },
        Refused {
            case: "dimension keys a limit of the gateway already covers",
            request: json!({
                "rateLimitId": "same-keys",
                "dimensionKeys": HELD_KEYS,
                "entries": [{
                    "dimensions": {"targetName": "orders", "toolName": "read"},
                    "requests": rate(6.0, "second"),
                }],
            }),
            status: 409,
            code: "ConflictException",
        },
        Refused {
            case: "the same dimension keys in a different order",
            request: json!({
                "rateLimitId": "permuted-keys",
                "dimensionKeys": ["toolName", "targetName"],
                "entries": [{
                    "dimensions": {"toolName": "search", "targetName": "orders"},
                    "requests": rate(6.0, "second"),
                }],
            }),
            status: 409,
            code: "ConflictException",
        },
        Refused {
            case: "a rule entry naming a dimension the limit does not declare",
            request: json!({
                "rateLimitId": "mismatched",
                "dimensionKeys": ["qualifiedModelId"],
                "entries": [{
                    "dimensions": {"toolName": "search"},
                    "requests": rate(5.0, "second"),
                }],
            }),
            status: 400,
            code: "ValidationException",
        },
        Refused {
            case: "a rate past the published upper bound of RateConfig's rate",
            request: json!({
                "rateLimitId": "over-the-bound",
                "dimensionKeys": ["$.context.iam.principal"],
                "entries": [{
                    "dimensions": {"$.context.iam.principal": "arn:aws:iam::000000000000:role/Caller"},
                    "requests": rate(10_000_001.0, "second"),
                }],
            }),
            status: 400,
            code: "ValidationException",
        },
    ]
}

/// A request the gateway cannot hold as a new rate limit is refused the modelled error and
/// writes nothing: the limit the gateway already held is untouched, and the id the refused
/// request named names nothing.
#[tokio::test]
async fn a_create_the_gateway_cannot_hold_is_refused_and_writes_nothing() {
    let router = router();
    let gateway_id = settled_gateway(&router, "createlimitrefused").await;
    let held = json!({
        "rateLimitId": HELD_ID,
        "description": "written before the refusals",
        "dimensionKeys": HELD_KEYS,
        "entries": [{
            "dimensions": {"targetName": "orders", "toolName": "create"},
            "requests": rate(4.0, "second"),
        }],
    });
    let written = create_rate_limit(&router, &gateway_id, &held).await;
    assert_eq!(
        written.status, 201,
        "setup: the held limit must be created, got {} ({})",
        written.status, written.body
    );

    for Refused {
        case,
        request,
        status,
        code,
    } in refusals()
    {
        let refused = create_rate_limit(&router, &gateway_id, &request).await;
        assert_eq!(
            (refused.status, error_code(&refused.body).as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {} ({})",
            refused.status,
            error_code(&refused.body),
            refused.body
        );

        let still = get_rate_limit(&router, &gateway_id, HELD_ID).await;
        assert_eq!(
            still.status, 200,
            "case {case:?}: the refused create leaves the held limit in place, got {} ({})",
            still.status, still.body
        );
        assert_eq!(
            still.body.get("entries"),
            held.get("entries"),
            "case {case:?}: the held limit keeps its own rule entries, got {}",
            still.body
        );
        let requested_id = text(&request, "rateLimitId");
        if requested_id != HELD_ID {
            let unwritten = get_rate_limit(&router, &gateway_id, &requested_id).await;
            assert_eq!(
                (unwritten.status, error_code(&unwritten.body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: the refused create writes no limit of its own, got {} ({})",
                unwritten.status,
                unwritten.body
            );
        }
    }
}

/// The create resolves its gateway first: an identifier no gateway of this account holds is
/// the modelled 404, a gateway the clock is still creating TAKES the rate limit (the limit
/// carries a clock-owed status of its own), and a gateway whose deletion has been accepted
/// takes no further configuration.
#[tokio::test]
async fn the_create_resolves_its_gateway_first() {
    let router = router();
    let body = json!({
        "rateLimitId": "per-target",
        "dimensionKeys": ["targetName"],
        "entries": [{
            "dimensions": {"targetName": "orders"},
            "requests": rate(5.0, "second"),
        }],
    });

    // `GatewayIdentifier`'s own pattern admits this id; no gateway holds it.
    let missing = create_rate_limit(&router, "nosuchgateway-abcdefghij", &body).await;
    assert_eq!(
        (missing.status, error_code(&missing.body).as_str()),
        (404, "ResourceNotFoundException"),
        "an unknown gateway is the modelled 404, got {} {} ({})",
        missing.status,
        error_code(&missing.body),
        missing.body
    );

    let created = send(
        &router,
        "POST",
        "/gateways/",
        Some(&json!({
            "name": "createlimitcreating",
            "roleArn": ROLE_ARN,
            "authorizerType": "AWS_IAM",
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: the gateway must be created, got {} ({})",
        created.status, created.body
    );
    let gateway_id = text(&created.body, "gatewayId");
    let creating = create_rate_limit(&router, &gateway_id, &body).await;
    assert_eq!(
        creating.status, 201,
        "a gateway the clock is still creating takes a rate limit, got {} ({})",
        creating.status, creating.body
    );
    let stored = get_rate_limit(&router, &gateway_id, "per-target").await;
    assert_eq!(
        stored.status, 200,
        "the limit written on a creating gateway is held, got {} ({})",
        stored.status, stored.body
    );

    // A gateway on its way out takes no further configuration.
    tokio::time::sleep(PAST_SETTLE).await;
    let deleted = send(&router, "DELETE", &format!("/gateways/{gateway_id}/"), None).await;
    assert_eq!(
        deleted.status, 202,
        "setup: the gateway's deletion must be accepted, got {} ({})",
        deleted.status, deleted.body
    );
    let deleting = create_rate_limit(
        &router,
        &gateway_id,
        &json!({
            "rateLimitId": "after-the-delete",
            "dimensionKeys": ["toolName"],
            "entries": [{
                "dimensions": {"toolName": "search"},
                "requests": rate(5.0, "second"),
            }],
        }),
    )
    .await;
    assert_eq!(
        (deleting.status, error_code(&deleting.body).as_str()),
        (400, "ValidationException"),
        "a gateway on its way out takes no rate limit, got {} {} ({})",
        deleting.status,
        error_code(&deleting.body),
        deleting.body
    );
}
