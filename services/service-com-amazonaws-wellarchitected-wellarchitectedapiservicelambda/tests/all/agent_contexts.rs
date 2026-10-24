// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for an agent profile's contexts: `ListAgentContexts`, the
//! `CreateAgentContext` that records one and the `GetAgentContext` that reads one back.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. A context is STORED under the profile it was created against: `CreateAgentContext`
//!    answers a `ContextSummary` carrying the request's `title`, `contextType` and typed
//!    `content`, a freshly minted `UUID` and the `createdBy` / `createdAt` the service
//!    owns, and `GetAgentContext` and `ListAgentContexts` both report that identical
//!    summary. `applicationType` and `criticality` are DERIVED from the stored content,
//!    so they agree with it member for member.
//! 2. `ListAgentContexts` reports the contexts of ONE profile: another profile's
//!    contexts never appear, a profile that holds none reports an empty `items`, and a
//!    well-formed `profileArn` that names no stored profile reports an empty `items`
//!    too — the operation models `AccessDenied`, `InternalServer`, `Throttling` and
//!    `Validation` and NO `ResourceNotFoundException`, so it has no modelled way to
//!    report a missing profile.
//! 3. Paging enumerates every context EXACTLY ONCE: no page is longer than the
//!    `maxResults` asked for, the ids are strictly ascending across the pages, the union
//!    of the pages is the whole stored set, and only the last page omits `nextToken`.
//! 4. `maxResults` is held to its published `@range(min: 1, max: 1000)`: both bounds are
//!    served, and a value one past either bound is refused `400 ValidationException`.
//! 5. A `nextToken` this service did not issue is refused `400 ValidationException` with
//!    reason `CANNOT_PARSE` naming the member — never an empty page, and never the first
//!    page again. A cursor another paginated operation issued is not a cursor here.
//! 6. A `clientToken` makes `CreateAgentContext` idempotent: the same token with the same
//!    parameters answers the context the original request created and records no second
//!    one, the same token with different parameters is a `409 ConflictException`, and a
//!    fresh token records a second context.
//! 7. A context is never stored naming a value its enums do not publish: `contextType`,
//!    `content.applicationType` and `content.criticality` outside their published sets
//!    are refused `400 ValidationException` and leave the list unchanged.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`; the profile the
//! contexts hang off is minted by `super::agent_goals::profile_arn_of`.

use serde_json::{json, Value};

use super::agent_goals::profile_arn_of;
use super::workload_harness::{encode_path_segment, Answer, Client};

/// Every value the `ContextType` enum publishes
/// (`smithy_query(query="shape", shape="ContextType")`). Spelled out here so the suite
/// pins the catalog rather than reading it back out of the code under test.
const CONTEXT_TYPES: [&str; 1] = ["APPLICATION"];

/// Every value the `ApplicationType` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="ApplicationType")`).
const APPLICATION_TYPES: [&str; 3] = ["SAS", "DESKTOP_APPLICATION", "OTHER"];

/// Every value the `Criticality` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="Criticality")`).
const CRITICALITIES: [&str; 4] = [
    "MISSION_CRITICAL",
    "BUSINESS_CRITICAL",
    "NON_CRITICAL",
    "TEST_DEVELOPMENT",
];

/// The published bounds of `ListAgentContextsRequest$maxResults`
/// (`@range(min: 1, max: 1000)` —
/// `smithy_query(query="operation", operation="ListAgentContexts")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 1_000;

/// `POST /api/v1/agent-profiles/{profileArn}/contexts`: the ARN is a non-greedy
/// `@httpLabel`, so it travels percent-encoded in one path segment, exactly as an SDK
/// sends it.
async fn create_context(client: &Client, profile_arn: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!(
                "/api/v1/agent-profiles/{}/contexts",
                encode_path_segment(profile_arn)
            ),
            Some(body),
        )
        .await
}

/// `GET /api/v1/agent-profiles/{profileArn}/contexts/{id}`.
async fn get_context(client: &Client, profile_arn: &str, id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-profiles/{}/contexts/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(id)
            ),
            None,
        )
        .await
}

/// `GET /api/v1/agent-profiles/{profileArn}/contexts`, with the paging members bound to
/// the query string. `query` is passed as sent, empty for a request that names none.
async fn list_contexts(client: &Client, profile_arn: &str, query: &str) -> Answer {
    let path = format!(
        "/api/v1/agent-profiles/{}/contexts",
        encode_path_segment(profile_arn)
    );
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    client.send("GET", &uri, None).await
}

