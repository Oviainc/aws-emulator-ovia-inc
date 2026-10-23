// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the memory lifecycle (`CreateMemory`, `GetMemory`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The settle span `src/state.rs` gives the `CREATING` -> `ACTIVE` transition, plus
/// slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// The namespace template a strategy whose records are facts about the actor gets when
/// the request supplies none (`src/ops/create_memory.rs`).
const ACTOR_NAMESPACE: &str = "/strategies/{memoryStrategyId}/actors/{actorId}";

/// The namespace template a strategy whose records cover one conversation gets when the
/// request supplies none.
const SESSION_NAMESPACE: &str = "/strategies/{memoryStrategyId}/actors/{actorId}/sessions/{sessionId}";

/// The strategies one memory holds, from `state::MAX_STRATEGIES_PER_MEMORY`.
const MAX_STRATEGIES_PER_MEMORY: usize = 100;

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
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

/// `GetMemory`: `GET /memories/{memoryId}/details` with the optional `view` query.
async fn get_memory(router: &MultiServiceRouter, id: &str, view: Option<&str>) -> (u16, Value) {
    let uri = match view {
        Some(view) => format!("/memories/{id}/details?view={view}"),
        None => format!("/memories/{id}/details"),
    };
    send(router, "GET", &uri, None).await
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

/// `MemoryId`'s pattern, `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$` with length
/// 12.. (`smithy_query shape=MemoryId`), checked by hand because this crate carries no
/// regex engine. `MemoryStrategyId` carries the same pattern.
fn is_modelled_memory_id(id: &str) -> bool {
    let Some((stem, suffix)) = id.rsplit_once('-') else {
        return false;
    };
    let suffix_ok = suffix.len() == 10 && suffix.bytes().all(|b| b.is_ascii_alphanumeric());
    let stem_ok = stem.len() <= 100
        && stem.starts_with(|c: char| c.is_ascii_alphabetic())
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    id.len() >= 12 && suffix_ok && stem_ok
}

/// A minimal `CreateMemory` body: the two members the model makes `@required`.
fn memory_body(name: &str, event_expiry_duration: i64) -> Value {
    json!({"name": name, "eventExpiryDuration": event_expiry_duration})
}

/// The `strategies` entry a response reports, or `Value::Null`.
fn strategy_of(memory: &Value, index: usize) -> Value {
    memory["strategies"][index].clone()
}

/// `memoryStrategies` with `count` distinct semantic strategies, for the rows that
/// probe the per-memory strategy ceiling.
fn strategy_list(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"semanticMemoryStrategy": {"name": format!("s{index}")}}))
            .collect(),
    )
}

