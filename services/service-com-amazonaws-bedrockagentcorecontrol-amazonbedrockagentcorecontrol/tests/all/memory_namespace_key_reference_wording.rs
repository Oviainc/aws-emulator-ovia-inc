// SPDX-License-Identifier: CC0-1.0

//! How `UpdateMemory` refuses a `namespaceKeys` replacement that drops a key a memory's
//! live strategies still resolve through, end to end over HTTP.
//!
//! One rule is under test: a replacement that omits a namespace variable key the memory
//! DEFINES and a live strategy's namespace list still references is refused with the
//! modelled `ValidationException` (HTTP 400, `smithy_query(query="operation",
//! operation="UpdateMemory")`) stating the operation that failed validation, the whole
//! bracketed list of the keys the memory defines that live strategies still reference, and
//! the one remedy — "Validation failed during UpdateMemory: [<keys>] namespaceKeys are
//! still in reference with existing strategies, please update the strategies before
//! proceeding with this update". The bracketed list is taken from the memory's stored
//! `namespaceKeys`, in the order the memory holds them, and names no strategy: a failed
//! update discloses no strategy name and no generated strategy id.
//!
//! The rows differ along how many keys the memory defines, how many of them a template
//! references, how many the replacement drops, which member the strategy's namespace list
//! arrived under (`namespaceTemplates` or the legacy `namespaces`), how many strategies
//! reference one key, and whether the template resolves caller-defined variables at all —
//! because the rule is about which defined keys are still referenced, not about one
//! request's keys. A key the memory does not define, and the three variables the service
//! defines itself, are referenced through no definition, so dropping them is accepted.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The settle span `src/state.rs` gives a memory's `CREATING` -> `ACTIVE` transition, plus
/// slack: an update of a memory still `CREATING` conflicts, so every row waits it out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// The shortest event expiry a memory may be created with ("between 7 and 365 days",
/// `smithy_query(query="doc", shape="CreateMemoryInput", member="eventExpiryDuration")`).
const EVENT_EXPIRY_DAYS: u32 = 7;

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
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
        .header("authorization", authorization())
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

/// The whole sentence the refusal carries: fixed text either side of the bracketed list of
/// the keys the memory defines that live strategies still reference.
fn still_referenced_message(keys: &[&str]) -> String {
    format!(
        "Validation failed during UpdateMemory: [{}] namespaceKeys are still in reference with \
         existing strategies, please update the strategies before proceeding with this update",
        keys.join(", ")
    )
}

/// A `semanticMemoryStrategy` whose namespace list arrives under `member`.
fn strategy(name: &str, member: &str, namespaces: Value) -> Value {
    let mut spec = json!({"name": name});
    spec[member] = namespaces;
    json!({"semanticMemoryStrategy": spec})
}

/// One row of the table: the case, the `namespaceKeys` the memory is created with, the
/// strategies it is created with, the replacement the update carries, and the bracketed
/// list the refusal must carry — `None` where no live strategy references a dropped key,
/// so the replacement is accepted.
type Row = (
    &'static str,
    Value,
    Value,
    Value,
    Option<&'static [&'static str]>,
);

