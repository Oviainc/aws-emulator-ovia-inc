// SPDX-License-Identifier: CC0-1.0

//! `BatchPutGatewayRateLimits`, and the read of one rate limit it wrote, end to end over
//! HTTP.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised against the one shared engine.
//!
//! The rule under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="BatchPutGatewayRateLimits,\
//! GetGatewayRateLimit")`): the batch write REPLACES a gateway's whole rate-limit set with
//! the list it submits — "the complete set of rate limits for this gateway. This operation
//! replaces all existing rate limits in a single request" — upserting an entry whose
//! `rateLimitId` an existing limit already holds, minting an id for an entry that carries
//! none, and dropping every limit the submitted list leaves out, so the stored set is
//! always exactly one submitted list and never a merge of two. Each written limit reports
//! `CREATING` (or `UPDATING` for an upsert) until the clock owes it `ACTIVE`, and reads
//! back through `GetGatewayRateLimit` exactly as it was submitted.
//!
//! The rows differ along what the rule generalises over: how many entries one batch
//! carries, whether each entry names its own `rateLimitId` or is minted one, which
//! dimension keys scope it, how many rule entries it holds and which of the three rate
//! metrics each configures.

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

async fn batch_put(router: &MultiServiceRouter, gateway: &str, body: &Value) -> Answer {
    send(
        router,
        "PUT",
        &format!("/gateways/{gateway}/rate-limits/batch"),
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

/// One submitted `BatchPutLimitEntry`.
struct Submitted {
    /// The `rateLimitId` the entry carries, or `None` to have one minted.
    rate_limit_id: Option<&'static str>,
    description: Option<&'static str>,
    dimension_keys: Vec<&'static str>,
    entries: Vec<Value>,
}

impl Submitted {
    fn body(&self) -> Value {
        let mut entry = json!({
            "dimensionKeys": self.dimension_keys,
            "entries": self.entries,
        });
        if let Some(rate_limit_id) = self.rate_limit_id {
            entry["rateLimitId"] = json!(rate_limit_id);
        }
        if let Some(description) = self.description {
            entry["description"] = json!(description);
        }
        entry
    }
}

/// A batch body carrying `submitted`, in submission order.
fn batch_body(submitted: &[Submitted]) -> Value {
    json!({"rateLimits": submitted.iter().map(Submitted::body).collect::<Vec<_>>()})
}

/// One row of the batch-write table.
struct Row {
    case: &'static str,
    /// The gateway name the row creates (`GatewayName` is `^[0-9a-zA-Z-]+$`).
    gateway: &'static str,
    submitted: Vec<Submitted>,
}

/// The rows. Every `dimensionKeys` value below is from `DimensionKey`'s own published
/// catalogue — `targetName | toolName | qualifiedModelId | $.context.iam.principal |
/// $.context.iam.sourceIdentity | $.context.jwt.<claim> |
/// $.context.request.header.<header> | $.context.request.body.<path>`
/// (`smithy_query(query="shape",shape="DimensionKeys")`) — including entries no recorded
/// request used.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one entry, caller-supplied id, one request rate per target",
            gateway: "ratelimitone",
            submitted: vec![Submitted {
                rate_limit_id: Some("per-target"),
                description: Some("one request rate"),
                dimension_keys: vec!["targetName"],
                entries: vec![json!({
                    "dimensions": {"targetName": "orders"},
                    "requests": rate(25.0, "second"),
                })],
            }],
        },
        Row {
            case: "one entry, id minted by the service, no description",
            gateway: "ratelimitminted",
            submitted: vec![Submitted {
                rate_limit_id: None,
                description: None,
                dimension_keys: vec!["toolName"],
                entries: vec![json!({
                    "dimensions": {"toolName": "search"},
                    "tokens": rate(100_000.0, "minute"),
                })],
            }],
        },
        Row {
            case: "three entries, mixed ids, two dimension keys and three rule entries",
            gateway: "ratelimitthree",
            submitted: vec![
                Submitted {
                    rate_limit_id: Some("by.model"),
                    description: Some("tokens by model"),
                    dimension_keys: vec!["qualifiedModelId"],
                    entries: vec![json!({
                        "dimensions": {"qualifiedModelId": "anthropic.claude"},
                        "tokens": rate(1_000.0, "minute"),
                    })],
                },
                Submitted {
                    rate_limit_id: None,
                    description: None,
                    dimension_keys: vec!["targetName", "toolName"],
                    entries: vec![
                        json!({
                            "dimensions": {"targetName": "orders", "toolName": "create"},
                            "requests": rate(5.0, "second"),
                        }),
                        json!({
                            "dimensions": {"targetName": "orders", "toolName": "read"},
                            "requests": rate(50.0, "second"),
                            "connections": rate(10.0, "second"),
                        }),
                        json!({
                            "dimensions": {"targetName": "*"},
                            "requests": rate(1.0, "minute"),
                        }),
                    ],
                },
                Submitted {
                    rate_limit_id: Some("jwt_sub"),
                    description: Some("per caller"),
                    dimension_keys: vec!["$.context.jwt.sub"],
                    entries: vec![json!({
                        "dimensions": {"$.context.jwt.sub": "caller-1"},
                        "connections": rate(3.0, "second"),
                    })],
                },
            ],
        },
        Row {
            case: "one entry scoped by an IAM principal, the longest id the pattern admits \
                   in this suite",
            gateway: "ratelimitprincipal",
            submitted: vec![Submitted {
                rate_limit_id: Some("principal.rate-limit_for-iam.callers"),
                description: Some(""),
                dimension_keys: vec!["$.context.iam.principal"],
                entries: vec![json!({
                    "dimensions": {"$.context.iam.principal": "arn:aws:iam::000000000000:role/Caller"},
                    "requests": rate(10_000_000.0, "minute"),
                })],
            }],
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

/// A batch write stores exactly the submitted list: every entry reads back as submitted,
/// under the id the caller named or one the service minted, and the response reports the
/// resulting set.
#[tokio::test]
async fn a_batch_write_stores_exactly_the_submitted_set() {
    for row in rows() {
        let Row {
            case,
            gateway,
            submitted,
        } = row;
        let router = router();
        let gateway_id = settled_gateway(&router, gateway).await;

        let written = batch_put(&router, &gateway_id, &batch_body(&submitted)).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected HTTP 200 from BatchPutGatewayRateLimits, got {} ({})",
            written.status, written.body
        );
        let reported = written
            .body
            .get("rateLimits")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            submitted.len(),
            "case {case:?}: the response reports the whole resulting set, got {}",
            written.body
        );

        for (index, (entry, expected)) in reported.iter().zip(submitted.iter()).enumerate() {
            let rate_limit_id = text(entry, "rateLimitId");
            match expected.rate_limit_id {
                Some(named) => assert_eq!(
                    rate_limit_id, named,
                    "case {case:?}: entry {index} keeps the id the request named, got \
                     {rate_limit_id:?}"
                ),
                None => assert!(
                    well_formed_id(&rate_limit_id),
                    "case {case:?}: entry {index} is minted a well-formed id, got \
                     {rate_limit_id:?}"
                ),
            }
            assert_eq!(
                text(entry, "gatewayIdentifier"),
                gateway_id,
                "case {case:?}: entry {index} names the gateway it was written on, got {entry}"
            );
            assert_eq!(
                text(entry, "status"),
                "CREATING",
                "case {case:?}: entry {index} is CREATING until the clock owes it ACTIVE, got \
                 {entry}"
            );
            assert_eq!(
                entry.get("dimensionKeys"),
                Some(&json!(expected.dimension_keys)),
                "case {case:?}: entry {index} reports the dimension keys it was submitted \
                 with, got {entry}"
            );
            assert_eq!(
                entry.get("entries"),
                Some(&json!(expected.entries)),
                "case {case:?}: entry {index} reports the rule entries it was submitted with, \
                 got {entry}"
            );

            // The round trip: the stored limit reads back through the single-limit read.
            let read = get_rate_limit(&router, &gateway_id, &rate_limit_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: entry {index} must read back, got {} ({})",
                read.status, read.body
            );
            for member in ["rateLimitId", "gatewayIdentifier", "dimensionKeys", "entries"] {
                assert_eq!(
                    read.body.get(member),
                    entry.get(member),
                    "case {case:?}: entry {index} reads back the {member} the write reported, \
                     got {}",
                    read.body
                );
            }
            assert_eq!(
                read.body.get("description").cloned(),
                expected.description.map(|description| json!(description)),
                "case {case:?}: entry {index} reads back its description, got {}",
                read.body
            );
        }

        // The clock owes every written limit its ACTIVE, and nothing but the clock moves
        // it there.
        tokio::time::sleep(PAST_SETTLE).await;
        for entry in &reported {
            let settled = get_rate_limit(&router, &gateway_id, &text(entry, "rateLimitId")).await;
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
}

/// A second batch REPLACES the set: the entry it keeps is upserted onto the limit that id
/// already named — same creation instant, status `UPDATING` — and every limit the second
/// list leaves out is gone.
#[tokio::test]
async fn a_second_batch_replaces_the_set_rather_than_merging_into_it() {
    let router = router();
    let gateway_id = settled_gateway(&router, "ratelimitreplace").await;

    let first = vec![
        Submitted {
            rate_limit_id: Some("kept"),
            description: Some("the first rate"),
            dimension_keys: vec!["targetName"],
            entries: vec![json!({
                "dimensions": {"targetName": "orders"},
                "requests": rate(5.0, "second"),
            })],
        },
        Submitted {
            rate_limit_id: Some("dropped"),
            description: Some("left out of the second batch"),
            dimension_keys: vec!["toolName"],
            entries: vec![json!({
                "dimensions": {"toolName": "search"},
                "requests": rate(7.0, "second"),
            })],
        },
    ];
    let written = batch_put(&router, &gateway_id, &batch_body(&first)).await;
    assert_eq!(
        written.status, 200,
        "setup: the first batch must be written, got {} ({})",
        written.status, written.body
    );
    let created_at = get_rate_limit(&router, &gateway_id, "kept")
        .await
        .body
        .get("createdAt")
        .cloned()
        .unwrap_or(Value::Null);
    assert!(
        !created_at.is_null(),
        "setup: the first write dates the limit it created"
    );

    let second = vec![Submitted {
        rate_limit_id: Some("kept"),
        description: Some("the second rate"),
        dimension_keys: vec!["targetName"],
        entries: vec![json!({
            "dimensions": {"targetName": "orders"},
            "requests": rate(9.0, "minute"),
        })],
    }];
    let replaced = batch_put(&router, &gateway_id, &batch_body(&second)).await;
    assert_eq!(
        replaced.status, 200,
        "the second batch must be written, got {} ({})",
        replaced.status, replaced.body
    );
    assert_eq!(
        replaced
            .body
            .get("rateLimits")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "the resulting set is exactly the second submitted list, got {}",
        replaced.body
    );

    let upserted = get_rate_limit(&router, &gateway_id, "kept").await;
    assert_eq!(
        (upserted.status, text(&upserted.body, "status").as_str()),
        (200, "UPDATING"),
        "the upserted limit reports UPDATING, got {} {} ({})",
        upserted.status,
        text(&upserted.body, "status"),
        upserted.body
    );
    assert_eq!(
        upserted.body.get("createdAt"),
        Some(&created_at),
        "an upsert keeps the instant the limit it replaces was created, got {}",
        upserted.body
    );
    assert_eq!(
        upserted.body.get("entries"),
        Some(&json!(second[0].entries)),
        "the upserted limit carries the second batch's rule entries, got {}",
        upserted.body
    );
    assert_eq!(
        text(&upserted.body, "description"),
        "the second rate",
        "the upserted limit carries the second batch's description, got {}",
        upserted.body
    );

    let gone = get_rate_limit(&router, &gateway_id, "dropped").await;
    assert_eq!(
        (gone.status, error_code(&gone.body).as_str()),
        (404, "ResourceNotFoundException"),
        "a limit the second list leaves out is gone, got {} {} ({})",
        gone.status,
        error_code(&gone.body),
        gone.body
    );
}

/// Two writers racing on the same `rateLimitId`: at least one is served, and the set the
/// gateway ends up with is exactly one of the two submitted lists — never a merge.
#[tokio::test]
async fn concurrent_writers_leave_exactly_one_submitted_set() {
    let router = router();
    let gateway_id = settled_gateway(&router, "ratelimitrace").await;

    let left = vec![Submitted {
        rate_limit_id: Some("contended"),
        description: Some("left"),
        dimension_keys: vec!["targetName"],
        entries: vec![json!({
            "dimensions": {"targetName": "orders"},
            "requests": rate(11.0, "second"),
        })],
    }];
    let right = vec![
        Submitted {
            rate_limit_id: Some("contended"),
            description: Some("right"),
            dimension_keys: vec!["targetName"],
            entries: vec![json!({
                "dimensions": {"targetName": "orders"},
                "requests": rate(22.0, "minute"),
            })],
        },
        Submitted {
            rate_limit_id: Some("right-only"),
            description: Some("right"),
            dimension_keys: vec!["toolName"],
            entries: vec![json!({
                "dimensions": {"toolName": "search"},
                "tokens": rate(33.0, "minute"),
            })],
        },
    ];

    let left_body = batch_body(&left);
    let right_body = batch_body(&right);
    let (first, second) = tokio::join!(
        batch_put(&router, &gateway_id, &left_body),
        batch_put(&router, &gateway_id, &right_body),
    );
    let outcomes = [
        (first.status, error_code(&first.body)),
        (second.status, error_code(&second.body)),
    ];
    assert!(
        outcomes.iter().any(|(status, _)| *status == 200),
        "at least one concurrent writer is served, got {outcomes:?}"
    );
    for (status, code) in &outcomes {
        assert!(
            *status == 200 || (*status == 409 && code == "ConflictException"),
            "a loser is refused the modelled conflict, got {status} {code}"
        );
    }

    let contended = get_rate_limit(&router, &gateway_id, "contended").await;
    assert_eq!(
        contended.status, 200,
        "the contended limit is stored by whichever writer won, got {} ({})",
        contended.status, contended.body
    );
    let winner = text(&contended.body, "description");
    let right_only = get_rate_limit(&router, &gateway_id, "right-only").await;
    match winner.as_str() {
        "left" => assert_eq!(
            (right_only.status, error_code(&right_only.body).as_str()),
            (404, "ResourceNotFoundException"),
            "the left list has one entry, so nothing else is stored, got {} ({})",
            right_only.status,
            right_only.body
        ),
        "right" => assert_eq!(
            right_only.status, 200,
            "the right list has two entries, so both are stored, got {} ({})",
            right_only.status, right_only.body
        ),
        other => panic!("the stored set must be one submitted list, got description {other:?}"),
    }
}

/// A gateway identifier no gateway of this account holds is the modelled 404, and a
/// gateway that has not settled yet is refused the modelled 400 — the batch writes nothing
/// in either case.
#[tokio::test]
async fn the_batch_resolves_its_gateway_first() {
    let router = router();
    let body = batch_body(&[Submitted {
        rate_limit_id: Some("per-target"),
        description: None,
        dimension_keys: vec!["targetName"],
        entries: vec![json!({
            "dimensions": {"targetName": "orders"},
            "requests": rate(5.0, "second"),
        })],
    }]);

    // `GatewayIdentifier`'s own pattern admits this id; no gateway holds it.
    let missing = batch_put(&router, "nosuchgateway-abcdefghij", &body).await;
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
            "name": "ratelimitcreating",
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
    let creating = batch_put(&router, &text(&created.body, "gatewayId"), &body).await;
    assert_eq!(
        (creating.status, error_code(&creating.body).as_str()),
        (400, "ValidationException"),
        "a gateway the clock is still creating takes no rate limit, got {} {} ({})",
        creating.status,
        error_code(&creating.body),
        creating.body
    );
}

/// One row of the refusal table: a submitted set that cannot BE one gateway's rate-limit
/// set.
struct Refused {
    case: &'static str,
    submitted: Vec<Submitted>,
}

fn refusals() -> Vec<Refused> {
    vec![
        Refused {
            case: "the same rateLimitId twice in one batch",
            submitted: vec![
                Submitted {
                    rate_limit_id: Some("twice"),
                    description: None,
                    dimension_keys: vec!["targetName"],
                    entries: vec![json!({
                        "dimensions": {"targetName": "orders"},
                        "requests": rate(5.0, "second"),
                    })],
                },
                Submitted {
                    rate_limit_id: Some("twice"),
                    description: None,
                    dimension_keys: vec!["toolName"],
                    entries: vec![json!({
                        "dimensions": {"toolName": "search"},
                        "requests": rate(6.0, "second"),
                    })],
                },
            ],
        },
        Refused {
            case: "two limits sharing one set of dimension keys",
            submitted: vec![
                Submitted {
                    rate_limit_id: Some("shared-one"),
                    description: None,
                    dimension_keys: vec!["targetName", "toolName"],
                    entries: vec![json!({
                        "dimensions": {"targetName": "orders", "toolName": "create"},
                        "requests": rate(5.0, "second"),
                    })],
                },
                Submitted {
                    rate_limit_id: Some("shared-two"),
                    description: None,
                    dimension_keys: vec!["targetName", "toolName"],
                    entries: vec![json!({
                        "dimensions": {"targetName": "orders", "toolName": "read"},
                        "requests": rate(6.0, "second"),
                    })],
                },
            ],
        },
        Refused {
            case: "two limits whose dimension keys are the same set in a different order",
            submitted: vec![
                Submitted {
                    rate_limit_id: Some("permuted-one"),
                    description: None,
                    dimension_keys: vec!["targetName", "qualifiedModelId"],
                    entries: vec![json!({
                        "dimensions": {"targetName": "orders"},
                        "requests": rate(5.0, "second"),
                    })],
                },
                Submitted {
                    rate_limit_id: Some("permuted-two"),
                    description: None,
                    dimension_keys: vec!["qualifiedModelId", "targetName"],
                    entries: vec![json!({
                        "dimensions": {"qualifiedModelId": "anthropic.claude"},
                        "requests": rate(6.0, "second"),
                    })],
                },
            ],
        },
        Refused {
            case: "a rule entry naming a dimension the limit does not declare",
            submitted: vec![Submitted {
                rate_limit_id: Some("mismatched"),
                description: None,
                dimension_keys: vec!["targetName"],
                entries: vec![json!({
                    "dimensions": {"toolName": "search"},
                    "requests": rate(5.0, "second"),
                })],
            }],
        },
    ]
}

/// A batch that cannot form one gateway's set is refused the modelled 400 and writes
/// nothing: the set the gateway already held is untouched — "if the operation fails, the
/// service applies no changes".
#[tokio::test]
async fn a_batch_that_cannot_form_a_set_is_refused_and_writes_nothing() {
    let router = router();
    let gateway_id = settled_gateway(&router, "ratelimitrefused").await;
    let held = vec![Submitted {
        rate_limit_id: Some("already-held"),
        description: Some("written before the refusals"),
        dimension_keys: vec!["$.context.request.header.x-tenant"],
        entries: vec![json!({
            "dimensions": {"$.context.request.header.x-tenant": "acme"},
            "requests": rate(4.0, "second"),
        })],
    }];
    let written = batch_put(&router, &gateway_id, &batch_body(&held)).await;
    assert_eq!(
        written.status, 200,
        "setup: the held set must be written, got {} ({})",
        written.status, written.body
    );

    for Refused { case, submitted } in refusals() {
        let refused = batch_put(&router, &gateway_id, &batch_body(&submitted)).await;
        assert_eq!(
            (refused.status, error_code(&refused.body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {} {} ({})",
            refused.status,
            error_code(&refused.body),
            refused.body
        );
        assert!(
            text(&refused.body, "message").contains("rateLimits"),
            "case {case:?}: the refusal names the member it came in on, got {}",
            text(&refused.body, "message")
        );

        let still = get_rate_limit(&router, &gateway_id, "already-held").await;
        assert_eq!(
            still.status, 200,
            "case {case:?}: the refused batch leaves the held set in place, got {} ({})",
            still.status, still.body
        );
        for id in submitted.iter().filter_map(|entry| entry.rate_limit_id) {
            let unwritten = get_rate_limit(&router, &gateway_id, id).await;
            assert_eq!(
                (unwritten.status, error_code(&unwritten.body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: the refused batch writes no entry of its own, got {} ({})",
                unwritten.status,
                unwritten.body
            );
        }
    }
}
