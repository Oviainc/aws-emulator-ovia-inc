Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# UnknownDimension refusal names an arbitrary Filters key, so the same request answers with a different message run to run

_Filed: 2026-10-25T04:47:34Z_
_Operation: QueryForecast | Class: error-response_

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StoredForecast::query` (the projection both query handlers call through `ResourceKind::resolve_query`) refuses a request whose `Filters` carries a key that is not a dimension of the forecast by naming ONE offending key in the message. It picks that key with `filters.keys().find(..)` over a `std::collections::HashMap`, whose iteration order is unspecified and randomised per map/process. So when a request carries TWO (or more) keys that are not dimensions of the forecast — which the model allows, `Filters` being a map of 1..50 entries — the `InvalidInputException` message names whichever key the hash order happened to visit first: "Filters key 'alpha_key' is not a dimension of this forecast; valid keys are [...]" on one run and "Filters key 'beta_key' ..." on the next, for a byte-identical request against byte-identical state.

This contradicts the determinism rule this same module states and applies on its sibling path: the shape-validation clause for `Filters` deliberately renders the map through a `BTreeMap` precisely because "a message that varied run to run could not be matched on", and `shape_validation_message` orders its clauses "in the model's own member order, so a request wrong in two places reads the same way every time". The `UnknownDimension` path, which needs exactly the same rule, skips it.

Nothing in the crate pins the current behaviour: the only test row with an unknown filter key sends a single one (`{"item_id": "sku-9", "region": "emea"}`) and asserts merely that the `Message` is non-empty, so the multi-key case is unpinned and the message is free to wobble. Both operations are affected, since both render the same shared outcome.

## Evidence

The non-deterministic selection — services/service-com-amazonaws-forecastquery-amazonforecastruntime/src/state.rs:586-595:
```rust
        let known = self.dimension_names();
        if let Some(unknown) = filters
            .keys()
            .find(|key| !known.iter().any(|name| name == *key))
        {
            return Err(QueryRejection::UnknownDimension {
                key: unknown.clone(),
                known,
            });
        }
```
and the map it iterates is a `HashMap` — src/state.rs:323-324: "/// `Filters`: the criteria that select the one time series to retrieve. \n pub filters: &'a HashMap<String, String>". The chosen key is interpolated verbatim into the reported message — src/state.rs:1008-1015: `Self::UnknownDimension { key, known } => format!("Filters key '{}' is not a dimension of this forecast; valid keys are [{}]", echoed(key), known.join(", "))`.

The rule this crate states for exactly this situation — src/state.rs:850-855:
```
/// A map as a message shows it: `{key=value, key=value}`.
///
/// Ordered by key — a map has no wire order, so the only deterministic rendering of one
/// is a sorted one, and a message that varied run to run could not be matched on.
fn rendered_map(map: &HashMap<String, String>) -> String {
    let ordered: BTreeMap<&String, &String> = map.iter().collect();
```
and src/state.rs:914-915: "// In the model's own member order, so a request wrong in two places reads the same way every time."

The rendering path that reaches the client — src/ops/query_forecast.rs:243-256 (`FORECAST.resolve_query(..)`) and src/ops/query_forecast.rs:288-295: `crate::state::QueryOutcome::Unqueryable(message) => return Err(QueryForecastErrorModeled::InvalidInputException(InvalidInputException { message: Some(message) },)`; the identical rendering in src/ops/query_what_if_forecast.rs:295-302.

Test coverage that leaves the case unpinned — tests/all/forecastquery.rs:667-673 is the only unknown-key row and sends ONE unknown key:
```rust
        Case {
            name: "a key that is not a dimension of this forecast",
            filters: json!({"item_id": "sku-9", "region": "emea"}),
            expect_status: 400,
            expect_code: "InvalidInputException",
```
and tests/all/forecastquery.rs:710-717 asserts only `!message.is_empty()`.

## Steps to Reproduce

1. Seed a forecast as tests/all/forecastquery.rs does: `StoredForecast::new(arn("forecast/dimensions"), "by_store", "D", ["0.5"])` with one series `StoredSeries::new([("item_id", "sku-9")])` carrying p50 points, via `put_forecast`.
2. Call QueryForecast with `{"ForecastArn": <that ARN>, "Filters": {"alpha_key": "1", "beta_key": "2"}}` — both keys satisfy the `AttributeName` key shape, and neither is a dimension of the forecast nor `item_id`.
3. Observe HTTP 400 InvalidInputException whose Message names only one of the two keys.
4. Repeat the call in a fresh process (or with more unknown keys) and observe the Message naming a different key for the identical request and identical stored state — compare with the sibling shape-validation refusal for bad filter KEYS, which renders the whole map sorted (`rendered_map`) and is byte-stable across runs.