/// A well-formed `AgentProfileArn` (the modelled
/// `^arn:aws…:wellarchitected:<region>:<account>:agent-profile/[a-zA-Z0-9_-]+$`) that no
/// stored profile carries.
fn absent_profile_arn(resource: &str) -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:agent-profile/{resource}",
        engine::emulator_account_id()
    )
}

/// The `ContextSummary` a context answer carries, under the member the operation names
/// it (`context` on the single-context operations).
fn summary(answer: &Answer) -> &Value {
    answer
        .body
        .get("context")
        .unwrap_or_else(|| panic!("expected a context member in {answer:?}"))
}

/// The `items` a `ListAgentContexts` answer carries.
fn items(answer: &Answer) -> &[Value] {
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_else(|| panic!("expected an items list in {answer:?}"))
}

fn id_of(value: &Value) -> String {
    value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A context id, checked against the modelled `UUID` pattern
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
fn assert_uuid(id: &str, case: &str) {
    let groups: Vec<&str> = id.split('-').collect();
    assert!(
        groups.len() == 5
            && [8usize, 4, 4, 4, 12]
                .iter()
                .zip(&groups)
                .all(|(len, group)| group.len() == *len
                    && group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))),
        "case {case:?}: context id must match the modelled UUID pattern, got {id:?}"
    );
}

/// The instant a create happened. The WIRE form belongs to the framework's timestamp
/// serializer (epoch seconds for a number, ISO-8601 for a string); what this suite pins
/// is that the stamp is the moment of the CALL rather than a constant, checked against
/// the test's own clock.
fn assert_create_instant(value: Option<&Value>, case: &str, label: &str) {
    match value {
        Some(Value::Number(number)) => {
            let seconds = number.as_f64().unwrap_or_default();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the test clock is after the Unix epoch")
                .as_secs_f64();
            assert!(
                seconds > 0.0 && (now - seconds).abs() < 300.0,
                "case {case:?}: {label} must be the instant of the call (about {now}), \
                 got {seconds}"
            );
        }
        Some(Value::String(stamp)) => {
            assert!(
                stamp.contains('T') && stamp.len() >= 20,
                "case {case:?}: {label} must be an ISO-8601 instant, got {stamp:?}"
            );
        }
        other => panic!("case {case:?}: {label} must be present, got {other:?}"),
    }
}

/// The rows every context test spans. They differ along what the rule generalises over:
/// which optional `content` members are present, how many entries each content list
/// carries (none, one, several), which published `applicationType` / `criticality` the
/// content names — including values the issue did not cite — and titles at the modelled
/// `SensitiveString` `@length(1, 1000)` bounds.
fn context_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "bare-content",
            json!({
                "title": "An application with nothing but a title",
                "contextType": CONTEXT_TYPES[0],
                "content": {},
            }),
        ),
        (
            "one-account-one-region",
            json!({
                "title": "Checkout service",
                "contextType": CONTEXT_TYPES[0],
                "content": {
                    "accountIds": [engine::emulator_account_id()],
                    "regions": ["us-east-1"],
                    "awsServices": ["lambda"],
                    "applicationType": APPLICATION_TYPES[0],
                    "criticality": CRITICALITIES[0],
                },
            }),
        ),
        (
            "several-of-every-list",
            json!({
                "title": "Fleet-wide inventory",
                "contextType": CONTEXT_TYPES[0],
                "content": {
                    "accountIds": [engine::emulator_account_id(), "210987654321"],
                    "regions": ["eu-west-1", "ap-southeast-2", "us-west-2"],
                    "awsServices": ["dynamodb", "ec2", "s3"],
                    "resourceTypes": ["AWS::EC2::Instance", "AWS::S3::Bucket"],
                    "resourceTags": [
                        { "key": "env", "value": "prod" },
                        { "key": "team", "value": "" },
                    ],
                    "applicationOverview": "Everything the platform team owns",
                    "industry": "Retail",
                    "architectureOverview": "Three tiers behind a load balancer",
                    "additionalContext": "Reviewed quarterly",
                    "applicationType": APPLICATION_TYPES[1],
                    "criticality": CRITICALITIES[3],
                },
            }),
        ),
        (
            "enum-values-the-issue-did-not-cite",
            json!({
                "title": "Batch reporting",
                "contextType": CONTEXT_TYPES[0],
                "content": {
                    "applicationType": APPLICATION_TYPES[2],
                    "criticality": CRITICALITIES[2],
                },
            }),
        ),
        (
            "shortest-title",
            json!({
                "title": "x",
                "contextType": CONTEXT_TYPES[0],
                "content": { "regions": ["sa-east-1"] },
            }),
        ),
        (
            "longest-title",
            json!({
                "title": "t".repeat(1000),
                "contextType": CONTEXT_TYPES[0],
                "content": {
                    "applicationOverview": "o".repeat(10_000),
                    "criticality": CRITICALITIES[1],
                },
            }),
        ),
    ]
}