/// Every `strategyId` a memory reports, which the refusal may not disclose.
fn strategy_ids(memory: &Value) -> Vec<String> {
    memory["strategies"]
        .as_array()
        .map(|strategies| {
            strategies
                .iter()
                .map(|one| text(one, "strategyId"))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_namespace_keys_replacement_dropping_a_referenced_key_is_refused_in_the_services_sentence()
{
    let router = router();

    let rows: [Row; 10] = [
        (
            "one defined key a template references, dropped",
            json!([{"key": "customattr"}]),
            json!([strategy(
                "facts",
                "namespaceTemplates",
                json!(["/custom/{customattr}/facts"])
            )]),
            json!([{"key": "unrelatedattr"}]),
            Some(&["customattr"]),
        ),
        (
            "two defined keys both referenced, one dropped",
            json!([{"key": "tenant"}, {"key": "region"}]),
            json!([strategy(
                "facts",
                "namespaceTemplates",
                json!(["/custom/{tenant}/{region}/facts"])
            )]),
            json!([{"key": "tenant"}]),
            Some(&["tenant", "region"]),
        ),
        (
            // The list is every DEFINED key still referenced, in the order the memory
            // holds them — not only the keys this request dropped, and not the defined
            // key no template mentions.
            "three defined keys, the two referenced ones dropped",
            json!([{"key": "tenant"}, {"key": "region"}, {"key": "stage"}]),
            json!([
                strategy(
                    "facts",
                    "namespaceTemplates",
                    json!(["/custom/{tenant}/facts"])
                ),
                strategy("stages", "namespaceTemplates", json!(["/other/{stage}"])),
            ]),
            json!([{"key": "region"}]),
            Some(&["tenant", "stage"]),
        ),
        (
            // `NamespaceKeysList` is `length ..5`, so an empty replacement is a modelled
            // value: it drops every key.
            "an empty replacement while a key is referenced",
            json!([{"key": "tenant"}]),
            json!([strategy(
                "facts",
                "namespaceTemplates",
                json!(["/custom/{tenant}"])
            )]),
            json!([]),
            Some(&["tenant"]),
        ),
        (
            // "Use namespaceTemplates instead" — the legacy member references the same
            // way (`smithy_query(query="shape", shape="SemanticMemoryStrategyInput")`).
            "the legacy namespaces member references the key",
            json!([{"key": "zone"}]),
            json!([strategy("facts", "namespaces", json!(["/custom/{zone}"]))]),
            json!([]),
            Some(&["zone"]),
        ),
        (
            // One key, two referencing strategies: the key is listed once.
            "two strategies referencing one key, dropped",
            json!([{"key": "tenant"}]),
            json!([
                strategy(
                    "facts",
                    "namespaceTemplates",
                    json!(["/custom/{tenant}/facts"])
                ),
                strategy(
                    "prefs",
                    "namespaceTemplates",
                    json!(["/custom/{tenant}/prefs"])
                ),
            ]),
            json!([{"key": "stage"}]),
            Some(&["tenant"]),
        ),
        (
            "a defined key no template references, dropped",
            json!([{"key": "tenant"}, {"key": "region"}]),
            json!([strategy(
                "facts",
                "namespaceTemplates",
                json!(["/custom/{tenant}"])
            )]),
            json!([{"key": "tenant"}]),
            None,
        ),
        (
            // `memoryStrategyId`, `actorId` and `sessionId` are the variables the service
            // defines itself — exactly the names `NamespaceVariableKey`'s pattern
            // `^(?!memoryStrategyId$|actorId$|sessionId$)[a-z][a-z0-9]*$` forbids a caller
            // to define (`smithy_query(query="shape", shape="NamespaceVariableKey")`), so
            // a template resolving only those references no definition.
            "a template resolving only service-defined variables",
            json!([{"key": "tenant"}]),
            json!([strategy(
                "facts",
                "namespaceTemplates",
                json!(["/strategies/{memoryStrategyId}/actors/{actorId}"])
            )]),
            json!([]),
            None,
        ),
        (
            // A variable no `namespaceKeys` entry defines resolves through no definition
            // whatever the replacement carries, so no replacement can drop its
            // definition.
            "a template referencing a key the memory never defined",
            json!([{"key": "tenant"}]),
            json!([strategy(
                "facts",
                "namespaceTemplates",
                json!(["/custom/{ghost}/facts"])
            )]),
            json!([]),
            None,
        ),
        (
            "a memory with no strategy at all",
            json!([{"key": "tenant"}]),
            Value::Null,
            json!([]),
            None,
        ),
    ];

    // Every memory is created first and the settle span waited out ONCE: an update of a
    // memory still `CREATING` is refused for a different reason.
    let mut created: Vec<(String, Vec<String>)> = Vec::new();
    for (index, (case, stored, strategies, _, _)) in rows.iter().enumerate() {
        let mut body = json!({
            "name": format!("nskeys{index}"),
            "eventExpiryDuration": EVENT_EXPIRY_DAYS,
            "namespaceKeys": stored.clone(),
        });
        if !strategies.is_null() {
            body["memoryStrategies"] = strategies.clone();
        }
        let (status, answer) = create_memory(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateMemory, got {status} ({answer})"
        );
        created.push((
            text(&answer["memory"], "id"),
            strategy_ids(&answer["memory"]),
        ));
    }
    tokio::time::sleep(PAST_SETTLE).await;

    for ((case, stored, _, replacement, expected), (id, strategy_ids)) in
        rows.into_iter().zip(created.iter())
    {
        let (status, answer) =
            update_memory(&router, id, &json!({"namespaceKeys": replacement.clone()})).await;
        let message = text(&answer, "message");

        let Some(expected_keys) = expected else {
            assert_eq!(
                status, 202,
                "case {case:?}: dropping a key no live strategy references is accepted, got \
                 {status} ({answer})"
            );
            assert_eq!(
                answer["memory"]["namespaceKeys"], replacement,
                "case {case:?}: expected the namespaceKeys the request carried, got {}",
                answer["memory"]["namespaceKeys"]
            );
            let (_, fetched) = get_memory(&router, id).await;
            assert_eq!(
                fetched["memory"]["namespaceKeys"], replacement,
                "case {case:?}: GetMemory must report the namespaceKeys the update stored, got {}",
                fetched["memory"]["namespaceKeys"]
            );
            continue;
        };

        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {} ({answer})",
            error_code(&answer)
        );
        assert_eq!(
            message,
            still_referenced_message(expected_keys),
            "case {case:?}: expected the service's still-referenced sentence {:?}, got {message:?}",
            still_referenced_message(expected_keys)
        );
        assert_eq!(
            text(&answer, "reason"),
            "FieldValidationFailed",
            "case {case:?}: a request member failed validation, so that is the \
             ValidationExceptionReason the refusal carries, got {answer}"
        );
        for strategy_id in strategy_ids {
            assert!(
                !message.contains(strategy_id),
                "case {case:?}: the refusal must disclose no strategy id, but {message:?} carries \
                 {strategy_id:?}"
            );
        }

        // A refused replacement wrote nothing: the memory still defines what it defined.
        let (_, fetched) = get_memory(&router, id).await;
        assert_eq!(
            fetched["memory"]["namespaceKeys"], stored,
            "case {case:?}: a refused replacement leaves the stored namespaceKeys alone, got {}",
            fetched["memory"]["namespaceKeys"]
        );
    }
}

/// Retargeting the strategy is what the refusal asks for, so the same replacement is
/// accepted once no live strategy resolves through the dropped key — whether the retarget
/// arrives in the same request or in an earlier one.
#[tokio::test]
async fn a_replacement_is_accepted_once_the_strategies_no_longer_reference_the_key() {
    let router = router();

    // (case, whether the retarget rides along in the refused request's successor)
    for (case, retarget_in_the_same_request) in
        [("retargeted first", false), ("retargeted in one call", true)]
    {
        let create = json!({
            "name": if retarget_in_the_same_request { "together" } else { "apart" },
            "eventExpiryDuration": EVENT_EXPIRY_DAYS,
            "namespaceKeys": [{"key": "customattr"}],
            "memoryStrategies": [strategy(
                "facts",
                "namespaceTemplates",
                json!(["/custom/{customattr}/facts"])
            )],
        });
        let (status, created) = create_memory(&router, &create).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateMemory, got {status} ({created})"
        );
        let id = text(&created["memory"], "id");
        let strategy_id = strategy_ids(&created["memory"])
            .first()
            .cloned()
            .unwrap_or_default();
        tokio::time::sleep(PAST_SETTLE).await;

        let replacement = json!([{"key": "unrelatedattr"}]);
        let (status, answer) =
            update_memory(&router, &id, &json!({"namespaceKeys": replacement.clone()})).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: the replacement is refused while the strategy references the key, got \
             {status} {} ({answer})",
            error_code(&answer)
        );

        let retarget = json!({"memoryStrategies": {"modifyMemoryStrategies": [{
            "memoryStrategyId": strategy_id,
            "namespaceTemplates": ["/generic/facts"]
        }]}});
        let accepted = if retarget_in_the_same_request {
            let mut body = retarget;
            body["namespaceKeys"] = replacement.clone();
            update_memory(&router, &id, &body).await
        } else {
            let (status, answer) = update_memory(&router, &id, &retarget).await;
            assert_eq!(
                status, 202,
                "case {case:?}: expected HTTP 202 from the retargeting update, got {status} \
                 ({answer})"
            );
            update_memory(&router, &id, &json!({"namespaceKeys": replacement.clone()})).await
        };
        let (status, answer) = accepted;
        assert_eq!(
            status, 202,
            "case {case:?}: the replacement is accepted once no strategy references the key, got \
             {status} ({answer})"
        );

        let (_, fetched) = get_memory(&router, &id).await;
        assert_eq!(
            fetched["memory"]["namespaceKeys"], replacement,
            "case {case:?}: GetMemory must report the replacement, got {}",
            fetched["memory"]["namespaceKeys"]
        );
    }
}