/// A memory is created from the request and read back through `GetMemory`, for every
/// member of the `MemoryStrategyInput` union.
///
/// The rows differ along the strategy the request carries, which is what decides the
/// `type` the stored strategy reports and the namespace it is scoped to. Source for the
/// value list: `MemoryStrategyInput`'s members and `MemoryStrategyType`'s values
/// (`smithy_query shape=MemoryStrategyInput,MemoryStrategyType`).
#[tokio::test]
async fn a_memory_is_created_and_read_back_for_every_strategy_type() {
    let router = router();

    // (case, the memoryStrategies entry, expected type, expected namespace)
    let rows: [(&str, Value, &str, &str); 6] = [
        (
            "a semantic strategy",
            json!({"semanticMemoryStrategy": {"name": "facts"}}),
            "SEMANTIC",
            ACTOR_NAMESPACE,
        ),
        (
            "a summary strategy",
            json!({"summaryMemoryStrategy": {"name": "recap"}}),
            "SUMMARIZATION",
            SESSION_NAMESPACE,
        ),
        (
            "a user preference strategy",
            json!({"userPreferenceMemoryStrategy": {"name": "prefs"}}),
            "USER_PREFERENCE",
            ACTOR_NAMESPACE,
        ),
        (
            "an episodic strategy",
            json!({"episodicMemoryStrategy": {"name": "episodes"}}),
            "EPISODIC",
            SESSION_NAMESPACE,
        ),
        (
            "a custom strategy overriding semantic extraction",
            json!({"customMemoryStrategy": {
                "name": "custom",
                "configuration": {"semanticOverride": {
                    "extraction": {"appendToPrompt": "be terse", "modelId": "amazon.nova-lite-v1:0"}
                }}
            }}),
            "CUSTOM",
            ACTOR_NAMESPACE,
        ),
        (
            "a custom strategy with a self-managed configuration",
            json!({"customMemoryStrategy": {
                "name": "selfmanaged",
                "configuration": {"selfManagedConfiguration": {
                    "triggerConditions": [{"messageBasedTrigger": {"messageCount": 4}}],
                    "invocationConfiguration": {
                        "topicArn": "arn:aws:sns:us-east-1:000000000000:memory-work",
                        "payloadDeliveryBucketName": "memory-payloads"
                    },
                    "historicalContextWindowSize": 10
                }}
            }}),
            "CUSTOM",
            ACTOR_NAMESPACE,
        ),
    ];

    // The memories the table creates, so the clock-owed transition is waited out ONCE
    // after the loop rather than per row.
    let mut settling: Vec<(&str, String)> = Vec::new();

    for (index, (case, requested, expected_type, expected_namespace)) in
        rows.into_iter().enumerate()
    {
        let name = format!("mem{index}");
        let mut body = memory_body(&name, 30);
        body["memoryStrategies"] = json!([requested]);

        let (status, created) = create_memory(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateMemory, got {status} ({created})"
        );
        let memory = created["memory"].clone();

        let id = text(&memory, "id");
        assert!(
            is_modelled_memory_id(&id),
            "case {case:?}: expected a memoryId matching \
             ^[a-zA-Z][a-zA-Z0-9-_]{{0,99}}-[a-zA-Z0-9]{{10}}$, got {id:?}"
        );
        assert_eq!(
            text(&memory, "arn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:memory/{id}"),
            "case {case:?}: expected the modelled MemoryArn for {id}, got {}",
            text(&memory, "arn")
        );
        assert_eq!(
            text(&memory, "name"),
            name,
            "case {case:?}: the name reads back as sent, got {}",
            text(&memory, "name")
        );
        assert_eq!(
            memory["eventExpiryDuration"], json!(30),
            "case {case:?}: expected the eventExpiryDuration the request carried, got {}",
            memory["eventExpiryDuration"]
        );
        assert_eq!(
            text(&memory, "status"),
            "CREATING",
            "case {case:?}: a memory is created in CREATING, got {}",
            text(&memory, "status")
        );

        let strategy = strategy_of(&memory, 0);
        assert_eq!(
            text(&strategy, "type"),
            expected_type,
            "case {case:?}: expected strategy type {expected_type}, got {}",
            text(&strategy, "type")
        );
        let strategy_id = text(&strategy, "strategyId");
        assert!(
            is_modelled_memory_id(&strategy_id),
            "case {case:?}: expected a strategyId matching MemoryStrategyId's pattern, got \
             {strategy_id:?}"
        );
        assert_eq!(
            strategy["namespaceTemplates"], json!([expected_namespace]),
            "case {case:?}: expected namespaceTemplates [{expected_namespace}], got {}",
            strategy["namespaceTemplates"]
        );
        assert_eq!(
            strategy["namespaces"], strategy["namespaceTemplates"],
            "case {case:?}: the legacy namespaces member reports the same list as \
             namespaceTemplates, got {} for {}",
            strategy["namespaces"], strategy["namespaceTemplates"]
        );
        assert_eq!(
            text(&strategy, "status"),
            "CREATING",
            "case {case:?}: a strategy of a CREATING memory is CREATING, got {}",
            text(&strategy, "status")
        );
        // What a custom strategy's configuration reports is the override the request
        // chose (`OverrideType`), not an empty configuration.
        if expected_type == "CUSTOM" {
            assert!(
                !strategy["configuration"]["type"].is_null(),
                "case {case:?}: a custom strategy reports the override type it was \
                 configured with, got {}",
                strategy["configuration"]
            );
        }

        // The round trip: every member the create reported is what the read reports,
        // except the statuses — those are the clock's, and are asserted after the loop.
        let (status, fetched) = get_memory(&router, &id, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetMemory, got {status} ({fetched})"
        );
        for member in ["arn", "id", "name", "eventExpiryDuration", "createdAt", "updatedAt"] {
            assert_eq!(
                fetched["memory"][member], memory[member],
                "case {case:?}: GetMemory must report the {member} CreateMemory stored, got {} \
                 for {}",
                fetched["memory"][member], memory[member]
            );
        }
        let fetched_strategy = strategy_of(&fetched["memory"], 0);
        for member in [
            "strategyId",
            "name",
            "type",
            "namespaces",
            "namespaceTemplates",
            "configuration",
            "createdAt",
            "updatedAt",
        ] {
            assert_eq!(
                fetched_strategy[member], strategy[member],
                "case {case:?}: GetMemory must report the strategy's {member} CreateMemory \
                 stored, got {} for {}",
                fetched_strategy[member], strategy[member]
            );
        }

        settling.push((case, id));
    }

    // The status is the one the clock owes, materialised on a read and not by it:
    // CREATING until the span passes (asserted on each row above), ACTIVE afterwards,
    // for every later read.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id) in settling {
        let (_, settled) = get_memory(&router, &id, Some("without_decryption")).await;
        assert_eq!(
            text(&settled["memory"], "status"),
            "ACTIVE",
            "case {case:?}: after the settle span the memory is ACTIVE, got {}",
            text(&settled["memory"], "status")
        );
        assert_eq!(
            text(&strategy_of(&settled["memory"], 0), "status"),
            "ACTIVE",
            "case {case:?}: a strategy of an ACTIVE memory is ACTIVE, got {}",
            text(&strategy_of(&settled["memory"], 0), "status")
        );
        let (_, again) = get_memory(&router, &id, None).await;
        assert_eq!(
            text(&again["memory"], "status"),
            "ACTIVE",
            "case {case:?}: a second read reports the same status, got {}",
            text(&again["memory"], "status")
        );
    }
}

