// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAttacks`.
//!
//! Every case goes over the wire — an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AWSShield_20160616.ListAttacks` through the same `MultiServiceRouter`
//! the production router serves — so routing, deserialization, modelled-constraint
//! validation, the handler and serialization are all exercised.
//!
//! The rules under test, grounded in this operation's own Smithy documentation:
//!
//! * the operation "returns all ongoing DDoS attacks or all DDoS attacks during a
//!   specified time period", so every reported summary is one attack of the account's
//!   attack log, reported with the identifier, the attacked resource, the instants and
//!   the vectors that log holds (`AttackSummary`), and an attack still under way carries
//!   no `EndTime`;
//! * `ResourceArns` narrows the listing to the resources that were attacked, and "if you
//!   leave this blank, all applicable resources for this account will be included";
//! * `StartTime` and `EndTime` are each a `TimeRange`, which is
//!   `[FromInclusive, ToExclusive)`, placing the attack's own start and end instants: a
//!   bound the call leaves out constrains nothing, and an attack with no end instant is
//!   in no `EndTime` range a call asks for;
//! * `MaxResults` is "the greatest number of objects that you want Shield Advanced to
//!   return to the list request", defaulting to 20, and "if there are more objects
//!   remaining, Shield Advanced will always also return a NextToken value in the
//!   response" — the token names the attack the page stopped at in the account's log, so
//!   resuming it reports the attacks logged after that one that the call selects, and a
//!   `NextToken` the listing did not issue is refused as a bad parameter;
//! * the account's subscription does not gate the listing: this operation models no
//!   not-found failure at all;
//! * the operation reports, so repeating it answers the same thing and leaves the
//!   account's attack log as it was.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_shield_awsshield_20160616::state::{service_state, AttackRecord};
use service_com_amazonaws_shield_awsshield_20160616::EmulatedService;

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AWSShield_20160616.";

/// How AWS words a `NextToken` the listing did not issue.
const INVALID_PAGINATION_TOKEN: &str = "Invalid pagination token.";

/// The page size the model documents for a call naming no `MaxResults`:
/// `ListAttacksRequest$MaxResults` — "The default setting is 20."
const DEFAULT_PAGE_SIZE: usize = 20;

/// One hour, the grid the seeded attack log sits on.
const HOUR: Duration = Duration::from_secs(3_600);

/// One answer from the emulator: the HTTP status and the parsed JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type`), or `""` for a success. A `__type` may be
    /// namespaced (`com.amazonaws.shield#Foo`); the code clients branch on is the shape
    /// name.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .map(|t| t.rsplit('#').next().unwrap_or(t))
            .unwrap_or_default()
    }

    /// The error message as the envelope carries it, under either spelling the
    /// `awsJson1_1` error shape may use.
    fn error_message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Assert this reply is a success, and hand back its body.
    fn expect_ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected HTTP 200, got {self:?}"
        );
        &self.body
    }
}

/// The emulator under test: one shared engine, one registered service, requests
/// dispatched through the production router.
struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's; the
        // routing key still comes from the `Authorization` header below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, operation: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/us-east-1/shield/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0123456789abcdef",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{operation}: response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Reply { status, body }
    }

    /// Record one attack Shield detected against the account. Reaches the store directly
    /// because Shield detects attacks rather than being asked to create them: no
    /// modelled operation writes the attack log.
    fn detect_attack(&self, seed: &Seed) {
        let state = service_state(&*self.engine).expect("the shield store");
        let mut guard = state.lock();
        guard.attacks.push(AttackRecord {
            attack_id: seed.attack_id.to_string(),
            resource_arn: resource_arn(seed.resource),
            start_time: base_instant() + HOUR * seed.started_hour,
            end_time: seed.ended_hour.map(|hour| base_instant() + HOUR * hour),
            max_bits_per_second: None,
            max_packets_per_second: None,
            max_requests_per_second: None,
            attack_vectors: seed
                .vectors
                .iter()
                .map(|vector| (*vector).to_string())
                .collect(),
        });
    }

    /// How many attacks the account's log holds, so a reporting call can be shown to
    /// leave it alone.
    fn attacks_held(&self) -> usize {
        let state = service_state(&*self.engine).expect("the shield store");
        let guard = state.lock();
        guard.attacks.len()
    }
}

/// The instant the seeded attack log is measured from: a fixed point, because this
/// listing's answer depends on the request and the stored log, never on the clock.
fn base_instant() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600)
}

/// An instant as the epoch seconds the `awsJson1_1` wire form carries.
fn epoch_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// An hour offset from [`base_instant`] as its wire timestamp.
fn hour_timestamp(hour: u32) -> f64 {
    epoch_seconds(base_instant() + HOUR * hour)
}

