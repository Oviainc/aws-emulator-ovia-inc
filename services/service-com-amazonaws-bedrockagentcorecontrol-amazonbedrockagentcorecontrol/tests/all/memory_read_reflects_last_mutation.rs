// SPDX-License-Identifier: CC0-1.0

//! What a memory READ reports after the memory has been mutated — end to end over HTTP.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers all run, against the one shared engine — so what
//! an update writes is what the later read reports.
//!
//! The rule under test: `GetMemory` projects the STORED memory record, so every member it
//! reports is the one the last accepted mutation left on that record — the members an
//! `UpdateMemory` carried replaced, the ones it omitted kept, and every strategy the memory
//! holds listed under `strategies` — plus the extraction-health object a read carries at
//! both levels (on the memory and inside each strategy it lists).
//!
//! The rows differ along WHICH members the update carried (both, one, the other), along the
//! value each is given — `description` is `length 1..4096` and `eventExpiryDuration` is
//! `range 3..365` on this request, both ends tested
//! (`smithy_query(query="operation",operation="UpdateMemory")`) — along how many strategies
//! the memory ends up holding, and along whether a strategy was minted by the create or
//! added by the update, because the rule is about the record a read projects and not about
//! any one member or any one way a strategy got there.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The settle span `src/state.rs` gives the `CREATING` -> `ACTIVE` transition, plus slack,
/// so a test can wait the clock out before updating an active memory.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// `Description`'s published maximum, `length 1..4096`
/// (`smithy_query(query="operation",operation="UpdateMemory")`).
const MAX_DESCRIPTION_LEN: usize = 4096;

/// `eventExpiryDuration`'s published bounds on this request, `range 3..365`.
const MIN_EVENT_EXPIRY_DAYS: i64 = 3;
const MAX_EVENT_EXPIRY_DAYS: i64 = 365;

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

/// `CreateMemory`: `POST /memories/create`.
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

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The strategy a response reports under `name`, or `Value::Null`.
fn strategy_by_name(memory: &Value, name: &str) -> Value {
    memory["strategies"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|strategy| text(strategy, "name") == name)
        .cloned()
        .unwrap_or(Value::Null)
}

/// How many strategies a response lists.
fn strategy_count(memory: &Value) -> usize {
    memory["strategies"].as_array().map_or(0, Vec::len)
}

/// One row of the table: what the memory was created with, what the update carried, and what
/// a read of it must then report.
struct Row {
    case: &'static str,
    /// The `CreateMemory` body, minus the name this test gives it.
    create: Value,
    /// The `UpdateMemory` body.
    update: Value,
    /// `description` the read must report (`Value::Null` when the memory has none).
    description: Value,
    /// `eventExpiryDuration` the read must report.
    event_expiry_duration: Value,
    /// (strategy name, `MemoryStrategyType`) for every strategy the memory must then hold —
    /// minted by the create or added by the update alike.
    strategies: Vec<(&'static str, &'static str)>,
}

