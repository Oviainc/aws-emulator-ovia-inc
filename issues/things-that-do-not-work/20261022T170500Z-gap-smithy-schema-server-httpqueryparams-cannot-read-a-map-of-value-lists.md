Issue-Severity: MEDIUM
Issue-Gap-Class: framework

# `vendor/smithy-schema-server`'s `@httpQueryParams` binding reads a map of STRINGS only, so a map of value LISTS can never be deserialized and every request carrying a query parameter is refused

_Filed: 2026-10-22T17:05:00Z_
_Operation: vendor/smithy-schema-server (HTTP binding, `@httpQueryParams`) | Class: framework_

## Description

Smithy's `@httpQueryParams` binds a map member to the request's query string. The map's
VALUE may be a list, which is how a service models a repeated query key — aps's
`ListScrapersRequest$filters` is `map<FilterKey, FilterValues>` where `FilterValues` is a
`list<String>`, and its own documentation states the semantics that needs:
"Filters on the same key are `OR`'d together ... For example,
`status=ACTIVE&status=CREATING&alias=Test`".

`vendor/smithy-schema-server/src/http_binding.rs` cannot read that shape:

- `HttpBindingDeserializer::read_struct` collects the query into `Vec<(String, String)>`
  — one entry per (key, value) pair — and hands it to `MapValueDeserializer`;
- `MapValueDeserializer::read_map` wraps each entry's value in a
  `StringValueDeserializer`, whose `read_list` answers
  `Err("cannot read a list from a single HTTP-bound string value")`.

So a `@httpQueryParams` member whose value is a list fails to deserialize as soon as the
query string carries ANY parameter, and the request is refused 400 before the handler
runs. With no query parameters the member is skipped, so the defect is invisible on an
empty query — which is what makes it look like a handler bug.

The Smithy spec also says a `@httpQueryParams` map excludes the parameters the same
structure binds with `@httpQuery`; the binding above collects every parameter, so
`nextToken`/`maxResults` land in the map too and count against its `@length`.

## Observable effect

aps, `ListScrapers` (`GET /scrapers`):

- recorded AWS: `200` with `scrapers: []` for any combination of the (all optional)
  `filters`, `nextToken` and `maxResults` inputs;
- emulator through the framework's binding: `400 ValidationException` with
  `message: "cannot read a list from a single HTTP-bound string value"` for every request
  that carries a query parameter.

## What the core library needs

`http_binding.rs` should bind a `@httpQueryParams` member from the MULTI-valued query map
it already builds (`query_multi`): hand `MapValueDeserializer` each key's values and let
it drive a `ListValueDeserializer` when the map's value shape is a list (and a
`StringValueDeserializer` when it is a string, as today), and leave out the parameters the
same structure binds with `@httpQuery`.

## Workaround in the tree

`services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/list_scrapers.rs`
binds its own input from the query string (`input_from_query`, parsed with the
`form_urlencoded` crate) and `src/service.rs` serves that operation through
`ListScrapersFromQuery`, which drives the same bind -> validate -> handle -> serialize
sequence the centralized walk does. Both carry a pointer to this issue. The workaround is
to be DELETED, not copied, once the binding above reads a list-valued map.
