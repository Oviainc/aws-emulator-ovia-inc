// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateMemory` reports for the strategies it adds, what a memory READ projects,
//! and how a memory this service does not hold is refused — end to end over HTTP.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised, against the one shared engine
//! — so what an update writes is what a later read reports.
//!
//! The rules under test:
//!
//! 1. A strategy an `UpdateMemory` ADDS is usable when the call returns: the update's own
//!    202 reports it `ACTIVE` (`MemoryStrategyStatus` is `CREATING | ACTIVE | DELETING |
//!    FAILED`, `smithy_query(query="shape",shape="MemoryStrategyStatus")`), and so does
//!    every later read — the clock owes an added strategy no transition of its own, and no
//!    strategy the memory still holds is reported `CREATING` once an update of that memory
//!    has been accepted. The memory itself is still `UPDATING` until the clock owes it
//!    `ACTIVE`, which is what distinguishes "the strategy is ready" from "nothing is
//!    transient". The rows differ along the kind of strategy added (one per member of the
//!    `MemoryStrategyInput` union), how many are added at once, and whether the memory
//!    already held strategies.
//! 2. A memory READ projects an extraction-health object at BOTH levels: `memory` carries
//!    `extractionHealth` with `failedMemoryExtractionJobCount`, and so does every entry of
//!    `memory.strategies`. The rows differ along how many strategies the memory holds and
//!    along whether the clock still owes the memory a transition.
//! 3. A `memoryId` naming no stored memory is refused with the operation's modelled
//!    `ResourceNotFoundException` (HTTP 404), worded as this service words a memory it
//!    cannot find: the operation that was asking and the id the request carried. The rows
//!    differ along the operation and along the SHAPE of the id (`MemoryId` is `length 12..`
//!    with pattern `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`,
//!    `smithy_query(query="shape",shape="MemoryId")`), because the sentence takes the id
//!    from the request whatever it looks like.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The settle span `src/state.rs` gives the `CREATING` -> `ACTIVE` and `UPDATING` ->
/// `ACTIVE` transitions, plus slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
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
        .header("authorization", authorization(REGION))
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

/// `CreateMemory`: `POST /memories/create` (`smithy_query operation=CreateMemory`).
async fn create_memory(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "POST", "/memories/create", Some(body)).await
}

/// `GetMemory`: `GET /memories/{memoryId}/details`.
async fn get_memory(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(router, "GET", &format!("/memories/{id}/details"), None).await
}

