// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateMemory`, read back through `GetMemory`.
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

/// The settle span `src/state.rs` gives the `CREATING` -> `ACTIVE` and
/// `UPDATING` -> `ACTIVE` transitions, plus slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// The strategies one memory holds, from `state::MAX_STRATEGIES_PER_MEMORY`.
const MAX_STRATEGIES_PER_MEMORY: usize = 100;

/// The metadata keys one memory indexes: `IndexedKeysList` is `length 1..10`
/// (`smithy_query shape=IndexedKeysList`).
const MAX_INDEXED_KEYS: usize = 10;

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

/// `GetMemory`: `GET /memories/{memoryId}/details`.
async fn get_memory(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(router, "GET", &format!("/memories/{id}/details"), None).await
}

/// `UpdateMemory`: `PUT /memories/{memoryId}/update`
/// (`smithy_query operation=UpdateMemory`).
async fn update_memory(router: &MultiServiceRouter, id: &str, body: &Value) -> (u16, Value) {
    send(
        router,
        "PUT",
        &format!("/memories/{id}/update"),
        Some(body),
    )
    .await
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

/// A `CreateMemory` body carrying every member a later update can replace, so a
/// row that updates ONE of them can assert the others kept their stored value.
fn configured_memory_body(name: &str) -> Value {
    json!({
        "name": name,
        "eventExpiryDuration": 30,
        "description": "the memory a test created",
        "memoryExecutionRoleArn": "arn:aws:iam::000000000000:role/MemoryRole",
        "indexedKeys": [{"key": "topic", "type": "STRING"}],
        "namespaceKeys": [{"key": "tenant"}],
        "streamDeliveryResources": {"resources": [{"kinesis": {
            "dataStreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/created",
            "contentConfigurations": [{"type": "MEMORY_RECORDS", "level": "METADATA_ONLY"}]
        }}]}
    })
}

/// A stream delivery configuration naming `stream`, for the row that replaces one.
fn stream_delivery(stream: &str) -> Value {
    json!({"resources": [{"kinesis": {
        "dataStreamArn": format!("arn:aws:kinesis:us-east-1:000000000000:stream/{stream}"),
        "contentConfigurations": [{"type": "MEMORY_RECORDS", "level": "FULL_CONTENT"}]
    }}]})
}

/// The strategy a response reports under `strategyId`, or `Value::Null`.
fn strategy_by_id(memory: &Value, strategy_id: &str) -> Value {
    memory["strategies"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|strategy| text(strategy, "strategyId") == strategy_id)
        .cloned()
        .unwrap_or(Value::Null)
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

/// Create one memory per name, then wait the `CREATING` -> `ACTIVE` transition
/// out ONCE, so the updates that follow address active memories. Answers the
/// minted `memoryId`s in the order the names were given.
async fn created_and_settled(router: &MultiServiceRouter, names: &[String]) -> Vec<String> {
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        let (status, created) = create_memory(router, &configured_memory_body(name)).await;
        assert_eq!(
            status, 202,
            "case {name:?}: expected HTTP 202 from CreateMemory, got {status} ({created})"
        );
        ids.push(text(&created["memory"], "id"));
    }
    tokio::time::sleep(PAST_SETTLE).await;
    ids
}

/// A partial update: the member the request carries replaces the stored one, and
/// every member it omits keeps the stored one.
///
/// The rows differ along WHICH member the request carries — a string, an integer,
/// an ARN, a nested structure, and no member at all — since the rule is about the
/// presence of a member and not about any one of them.
#[tokio::test]
async fn a_member_the_request_carries_replaces_the_stored_one_and_an_omitted_one_keeps_it() {
    let router = router();

    // (case, the member the update carries, its new value)
    let rows: [(&str, &str, Value); 5] = [
        ("a string member", "description", json!("the memory a test updated")),
        ("an integer member", "eventExpiryDuration", json!(180)),
        (
            "an ARN member",
            "memoryExecutionRoleArn",
            json!("arn:aws:iam::000000000000:role/UpdatedMemoryRole"),
        ),
        (
            "a nested structure member",
            "streamDeliveryResources",
            stream_delivery("updated"),
        ),
        // The whole body is optional, so an update that carries nothing changes
        // nothing: there is no member to replace.
        ("no member at all", "", Value::Null),
    ];

    let names: Vec<String> = (0..rows.len()).map(|index| format!("patch{index}")).collect();
    let ids = created_and_settled(&router, &names).await;

    let every_member = [
        "description",
        "eventExpiryDuration",
        "memoryExecutionRoleArn",
        "streamDeliveryResources",
        "indexedKeys",
        "namespaceKeys",
    ];

    for ((case, member, value), id) in rows.into_iter().zip(ids.iter()) {
        let (status, before) = get_memory(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetMemory, got {status} ({before})"
        );
        let before = before["memory"].clone();

        let mut body = json!({});
        if !member.is_empty() {
            body[member] = value.clone();
        }
        let (status, updated) = update_memory(&router, id, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from UpdateMemory, got {status} ({updated})"
        );
        let updated = updated["memory"].clone();

        for other in every_member {
            let expected = if other == member {
                value.clone()
            } else {
                before[other].clone()
            };
            assert_eq!(
                updated[other], expected,
                "case {case:?}: expected {other} {expected}, got {}",
                updated[other]
            );
        }
        // The identity never moves, and the update is dated.
        for stable in ["arn", "id", "name", "createdAt"] {
            assert_eq!(
                updated[stable], before[stable],
                "case {case:?}: expected the {stable} the create stored, got {}",
                updated[stable]
            );
        }
        assert_eq!(
            text(&updated, "status"),
            "UPDATING",
            "case {case:?}: an updated memory is UPDATING, got {}",
            text(&updated, "status")
        );

        // The round trip: the read reports what the update wrote.
        let (status, fetched) = get_memory(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetMemory, got {status} ({fetched})"
        );
        for other in every_member {
            assert_eq!(
                fetched["memory"][other], updated[other],
                "case {case:?}: GetMemory must report the {other} UpdateMemory stored, got {} for \
                 {}",
                fetched["memory"][other], updated[other]
            );
        }
    }

    // The `UPDATING` -> `ACTIVE` transition is the clock's, materialised on a read
    // and not by it.
    tokio::time::sleep(PAST_SETTLE).await;
    for id in &ids {
        let (_, settled) = get_memory(&router, id).await;
        assert_eq!(
            text(&settled["memory"], "status"),
            "ACTIVE",
            "case \"after the settle span\": memory {id} is ACTIVE, got {}",
            text(&settled["memory"], "status")
        );
    }
}

/// `addIndexedKeys` MERGES into the keys already indexed; `namespaceKeys` REPLACES
/// the stored set.
///
/// The rows differ along the stored set and the set the request carries — a new
/// key, a key already indexed, several at once, and an omitted member — because
/// the rule is about how the two sets combine. The key types come from
/// `MetadataValueType`'s values, `STRING | STRINGLIST | NUMBER`
/// (`smithy_query shape=MetadataValueType`).
#[tokio::test]
async fn indexed_keys_merge_and_namespace_keys_replace() {
    let router = router();

    // (case, indexedKeys at create, addIndexedKeys, expected indexedKeys)
    let rows: [(&str, Value, Value, Value); 5] = [
        (
            "a key the memory does not index yet",
            json!([{"key": "topic", "type": "STRING"}]),
            json!([{"key": "region", "type": "STRING"}]),
            json!([{"key": "topic", "type": "STRING"}, {"key": "region", "type": "STRING"}]),
        ),
        (
            // "Previously indexed keys cannot be removed", so a key already
            // indexed keeps the type it was declared with and is not doubled.
            "a key the memory already indexes",
            json!([{"key": "topic", "type": "STRING"}]),
            json!([{"key": "topic", "type": "NUMBER"}]),
            json!([{"key": "topic", "type": "STRING"}]),
        ),
        (
            "several keys at once, one of them already indexed",
            json!([{"key": "topic", "type": "STRING"}]),
            json!([
                {"key": "topic", "type": "STRING"},
                {"key": "score", "type": "NUMBER"},
                {"key": "labels", "type": "STRINGLIST"}
            ]),
            json!([
                {"key": "topic", "type": "STRING"},
                {"key": "score", "type": "NUMBER"},
                {"key": "labels", "type": "STRINGLIST"}
            ]),
        ),
        (
            "a memory that indexes nothing yet",
            Value::Null,
            json!([{"key": "labels", "type": "STRINGLIST"}]),
            json!([{"key": "labels", "type": "STRINGLIST"}]),
        ),
        (
            "an omitted addIndexedKeys",
            json!([{"key": "topic", "type": "STRING"}]),
            Value::Null,
            json!([{"key": "topic", "type": "STRING"}]),
        ),
    ];

    for (index, (case, stored, added, expected)) in rows.into_iter().enumerate() {
        let mut create = json!({"name": format!("merge{index}"), "eventExpiryDuration": 30});
        if !stored.is_null() {
            create["indexedKeys"] = stored;
        }
        let (status, created) = create_memory(&router, &create).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateMemory, got {status} ({created})"
        );
        let id = text(&created["memory"], "id");
        tokio::time::sleep(PAST_SETTLE).await;

        let mut body = json!({});
        if !added.is_null() {
            body["addIndexedKeys"] = added;
        }
        let (status, updated) = update_memory(&router, &id, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from UpdateMemory, got {status} ({updated})"
        );
        assert_eq!(
            updated["memory"]["indexedKeys"], expected,
            "case {case:?}: expected indexedKeys {expected}, got {}",
            updated["memory"]["indexedKeys"]
        );
        let (_, fetched) = get_memory(&router, &id).await;
        assert_eq!(
            fetched["memory"]["indexedKeys"], expected,
            "case {case:?}: GetMemory must report the indexedKeys UpdateMemory stored, got {}",
            fetched["memory"]["indexedKeys"]
        );
    }

    // (case, namespaceKeys at create, namespaceKeys on update, expected)
    let namespace_rows: [(&str, Value, Value, Value); 4] = [
        (
            "a replacement that drops one key",
            json!([{"key": "tenant"}, {"key": "region"}]),
            json!([{"key": "tenant"}]),
            json!([{"key": "tenant"}]),
        ),
        (
            "a replacement by a disjoint set",
            json!([{"key": "tenant"}]),
            json!([{"key": "stage", "validation": {"allowedValues": ["dev", "prod"]}}]),
            json!([{"key": "stage", "validation": {"allowedValues": ["dev", "prod"]}}]),
        ),
        (
            // `NamespaceKeysList` is `length ..5`, so an empty replacement is a
            // modelled value: it removes every key.
            "an empty replacement",
            json!([{"key": "tenant"}]),
            json!([]),
            json!([]),
        ),
        (
            "an omitted namespaceKeys",
            json!([{"key": "tenant"}]),
            Value::Null,
            json!([{"key": "tenant"}]),
        ),
    ];

    for (index, (case, stored, replacement, expected)) in namespace_rows.into_iter().enumerate() {
        let mut create = json!({"name": format!("replace{index}"), "eventExpiryDuration": 30});
        create["namespaceKeys"] = stored;
        let (status, created) = create_memory(&router, &create).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateMemory, got {status} ({created})"
        );
        let id = text(&created["memory"], "id");
        tokio::time::sleep(PAST_SETTLE).await;

        let mut body = json!({});
        if !replacement.is_null() {
            body["namespaceKeys"] = replacement;
        }
        let (status, updated) = update_memory(&router, &id, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from UpdateMemory, got {status} ({updated})"
        );
        assert_eq!(
            updated["memory"]["namespaceKeys"], expected,
            "case {case:?}: expected namespaceKeys {expected}, got {}",
            updated["memory"]["namespaceKeys"]
        );
        let (_, fetched) = get_memory(&router, &id).await;
        assert_eq!(
            fetched["memory"]["namespaceKeys"], expected,
            "case {case:?}: GetMemory must report the namespaceKeys UpdateMemory stored, got {}",
            fetched["memory"]["namespaceKeys"]
        );
    }
}