/// The ARN of one of the account's attacked resources.
fn resource_arn(resource: &str) -> String {
    format!("arn:aws:cloudfront::000000000000:distribution/{resource}")
}

/// One attack to record before the call.
#[derive(Debug, Clone, Copy)]
struct Seed {
    attack_id: &'static str,
    /// The resource the attack was directed at, as [`resource_arn`] names it.
    resource: &'static str,
    /// When the attack started, in hours after [`base_instant`].
    started_hour: u32,
    /// When the attack ended, in hours after [`base_instant`], or `None` for an attack
    /// still under way.
    ended_hour: Option<u32>,
    /// The `AttackVectorDescription$VectorType` values the attack was attributed to.
    vectors: &'static [&'static str],
}

/// The account's attack log every table below reads, in the order Shield detected the
/// attacks. The vector types are values `AttackVectorDescription$VectorType` documents
/// ("The attack type. Valid values: UDP_TRAFFIC ... SYN_FLOOD ... REQUEST_FLOOD ...").
const LOG: &[Seed] = &[
    Seed {
        attack_id: "AttackId-alpha-1",
        resource: "alpha",
        started_hour: 0,
        ended_hour: Some(1),
        vectors: &["UDP_TRAFFIC"],
    },
    Seed {
        attack_id: "AttackId-beta-1",
        resource: "beta",
        started_hour: 2,
        ended_hour: Some(3),
        vectors: &["REQUEST_FLOOD", "HTTP_REFLECTION"],
    },
    Seed {
        // Still under way: no end instant at all.
        attack_id: "AttackId-alpha-2",
        resource: "alpha",
        started_hour: 4,
        ended_hour: None,
        vectors: &[],
    },
    Seed {
        attack_id: "AttackId-gamma-1",
        resource: "gamma",
        started_hour: 6,
        ended_hour: Some(7),
        vectors: &["SYN_FLOOD"],
    },
];

/// One row of the filter table: the request body, and the attacks it must report, in
/// order.
#[derive(Debug)]
struct Case {
    name: &'static str,
    request: fn() -> Value,
    expected: &'static [&'static str],
}

/// The `AttackId`s a reply reports, in the order it reported them.
fn reported_ids(case: &str, body: &Value) -> Vec<String> {
    summaries(case, body)
        .iter()
        .map(|summary| {
            summary
                .get("AttackId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: no AttackId in {summary}"))
                .to_string()
        })
        .collect()
}

/// The `AttackSummaries` a reply carries. The member is always present, so an account
/// with no matching attack reports an empty list rather than nothing.
fn summaries<'a>(case: &str, body: &'a Value) -> &'a Vec<Value> {
    body.get("AttackSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: no AttackSummaries in {body}"))
}

/// An emulator whose account has seen every attack of [`LOG`].
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    for seed in LOG {
        emulator.detect_attack(seed);
    }
    emulator
}

