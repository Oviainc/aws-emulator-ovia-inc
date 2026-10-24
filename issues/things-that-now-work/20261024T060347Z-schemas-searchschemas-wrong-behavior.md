Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: df1261813c70037074c7525ba4fddd96df97f3b7

# SearchSchemas matches only x-amazon-events-* extensions, so a caller's own schemas are unfindable by any keyword

_Filed: 2026-10-24T06:03:47Z_
_Operation: SearchSchemas | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `df1261813c70037074c7525ba4fddd96df97f3b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Keyword matching is computed exclusively from two EventBridge document extensions (`x-amazon-events-source`, `x-amazon-events-detail-type`) found anywhere in a stored version's content. A schema whose document declares neither is treated as indexed under nothing and is returned for NO keyword at all — the handler short-circuits to `false` before the keywords are even consulted.

Two observable consequences:
1. A registry of caller-created schemas (the ordinary CreateSchema case: a JSON Schema draft-4 document, or an OpenAPI document without the EventBridge extensions) answers `Schemas: []` for every possible `Keywords` value, including a keyword that is exactly the schema's name. Expected: `Keywords` LIMITS the registry's schemas to the ones that include the keyword (the schema name is the identifier a caller searches by), so at least a name match must select the schema.
2. A `Keywords` value that carries no keyword at all (empty, or only separators such as `,` / spaces) still returns an empty list, even though the handler's own stated rule is that such a value "limits nothing". Per the model's documentation ("Specifying this limits the results to only schemas that include the provided keywords") the unlimited base set is every schema in the named registry, so a value that constrains nothing must not filter everything away.

Actual: SearchSchemas is effectively a lookup over the seeded `aws.events` catalogue only; against a caller's registry it is a 200 with an always-empty `Schemas` list, which is indistinguishable from "the registry is empty" to a client that uses search to find a schema before DescribeSchema/PutCodeBinding.

## Evidence

Spec: `smithy_query(query="operation", operation="SearchSchemas")` — RegistryName (@httpLabel, required) + Keywords (@httpQuery, required); the operation is modeled for ANY registry name, and `smithy_query(query="doc", shape="SearchSchemasRequest", member="Keywords")` reads "Specifying this limits the results to only schemas that include the provided keywords" — a narrowing filter over the registry's schemas, with no statement that only documents carrying EventBridge extensions participate.
Handler: services/service-com-amazonaws-schemas-schemas/src/ops/search_schemas.rs:287-295 — `matches_every_keyword` returns `false` whenever `declared_event_keywords(schema)` is empty, before any keyword comparison; search_schemas.rs:310-345 collects candidate text ONLY from members named in `EVENT_KEYWORD_EXTENSIONS` (search_schemas.rs:349-352 = `x-amazon-events-source`, `x-amazon-events-detail-type`); the schema's `name` (and the rest of the document) is never consulted, and search_schemas.rs:216-219 filters the registry's schemas with that predicate.
Internal inconsistency that corroborates the second symptom: `keywords_of` (search_schemas.rs:264-277) documents "A value that carries no keyword at all therefore limits nothing", yet an empty keyword list still yields an empty result for every schema that declares no extension.
Test coverage: tests/all/search_schemas.rs:37-44 and :121 (`keywords_select_exactly_the_schemas_that_declare_them`) encode the current rule ("a name is not what a search matches", plus rows "no keyword can select"), so they pin the divergence and would change with the fix. The recorded check `schemas.search-schemas-keyword-filters-results` (issues/things-that-now-work/20261022T034503Z-schemas-searchschemas-stub.md) exercises keyword filtering but says nothing about a schema that declares no extension.

## Steps to Reproduce

1. POST /v1/registries/name/mine -> 201 (CreateRegistry)
2. POST /v1/registries/name/mine/schemas/name/order-placed with {"Type":"JSONSchemaDraft4","Content":"{\"type\":\"object\",\"properties\":{\"orderId\":{\"type\":\"string\"}}}"} -> 201 (CreateSchema)
3. GET /v1/registries/name/mine/schemas/search?keywords=order -> observe 200 with {"Schemas":[]}; the schema whose name contains the keyword is never returned
4. GET /v1/registries/name/mine/schemas/search?keywords=order-placed (the exact schema name) -> still {"Schemas":[]}
5. GET /v1/registries/name/mine/schemas/search?keywords=%20 (a value carrying no keyword, which by the handler's own rule limits nothing) -> still {"Schemas":[]} instead of the registry's schemas
6. GET /v1/registries/name/mine/schemas -> 200 listing order-placed, proving the schema is stored and only the search predicate excludes it
