Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36

# A variable or pool created through InvokeRestApi under a key containing "/" can never be read, changed or deleted again

_Filed: 2026-10-29T01:13:06Z_
_Operation: InvokeRestApi | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InvokeRestApi`'s Apache Airflow dispatcher addresses one entity with a TWO-segment path (`["variables", key]` / `["pools", name]`), and `Call::new` builds those segments by splitting `Path` on `/`. The write endpoints, however, accept ANY non-empty string (up to `MAX_AIRFLOW_KEY_LENGTH`) as the entity's key: `POST /variables` with body `{"key":"team/schedule","value":"x"}` and `POST /pools` with `{"name":"team/etl","slots":4}` both answer 200 and INSERT the entity into the environment's stored `AirflowMetadata`.

That stored entity is then permanently unaddressable through the very API that created it: `/variables/team/schedule` splits into three segments (`["variables","team","schedule"]`), which matches no arm of `serve`, so it falls through to `path_not_found` — a 404 — for `GET`, `PATCH` and `DELETE` alike. There is no escape hatch: percent-encoding the separator (`/variables/team%2Fschedule`) looks up the literal key `team%2Fschedule`, which is a different key.

Consequences, all from stored state the caller can see but not reach:
* The entity is reported by `GET /variables` / `GET /pools` (so the caller is told it exists), but can never be read individually, patched, or deleted.
* It occupies the store's own per-environment budget (`MAX_AIRFLOW_VARIABLES` = 500, `MAX_AIRFLOW_POOLS` = 500) with no API path to free it, so a caller can permanently wedge the budget and later writes are refused with "this emulator holds at most 500 variables for one environment" while nothing can be removed.
* For a pool the loss is total — `create_pool` refuses a name already taken (409), so such a pool cannot even be overwritten the way `set_variable` can overwrite a variable's value.

The handler already validates these keys — it refuses an empty key and one past `MAX_AIRFLOW_KEY_LENGTH` — so this is a missing case in an existing validator, not new behaviour: a key the dispatcher's own path grammar cannot express must be refused at the write instead of being stored unreachably.

## Evidence

The dispatcher addresses an entity with exactly two path segments — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:373-389:
```
        ["variables", key] => match call.method {
            GET => read_variable(&environment.airflow, key),
            PATCH => change_variable(&mut environment.airflow, key, call),
            DELETE => delete_variable(&mut environment.airflow, key),
            _ => method_not_allowed(call),
        },
...
        ["pools", name] => match call.method {
            GET => read_pool(&environment.airflow, name),
            PATCH => change_pool(&mut environment.airflow, name, call),
            DELETE => delete_pool(&mut environment.airflow, name),
            _ => method_not_allowed(call),
        },
```
and the segments are the `Path` split on `/` — invoke_rest_api.rs:289-301:
```
    fn new(input: &'r InvokeRestApiInput) -> Self {
        Self {
            method: input.method.as_str(),
            path: input.path.as_str(),
            segments: input
                .path
                .split('/')
                .filter(|segment| !segment.is_empty())
                .collect(),
```
Anything else falls through to a 404 — invoke_rest_api.rs:398 `_ => path_not_found(call),` with invoke_rest_api.rs:664-668:
```
fn path_not_found(call: &Call<'_>) -> AirflowAnswer {
    AirflowAnswer::not_found(format!(
        "{} is not one of the paths this environment's Apache Airflow REST API serves",
        call.path
    ))
```
The write side accepts a key with any content and stores it — invoke_rest_api.rs:521-536:
```
fn set_variable(metadata: &mut AirflowMetadata, call: &Call<'_>) -> AirflowAnswer {
    let requested = match requested_variable(call.body) {
        Ok(requested) => requested,
        Err(refusal) => return refusal,
    };
    let fresh = !metadata.variables.contains_key(&requested.key);
    if fresh && metadata.variables.len() >= crate::state::MAX_AIRFLOW_VARIABLES {
```
```
    let key = requested.key.clone();
    metadata.variables.insert(key.clone(), requested);
    stored_variable(metadata, &key)
```
and the only key check it applies is non-emptiness plus a length bound — invoke_rest_api.rs:844-851 and :865-872:
```
fn required_text(body: &Document, name: &str) -> Result<String, AirflowAnswer> {
    match member(body, name).and_then(text) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
```
```
fn bounded(value: &str, name: &str, longest: usize) -> Result<(), AirflowAnswer> {
    if value.chars().count() > longest {
```
The pool path is worse because the name is its identity — invoke_rest_api.rs:586-603:
```
    if metadata.pools.contains_key(&requested.name) {
        return AirflowAnswer::conflict(format!("the pool {:?} already exists", requested.name));
    }
    if metadata.pools.len() >= crate::state::MAX_AIRFLOW_POOLS {
```
The model allows every byte of this request: `Path` is `RestApiPath` `@length(1, 64)` with no pattern and `Body` is a free `Document` (smithy_query query="operation" operation="InvokeRestApi": `"Path": {"target": "RestApiPath", "required": true, "length": {"min": 1, "max": 64}}`, `"Body": {"target": "RestApiRequestBody", "sensitive": true, "type": "document", "document": true}`), so nothing refuses it before the handler runs.

Test coverage: the existing suite pins the key validator only for length and type — tests/all/airflow_rest_api.rs:591 `json!({"key": "k".repeat(state::MAX_AIRFLOW_KEY_LENGTH + 1), "value": "1"})` and :568 `json!({"key": "k"})` inside `an_airflow_refusal_is_the_modelled_client_exception` (airflow_rest_api.rs:514) — and `a_write_is_visible_to_the_next_read` (airflow_rest_api.rs:393) only uses keys with no separator in them, so no test pins a key containing `/` in either direction.

## Steps to Reproduce

1. Create an environment and wait for it to settle into AVAILABLE (PUT /environments/MyMWAAEnvironment with a valid body, then GET it until Status is AVAILABLE).
2. POST /restapi/MyMWAAEnvironment with {"Path": "/variables", "Method": "POST", "Body": {"key": "team/schedule", "value": "@daily"}} — observe HTTP 200 with RestApiStatusCode 200 and RestApiResponse naming the stored variable, i.e. it was inserted into AirflowMetadata.variables.
3. POST /restapi/MyMWAAEnvironment with {"Path": "/variables", "Method": "GET"} — the variable is listed with key "team/schedule" and total_entries 1, confirming it is stored.
4. POST /restapi/MyMWAAEnvironment with {"Path": "/variables/team/schedule", "Method": "GET"} — expect the stored variable; observe RestApiClientException (HTTP 400) carrying RestApiStatusCode 404 and the detail "/variables/team/schedule is not one of the paths this environment's Apache Airflow REST API serves", because Call::new split the path into three segments and serve matched no arm.
5. Repeat step 4 with Method "DELETE" (and "PATCH") — the same 404: the variable can never be removed or changed, yet it keeps occupying one of the MAX_AIRFLOW_VARIABLES slots.
6. Do the same for a pool: {"Path": "/pools", "Method": "POST", "Body": {"name": "team/etl", "slots": 4}} answers 200, {"Path": "/pools", "Method": "GET"} lists it, and {"Path": "/pools/team/etl", "Method": "DELETE"} answers 404 — while re-POSTing the same name now answers 409 "the pool \"team/etl\" already exists", so the entry is unreachable and unreplaceable.