/// The namespaces a strategy is stored with: the caller's when the request carries them
/// under either member, the per-type default only when it carries neither.
///
/// The rows differ along which of the two members the request supplies, since
/// `MemoryStrategy` reports both as `@required` with exactly one entry whichever the
/// caller used (`smithy_query shape=MemoryStrategy`).
#[tokio::test]
async fn a_supplied_namespace_is_kept_and_only_an_absent_one_is_defaulted() {
    let router = router();

    // (case, the semantic strategy body, expected namespace list)
    let rows: [(&str, Value, &str); 4] = [
        (
            "neither member supplied",
            json!({"name": "plain"}),
            ACTOR_NAMESPACE,
        ),
        (
            "namespaceTemplates supplied",
            json!({"name": "templated", "namespaceTemplates": ["/custom/{actorId}"]}),
            "/custom/{actorId}",
        ),
        (
            "only the legacy namespaces supplied",
            json!({"name": "legacy", "namespaces": ["/legacy/{actorId}"]}),
            "/legacy/{actorId}",
        ),
        (
            "both supplied, so namespaceTemplates wins",
            json!({
                "name": "both",
                "namespaces": ["/legacy/{actorId}"],
                "namespaceTemplates": ["/preferred/{actorId}"]
            }),
            "/preferred/{actorId}",
        ),
    ];

    for (index, (case, strategy_body, expected)) in rows.into_iter().enumerate() {
        let mut body = memory_body(&format!("ns{index}"), 7);
        body["memoryStrategies"] = json!([{"semanticMemoryStrategy": strategy_body}]);
        let (status, created) = create_memory(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateMemory, got {status} ({created})"
        );
        let strategy = strategy_of(&created["memory"], 0);
        assert_eq!(
            strategy["namespaceTemplates"], json!([expected]),
            "case {case:?}: expected namespaceTemplates [{expected}], got {}",
            strategy["namespaceTemplates"]
        );
        assert_eq!(
            strategy["namespaces"], json!([expected]),
            "case {case:?}: expected namespaces [{expected}], got {}",
            strategy["namespaces"]
        );
    }
}

