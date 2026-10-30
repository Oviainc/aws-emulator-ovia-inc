Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 7027025c5fd5dfe34e91978241957b132a94afec

# SearchSchemas fingerprints the raw Keywords string, so a NextToken is refused for a request that selects exactly the same schemas

_Filed: 2026-10-28T19:07:12Z_
_Operation: SearchSchemas | Class: pagination_

> _Note: this issue was identified against the code base at commit `7027025c5fd5dfe34e91978241957b132a94afec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing in this crate binds a "selection fingerprint" into the `NextToken` it mints, and `crate::state::selection_fingerprint` states the rule the fingerprint must satisfy: an absent member and a value that selects the SAME items must render ALIKE, "because they ARE the same listing". `ListRegistries` implements exactly that — it resolves `Scope` to one of three canonical values (`"all"`/`"Local"`/`"AWS"`) and fingerprints the CANONICAL value, so the case-insensitive spellings the handler accepts (and that `tests/all/registry_schema_lifecycle.rs` pins: `Local`, `local`, `LOCAL`, `AWS`, `aws`) all share one token.

`SearchSchemas` breaks that rule. Its one selecting member, `Keywords`, is normalised by `keywords_of` before it decides anything — the value is split on commas and whitespace, empty pieces are dropped, and every keyword is lower-cased — but the fingerprint is rendered from `input.keywords.as_str()`, the RAW query-string value. So values that normalise to the identical keyword list, and therefore select the identical schemas in the identical collation order, render different fingerprints:

* `?keywords=orders` and `?keywords=ORDERS` -> both `["orders"]`
* `?keywords=orders` and `?keywords=orders,` and `?keywords=%20orders` -> all `["orders"]`
* `?keywords=order,placed` and `?keywords=placed%20order` -> both select the schemas declaring BOTH keywords (`matches_every_keyword` folds with `all`, so keyword order is irrelevant)

In each pair a `NextToken` minted by the first request is refused by the second with 400 `BadRequestException` and the `invalid_next_token()` sentence ("The NextToken provided is not a token this service issued for this listing"), although the second request walks the very same listing the token names a position in. The page the caller is entitled to is unreachable: the service refuses to resume a listing it is in fact serving.

The fix is the one `ListRegistries` already applies — fingerprint the selection as the handler resolved it (the normalised keyword list, in the order `keywords_of` produced it), not the bytes the caller happened to type. `ListSchemas`/`ListSchemaVersions` need no change: a registry/schema name and a `SchemaNamePrefix` are used raw by the selection itself, so the raw value IS the canonical one there.

## Evidence

The rule, stated by the shared helper — services/service-com-amazonaws-schemas-schemas/src/state.rs:3236-3240:
```
/// Render the fingerprint of a listing's selection: the listing itself, and the
/// request members that decide WHICH items it contains, each with the value this
/// request sent (an absent member and the value that selects the same items render
/// alike, because they ARE the same listing).
```

The sibling that follows it — services/service-com-amazonaws-schemas-schemas/src/ops/list_registries.rs:184-200 (the fingerprint is rendered from the RESOLVED scope, not from `input.scope`):
```
        let selection = crate::state::selection_fingerprint(
            Self::NAME,
            &[
                (
                    "registryNamePrefix",
                    input.registry_name_prefix.as_deref().unwrap_or_default(),
                ),
                (
                    "scope",
                    match selected_scope {
                        None => "all",
                        Some(crate::state::RegistryScope::Local) => "Local",
                        Some(crate::state::RegistryScope::Aws) => "AWS",
                    },
                ),
            ],
        );
```
whose accepted spellings are case-insensitive — list_registries.rs:156-170:
```
            Some(scope) if scope.eq_ignore_ascii_case("local") => {
                Some(crate::state::RegistryScope::Local)
            }
            Some(scope) if scope.eq_ignore_ascii_case("aws") => {
                Some(crate::state::RegistryScope::Aws)
            }
```

The handler that breaks it — services/service-com-amazonaws-schemas-schemas/src/ops/search_schemas.rs:235-241:
```
        let selection = crate::state::selection_fingerprint(
            Self::NAME,
            &[
                ("registryName", input.registry_name.as_str()),
                ("keywords", input.keywords.as_str()),
            ],
        );
```
while the value that actually decides the selection is the NORMALISED list — search_schemas.rs:197 (`let keywords = keywords_of(&input.keywords);`), search_schemas.rs:311-316:
```
fn keywords_of(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|keyword| !keyword.is_empty())
        .map(str::to_lowercase)
        .collect()
}
```
and keyword ORDER is irrelevant to the selection — search_schemas.rs:341-343:
```
    keywords
        .iter()
        .all(|keyword| indexed.iter().any(|value| value.contains(keyword.as_str())))
```

What the mismatch costs — the token is refused outright, state.rs:3360-3362 inside `page_by_key`:
```
    let after = match token {
        None => None,
        Some(token) => Some(decode_next_token(selection, token, now).ok_or(InvalidToken)?),
    };
```
mapped by search_schemas.rs:265-269 to `crate::state::invalid_next_token()` (400 `BadRequestException`), whose sentence claims the token was not issued for this listing (state.rs:4016-4024).

Test coverage: `tests/all/registry_schema_lifecycle.rs:978-985` pins that `Local`/`local`/`LOCAL` and `AWS`/`aws` are all accepted and select the same registries, and :1337-1398 pins cross-selection token refusal for `ListRegistries`. `tests/all/search_schemas.rs:440-474` pins cross-selection refusal for `SearchSchemas` only with genuinely different keyword sets (`"orders"` -> `"placed"` and `"orders"` -> `""`); no case presents a token under a differently-CASED or differently-SEPARATED spelling of the same keywords, so this behaviour is unpinned.

## Steps to Reproduce

1. POST /v1/registries/name/demo -> 201 (CreateRegistry)
2. Create at least two schemas in `demo` whose documents both declare the event keyword `orders` (an `x-amazon-events-source` of `my.orders`), so a search for it has more than one page
3. GET /v1/registries/name/demo/schemas/search?keywords=orders&limit=1 -> 200 with a `NextToken` T (search_schemas.rs:235-241 fingerprints `keywords=6:orders;`)
4. GET /v1/registries/name/demo/schemas/search?keywords=ORDERS&limit=1&nextToken=<T> -> 400 BadRequestException "The NextToken provided is not a token this service issued for this listing..." — although `keywords_of` lower-cases both to `["orders"]` (search_schemas.rs:311-316), so the two requests select the same schemas in the same order; the fingerprint is `keywords=6:ORDERS;`
5. Same refusal for `?keywords=orders,` and for `?keywords=%20orders`, and for `?keywords=placed%20order` against a token from `?keywords=order,placed` (keyword order does not affect the selection, search_schemas.rs:341-343)
6. Contrast ListRegistries, which canonicalises before fingerprinting: GET /v1/registries?scope=local&limit=1 -> 200 with token T2, then GET /v1/registries?scope=LOCAL&limit=1&nextToken=<T2> -> 200 and resumes (list_registries.rs:184-200)