/// Assert one `ContextSummary` reports exactly what `request` asked for plus the members
/// the SERVICE owns.
fn assert_summary_matches(value: &Value, request: &Value, profile_arn: &str, case: &str) {
    assert_uuid(&id_of(value), case);
    for member in ["title", "contextType", "content"] {
        assert_eq!(
            value.get(member),
            request.get(member),
            "case {case:?}: {member} must report what the request carried, got {value:?}"
        );
    }
    assert_eq!(
        value.get("profileArn").and_then(Value::as_str),
        Some(profile_arn),
        "case {case:?}: profileArn must name the profile the context was created \
         against, got {value:?}"
    );
    // `applicationType` / `criticality` describe the application the CONTENT describes,
    // so they agree with the content member for member — including by being absent when
    // the content names neither.
    for member in ["applicationType", "criticality"] {
        assert_eq!(
            value.get(member),
            request.get("content").and_then(|content| content.get(member)),
            "case {case:?}: {member} must agree with the stored content, got {value:?}"
        );
    }
    assert_eq!(
        value.get("createdBy").and_then(Value::as_str),
        Some(engine::emulator_account_id()),
        "case {case:?}: createdBy must be the account the emulator serves, got {value:?}"
    );
    assert_create_instant(value.get("createdAt"), case, "createdAt");
    for member in ["lastModifiedBy", "lastModifiedAt"] {
        assert!(
            value.get(member).is_none_or(Value::is_null),
            "case {case:?}: {member} must be absent on a context nothing has modified, \
             got {value:?}"
        );
    }
}

/// Rule 1: every row round-trips — the create's summary, the `GetAgentContext` of its id
/// and the entry `ListAgentContexts` reports are the same summary, and each reports what
/// the request carried plus the service-owned members.
#[tokio::test]
async fn a_created_context_round_trips_through_get_and_list() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-host", 1).await;

    let mut created: Vec<(&str, Value)> = Vec::new();
    for (case, request) in context_rows() {
        let answer = create_context(&client, &profile_arn, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateAgentContext 200, got {answer:?}"
        );
        let created_summary = summary(&answer).clone();
        assert_summary_matches(&created_summary, &request, &profile_arn, case);

        let read = get_context(&client, &profile_arn, &id_of(&created_summary)).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetAgentContext 200, got {read:?}"
        );
        assert_eq!(
            summary(&read),
            &created_summary,
            "case {case:?}: GetAgentContext must report the stored context, got {read:?}"
        );
        created.push((case, created_summary));
    }

    let listed = list_contexts(&client, &profile_arn, "maxResults=1000").await;
    assert_eq!(
        listed.status, 200,
        "expected ListAgentContexts 200, got {listed:?}"
    );
    let reported = items(&listed);
    assert_eq!(
        reported.len(),
        created.len(),
        "the list must report every stored context, got {listed:?}"
    );
    for (case, summary) in &created {
        assert!(
            reported.contains(summary),
            "case {case:?}: the list must report the stored summary {summary:?}, \
             got {reported:?}"
        );
    }    let ids: Vec<String> = reported.iter().map(id_of).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "the list must report contexts in one stable id order, got {ids:?}"
    );
    assert!(
        listed.body.get("nextToken").is_none_or(Value::is_null),
        "a page that reported every context must carry no nextToken, got {listed:?}"
    );
}