/// The table: one row per way the last mutation can have left the record.
fn rows() -> Vec<Row> {
    // `MemoryStrategyType` is `SEMANTIC | SUMMARIZATION | USER_PREFERENCE | CUSTOM |
    // EPISODIC` (`smithy_query(query="shape",shape="MemoryStrategyType")`), one value per
    // member of the `MemoryStrategyInput` union a request carries.
    let long_description = "d".repeat(MAX_DESCRIPTION_LEN);
    vec![
        Row {
            case: "both members replaced, the description at its shortest and the expiry at \
                   its published minimum, with one strategy added",
            create: json!({"description": "the memory a test created", "eventExpiryDuration": 30}),
            update: json!({
                "description": "x",
                "eventExpiryDuration": MIN_EVENT_EXPIRY_DAYS,
                "memoryStrategies": {"addMemoryStrategies": [
                    {"semanticMemoryStrategy": {"name": "facts"}}
                ]}
            }),
            description: json!("x"),
            event_expiry_duration: json!(MIN_EVENT_EXPIRY_DAYS),
            strategies: vec![("facts", "SEMANTIC")],
        },
        Row {
            case: "both members replaced, the description at its longest and the expiry at \
                   its published maximum, with two strategies added to one the create minted",
            create: json!({
                "description": "the memory a test created",
                "eventExpiryDuration": 30,
                "memoryStrategies": [{"semanticMemoryStrategy": {"name": "minted"}}]
            }),
            update: json!({
                "description": long_description,
                "eventExpiryDuration": MAX_EVENT_EXPIRY_DAYS,
                "memoryStrategies": {"addMemoryStrategies": [
                    {"summaryMemoryStrategy": {"name": "digest"}},
                    {"episodicMemoryStrategy": {"name": "episodes"}}
                ]}
            }),
            description: json!(long_description),
            event_expiry_duration: json!(MAX_EVENT_EXPIRY_DAYS),
            strategies: vec![
                ("minted", "SEMANTIC"),
                ("digest", "SUMMARIZATION"),
                ("episodes", "EPISODIC"),
            ],
        },
        Row {
            case: "only the expiry replaced, so the read keeps the description the create \
                   stored",
            create: json!({"description": "the memory a test created", "eventExpiryDuration": 7}),
            update: json!({"eventExpiryDuration": 120}),
            description: json!("the memory a test created"),
            event_expiry_duration: json!(120),
            strategies: vec![],
        },
        Row {
            case: "only the description replaced, so the read keeps the expiry the create \
                   stored",
            create: json!({"description": "the memory a test created", "eventExpiryDuration": 45}),
            update: json!({"description": "the memory a test updated"}),
            description: json!("the memory a test updated"),
            event_expiry_duration: json!(45),
            strategies: vec![],
        },
        Row {
            case: "a memory created without a description: the read reports none, the expiry \
                   the update carried, and the custom strategy it added",
            create: json!({"eventExpiryDuration": 30}),
            update: json!({
                "eventExpiryDuration": 90,
                "memoryStrategies": {"addMemoryStrategies": [{"customMemoryStrategy": {
                    "name": "tailored",
                    "configuration": {"semanticOverride": {}}
                }}]}
            }),
            description: Value::Null,
            event_expiry_duration: json!(90),
            strategies: vec![("tailored", "CUSTOM")],
        },
        Row {
            case: "an update carrying no member at all: the read reports what the create \
                   stored, strategy included",
            create: json!({
                "description": "the memory a test created",
                "eventExpiryDuration": 180,
                "memoryStrategies": [{"userPreferenceMemoryStrategy": {"name": "preferences"}}]
            }),
            update: json!({}),
            description: json!("the memory a test created"),
            event_expiry_duration: json!(180),
            strategies: vec![("preferences", "USER_PREFERENCE")],
        },
    ]
}

/// The extraction-health object a read carries, asserted wherever it must appear.
fn assert_extraction_health(case: &str, level: &str, carrier: &Value) {
    assert_eq!(
        carrier["extractionHealth"]["failedMemoryExtractionJobCount"],
        json!(0),
        "case {case:?}: {level} must report its extraction health, got {}",
        carrier["extractionHealth"]
    );
}

/// A strategy a read lists carries the members `MemoryStrategy` requires, the type its union
/// member named, and its own extraction health.
fn assert_strategy(case: &str, name: &str, strategy_type: &str, strategy: &Value) {
    assert!(
        !strategy.is_null(),
        "case {case:?}: the memory read must list the strategy {name:?}, got nothing"
    );
    assert_eq!(
        text(strategy, "type"),
        strategy_type,
        "case {case:?}: expected strategy {name:?} of type {strategy_type}, got {strategy}"
    );
    assert_eq!(
        text(strategy, "status"),
        "ACTIVE",
        "case {case:?}: strategy {name:?} is provisioned once the mutation that added it was \
         accepted, got {strategy}"
    );
    // `MemoryStrategyId` is `length 12..` with pattern
    // `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`
    // (`smithy_query(query="shape",shape="MemoryStrategy")`), so a client can address the
    // strategy it just created by the id the read reports.
    let strategy_id = text(strategy, "strategyId");
    assert!(
        strategy_id.len() >= 12
            && strategy_id.starts_with(|first: char| first.is_ascii_alphabetic()),
        "case {case:?}: strategy {name:?} must report an addressable strategyId, got \
         {strategy_id:?}"
    );
    // Both namespace members are `@required`, `length 1..1`.
    for member in ["namespaces", "namespaceTemplates"] {
        assert_eq!(
            strategy[member].as_array().map_or(0, Vec::len),
            1,
            "case {case:?}: strategy {name:?} must report one {member} entry, got {}",
            strategy[member]
        );
    }
    for member in ["createdAt", "updatedAt"] {
        assert!(
            !strategy[member].is_null(),
            "case {case:?}: strategy {name:?} must report its {member}, got {strategy}"
        );
    }
    assert_extraction_health(case, &format!("strategy {name:?}"), strategy);
}