/// The strategies a request adds, modifies and deletes, and the statuses the
/// clock owes each of them.
#[tokio::test]
async fn strategies_are_added_modified_and_removed_with_the_statuses_the_clock_owes() {
    let router = router();

    let create = json!({
        "name": "strategies",
        "eventExpiryDuration": 30,
        "memoryStrategies": [
            {"semanticMemoryStrategy": {
                "name": "facts",
                "recordExpirySpecification": {"recordExpiryDurationDays": 40}
            }},
            {"summaryMemoryStrategy": {"name": "recap"}}
        ]
    });
    let (status, created) = create_memory(&router, &create).await;
    assert_eq!(
        status, 202,
        "case \"a memory with two strategies\": expected HTTP 202, got {status} ({created})"
    );
    let memory = created["memory"].clone();
    let id = text(&memory, "id");
    let facts_id = text(&strategy_by_name(&memory, "facts"), "strategyId");
    let recap_id = text(&strategy_by_name(&memory, "recap"), "strategyId");
    tokio::time::sleep(PAST_SETTLE).await;

    let body = json!({"memoryStrategies": {
        "addMemoryStrategies": [{"episodicMemoryStrategy": {"name": "episodes"}}],
        "modifyMemoryStrategies": [{
            "memoryStrategyId": facts_id,
            "description": "the strategy a test modified",
            "namespaceTemplates": ["/custom/{actorId}"],
            "recordExpirySpecification": {"recordExpiryDurationDays": 90}
        }],
        "deleteMemoryStrategies": [{"memoryStrategyId": recap_id}]
    }});
    let (status, updated) = update_memory(&router, &id, &body).await;
    assert_eq!(
        status, 202,
        "case \"one add, one modify, one delete\": expected HTTP 202, got {status} ({updated})"
    );
    let updated = updated["memory"].clone();

    assert_eq!(
        text(&updated, "status"),
        "UPDATING",
        "case \"one add, one modify, one delete\": an updated memory is UPDATING, got {}",
        text(&updated, "status")
    );

    let added = strategy_by_name(&updated, "episodes");
    let added_id = text(&added, "strategyId");
    // An update answers with the strategy it added already ACTIVE: the strategy is usable
    // when the call returns, so the clock owes it no transition of its own.
    assert_eq!(
        text(&added, "status"),
        "ACTIVE",
        "case \"an added strategy\": a strategy an update mints is ACTIVE, got {added}"
    );
    assert_eq!(
        text(&added, "type"),
        "EPISODIC",
        "case \"an added strategy\": expected type EPISODIC, got {added}"
    );

    let modified = strategy_by_id(&updated, &facts_id);
    assert_eq!(
        text(&modified, "description"),
        "the strategy a test modified",
        "case \"a modified strategy\": expected the description the request carried, got {modified}"
    );
    assert_eq!(
        modified["namespaceTemplates"], json!(["/custom/{actorId}"]),
        "case \"a modified strategy\": expected the namespaceTemplates the request carried, got {}",
        modified["namespaceTemplates"]
    );
    assert_eq!(
        modified["namespaces"], modified["namespaceTemplates"],
        "case \"a modified strategy\": the legacy namespaces member reports the same list, got {}",
        modified["namespaces"]
    );
    assert_eq!(
        modified["recordExpiryDurationDays"], json!(90),
        "case \"a modified strategy\": expected recordExpiryDurationDays 90, got {}",
        modified["recordExpiryDurationDays"]
    );
    assert_eq!(
        text(&modified, "name"),
        "facts",
        "case \"a modified strategy\": a member the request omitted keeps its value, got {modified}"
    );

    let deleted = strategy_by_id(&updated, &recap_id);
    assert_eq!(
        text(&deleted, "status"),
        "DELETING",
        "case \"a deleted strategy\": a strategy an update removes is DELETING, got {deleted}"
    );

    // The removal and the transitions are the clock's: once the span has passed the
    // memory is ACTIVE, every strategy it still holds is ACTIVE, and the removed one
    // is reported by no read.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, fetched) = get_memory(&router, &id).await;
    assert_eq!(
        status, 200,
        "case \"after the settle span\": expected HTTP 200 from GetMemory, got {status} ({fetched})"
    );
    let settled = fetched["memory"].clone();
    assert_eq!(
        text(&settled, "status"),
        "ACTIVE",
        "case \"after the settle span\": the memory is ACTIVE, got {}",
        text(&settled, "status")
    );
    assert!(
        strategy_by_id(&settled, &recap_id).is_null(),
        "case \"after the settle span\": the removed strategy is gone, got {}",
        settled["strategies"]
    );
    for still_held in [&facts_id, &added_id] {
        assert_eq!(
            text(&strategy_by_id(&settled, still_held), "status"),
            "ACTIVE",
            "case \"after the settle span\": strategy {still_held} is ACTIVE, got {}",
            strategy_by_id(&settled, still_held)
        );
    }

    // A strategy the memory no longer holds is addressed by no later request.
    let body = json!({"memoryStrategies": {
        "modifyMemoryStrategies": [{"memoryStrategyId": recap_id, "description": "gone"}]
    }});
    let (status, answer) = update_memory(&router, &id, &body).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"modifying a removed strategy\": expected HTTP 404 ResourceNotFoundException, got \
         {status} {} ({answer})",
        error_code(&answer)
    );

    // "Set to REMOVE to remove a previously configured time-to-live": the stored
    // duration is cleared, and only on the modify path.
    let body = json!({"memoryStrategies": {
        "modifyMemoryStrategies": [{
            "memoryStrategyId": facts_id,
            "recordExpirySpecification": {"action": "REMOVE"}
        }]
    }});
    let (status, answer) = update_memory(&router, &id, &body).await;
    assert_eq!(
        status, 202,
        "case \"a record expiry REMOVE on the modify path\": expected HTTP 202, got {status} \
         ({answer})"
    );
    assert!(
        strategy_by_id(&answer["memory"], &facts_id)["recordExpiryDurationDays"].is_null(),
        "case \"a record expiry REMOVE on the modify path\": expected no \
         recordExpiryDurationDays, got {}",
        strategy_by_id(&answer["memory"], &facts_id)
    );
}