/// `UpdateMemory`: `PUT /memories/{memoryId}/update`.
async fn update_memory(router: &MultiServiceRouter, id: &str, body: &Value) -> (u16, Value) {
    send(router, "PUT", &format!("/memories/{id}/update"), Some(body)).await
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

/// The strategies a response reports, in the order it reports them.
fn strategies(memory: &Value) -> Vec<Value> {
    memory["strategies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The strategy a response reports under `name`, or `Value::Null`.
fn strategy_by_name(memory: &Value, name: &str) -> Value {
    strategies(memory)
        .into_iter()
        .find(|strategy| text(strategy, "name") == name)
        .unwrap_or(Value::Null)
}

/// One strategy body per member of the `MemoryStrategyInput` union
/// (`smithy_query(query="shape",shape="MemoryStrategyInput")`), named after the member so a
/// row can find what it added.
fn strategy_inputs() -> [(&'static str, &'static str, Value); 5] {
    [
        (
            "a semantic strategy",
            "semantic",
            json!({"semanticMemoryStrategy": {"name": "semantic"}}),
        ),
        (
            "a summarization strategy",
            "summary",
            json!({"summaryMemoryStrategy": {"name": "summary"}}),
        ),
        (
            "a user-preference strategy",
            "preference",
            json!({"userPreferenceMemoryStrategy": {"name": "preference"}}),
        ),
        (
            "an episodic strategy",
            "episodes",
            json!({"episodicMemoryStrategy": {"name": "episodes"}}),
        ),
        (
            "a custom strategy overriding semantic extraction",
            "custom",
            json!({"customMemoryStrategy": {
                "name": "custom",
                "configuration": {"semanticOverride": {}}
            }}),
        ),
    ]
}

/// Create a memory and wait the `CREATING` -> `ACTIVE` transition out, so the update that
/// follows addresses an active memory. Answers its `memoryId`.
async fn settled_memory(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, created) = create_memory(router, body).await;
    assert_eq!(
        status, 202,
        "setup: expected HTTP 202 from CreateMemory, got {status} ({created})"
    );
    let id = text(&created["memory"], "id");
    tokio::time::sleep(PAST_SETTLE).await;
    id
}

/// Rule 1: every strategy an update adds is reported `ACTIVE` by the update itself and by
/// every later read, while the memory is still `UPDATING`.
#[tokio::test]
async fn a_strategy_an_update_adds_is_active_when_the_update_returns() {
    // (case, the strategies this row adds, the names it must find ACTIVE, whether the
    // memory already holds one)
    let mut rows: Vec<(String, Vec<Value>, Vec<&'static str>, bool)> = strategy_inputs()
        .into_iter()
        .map(|(case, name, body)| (case.to_string(), vec![body], vec![name], false))
        .collect();
    // More than one at a time, and a memory that already holds a strategy of its own.
    let inputs = strategy_inputs();
    rows.push((
        "two strategies at once".to_string(),
        vec![inputs[0].2.clone(), inputs[3].2.clone()],
        vec![inputs[0].1, inputs[3].1],
        false,
    ));
    rows.push((
        "one strategy added to a memory that already holds one".to_string(),
        vec![inputs[1].2.clone()],
        vec![inputs[1].1],
        true,
    ));

    for (index, (case, added, names, preloaded)) in rows.into_iter().enumerate() {
        let router = router();
        let mut create = json!({
            "name": format!("addstrategy{index}"),
            "eventExpiryDuration": 30,
        });
        if preloaded {
            create["memoryStrategies"] =
                json!([{"semanticMemoryStrategy": {"name": "preloaded"}}]);
        }
        let id = settled_memory(&router, &create).await;

        let (status, updated) =
            update_memory(&router, &id, &json!({"memoryStrategies": {"addMemoryStrategies": added}}))
                .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from UpdateMemory, got {status} ({updated})"
        );
        let memory = updated["memory"].clone();

        // The memory itself is still transient — the added strategy being ACTIVE is not a
        // side effect of nothing being in flight.
        assert_eq!(
            text(&memory, "status"),
            "UPDATING",
            "case {case:?}: an updated memory is UPDATING, got {}",
            text(&memory, "status")
        );
        for name in &names {
            let added = strategy_by_name(&memory, name);
            assert_eq!(
                text(&added, "status"),
                "ACTIVE",
                "case {case:?}: the strategy {name:?} the update added must be ACTIVE in the \
                 update's own response, got {added}"
            );
        }
        if preloaded {
            assert_eq!(
                text(&strategy_by_name(&memory, "preloaded"), "status"),
                "ACTIVE",
                "case {case:?}: the strategy the create minted is ACTIVE too once an update \
                 has been accepted, got {}",
                strategy_by_name(&memory, "preloaded")
            );
        }

        // A read taken straight after the update — still inside the memory's own UPDATING
        // span — reports the same statuses, and so does one taken after it.
        let (status, during) = get_memory(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetMemory, got {status} ({during})"
        );
        for name in &names {
            assert_eq!(
                text(&strategy_by_name(&during["memory"], name), "status"),
                "ACTIVE",
                "case {case:?}: a read inside the memory's UPDATING span reports {name:?} as \
                 ACTIVE, got {}",
                strategy_by_name(&during["memory"], name)
            );
        }

        tokio::time::sleep(PAST_SETTLE).await;
        let (_, after) = get_memory(&router, &id).await;
        assert_eq!(
            text(&after["memory"], "status"),
            "ACTIVE",
            "case {case:?}: the clock owes the memory ACTIVE, got {}",
            text(&after["memory"], "status")
        );
        for name in &names {
            assert_eq!(
                text(&strategy_by_name(&after["memory"], name), "status"),
                "ACTIVE",
                "case {case:?}: {name:?} is still ACTIVE after the settle span, got {}",
                strategy_by_name(&after["memory"], name)
            );
        }
    }
}

/// Rule 2: a memory read carries the extraction health on the memory and inside each
/// strategy.
#[tokio::test]
async fn a_memory_read_reports_extraction_health_at_both_levels() {
    let router = router();
    let inputs = strategy_inputs();

    // (case, the strategies the memory is created with)
    let rows: [(&str, Vec<Value>); 4] = [
        ("a memory with no strategy at all", vec![]),
        ("a memory with one strategy", vec![inputs[0].2.clone()]),
        (
            "a memory with three strategies",
            vec![
                inputs[1].2.clone(),
                inputs[2].2.clone(),
                inputs[4].2.clone(),
            ],
        ),
        (
            "a memory whose strategies were added by an update",
            vec![inputs[3].2.clone()],
        ),
    ];

    for (index, (case, configured)) in rows.into_iter().enumerate() {
        let added_by_update = case == "a memory whose strategies were added by an update";
        let mut create = json!({
            "name": format!("health{index}"),
            "eventExpiryDuration": 30,
        });
        if !added_by_update && !configured.is_empty() {
            create["memoryStrategies"] = Value::Array(configured.clone());
        }
        let (status, created) = create_memory(&router, &create).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateMemory, got {status} ({created})"
        );
        let id = text(&created["memory"], "id");

        // A read while the clock still owes the memory a transition carries the health too:
        // it is a projection of the memory, not of the status it happens to report.
        let (status, creating) = get_memory(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetMemory, got {status} ({creating})"
        );
        assert_eq!(
            creating["memory"]["extractionHealth"]["failedMemoryExtractionJobCount"],
            json!(0),
            "case {case:?}: a read inside the CREATING span reports the memory's extraction \
             health, got {}",
            creating["memory"]["extractionHealth"]
        );

        tokio::time::sleep(PAST_SETTLE).await;
        if added_by_update {
            let (status, updated) = update_memory(
                &router,
                &id,
                &json!({"memoryStrategies": {"addMemoryStrategies": configured.clone()}}),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: expected HTTP 202 from UpdateMemory, got {status} ({updated})"
            );
        }

        let (status, fetched) = get_memory(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetMemory, got {status} ({fetched})"
        );
        let memory = fetched["memory"].clone();
        assert_eq!(
            memory["extractionHealth"]["failedMemoryExtractionJobCount"],
            json!(0),
            "case {case:?}: the memory must report its extraction health, got {}",
            memory["extractionHealth"]
        );
        assert_eq!(
            strategies(&memory).len(),
            configured.len(),
            "case {case:?}: expected {} strategies to read health off, got {}",
            configured.len(),
            memory["strategies"]
        );
        for strategy in strategies(&memory) {
            assert_eq!(
                strategy["extractionHealth"]["failedMemoryExtractionJobCount"],
                json!(0),
                "case {case:?}: strategy {} must report its own extraction health, got \
                 {strategy}",
                text(&strategy, "strategyId")
            );
        }
    }
}

/// Rule 3: a `memoryId` naming no memory is refused 404 in this service's memory wording,
/// with the asking operation and the id the request carried.
#[tokio::test]
async fn a_memory_id_naming_no_memory_is_refused_in_this_services_memory_wording() {
    let router = router();

    // Ids inside `MemoryId`'s `length 12..` and
    // `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`, differing in length, case and
    // separators, that no request here ever creates.
    let ids: [(&str, String); 4] = [
        ("the shortest id the pattern admits", "a-bcdefghijk".to_string()),
        (
            "an id carrying both admitted separators",
            "mem_test-part-0123456789".to_string(),
        ),
        ("an id in mixed case", "MemoryABC-0123456789".to_string()),
        (
            "an id near the longest the pattern admits",
            format!("{}-0123456789", "m".repeat(60)),
        ),
    ];

    for (case, memory_id) in ids {
        // (the operation, the answer its route gave)
        let calls: [(&str, (u16, Value)); 2] = [
            ("GetMemory", get_memory(&router, &memory_id).await),
            (
                "UpdateMemory",
                update_memory(&router, &memory_id, &json!({"description": "whatever"})).await,
            ),
        ];

        for (operation, (status, answer)) in calls {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} ({operation} on {memory_id:?}): expected HTTP 404 \
                 ResourceNotFoundException, got {status} {} ({answer})",
                error_code(&answer)
            );
            assert_eq!(
                text(&answer, "message"),
                format!(
                    "Resource not found during {operation}: Failed to retrieve memory item for \
                     ID: {memory_id}"
                ),
                "case {case:?} ({operation}): the refusal names the operation and the id the \
                 request carried, got {}",
                text(&answer, "message")
            );
        }
    }

    // A memory that IS there is not refused, so the rows above are about the lookup and not
    // about every id being rejected.
    let id = settled_memory(
        &router,
        &json!({"name": "presentmemory", "eventExpiryDuration": 30}),
    )
    .await;
    let (status, fetched) = get_memory(&router, &id).await;
    assert_eq!(
        status, 200,
        "a stored memory still reads back, got {status} ({fetched})"
    );
}
