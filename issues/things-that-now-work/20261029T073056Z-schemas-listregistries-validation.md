Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ff66eaaa221f5d1d5d406f24a736c9617410f815

# An empty scope query value is refused 400 "Scope must be either Local or AWS" instead of listing both scopes, unlike every other empty selecting value in the same handler

_Filed: 2026-10-29T07:30:56Z_
_Operation: ListRegistries | Class: validation_

> _Note: this issue was identified against the code base at commit `ff66eaaa221f5d1d5d406f24a736c9617410f815`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRegistries` resolves `Scope` with a `match` whose final arm refuses any value that is neither "local" nor "aws" (case-insensitively) with 400 `BadRequestException`. A request that carries the query parameter with an EMPTY value (`?scope=`) deserializes to `Some("")`, not `None`, so it falls into that arm and the listing is refused instead of reporting every registry — which is what the same request without the parameter returns.

The sibling selecting member of the very same handler is treated the opposite way: an empty `registryNamePrefix` is a prefix every name starts with, so it narrows nothing, and this crate states that rule explicitly for a selecting member sent empty in `SearchSchemas`'s `keywords_of` doc ("A value that carries no keyword at all (empty, or separators only) therefore limits nothing, which is all 'specifying this limits the results' can mean for a value that specifies none"). `Scope`'s documentation has the same limiting form — "Can be set to Local or AWS to limit responses to your custom registries, or the ones provided by AWS" — and the handler's own comment restates it ("so it LIMITS a listing that otherwise reports both"), so a value that sets no scope limits nothing and both scopes should be reported.

Note this is not the already-filed empty-`NextToken` defect (20261029T065508Z-schemas-listregistries-pagination), which is about `crate::state::page_by_key` and the `nextToken` member; this is the `scope` member and the handler's own match, and the fix is in a different place.

## Evidence

Model (smithy_query query="shape" shape="ListRegistriesRequest"):
  "Scope": {"target": "__string", "httpQuery": "scope", "doc": "<p>Can be set to Local or AWS to limit responses to your custom registries, or the ones provided by AWS.</p>"}

Handler — services/service-com-amazonaws-schemas-schemas/src/ops/list_registries.rs:152-170:
        // `Scope` is documented as "Local or AWS to limit responses to your custom
        // registries, or the ones provided by AWS" — so it LIMITS a listing that
        // otherwise reports both: a caller's own registries and the service-owned
        // ones. Any other value is not a scope.
        let selected_scope = match input.scope.as_deref() {
            None => None,
            Some(scope) if scope.eq_ignore_ascii_case("local") => {
                Some(crate::state::RegistryScope::Local)
            }
            Some(scope) if scope.eq_ignore_ascii_case("aws") => {
                Some(crate::state::RegistryScope::Aws)
            }
            Some(_) => {
                return Err(ListRegistriesErrorModeled::BadRequestException(
                    crate::state::bad_request("Scope must be either Local or AWS."),
                )
                .into())
            }
        };
`Some("")` reaches the `Some(_)` arm.

The sibling member of the SAME handler, two dozen lines later — list_registries.rs:214-220:
        let matching = state.registries.iter().filter(|(name, registry)| {
            selected_scope.is_none_or(|scope| registry.scope == scope)
                && input
                    .registry_name_prefix
                    .as_deref()
                    .is_none_or(|prefix| name.starts_with(prefix))
        });
(an empty prefix matches every name).

The crate's stated rule for a selecting member sent empty — services/service-com-amazonaws-schemas-schemas/src/ops/search_schemas.rs:303-310:
/// `Keywords` is a single `__string`, but the member is plural — "limits the results
/// to only schemas that include the provided keywords" — so one value carries a LIST
/// of keywords, separated by commas or whitespace; empty pieces are dropped. A value
/// that carries no keyword at all (empty, or separators only) therefore limits
/// nothing, which is all "specifying this limits the results" can mean for a value
/// that specifies none.

A present-but-empty query parameter arrives as `Some("")` — vendor/smithy-schema-server/src/http_binding.rs:104-110 (`let v = percent_decode_query(it.next().unwrap_or("")); query.entry(k.clone()).or_insert_with(|| v.clone());`) and :277-292 (`if values.is_empty() { continue; } ... StringValueDeserializer::new(v)`).

Test coverage: tests/all/registry_schema_lifecycle.rs:960-1021 drives `Scope` over the rows (Some("Local"), Some("local"), Some("LOCAL"), Some("AWS"), Some("aws"), None, Some("Everything")) — the comment at :978-979 says "the two documented values in both casings, absent, and a value that is neither". The EMPTY value is not a row, so nothing pins the current 400.

## Steps to Reproduce

1. Start the emulator. POST /v1/registries/name/mine -> 201 (CreateRegistry).
2. GET /v1/registries -> 200 listing both `mine` and the service-owned `aws.events` registry.
3. GET /v1/registries?scope= (the parameter present with an empty value) -> 400 BadRequestException with Message "Scope must be either Local or AWS."
4. Expected: a value that sets no scope limits nothing, so the listing reports both scopes exactly as the request without the parameter does — the rule this crate states for an empty selecting value (search_schemas.rs:303-310) and applies to `registryNamePrefix` in the same handler (list_registries.rs:216-219).