/// What `UpdateMemory` refuses, and with which modelled error.
#[tokio::test]
async fn the_update_refuses_what_the_model_states_it_must() {
    let router = router();

    // A memory with a strategy whose namespace template resolves a caller-defined
    // variable, so a replacement that omits that variable has something to refuse.
    let create = json!({
        "name": "refusals",
        "eventExpiryDuration": 30,
        "namespaceKeys": [{"key": "tenant"}],
        "memoryStrategies": [{"semanticMemoryStrategy": {
            "name": "facts",
            "namespaceTemplates": ["/custom/{tenant}/{actorId}"]
        }}]
    });
    let (status, created) = create_memory(&router, &create).await;
    assert_eq!(
        status, 202,
        "the memory the refusal rows address is created, got {status} ({created})"
    );
    let id = text(&created["memory"], "id");
    let facts_id = text(&strategy_by_name(&created["memory"], "facts"), "strategyId");

    // A memory still CREATING cannot be updated, and this is the only row that can
    // observe that: the rows below run after the span has passed.
    let (status, answer) = update_memory(&router, &id, &json!({"description": "too soon"})).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (409, "ConflictException"),
        "case \"an update of a memory that is still CREATING\": expected HTTP 409 \
         ConflictException, got {status} {} ({answer})",
        error_code(&answer)
    );

    tokio::time::sleep(PAST_SETTLE).await;

    let too_many_indexed_keys: Vec<Value> = (0..=MAX_INDEXED_KEYS)
        .map(|index| json!({"key": format!("key{index}"), "type": "STRING"}))
        .collect();
    let too_many_strategies: Vec<Value> = (0..=MAX_STRATEGIES_PER_MEMORY)
        .map(|index| json!({"semanticMemoryStrategy": {"name": format!("s{index}")}}))
        .collect();

    // (case, memoryId, body, expected status, expected error code)
    let rows: [(&str, String, Value, u16, &str); 11] = [
        (
            "a memoryId naming no stored memory",
            "noSuchMemory-0123456789".to_string(),
            json!({"description": "whatever"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a modifyMemoryStrategies id the memory does not hold",
            id.clone(),
            json!({"memoryStrategies": {"modifyMemoryStrategies": [
                {"memoryStrategyId": "noSuchStrategy-0123456789", "description": "x"}
            ]}}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a deleteMemoryStrategies id the memory does not hold",
            id.clone(),
            json!({"memoryStrategies": {"deleteMemoryStrategies": [
                {"memoryStrategyId": "noSuchStrategy-0123456789"}
            ]}}),
            404,
            "ResourceNotFoundException",
        ),
        (
            // "Any referenced namespaceKey omission will throw ValidationException."
            "a namespaceKeys replacement that omits a referenced variable",
            id.clone(),
            json!({"namespaceKeys": []}),
            400,
            "ValidationException",
        ),
        (
            "a namespaceKeys replacement that keeps the referenced variable",
            id.clone(),
            json!({"namespaceKeys": [{"key": "tenant"}, {"key": "region"}]}),
            202,
            "<no __type>",
        ),
        (
            // `RecordExpiryDurationDays` is `range 31..`.
            "a modified record expiry one day below the published minimum",
            id.clone(),
            json!({"memoryStrategies": {"modifyMemoryStrategies": [{
                "memoryStrategyId": facts_id.clone(),
                "recordExpirySpecification": {"recordExpiryDurationDays": 30}
            }]}}),
            400,
            "ValidationException",
        ),
        (
            "a modified record expiry at the published minimum",
            id.clone(),
            json!({"memoryStrategies": {"modifyMemoryStrategies": [{
                "memoryStrategyId": facts_id.clone(),
                "recordExpirySpecification": {"recordExpiryDurationDays": 31}
            }]}}),
            202,
            "<no __type>",
        ),
        (
            // `eventExpiryDuration` is `range 3..365` on this request.
            "an eventExpiryDuration one below the published minimum",
            id.clone(),
            json!({"eventExpiryDuration": 2}),
            400,
            "ValidationException",
        ),
        (
            "an eventExpiryDuration at the published maximum",
            id.clone(),
            json!({"eventExpiryDuration": 365}),
            202,
            "<no __type>",
        ),
        (
            "an eventExpiryDuration one past the published maximum",
            id.clone(),
            json!({"eventExpiryDuration": 366}),
            400,
            "ValidationException",
        ),
        (
            "more strategies than one memory holds",
            id.clone(),
            json!({"memoryStrategies": {"addMemoryStrategies": too_many_strategies}}),
            402,
            "ServiceQuotaExceededException",
        ),
    ];

    for (case, memory_id, body, expected_status, expected_code) in rows {
        let (status, answer) = update_memory(&router, &memory_id, &body).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} {} \
             ({answer})",
            error_code(&answer)
        );
    }

    // `addIndexedKeys` is `length 1..10`, so a merge the memory cannot render is
    // refused: eleven keys at once is past the ceiling the boundary leaves open.
    let (status, answer) = update_memory(
        &router,
        &id,
        &json!({"addIndexedKeys": too_many_indexed_keys}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "ValidationException"),
        "case \"a merge past the indexed-key ceiling\": expected HTTP 400 ValidationException, got \
         {status} {} ({answer})",
        error_code(&answer)
    );

    // Nothing a refused row asked for was written: the strategy it named is still
    // the only one, the namespace key it tried to drop is still defined, and the
    // record expiry is the one the accepted row set.
    let (_, fetched) = get_memory(&router, &id).await;
    let memory = fetched["memory"].clone();
    assert_eq!(
        memory["namespaceKeys"], json!([{"key": "tenant"}, {"key": "region"}]),
        "case \"a refused update persists nothing\": expected the namespaceKeys the accepted row \
         wrote, got {}",
        memory["namespaceKeys"]
    );
    assert_eq!(
        memory["strategies"].as_array().map(Vec::len),
        Some(1),
        "case \"a refused update persists nothing\": expected the one strategy the create minted, \
         got {}",
        memory["strategies"]
    );
    assert_eq!(
        strategy_by_id(&memory, &facts_id)["recordExpiryDurationDays"], json!(31),
        "case \"a refused update persists nothing\": expected the record expiry the accepted row \
         wrote, got {}",
        strategy_by_id(&memory, &facts_id)
    );
}