#[tokio::test]
async fn list_attacks_reports_the_logged_attacks_the_filters_select() {
    // Every row varies what it filters on: the attacked resources (one, several, one
    // never attacked, and the blank list), each time range's two bounds on their own and
    // together, and the two filter kinds combined.
    const CASES: &[Case] = &[
        Case {
            name: "no filter at all reports the whole log in detection order",
            request: || json!({}),
            expected: &[
                "AttackId-alpha-1",
                "AttackId-beta-1",
                "AttackId-alpha-2",
                "AttackId-gamma-1",
            ],
        },
        Case {
            name: "one attacked resource",
            request: || json!({"ResourceArns": [resource_arn("alpha")]}),
            expected: &["AttackId-alpha-1", "AttackId-alpha-2"],
        },
        Case {
            name: "two attacked resources",
            request: || json!({"ResourceArns": [resource_arn("gamma"), resource_arn("beta")]}),
            expected: &["AttackId-beta-1", "AttackId-gamma-1"],
        },
        Case {
            name: "a resource no attack hit reports nothing",
            request: || json!({"ResourceArns": [resource_arn("delta")]}),
            expected: &[],
        },
        Case {
            name: "a blank resource list narrows nothing",
            request: || json!({"ResourceArns": []}),
            expected: &[
                "AttackId-alpha-1",
                "AttackId-beta-1",
                "AttackId-alpha-2",
                "AttackId-gamma-1",
            ],
        },
        Case {
            name: "a start range includes its lower bound and excludes its upper bound",
            request: || {
                json!({"StartTime": {
                    "FromInclusive": hour_timestamp(2),
                    "ToExclusive": hour_timestamp(6),
                }})
            },
            expected: &["AttackId-beta-1", "AttackId-alpha-2"],
        },
        Case {
            name: "a start range with only a lower bound is open above",
            request: || json!({"StartTime": {"FromInclusive": hour_timestamp(4)}}),
            expected: &["AttackId-alpha-2", "AttackId-gamma-1"],
        },
        Case {
            name: "a start range with only an upper bound is open below",
            request: || json!({"StartTime": {"ToExclusive": hour_timestamp(2)}}),
            expected: &["AttackId-alpha-1"],
        },
        Case {
            name: "a start range no attack started in reports nothing",
            request: || {
                json!({"StartTime": {
                    "FromInclusive": hour_timestamp(100),
                    "ToExclusive": hour_timestamp(200),
                }})
            },
            expected: &[],
        },
        Case {
            name: "an end range places the attack's end, and an ongoing attack has none",
            request: || {
                json!({"EndTime": {
                    "FromInclusive": hour_timestamp(1),
                    "ToExclusive": hour_timestamp(7),
                }})
            },
            expected: &["AttackId-alpha-1", "AttackId-beta-1"],
        },
        Case {
            name: "an end range open above still excludes the ongoing attack",
            request: || json!({"EndTime": {"FromInclusive": hour_timestamp(0)}}),
            expected: &[
                "AttackId-alpha-1",
                "AttackId-beta-1",
                "AttackId-gamma-1",
            ],
        },
        Case {
            name: "both ranges and a resource list all narrow together",
            request: || {
                json!({
                    "ResourceArns": [resource_arn("alpha"), resource_arn("beta")],
                    "StartTime": {"FromInclusive": hour_timestamp(0)},
                    "EndTime": {"ToExclusive": hour_timestamp(3)},
                })
            },
            expected: &["AttackId-alpha-1"],
        },
    ];

    for case in CASES {
        let emulator = seeded();
        let reply = emulator.call("ListAttacks", &(case.request)()).await;
        let body = reply.expect_ok(case.name);
        let reported = reported_ids(case.name, body);
        assert_eq!(
            reported,
            case.expected.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "case {}: expected {:?}, got {:?}",
            case.name,
            case.expected,
            reported
        );
        // The whole log fits one default page, so a complete answer ends the iteration.
        assert!(
            LOG.len() <= DEFAULT_PAGE_SIZE && body.get("NextToken").is_none(),
            "case {}: a complete page must carry no NextToken, got {body}",
            case.name
        );
    }
}

#[tokio::test]
async fn an_account_with_no_detected_attacks_reports_an_empty_list() {
    let emulator = Emulator::new();
    let reply = emulator.call("ListAttacks", &json!({})).await;
    let body = reply.expect_ok("no attacks at all");
    assert!(
        summaries("no attacks at all", body).is_empty(),
        "case no attacks at all: expected an empty AttackSummaries, got {body}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "case no attacks at all: expected no NextToken, got {body}"
    );
}

#[tokio::test]
async fn each_summary_reports_the_logged_attack() {
    let emulator = seeded();
    let reply = emulator.call("ListAttacks", &json!({})).await;
    let body = reply.expect_ok("summary fidelity");
    let reported = summaries("summary fidelity", body);
    assert_eq!(
        reported.len(),
        LOG.len(),
        "case summary fidelity: expected {} summaries, got {reported:?}",
        LOG.len()
    );

    for (seed, summary) in LOG.iter().zip(reported) {
        let case = seed.attack_id;
        assert_eq!(
            summary.get("AttackId").and_then(Value::as_str),
            Some(seed.attack_id),
            "case {case}: expected AttackId {}, got {summary}",
            seed.attack_id
        );
        assert_eq!(
            summary.get("ResourceArn").and_then(Value::as_str),
            Some(resource_arn(seed.resource).as_str()),
            "case {case}: expected ResourceArn {}, got {summary}",
            resource_arn(seed.resource)
        );
        assert_eq!(
            summary.get("StartTime").and_then(Value::as_f64),
            Some(hour_timestamp(seed.started_hour)),
            "case {case}: expected StartTime {}, got {summary}",
            hour_timestamp(seed.started_hour)
        );
        assert_eq!(
            summary.get("EndTime").and_then(Value::as_f64),
            seed.ended_hour.map(hour_timestamp),
            "case {case}: expected EndTime {:?}, got {summary}",
            seed.ended_hour.map(hour_timestamp)
        );
        let vectors: Vec<&str> = summary
            .get("AttackVectors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: no AttackVectors in {summary}"))
            .iter()
            .map(|vector| {
                vector
                    .get("VectorType")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: no VectorType in {vector}"))
            })
            .collect();
        assert_eq!(
            vectors, seed.vectors,
            "case {case}: expected AttackVectors {:?}, got {summary}",
            seed.vectors
        );
    }
}

