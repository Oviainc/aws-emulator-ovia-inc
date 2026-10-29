Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: fad1fdecc61c47880f0bf921597b8afa67a7a57d

# A non-object Body is silently dropped on every call that parses no entity, breaking the rule the crate states for both documents

_Filed: 2026-10-29T18:22:16Z_
_Operation: InvokeRestApi | Class: validation_

> _Note: this issue was identified against the code base at commit `fad1fdecc61c47880f0bf921597b8afa67a7a57d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InvokeRestApi` carries TWO document members, `QueryParameters` and `Body`, and the crate states ONE rule for both of them: a document of any shape other than a JSON object names nothing and is REFUSED rather than read as if the caller had supplied nothing.

The rule is enforced for `QueryParameters` unconditionally — `serve` runs `object_document(call.query, QUERY_PARAMETERS)` before the path is even dispatched, deliberately "for every method".

It is NOT enforced for `Body`. The only call site of `object_body` is inside `requested_variable` and `requested_pool`, which are reached from exactly four arms: `POST /variables`, `PATCH /variables/{key}`, `POST /pools`, `PATCH /pools/{name}`. Every other call simply never looks at `call.body`, so a `Body` that is an array, a string, a number or a boolean is silently dropped and the call is served:

* `GET /version` with `Body: 42` -> HTTP 200, `RestApiStatusCode` 200, the version document.
* `GET /variables` with `Body: "limit=1&offset=0"` -> HTTP 200, the default page.
* `GET /variables/{key}` with `Body: [1,2]` -> HTTP 200, the variable.
* `DELETE /variables/{key}` with `Body: true` -> `RestApiStatusCode` 204, the variable is removed.
* `DELETE /pools/{name}` with `Body: "oops"` -> `RestApiStatusCode` 204, the pool is removed.
* `PUT /variables` with `Body: 7` -> the arm's 405, the malformed body never judged.

Every one of those is reachable with input the model allows (`Body` targets `RestApiRequestBody`, a `document`, so any JSON value deserializes into it), and every one of them contradicts the sentence the module and `object_document` both write down. The asymmetry is a leftover: the companion gap for `QueryParameters` was closed by adding the unconditional check at the top of `serve`, and the `Body` half of the same rule was left behind — which is why the suite that pins the query rule describes `Body` as already being governed by it while sending no non-object `Body` on a read or a delete.

## Evidence

The model documents `Body` as an object — smithy_query(query="operation", operation="InvokeRestApi"): `"Body": {"target": "RestApiRequestBody", "sensitive": true, "type": "document", "document": true}`, whose member doc is quoted in the handler itself at services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:106-107:

    //! * `Body` (RestApiRequestBody) — The request body for the Apache Airflow REST API call,
    //!   provided as a JSON object.

The crate states ONE rule for BOTH documents — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:16-21:

    //! `Method`, `Path`, `QueryParameters` and `Body` are all acted on: ... Both documents
    //! are the JSON objects the model documents them as, so one of any other shape names no
    //! parameter and carries no entity, and is refused ([`object_document`]) rather than
    //! dropped.

and restates it on the helper — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:889-897:

    /// Each of this operation's two documents is one the model documents as a JSON object
    /// (`smithy_query(query="doc",shape="InvokeRestApiRequest",member="QueryParameters"|
    /// "Body")`), because an object is what names the values it carries: a document of any
    /// other shape — an array, a string, a number, a boolean — names none of them, so it is
    /// refused rather than read as if the caller had supplied nothing.

The query half IS enforced for every method — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:376-385:

    // The query parameters are ... refused here — before the path is
    // addressed, since the query string belongs to the request rather than to the endpoint,
    // and for every method, since every one of them is called with a URL — rather than read
    // as if the caller had asked for no parameters.
    let query = match object_document(call.query, QUERY_PARAMETERS) {
        Ok(query) => query,
        Err(refusal) => return refusal,
    };

The body half is reached only from the two entity parsers — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:909-913:

    /// A write's body as the JSON object every one of these endpoints takes, which — unlike
    /// the query parameters — a write has to carry: a body stating nothing states no entity.
    fn object_body(body: Option<&Document>) -> Result<&Document, AirflowAnswer> {
        object_document(body, REQUEST_BODY)?.ok_or_else(|| not_an_object(REQUEST_BODY))
    }

called from `requested_variable` (services/.../invoke_rest_api.rs:818-819: `let body = object_body(body)?;`) and `requested_pool` (services/.../invoke_rest_api.rs:841-842: `let body = object_body(body)?;`) and nowhere else, so the read arms (`airflow_version`, `list_variables`, `read_variable`, `list_pools`, `read_pool`, `list_dags`), the delete arms (`delete_variable`, `delete_pool`) and `method_not_allowed` / `path_not_found` / `dag_not_found` never judge `call.body`.

Test coverage: the suite that pins the query rule already describes `Body` as governed by the SAME refusal — services/service-com-amazonaws-mwaa-amazonmwaa/tests/all/airflow_rest_api.rs:870-873:

    /// A `QueryParameters` document that is not a JSON object names no parameter at all, so
    /// the call carrying it is refused — with the same Apache Airflow problem document its
    /// sibling `Body` is refused by — instead of being served as if the caller had asked for
    /// nothing.

but its rows (tests/all/airflow_rest_api.rs:904-946) vary only `QueryParameters`; the single `Body` they send is the valid object `{"key": "fresh", "value": "1"}` on a write. No existing assertion in `airflow_rest_api.rs`, `airflow_rest_api_entity_keys.rs`, `environment_teardown_write_gate.rs` or `env_endpoint_payload.rs` sends a non-object `Body` on a read, a delete or a method-not-allowed call, so nothing pins the current dropping behaviour.

Expected (from the crate's own rule): HTTP 400 `RestApiClientException`, `RestApiStatusCode` 400, problem document `detail` = "the request body must be a JSON object" — the same answer `POST /variables` with `Body: 42` already gives.
Actual: the malformed `Body` is dropped and the call is served (200 / 204) or refused by the arm's own unrelated rule (405 / 404).

## Steps to Reproduce

1. Create an environment: PUT /environments/BodyDocument with a valid CreateEnvironment body, and wait for it to settle into AVAILABLE.
2. POST /restapi/BodyDocument with {"Path": "/variables", "Method": "POST", "Body": 42} — observe HTTP 400 RestApiClientException, RestApiStatusCode 400, detail "the request body must be a JSON object". This is the rule the crate documents.
3. POST /restapi/BodyDocument with {"Path": "/version", "Method": "GET", "Body": 42} — observe HTTP 200 and RestApiStatusCode 200: the same non-object document on the same operation is dropped instead of refused.
4. POST /restapi/BodyDocument with {"Path": "/variables", "Method": "POST", "Body": {"key": "zz", "value": "v"}} to seed a variable, then POST /restapi/BodyDocument with {"Path": "/variables/zz", "Method": "DELETE", "Body": [1,2]} — observe RestApiStatusCode 204 and the variable gone: a delete carrying a body that names nothing still ran.
5. Read services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:16-21 and :889-897 for the stated rule, then :376-385 (query checked for every method) against :909-913 plus its only two call sites at :819 and :842 (body checked on four write arms only).