/// `eventExpiryDuration` is accepted on each published bound of its range and refused one
/// past either of them.
///
/// Source for the range: `CreateMemoryInput$eventExpiryDuration`, `range 3..365`
/// (`smithy_query operation=CreateMemory`).
#[tokio::test]
async fn event_expiry_duration_is_accepted_inside_its_range_and_refused_outside_it() {
    let router = router();

    // (case, the duration sent, expected status)
    let rows: [(&str, i64, u16); 5] = [
        ("one below the minimum", 2, 400),
        ("the published minimum", 3, 202),
        ("a value in the middle", 90, 202),
        ("the published maximum", 365, 202),
        ("one past the maximum", 366, 400),
    ];

    for (index, (case, duration, expected_status)) in rows.into_iter().enumerate() {
        let body = memory_body(&format!("expiry{index}"), duration);
        let (status, answer) = create_memory(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} (eventExpiryDuration {duration}): expected HTTP {expected_status}, got \
             {status} ({answer})"
        );
        if expected_status == 202 {
            let id = text(&answer["memory"], "id");
            let (status, fetched) = get_memory(&router, &id, None).await;
            assert_eq!(
                (status, fetched["memory"]["eventExpiryDuration"].clone()),
                (200, json!(duration)),
                "case {case:?}: GetMemory reports the duration the create stored, got {status} \
                 ({fetched})"
            );
        } else {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {} ({answer})",
                error_code(&answer)
            );
        }
    }
}

/// What the memory operations refuse, and with which modelled error.
#[tokio::test]
async fn the_memory_operations_refuse_what_the_model_states_they_must() {
    let router = router();

    // The memory a conflict row collides with.
    let (status, _) = create_memory(&router, &memory_body("taken", 30)).await;
    assert_eq!(status, 202, "the memory a later case collides with is created");

    let mut action_on_create = memory_body("expiryaction", 30);
    action_on_create["memoryStrategies"] = json!([{"semanticMemoryStrategy": {
        "name": "facts",
        "recordExpirySpecification": {"action": "REMOVE"}
    }}]);

    let mut below_record_expiry = memory_body("expirytooshort", 30);
    below_record_expiry["memoryStrategies"] = json!([{"semanticMemoryStrategy": {
        "name": "facts",
        "recordExpirySpecification": {"recordExpiryDurationDays": 30}
    }}]);

    let mut at_record_expiry = memory_body("expiryminimum", 30);
    at_record_expiry["memoryStrategies"] = json!([{"semanticMemoryStrategy": {
        "name": "facts",
        "recordExpirySpecification": {"recordExpiryDurationDays": 31}
    }}]);

    let mut at_strategy_ceiling = memory_body("strategyceiling", 30);
    at_strategy_ceiling["memoryStrategies"] = strategy_list(MAX_STRATEGIES_PER_MEMORY);

    let mut past_strategy_ceiling = memory_body("strategyoverflow", 30);
    past_strategy_ceiling["memoryStrategies"] = strategy_list(MAX_STRATEGIES_PER_MEMORY + 1);

    // (case, body, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 7] = [
        (
            "a name another memory already has",
            memory_body("taken", 30),
            409,
            "ConflictException",
        ),
        (
            "a name outside Name's pattern",
            memory_body("not a valid name!", 30),
            400,
            "ValidationException",
        ),
        (
            // "Set to REMOVE to remove a previously configured time-to-live … This value
            // is not applicable on create paths."
            "a record expiry action on a create path",
            action_on_create,
            400,
            "ValidationException",
        ),
        (
            // `RecordExpiryDurationDays` is `range 31..`.
            "a record expiry one day below the published minimum",
            below_record_expiry,
            400,
            "ValidationException",
        ),
        (
            "a record expiry at the published minimum",
            at_record_expiry,
            202,
            "<no __type>",
        ),
        (
            // `MemoryStrategyInputList` carries no length constraint, so the emulator's
            // own per-memory ceiling is what bounds it.
            "as many strategies as one memory holds",
            at_strategy_ceiling,
            202,
            "<no __type>",
        ),
        (
            "one strategy past that ceiling",
            past_strategy_ceiling,
            402,
            "ServiceQuotaExceededException",
        ),
    ];

    for (case, body, expected_status, expected_code) in rows {
        let (status, answer) = create_memory(&router, &body).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} {} \
             ({answer})",
            error_code(&answer)
        );
    }

    // Nothing the refused rows named was written: each is still absent by name.
    let (status, answer) = create_memory(&router, &memory_body("expiryaction", 30)).await;
    assert_eq!(
        status, 202,
        "case \"a refused create persists nothing\": the name is free to use, got {status} \
         ({answer})"
    );

    // A memory id naming no stored memory is the modelled 404, not an empty envelope. The
    // id is inside `MemoryId`'s pattern, so this is the handler's refusal and not the
    // boundary's.
    let (status, answer) = get_memory(&router, "noSuchMemory-0123456789", None).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"GetMemory on an unknown id\": expected HTTP 404 ResourceNotFoundException, got \
         {status} {} ({answer})",
        error_code(&answer)
    );
}