/// A removed strategy is reclaimed on ITS OWN deadline, so later updates of the
/// same memory cannot defer the removal, and what one memory retains stays
/// bounded by the per-memory strategy ceiling.
///
/// The rows differ along how many updates follow the removal and how fast they
/// come, which is what decides whether a shared deadline would ever be reached.
#[tokio::test]
async fn a_removal_is_reclaimed_on_its_own_deadline_under_later_updates() {
    let router = router();

    // (case, how many updates follow the removal, the pause between them)
    let rows: [(&str, usize, Duration); 3] = [
        ("no update follows the removal", 0, Duration::from_millis(0)),
        (
            "updates faster than the settle span",
            8,
            Duration::from_millis(120),
        ),
        (
            "one update after the settle span",
            1,
            Duration::from_millis(600),
        ),
    ];

    for (index, (case, updates, pause)) in rows.into_iter().enumerate() {
        let create = json!({
            "name": format!("reclaim{index}"),
            "eventExpiryDuration": 30,
            "memoryStrategies": [
                {"semanticMemoryStrategy": {"name": "kept"}},
                {"summaryMemoryStrategy": {"name": "removed"}}
            ]
        });
        let (status, created) = create_memory(&router, &create).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateMemory, got {status} ({created})"
        );
        let id = text(&created["memory"], "id");
        let removed_id = text(&strategy_by_name(&created["memory"], "removed"), "strategyId");
        tokio::time::sleep(PAST_SETTLE).await;

        let (status, answer) = update_memory(
            &router,
            &id,
            &json!({"memoryStrategies": {"deleteMemoryStrategies": [
                {"memoryStrategyId": removed_id.clone()}
            ]}}),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from the removing update, got {status} ({answer})"
        );

        for update in 0..updates {
            tokio::time::sleep(pause).await;
            let (status, answer) =
                update_memory(&router, &id, &json!({"description": format!("u{update}")})).await;
            assert_eq!(
                status, 202,
                "case {case:?}: expected HTTP 202 from follow-up update {update}, got {status} \
                 ({answer})"
            );
        }
        if updates == 0 {
            tokio::time::sleep(PAST_SETTLE).await;
        }

        let (_, fetched) = get_memory(&router, &id).await;
        assert!(
            strategy_by_id(&fetched["memory"], &removed_id).is_null(),
            "case {case:?}: the removed strategy is gone once its own deadline passed, got {}",
            fetched["memory"]["strategies"]
        );
        assert_eq!(
            fetched["memory"]["strategies"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected the one strategy the memory still holds, got {}",
            fetched["memory"]["strategies"]
        );
    }

    // A request that would leave one memory holding more strategies than the
    // per-memory ceiling is refused, whatever it removes in the same breath.
    let at_ceiling: Vec<Value> = (0..MAX_STRATEGIES_PER_MEMORY)
        .map(|index| json!({"semanticMemoryStrategy": {"name": format!("s{index}")}}))
        .collect();
    let (status, created) = create_memory(
        &router,
        &json!({
            "name": "ceiling",
            "eventExpiryDuration": 30,
            "memoryStrategies": at_ceiling
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "the memory at the strategy ceiling is created, got {status} ({created})"
    );
    let id = text(&created["memory"], "id");
    let first_id = text(&strategy_by_name(&created["memory"], "s0"), "strategyId");
    tokio::time::sleep(PAST_SETTLE).await;

    let (status, answer) = update_memory(
        &router,
        &id,
        &json!({"memoryStrategies": {
            "deleteMemoryStrategies": [{"memoryStrategyId": first_id}],
            "addMemoryStrategies": [{"semanticMemoryStrategy": {"name": "onemore"}}]
        }}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (402, "ServiceQuotaExceededException"),
        "case \"a delete-and-add at the strategy ceiling\": expected HTTP 402 \
         ServiceQuotaExceededException, got {status} {} ({answer})",
        error_code(&answer)
    );
}

/// A repeated `clientToken` replays the update instead of applying it a second
/// time, and a fresh token applies the next one.
#[tokio::test]
async fn a_repeated_client_token_replays_the_update() {
    let router = router();

    let (status, created) =
        create_memory(&router, &configured_memory_body("idempotent")).await;
    assert_eq!(
        status, 202,
        "the memory the replay addresses is created, got {status} ({created})"
    );
    let id = text(&created["memory"], "id");
    tokio::time::sleep(PAST_SETTLE).await;

    let mut body = json!({
        "description": "the first update",
        "clientToken": "idempotent-update-memory-token-0001"
    });
    let (status, first) = update_memory(&router, &id, &body).await;
    assert_eq!(
        status, 202,
        "case \"the first update\": expected HTTP 202, got {status} ({first})"
    );
    assert_eq!(
        text(&first["memory"], "description"),
        "the first update",
        "case \"the first update\": expected the description the request carried, got {}",
        text(&first["memory"], "description")
    );

    body["description"] = json!("the replayed update");
    let (status, replayed) = update_memory(&router, &id, &body).await;
    assert_eq!(
        (status, text(&replayed["memory"], "description")),
        (202, "the first update".to_string()),
        "case \"a replayed update\": expected HTTP 202 and the first update's description, got \
         {status} ({replayed})"
    );

    body["clientToken"] = json!("idempotent-update-memory-token-0002");
    let (status, applied) = update_memory(&router, &id, &body).await;
    assert_eq!(
        (status, text(&applied["memory"], "description")),
        (202, "the replayed update".to_string()),
        "case \"a second update under a new token\": expected HTTP 202 and the new description, \
         got {status} ({applied})"
    );
}