/// One row of the pagination table: the page size asked for, the filter applied, and the
/// pages the walk must then see.
#[derive(Debug)]
struct PageCase {
    name: &'static str,
    max_results: Option<i32>,
    resource: Option<&'static str>,
    pages: &'static [&'static [&'static str]],
}

#[tokio::test]
async fn max_results_pages_the_selected_attacks() {
    // The rows differ in the page size (including the 0 the member's range admits and the
    // absent one that takes the documented default), in how evenly the page size divides
    // the selection, and in whether a filter narrows what is paged.
    const CASES: &[PageCase] = &[
        PageCase {
            name: "two per page over four attacks",
            max_results: Some(2),
            resource: None,
            pages: &[
                &["AttackId-alpha-1", "AttackId-beta-1"],
                &["AttackId-alpha-2", "AttackId-gamma-1"],
            ],
        },
        PageCase {
            name: "one per page over four attacks",
            max_results: Some(1),
            resource: None,
            pages: &[
                &["AttackId-alpha-1"],
                &["AttackId-beta-1"],
                &["AttackId-alpha-2"],
                &["AttackId-gamma-1"],
            ],
        },
        PageCase {
            name: "a page size the selection does not divide by",
            max_results: Some(3),
            resource: None,
            pages: &[
                &["AttackId-alpha-1", "AttackId-beta-1", "AttackId-alpha-2"],
                &["AttackId-gamma-1"],
            ],
        },
        PageCase {
            name: "a page size larger than the selection is one page",
            max_results: Some(9),
            resource: None,
            pages: &[&[
                "AttackId-alpha-1",
                "AttackId-beta-1",
                "AttackId-alpha-2",
                "AttackId-gamma-1",
            ]],
        },
        PageCase {
            name: "a page size of zero takes the documented default of 20",
            max_results: Some(0),
            resource: None,
            pages: &[&[
                "AttackId-alpha-1",
                "AttackId-beta-1",
                "AttackId-alpha-2",
                "AttackId-gamma-1",
            ]],
        },
        PageCase {
            name: "no page size takes the documented default of 20",
            max_results: None,
            resource: None,
            pages: &[&[
                "AttackId-alpha-1",
                "AttackId-beta-1",
                "AttackId-alpha-2",
                "AttackId-gamma-1",
            ]],
        },
        PageCase {
            name: "paging a filtered selection pages only what the filter selected",
            max_results: Some(1),
            resource: Some("alpha"),
            pages: &[&["AttackId-alpha-1"], &["AttackId-alpha-2"]],
        },
    ];

    for case in CASES {
        let emulator = seeded();
        let mut next_token: Option<String> = None;
        let mut seen: Vec<Vec<String>> = Vec::new();

        loop {
            let mut request = json!({});
            if let Some(max_results) = case.max_results {
                request["MaxResults"] = json!(max_results);
            }
            if let Some(resource) = case.resource {
                request["ResourceArns"] = json!([resource_arn(resource)]);
            }
            if let Some(token) = next_token.as_deref() {
                request["NextToken"] = json!(token);
            }

            let reply = emulator.call("ListAttacks", &request).await;
            let body = reply.expect_ok(case.name);
            seen.push(reported_ids(case.name, body));
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert!(
                seen.len() <= case.pages.len() + 1,
                "case {}: the walk did not end after {} pages, got {seen:?}",
                case.name,
                case.pages.len()
            );
            if next_token.is_none() {
                break;
            }
        }

        let expected: Vec<Vec<String>> = case
            .pages
            .iter()
            .map(|page| page.iter().map(|id| id.to_string()).collect())
            .collect();
        assert_eq!(
            seen, expected,
            "case {}: expected the pages {expected:?}, got {seen:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    // Each row is a token the listing cannot have minted: free text, a hex-encoded
    // position without the listing's own prefix, and a token of ANOTHER listing of this
    // service. The encoder is the one the production token spelling uses, so a row
    // probes the real decoder rather than a second encoding written here.
    let cases: Vec<(&str, String)> = vec![
        ("free text", "not-a-token".to_string()),
        ("a bare position", ::hex::encode("0")),
        (
            "another listing's token",
            ::hex::encode("protection-groups/v1:0"),
        ),
        (
            "this listing's prefix with no position",
            ::hex::encode("attacks/v1:"),
        ),
    ];

    for (name, token) in cases {
        let emulator = seeded();
        let reply = emulator
            .call("ListAttacks", &json!({"NextToken": token}))
            .await;
        assert_eq!(
            (
                reply.status,
                reply.error_code(),
                reply.error_message(),
            ),
            (400, "InvalidParameterException", INVALID_PAGINATION_TOKEN),
            "case {name}: expected HTTP 400 InvalidParameterException \
             {INVALID_PAGINATION_TOKEN:?}, got {reply:?}"
        );
        let fields: Vec<&str> = reply
            .body
            .get("fields")
            .or_else(|| reply.body.get("Fields"))
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| {
                        field
                            .get("name")
                            .or_else(|| field.get("Name"))
                            .and_then(Value::as_str)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fields,
            vec!["NextToken"],
            "case {name}: expected the refusal to name the NextToken member, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_token_names_the_attack_it_stopped_at_in_the_log() {
    // The coordinate a token carries is the attack's own place in the account's log, so
    // resuming it under narrower filters reports the attacks logged after it that the
    // new call selects — it neither skips an attack the caller has not seen nor repeats
    // one it has.
    let emulator = seeded();
    let first = emulator
        .call("ListAttacks", &json!({"MaxResults": 2}))
        .await;
    let first_body = first.expect_ok("resumed under a narrower filter");
    assert_eq!(
        reported_ids("resumed under a narrower filter", first_body),
        vec![
            "AttackId-alpha-1".to_string(),
            "AttackId-beta-1".to_string()
        ],
        "case resumed under a narrower filter: expected the first two logged attacks, \
         got {first_body}"
    );
    let token = first_body
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a first page of two of four attacks carries a NextToken")
        .to_string();

    let resumed = emulator
        .call(
            "ListAttacks",
            &json!({"NextToken": token, "ResourceArns": [resource_arn("alpha")]}),
        )
        .await;
    let resumed_body = resumed.expect_ok("resumed under a narrower filter");
    assert_eq!(
        reported_ids("resumed under a narrower filter", resumed_body),
        vec!["AttackId-alpha-2".to_string()],
        "case resumed under a narrower filter: expected only the attack on that \
         resource logged after the token's attack, got {resumed_body}"
    );
    assert!(
        resumed_body.get("NextToken").is_none(),
        "case resumed under a narrower filter: expected the iteration to end, \
         got {resumed_body}"
    );
}

#[tokio::test]
async fn a_token_the_selection_has_run_past_is_an_empty_last_page() {
    let emulator = seeded();
    let first = emulator
        .call("ListAttacks", &json!({"MaxResults": 3}))
        .await;
    let token = first
        .expect_ok("a token the selection has run past")
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a first page of three of four attacks carries a NextToken")
        .to_string();

    // Every attack on this resource was logged before the token's attack, so the
    // selection has nothing left: an empty last page, not an error.
    let reply = emulator
        .call(
            "ListAttacks",
            &json!({"NextToken": token, "ResourceArns": [resource_arn("alpha")]}),
        )
        .await;
    let body = reply.expect_ok("a token the selection has run past");
    assert!(
        summaries("a token the selection has run past", body).is_empty()
            && body.get("NextToken").is_none(),
        "case a token the selection has run past: expected an empty last page, got {body}"
    );
}

#[tokio::test]
async fn listing_attacks_changes_nothing_a_later_caller_sees() {
    let emulator = seeded();
    let held = emulator.attacks_held();

    let first = emulator.call("ListAttacks", &json!({})).await;
    let first_body = first.expect_ok("repeated read").clone();
    let second = emulator.call("ListAttacks", &json!({})).await;
    let second_body = second.expect_ok("repeated read");

    assert_eq!(
        &first_body, second_body,
        "case repeated read: a reporting operation must answer the same thing twice, \
         got {first_body:?} then {second_body:?}"
    );
    assert_eq!(
        emulator.attacks_held(),
        held,
        "case repeated read: the attack log must be untouched, it held {held} records"
    );
}

#[tokio::test]
async fn the_listing_does_not_depend_on_the_account_subscription() {
    // `ListAttacks` models no not-found failure, so an account that holds no Shield
    // Advanced subscription still has its detected attacks reported.
    let emulator = seeded();
    let state = service_state(&*emulator.engine).expect("the shield store");
    assert!(
        state.lock().subscription.is_none(),
        "case no subscription: the test account must hold no subscription"
    );
    let reply = emulator.call("ListAttacks", &json!({})).await;
    let body = reply.expect_ok("no subscription");
    assert_eq!(
        reported_ids("no subscription", body).len(),
        LOG.len(),
        "case no subscription: expected every logged attack, got {body}"
    );
}