/// The members a request carries beside the strategies are stored and read back, and a
/// repeated `clientToken` replays the memory it first created.
#[tokio::test]
async fn the_configuration_round_trips_and_a_repeated_client_token_replays_it() {
    let router = router();

    let mut body = memory_body("configured", 45);
    body["description"] = json!("the memory a test configured");
    body["encryptionKeyArn"] =
        json!("arn:aws:kms:us-east-1:000000000000:key/00000000-0000-0000-0000-000000000000");
    body["memoryExecutionRoleArn"] = json!("arn:aws:iam::000000000000:role/MemoryRole");
    body["indexedKeys"] = json!([{"key": "topic", "type": "STRING"}]);
    body["namespaceKeys"] = json!([{"key": "tenant", "validation": {"allowedValues": ["acme"]}}]);
    body["tags"] = json!({"env": "test"});
    body["clientToken"] = json!("idempotent-create-memory-token-0001");

    let (status, created) = create_memory(&router, &body).await;
    assert_eq!(
        status, 202,
        "case \"a fully configured create\": expected HTTP 202, got {status} ({created})"
    );
    let memory = created["memory"].clone();
    let id = text(&memory, "id");

    // Every member the request supplied, reported back off the stored record.
    for (member, expected) in [
        ("description", body["description"].clone()),
        ("encryptionKeyArn", body["encryptionKeyArn"].clone()),
        ("memoryExecutionRoleArn", body["memoryExecutionRoleArn"].clone()),
        ("indexedKeys", body["indexedKeys"].clone()),
        ("namespaceKeys", body["namespaceKeys"].clone()),
    ] {
        assert_eq!(
            memory[member], expected,
            "case \"a fully configured create\": expected {member} {expected}, got {}",
            memory[member]
        );
    }
    // A memory asked for no strategy carries none, rather than one nothing requested.
    assert_eq!(
        memory["strategies"], json!([]),
        "case \"a create with no memoryStrategies\": expected no strategies, got {}",
        memory["strategies"]
    );
    // `managedByResourceArn` is set only on a memory another resource manages, and
    // `failureReason` only on a failed one: neither applies here.
    for member in ["managedByResourceArn", "failureReason"] {
        assert!(
            memory[member].is_null(),
            "case \"a fully configured create\": expected no {member}, got {}",
            memory[member]
        );
    }

    let (status, fetched) = get_memory(&router, &id, Some("full")).await;
    assert_eq!(
        status, 200,
        "case \"the round trip\": expected HTTP 200 from GetMemory, got {status} ({fetched})"
    );
    // Every member EXCEPT the status, which is the clock's: the settle span may have
    // passed between the two calls, and that difference is the transition this emulator
    // owes, not a stored difference.
    for member in [
        "arn",
        "id",
        "name",
        "description",
        "encryptionKeyArn",
        "memoryExecutionRoleArn",
        "eventExpiryDuration",
        "createdAt",
        "updatedAt",
        "strategies",
        "indexedKeys",
        "namespaceKeys",
        "managedByResourceArn",
        "failureReason",
    ] {
        assert_eq!(
            fetched["memory"][member], memory[member],
            "case \"the round trip\": GetMemory must report the {member} CreateMemory stored, got \
             {} for {}",
            fetched["memory"][member], memory[member]
        );
    }
    assert!(
        ["CREATING", "ACTIVE"].contains(&text(&fetched["memory"], "status").as_str()),
        "case \"the round trip\": the status is the one the clock owes (CREATING, then ACTIVE), \
         got {}",
        text(&fetched["memory"], "status")
    );

    // The replay: the same token answers with the first memory instead of a second one.
    let (status, replayed) = create_memory(&router, &body).await;
    assert_eq!(
        (status, text(&replayed["memory"], "id")),
        (202, id.clone()),
        "case \"a replayed create\": expected HTTP 202 and memoryId {id}, got {status} ({replayed})"
    );

    // A DIFFERENT token under the same name is a conflict, not a replay: the name is what
    // must be unique.
    body["clientToken"] = json!("idempotent-create-memory-token-0002");
    let (status, answer) = create_memory(&router, &body).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (409, "ConflictException"),
        "case \"a second create under a new token\": expected HTTP 409 ConflictException, got \
         {status} {} ({answer})",
        error_code(&answer)
    );
}