/// Rule 2: the list is scoped to ONE profile — a sibling profile's contexts, a profile
/// holding none and an ARN no profile carries.
#[tokio::test]
async fn contexts_are_listed_per_profile() {
    let client = Client::new();
    let first = profile_arn_of(&client, "context-host-first", 1).await;
    let second = profile_arn_of(&client, "context-host-second", 2).await;
    let empty = profile_arn_of(&client, "context-host-empty", 0).await;

    // A different number of contexts under each profile, so a handler that ignored the
    // ARN could not pass by accident.
    let mut expected: Vec<(&str, Vec<String>)> = Vec::new();
    for (case, profile_arn, count) in [("first", &first, 1usize), ("second", &second, 3)] {
        let mut ids = Vec::new();
        for index in 0..count {
            let answer = create_context(
                &client,
                profile_arn,
                json!({
                    "title": format!("{case} context {index}"),
                    "contextType": CONTEXT_TYPES[0],
                    "content": { "regions": ["us-east-1"] },
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected CreateAgentContext 200, got {answer:?}"
            );
            ids.push(id_of(summary(&answer)));
        }
        ids.sort();
        expected.push((case, ids));
    }

    for ((case, ids), profile_arn) in expected.iter().zip([&first, &second]) {
        let answer = list_contexts(&client, profile_arn, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListAgentContexts 200, got {answer:?}"
        );
        let reported: Vec<String> = items(&answer).iter().map(id_of).collect();
        assert_eq!(
            &reported, ids,
            "case {case:?}: the list must report exactly this profile's contexts, \
             got {answer:?}"
        );
    }

    // A profile that holds none, and an ARN that names no stored profile: the operation
    // models no `ResourceNotFoundException`, so both report an empty page rather than a
    // failure, and neither reports another profile's contexts.
    for (case, profile_arn) in [
        ("profile-with-no-contexts", empty),
        ("profile-that-does-not-exist", absent_profile_arn("absent")),
    ] {
        let answer = list_contexts(&client, &profile_arn, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListAgentContexts 200, got {answer:?}"
        );
        assert!(
            items(&answer).is_empty(),
            "case {case:?}: expected an empty items list, got {answer:?}"
        );
        assert!(
            answer.body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: an empty page carries no nextToken, got {answer:?}"
        );
    }
}

/// Rule 3: paging over a stored set with several page sizes enumerates every context
/// exactly once, in ascending id order, and only the last page omits `nextToken`.
#[tokio::test]
async fn paging_enumerates_every_context_exactly_once() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-paging", 1).await;

    let stored = 7;
    let mut all_ids: Vec<String> = Vec::new();
    for index in 0..stored {
        let answer = create_context(
            &client,
            &profile_arn,
            json!({
                "title": format!("paged context {index}"),
                "contextType": CONTEXT_TYPES[0],
                "content": { "awsServices": ["s3"] },
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "expected CreateAgentContext 200 for context {index}, got {answer:?}"
        );
        all_ids.push(id_of(summary(&answer)));
    }
    all_ids.sort();

    // A page size below, at and above the stored count, plus the request that names none
    // (the member's `@default(100)`), so the rule is asserted where the page divides the
    // set evenly, unevenly and not at all.
    for page_size in [None, Some(1), Some(2), Some(3), Some(stored), Some(stored + 4)] {
        let case = format!("maxResults={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = match page_size {
                Some(size) => format!("maxResults={size}"),
                None => String::new(),
            };
            if let Some(token) = &next_token {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("nextToken={token}"));
            }
            let answer = list_contexts(&client, &profile_arn, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected ListAgentContexts 200, got {answer:?}"
            );
            let page: Vec<String> = items(&answer).iter().map(id_of).collect();
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case {case:?}: a page must not be longer than maxResults, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= stored + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {stored} \
                 contexts"
            );
            next_token = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all_ids,
            "case {case:?}: paging must enumerate every stored context exactly once, in \
             ascending id order"
        );
    }
}

/// Rule 4: `maxResults` at each published bound is served; one past either bound is
/// refused by the boundary as a `400 ValidationException`.
#[tokio::test]
async fn max_results_is_held_to_its_published_range() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-max-results", 1).await;
    for index in 0..2 {
        let answer = create_context(
            &client,
            &profile_arn,
            json!({
                "title": format!("bounded context {index}"),
                "contextType": CONTEXT_TYPES[0],
                "content": {},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "expected CreateAgentContext 200 for context {index}, got {answer:?}"
        );
    }

    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let query = format!("maxResults={max_results}");
        let answer = list_contexts(&client, &profile_arn, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {max_results}: a published bound must be served, got {answer:?}"
        );
        let served = items(&answer).len();
        assert!(
            served >= 1 && served <= usize::try_from(max_results).expect("a positive bound"),
            "case {max_results}: expected at most {max_results} of the 2 stored \
             contexts, got {served}"
        );
    }

    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let query = format!("maxResults={max_results}");
        let answer = list_contexts(&client, &profile_arn, &query).await;
        assert_eq!(
            answer.status, 400,
            "case {max_results}: a value past a published bound must be refused, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {max_results}: expected a ValidationException, got {answer:?}"
        );
    }
}

