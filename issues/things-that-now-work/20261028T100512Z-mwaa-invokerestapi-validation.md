Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# A QueryParameters document that is not a JSON object is silently dropped instead of refused

_Filed: 2026-10-28T10:05:12Z_
_Operation: InvokeRestApi | Class: validation_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InvokeRestApi` reads the `limit` / `offset` paging parameters out of the `QueryParameters` document through `member()`, which answers `None` for ANY document that is not an object. A call whose `QueryParameters` is a JSON array, string or number therefore has its whole query document silently discarded: `page_window` falls back to the default window (`PAGE_LIMIT` entries from offset 0) and the collection call is answered with HTTP 200 as if the caller had asked for no paging at all.

The handler already refuses every neighbouring malformation of the same request: the sibling `Body` document is refused by `object_body()` with the Apache Airflow problem document `"the request body must be a JSON object"` (400), and a `limit` / `offset` MEMBER that is not a whole number is refused by `whole_parameter()` with `"{name} must be a whole number"` (400, pinned by a test row `("an offset that is not a number", json!({"offset": {}}))`). Only the query DOCUMENT itself escapes the check, so a malformed request is turned into a success that silently ignores the paging the caller asked for.

## Evidence

Smithy model — the member is documented as an object, exactly like `Body`:
- `smithy_query(query="doc", shape="InvokeRestApiRequest", member="QueryParameters")`: "Query parameters to be included in the Apache Airflow REST API call, provided as a JSON object."
- `smithy_query(query="doc", shape="InvokeRestApiRequest", member="Body")`: "The request body for the Apache Airflow REST API call, provided as a JSON object."

The crate refuses a non-object `Body` — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:833-841:
```rust
/// A write's body as the JSON object every one of these endpoints takes.
fn object_body(body: Option<&Document>) -> Result<&Document, AirflowAnswer> {
    match body {
        Some(body @ Document::Object(_)) => Ok(body),
        _ => Err(AirflowAnswer::bad_request(
            "the request body must be a JSON object".to_string(),
        )),
    }
}
```

…and refuses a malformed parameter INSIDE the query object — invoke_rest_api.rs:874-889:
```rust
fn whole_parameter(query: Option<&Document>, name: &str) -> Result<Option<i64>, AirflowAnswer> {
    let Some(value) = query.and_then(|query| member(query, name)) else {
        return Ok(None);
    };
    ...
    whole(value).map(Some).ok_or_else(|| {
        AirflowAnswer::bad_request(format!("{name} must be a whole number"))
    })
}
```

…but the lookup it goes through answers `None` for a non-object document, so the malformed query document reads as "no parameters at all" — invoke_rest_api.rs:891-897:
```rust
/// One member of a document that is an object.
fn member<'d>(document: &'d Document, name: &str) -> Option<&'d Document> {
    match document {
        Document::Object(members) => members.get(name),
        _ => None,
    }
}
```
and `page_window` then takes the default window — invoke_rest_api.rs:748-767 (`.unwrap_or(PAGE_LIMIT)`, `offset … .unwrap_or_default()`).

Test coverage: services/service-com-amazonaws-mwaa-amazonmwaa/tests/all/airflow_rest_api.rs:846-867 pins the refusal of malformed MEMBERS (`("a limit that is not a number", json!({"limit": "two"}))`, `("an offset that is not a number", json!({"offset": {}}))` → 400 / RestApiStatusCode 400), and :757-808 pins every accepted window; no row carries a `QueryParameters` document that is not an object, so the gap is untested.

## Steps to Reproduce

1. Create an environment: PUT /environments/Paged with a valid body (the harness in tests/all/airflow_rest_api.rs does this).
2. Seed a few Apache Airflow variables: POST /restapi/Paged with {"Method":"POST","Path":"/variables","Body":{"key":"key0","value":"0"}} (repeat for key1..key9).
3. Page the collection with a malformed query DOCUMENT: POST /restapi/Paged with {"Method":"GET","Path":"/variables","QueryParameters":[{"limit":3}]} (or "QueryParameters":"limit=3").
4. Observe HTTP 200 with RestApiStatusCode 200 and all ten variables returned — the query document was dropped without a word.
5. Compare with the same call carrying a malformed Body ({"Method":"POST","Path":"/variables","Body":[1,2]}), which is refused with RestApiClientException / RestApiStatusCode 400 "the request body must be a JSON object", and with {"QueryParameters":{"offset":{}}}, which is refused with "offset must be a whole number".
