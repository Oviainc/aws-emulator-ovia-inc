Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3a384b4a12755a7fa8fedde9fe2fff5778558e98

# An Apache Airflow variable key / pool name longer than the modelled RestApiPath can carry is stored, so no path can ever read, change or delete it again

_Filed: 2026-10-29T07:02:05Z_
_Operation: InvokeRestApi | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3a384b4a12755a7fa8fedde9fe2fff5778558e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InvokeRestApi`'s Apache Airflow write endpoints bound an entity key only by the crate's own `MAX_AIRFLOW_KEY_LENGTH` (250 characters) and by the separator rule `addressable` enforces. But an entity is only reachable afterwards through `Path`, which the model caps at 64 characters (`RestApiPath` `@length(1, 64)`). `/variables/` is 11 characters and `/pools/` is 7, so only a key of at most 53 (variable) or 57 (pool) characters can ever appear in a legal `Path`.

A `POST /variables` with a 200-character `key` (or a `POST /pools` with a 200-character `name`) therefore answers 200 and INSERTS the entity into the environment's stored `AirflowMetadata` — the write path's own `Path` is just `/variables`, well inside 64 — while every later `GET` / `PATCH` / `DELETE` of it needs a `Path` of 211 characters, which the boundary validator refuses with `ValidationException` (HTTP 400) before the handler even runs. The entity is reported by `GET /variables` / `GET /pools` (so the caller is told it exists) but can never be read individually, patched or removed, and it permanently occupies one of the per-environment `MAX_AIRFLOW_VARIABLES` / `MAX_AIRFLOW_POOLS` slots with no API path to free it. For a pool the loss is total: `create_pool` answers 409 for a name already taken, so it cannot even be overwritten the way `set_variable` can overwrite a variable's value.

This is the exact harm the crate's own `addressable` helper exists to prevent — its doc states the rule in full — and the writes already run a length check on the same value, so this is a missing case in an existing validator. It is NOT the separator case (a key containing `/`): that one is handled, and the handling introduced the rule this length ceiling now breaks.

## Evidence

The crate states the rule it breaks, in the helper the writes call — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:899-906:
```
/// Refuse a key this API could store but no path of it could name again.
///
/// An entity is addressed by the segments of its path, which carry no empty one
/// ([`Call::new`]), so a key with an empty [`SEPARATOR`]-delimited segment — one that
/// opens or closes with the separator, or carries two in a row — is a key no path splits
/// into: storing it would put an entity in the collection that the API's own read, change
/// and delete could never reach, and whose slot nothing could free.
```
The only LENGTH the write judges a key by is the crate's own ceiling — invoke_rest_api.rs:808-809 (variable) and :842-843 (pool):
```
    bounded(&key, "key", crate::state::MAX_AIRFLOW_KEY_LENGTH)?;
    addressable(&key, "key")?;
```
```
    bounded(&name, "name", crate::state::MAX_AIRFLOW_KEY_LENGTH)?;
    addressable(&name, "name")?;
```
and that ceiling is nearly five times what any path can carry — services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:132-135:
```
/// The longest key (a variable's key, a pool's name) this store keeps, in characters. The
/// emulator's own ceiling, for the same reason as the counts above: the stored size of one
/// entry is otherwise whatever a request carries.
pub const MAX_AIRFLOW_KEY_LENGTH: usize = 250;
```
The model caps the only way an entity can be addressed again — `smithy_query(query="shape", shape="RestApiPath")`:
```
"RestApiPath": {"shape_id": "com.amazonaws.mwaa#RestApiPath", "type": "string", "length": {"min": 1, "max": 64}}
```
and the handler reaches an entity only through that path — invoke_rest_api.rs:385-393:
```
        ["variables", key @ ..] => {
            let key = entity_key(key);
            match call.method {
                GET => read_variable(&environment.airflow, &key),
                PATCH => change_variable(&mut environment.airflow, &key, call),
                DELETE => delete_variable(&mut environment.airflow, &key),
```
The stored entity is then unfreeable against the budget — state.rs:126 `pub const MAX_AIRFLOW_VARIABLES: usize = 500;` and state.rs:130 `pub const MAX_AIRFLOW_POOLS: usize = 500;`, enforced in invoke_rest_api.rs:546-551 and :613-618.

Test coverage: the addressing suite deliberately stays inside the path bound and so never exercises this case — services/service-com-amazonaws-mwaa-amazonmwaa/tests/all/airflow_rest_api_entity_keys.rs:179-181:
```
/// Every key here keeps `/variables/{key}` and `/pools/{name}` inside the modelled
/// `RestApiPath` bound (`@length(1, 64)`), which is what a path can carry at all.
fn addressable_keys() -> Vec<(String, String)> {
```
and `unaddressable_keys()` (airflow_rest_api_entity_keys.rs:206-215) lists only separator shapes (`"/leading"`, `"trailing/"`, `"two//inside"`, …) — no over-long key. The length check that IS pinned is the 250-character one only: tests/all/airflow_rest_api.rs uses `json!({"key": "k".repeat(state::MAX_AIRFLOW_KEY_LENGTH + 1), "value": "1"})`.

## Steps to Reproduce

1. Create an environment and let it settle into AVAILABLE: PUT /environments/MyMWAAEnvironment with a valid body, then GET /environments/MyMWAAEnvironment until Status is AVAILABLE.
2. POST /restapi/MyMWAAEnvironment with {"Path": "/variables", "Method": "POST", "Body": {"key": "<200 'k' characters>", "value": "x"}} — observe HTTP 200 with RestApiStatusCode 200 and the stored variable echoed back, i.e. it was inserted into AirflowMetadata.variables (Path here is only "/variables", 10 characters, so the RestApiPath bound is satisfied).
3. POST /restapi/MyMWAAEnvironment with {"Path": "/variables", "Method": "GET"} — the variable is listed and total_entries is 1, confirming it is stored.
4. POST /restapi/MyMWAAEnvironment with {"Path": "/variables/<the same 200 'k' characters>", "Method": "GET"} — expect the stored variable; observe HTTP 400 ValidationException from the boundary validator ("1 validation error detected: Value ... at 'Path' failed to satisfy constraint: Member must have length less than or equal to 64"), because the only path that names the entity is 211 characters long.
5. Repeat step 4 with Method "DELETE" and "PATCH" — the same 400: the variable can never be removed, yet it keeps occupying one of the MAX_AIRFLOW_VARIABLES slots.
6. Do the same for a pool: {"Path": "/pools", "Method": "POST", "Body": {"name": "<200 'p' characters>", "slots": 4}} answers 200, {"Path": "/pools", "Method": "GET"} lists it, {"Path": "/pools/<same name>", "Method": "DELETE"} is refused 400 on the path bound, and re-POSTing the same name now answers 409 "the pool ... already exists" — the entry is unreachable, unreplaceable and unfreeable.
7. Repeat the whole flow with a key of exactly 53 characters (variable) and 54 characters to see the boundary: 53 is addressable through /variables/{key}, 54 is not, while both are accepted by the write.