/// Rule 5: a `nextToken` this service did not issue is a refusal, not a page.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-tokens", 1).await;
    let created = create_context(
        &client,
        &profile_arn,
        json!({
            "title": "the only context",
            "contextType": CONTEXT_TYPES[0],
            "content": {},
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "expected CreateAgentContext 200, got {created:?}"
    );

    // Each row satisfies the modelled `NextToken` pattern `^[A-Za-z0-9+/=_-]+$`, so the
    // boundary passes it to the handler: what is under test is the handler's own refusal.
    // The last row is a cursor ANOTHER paginated operation of this service issues, which
    // is not a cursor here.
    let rows: Vec<(&str, String)> = vec![
        ("not-a-cursor", "notacursor".to_string()),
        ("hex-of-nothing", "6e756c6c".to_string()),
        ("one-character", "a".to_string()),
        (
            "another-operations-cursor",
            service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::encode_share_next_token(
                "0123456789abcdef0123456789abcdef",
            ),
        ),
    ];
    for (case, token) in rows {
        let answer = list_contexts(&client, &profile_arn, &format!("nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unissued nextToken must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: an undecodable cursor is CANNOT_PARSE, got {answer:?}"
        );
        let fields: Vec<&str> = answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("Name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fields,
            vec!["NextToken"],
            "case {case:?}: the finding must name the member whose value could not be \
             decoded, got {answer:?}"
        );
    }
}

/// Rule 6: `clientToken` idempotency — a replay answers the original context, a reused
/// token with changed parameters is a conflict, and a fresh token records a second
/// context.
#[tokio::test]
async fn a_client_token_makes_the_create_idempotent() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-idempotency", 1).await;
    let original = json!({
        "clientToken": "token-one",
        "title": "Payments API",
        "contextType": CONTEXT_TYPES[0],
        "content": { "awsServices": ["apigateway"], "criticality": CRITICALITIES[0] },
    });

    let first = create_context(&client, &profile_arn, original.clone()).await;
    assert_eq!(
        first.status, 200,
        "expected the first CreateAgentContext 200, got {first:?}"
    );
    let created = summary(&first).clone();

    let replay = create_context(&client, &profile_arn, original.clone()).await;
    assert_eq!(
        replay.status, 200,
        "case \"replay\": the same token and parameters must answer 200, got {replay:?}"
    );
    assert_eq!(
        summary(&replay),
        &created,
        "case \"replay\": the replay must answer the context the original created, \
         got {replay:?}"
    );

    // One changed member per row, so the fingerprint is what decides rather than the
    // whole body.
    for (case, changed) in [
        ("changed-title", json!("Payments API v2")),
        ("changed-content", Value::Null),
    ] {
        let mut body = original.clone();
        if case == "changed-title" {
            body["title"] = changed;
        } else {
            body["content"] = json!({ "awsServices": ["apigateway", "lambda"] });
        }
        let answer = create_context(&client, &profile_arn, body).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: a reused token with different parameters must conflict, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ConflictException",
            "case {case:?}: expected a ConflictException, got {answer:?}"
        );
    }

    let fresh = create_context(
        &client,
        &profile_arn,
        json!({
            "clientToken": "token-two",
            "title": "Payments API",
            "contextType": CONTEXT_TYPES[0],
            "content": { "awsServices": ["apigateway"], "criticality": CRITICALITIES[0] },
        }),
    )
    .await;
    assert_eq!(
        fresh.status, 200,
        "case \"fresh-token\": a new token must record a second context, got {fresh:?}"
    );
    assert_ne!(
        id_of(summary(&fresh)),
        id_of(&created),
        "case \"fresh-token\": a new token must mint a new id, got {fresh:?}"
    );

    let listed = list_contexts(&client, &profile_arn, "").await;
    assert_eq!(
        items(&listed).len(),
        2,
        "the replay and the conflicts must have recorded nothing: expected the 2 \
         contexts the two distinct tokens created, got {listed:?}"
    );
}

/// Rule 7: a value outside a published enum set is refused and stores nothing.
#[tokio::test]
async fn a_value_outside_a_published_enum_set_is_refused() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-enums", 1).await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "unknown-context-type",
            json!({
                "title": "an unknown kind of context",
                "contextType": "ENVIRONMENT",
                "content": {},
            }),
        ),
        (
            "unknown-application-type",
            json!({
                "title": "an unknown application type",
                "contextType": CONTEXT_TYPES[0],
                "content": { "applicationType": "WEB_APPLICATION" },
            }),
        ),
        (
            "unknown-criticality",
            json!({
                "title": "an unknown criticality",
                "contextType": CONTEXT_TYPES[0],
                "content": { "criticality": "SOMEWHAT_CRITICAL" },
            }),
        ),
    ];
    for (case, body) in rows {
        let answer = create_context(&client, &profile_arn, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value outside the enum's published set must be refused, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
    }

    let listed = list_contexts(&client, &profile_arn, "").await;
    assert!(
        items(&listed).is_empty(),
        "a refused create must store nothing, got {listed:?}"
    );
}