/// The rule: a memory read projects the record the last accepted mutation left, and carries
/// the extraction health at both levels.
#[tokio::test]
async fn a_memory_read_reports_the_record_the_last_mutation_left() {
    let router = router();
    let table = rows();

    // Every memory is created first and the `CREATING` -> `ACTIVE` transition waited out
    // ONCE, so each update below addresses an active memory.
    let mut ids = Vec::with_capacity(table.len());
    for (index, row) in table.iter().enumerate() {
        let mut create = row.create.clone();
        create["name"] = json!(format!("readback{index}"));
        let (status, created) = create_memory(&router, &create).await;
        assert_eq!(
            status, 202,
            "case {:?}: expected HTTP 202 from CreateMemory, got {status} ({created})",
            row.case
        );
        ids.push(text(&created["memory"], "id"));
    }
    tokio::time::sleep(PAST_SETTLE).await;

    for (row, id) in table.iter().zip(ids.iter()) {
        let case = row.case;

        // A read BEFORE the update already carries the extraction health: it is a projection
        // of the memory, not something an update puts there.
        let (status, before) = get_memory(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetMemory, got {status} ({before})"
        );
        assert_extraction_health(case, "the memory", &before["memory"]);

        let (status, updated) = update_memory(&router, id, &row.update).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from UpdateMemory, got {status} ({updated})"
        );

        let (status, fetched) = get_memory(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetMemory, got {status} ({fetched})"
        );
        let memory = fetched["memory"].clone();

        assert_eq!(
            memory["description"], row.description,
            "case {case:?}: expected description {}, got {}",
            row.description, memory["description"]
        );
        assert_eq!(
            memory["eventExpiryDuration"], row.event_expiry_duration,
            "case {case:?}: expected eventExpiryDuration {}, got {}",
            row.event_expiry_duration, memory["eventExpiryDuration"]
        );
        assert_extraction_health(case, "the memory", &memory);
        assert_eq!(
            strategy_count(&memory),
            row.strategies.len(),
            "case {case:?}: expected {} strategies, got {}",
            row.strategies.len(),
            memory["strategies"]
        );
        for (name, strategy_type) in &row.strategies {
            assert_strategy(case, name, strategy_type, &strategy_by_name(&memory, name));
        }

        // The update's own answer already reports what the read reports: the write is
        // applied by the update and not by the read that follows it.
        for member in ["description", "eventExpiryDuration"] {
            assert_eq!(
                updated["memory"][member], memory[member],
                "case {case:?}: UpdateMemory and GetMemory must agree on {member}, got {} and {}",
                updated["memory"][member], memory[member]
            );
        }

        // A read changes nothing a later caller sees: the next one reports the same memory,
        // status apart — that one is the clock's.
        let (_, again) = get_memory(&router, id).await;
        let mut first = memory.clone();
        let mut second = again["memory"].clone();
        for projection in [&mut first, &mut second] {
            if let Some(object) = projection.as_object_mut() {
                object.remove("status");
            }
            for strategy in projection["strategies"]
                .as_array_mut()
                .into_iter()
                .flatten()
            {
                if let Some(object) = strategy.as_object_mut() {
                    object.remove("status");
                }
            }
        }
        assert_eq!(
            first, second,
            "case {case:?}: a second read must report the same memory, got {second} after \
             {first}"
        );
    }
}
