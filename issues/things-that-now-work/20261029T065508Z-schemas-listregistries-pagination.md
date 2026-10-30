Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# An empty NextToken is refused with 400 instead of serving the first page, which the member's own documentation says it must

_Filed: 2026-10-29T06:55:08Z_
_Operation: ListRegistries | Class: pagination_

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing of this service (`ListRegistries`, `ListSchemas`, `ListSchemaVersions`, `ListDiscoverers`, `SearchSchemas`) passes `input.next_token.as_deref()` straight into `crate::state::page_by_key`, and maps any decode failure to `BadRequestException`. A request that carries the query parameter with an EMPTY value (`?nextToken=`) deserializes to `Some("")`, not `None`, so `decode_next_token` rejects it (its `split('.')` yields a single field) and the listing answers HTTP 400 `BadRequestException` — "The NextToken provided is not a token this service issued for this listing." — instead of serving the first page.

The model's documentation for the member states the opposite: an empty `NextToken` is how the first page is requested. The same sentence is carried by the `NextToken` member of every paginated request shape of this service, so all five listings share the defect; the handler quoted below is `ListRegistries`, and the refusal itself lives in the one shared helper (`crate::state::page_by_key` / `decode_next_token`).

The crate already treats an empty query-string VALUE as "this member narrows nothing" for the sibling selecting members of the same listings — an empty `registryNamePrefix` matches every registry, and `SearchSchemas` documents an empty `Keywords` as limiting nothing — so `NextToken` is the one member of the family for which an empty value is an error rather than "no value".

## Evidence

Model (smithy_query query="shape" shape="ListRegistriesRequest"; identical text on ListSchemasRequest$NextToken, ListDiscoverersRequest$NextToken, SearchSchemasRequest$NextToken):
  "NextToken": {"target": "__string", "httpQuery": "nextToken", "doc": "<p>The token that specifies the next page of results to return. To request the first page, leave NextToken empty. The token will expire in 24 hours, and cannot be shared with other accounts.</p>"}

Handler — services/service-com-amazonaws-schemas-schemas/src/ops/list_registries.rs:221-232:
        let page = crate::state::page_by_key(
            matching,
            &selection,
            input.next_token.as_deref(),
            page_size,
            now,
        )
        .map_err(|_| {
            ListRegistriesError::from(ListRegistriesErrorModeled::BadRequestException(
                crate::state::invalid_next_token(),
            ))
        })?;

Shared helper — services/service-com-amazonaws-schemas-schemas/src/state.rs:3392-3395:
    let after = match token {
        None => None,
        Some(token) => Some(decode_next_token(selection, token, now).ok_or(InvalidToken)?),
    };
and state.rs:3340-3345, which refuses "" because it is not three dot-separated fields:
    let mut fields = token.split('.');
    let (encoded_selection, encoded_key, encoded_issued_at) =
        match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some(selection), Some(key), Some(issued_at), None) => (selection, key, issued_at),
            _ => return None,
        };

A present-but-empty query parameter arrives as `Some("")` — vendor/smithy-schema-server/src/http_binding.rs:104-110 (`let v = percent_decode_query(it.next().unwrap_or(""));  query.entry(k.clone()).or_insert_with(|| v.clone());`) and :277-292 (`if values.is_empty() { continue; } ... StringValueDeserializer::new(v)`) — so the member is Some(""), never None.

Same-crate inconsistency: the crate treats an empty query VALUE as "narrows nothing" for the sibling members of these very listings — list_registries.rs:216-219 `input.registry_name_prefix.as_deref().is_none_or(|prefix| name.starts_with(prefix))`, and search_schemas.rs:308-310 "A value that carries no keyword at all (empty, or separators only) therefore limits nothing".

Test coverage: tests/all/next_token_lifetime.rs and tests/all/list_discoverers.rs:596 pin refusals for `nextToken=not-hex` and `nextToken=zz`; no test covers the empty value, so nothing pins the current 400.

## Steps to Reproduce

1. Start the emulator and create two registries so a listing has more than one page: POST /v1/registries/name/reg-one and POST /v1/registries/name/reg-two.
2. Call GET /v1/registries?nextToken= (the parameter present with an empty value).
3. Observe HTTP 400 BadRequestException with Message "The NextToken provided is not a token this service issued for this listing. ...".
4. Expected per ListRegistriesRequest$NextToken ("To request the first page, leave NextToken empty"): the first page of registries, as the same request without the parameter returns.
5. The same request shape against GET /v1/discoverers?nextToken=, GET /v1/registries/name/{reg}/schemas?nextToken=, .../versions?nextToken= and .../schemas/search?keywords=x&nextToken= is refused identically, through the one crate::state::page_by_key helper.
