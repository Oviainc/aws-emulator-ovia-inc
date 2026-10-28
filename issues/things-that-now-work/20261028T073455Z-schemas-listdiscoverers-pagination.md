Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: b056d947c24faab0377cbe866250e0547cccc526

# NextToken never expires, and ListDiscoverers' selection fingerprint is ambiguous so a token issued for one filter resumes another

_Filed: 2026-10-28T07:34:55Z_
_Operation: ListDiscoverers | Class: pagination_

> _Note: this issue was identified against the code base at commit `b056d947c24faab0377cbe866250e0547cccc526`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two defects in this service's shared `NextToken` machinery, both visible at ListDiscoverers (the first paginated operation on the checklist that calls `crate::state::page_by_key`).

1. The token carries NO issue instant at all, so it never expires. `encode_next_token` mints `hex(selection) + "." + hex(last_key)` and `decode_next_token` checks only that the selection half matches; a token minted at any moment in the past is accepted forever. The modelled documentation for every `NextToken` member of this service (reproduced verbatim in this very handler's module doc) states that the token expires after a fixed window. Nothing in the crate — no comment, no constant, no test — mentions or justifies the absence of expiry; the state module's token section discusses only opacity and selection binding.

2. ListDiscoverers renders its selection fingerprint by concatenating TWO unconstrained free-form request members with an unescaped `;`/`=` separator, so two DIFFERENT selections can render the SAME fingerprint and `decode_next_token` then accepts a token minted for the other listing. Example: `?discovererIdPrefix=a;sourceArnPrefix=b` (SourceArnPrefix absent) and `?discovererIdPrefix=a&sourceArnPrefix=b;sourceArnPrefix=` both render `discovererIdPrefix=a;sourceArnPrefix=b;sourceArnPrefix=`. This is the exact failure the shared helper's doc says the selection binding exists to prevent, and the comment directly above the `format!` repeats it. ListDiscoverers is the only listing where this can happen: ListSchemas/ListSchemaVersions/SearchSchemas interpolate only names already forced through `validate_names` (which admits no `;`), and ListRegistries' second field is drawn from a closed three-value set.

## Evidence

Modelled doc, carried verbatim in the handler itself — services/service-com-amazonaws-schemas-schemas/src/ops/list_discoverers.rs:52-54:
```
//! * `NextToken` (__string) — The token that specifies the next page of results to return. To
//!   request the first page, leave NextToken empty. The token will expire in 24 hours, and
//!   cannot be shared with other accounts.
```
(repeated at list_discoverers.rs:61-63 for the output member, and in list_registries.rs:51/61, list_schemas.rs:51/60, list_schema_versions.rs:51/59, search_schemas.rs:77/84).

The minted token holds nothing but the selection and the resume key — services/service-com-amazonaws-schemas-schemas/src/state.rs:3247-3249:
```
fn encode_next_token(selection: &str, last_key: &str) -> String {
    format!("{}.{}", ::hex::encode(selection), ::hex::encode(last_key))
}
```
and the only check on a returned token — state.rs:3254-3260:
```
fn decode_next_token(selection: &str, token: &str) -> Option<String> {
    let (encoded_selection, encoded_key) = token.split_once('.')?;
    if ::hex::decode(encoded_selection).ok()? != selection.as_bytes() {
        return None;
    }
    String::from_utf8(::hex::decode(encoded_key).ok()?).ok()
}
```
No instant is stored and none is compared, so no token can ever be too old. Nothing in the crate mentions expiry: `grep -n '24 hours|expire|expiry'` over the crate matches ONLY the five handlers' copies of the modelled sentence quoted above.

For symptom 2 — the rule the shared helper promises, state.rs:3243-3246:
```
/// The selection is carried IN the token because a token names a position within one
/// listing: resuming a differently-filtered listing from it would skip or repeat
/// items with no error. A token presented with another selection is therefore not a
/// token for that listing, and is refused.
```
and the code that breaks it, list_discoverers.rs:159-165:
```
        // The members that decide WHICH discoverers this listing contains. Bound into
        // the `NextToken`, so a token cannot resume a differently-selected listing.
        let selection = format!(
            "discovererIdPrefix={};sourceArnPrefix={}",
            input.discoverer_id_prefix.as_deref().unwrap_or_default(),
            input.source_arn_prefix.as_deref().unwrap_or_default(),
        );
```
Both members are modelled as plain unconstrained `__string` query parameters (OPERATIONS.md:142: "`DiscovererIdPrefix` (string, @httpQuery(discovererIdPrefix))", "`SourceArnPrefix` (string, @httpQuery(sourceArnPrefix))"), and no handler check restricts their characters, so the separator can appear inside either half.

Contrast the sibling listings, whose first interpolated field cannot contain the separator: list_schemas.rs:195-199 interpolates `input.registry_name` after `crate::state::validate_names(Some(&input.registry_name), None)` at list_schemas.rs:172, and `REGISTRY_NAME.characters` admits only `. - _ A-Z a-z 0-9` (state.rs:2868-2900); list_registries.rs:181-189 renders its second field from the closed set `"all" | "Local" | "AWS"`.

Test coverage: tests/all/list_discoverers.rs exercises paging and both prefix filters, but no case presents a token minted for a different selection that renders the same fingerprint, and no case ages a token.

## Steps to Reproduce

1. Create two or more discoverers (CreateDiscoverer) so that a listing of them has more than one page.
2. Call `GET /v1/discoverers?limit=1` and keep the returned `NextToken`.
3. Observe that the token can be replayed indefinitely — there is no issue instant in `encode_next_token` (state.rs:3247) and no age check in `decode_next_token` (state.rs:3254), so the token the model documents as expiring never does.
4. For the second symptom: call `GET /v1/discoverers?discovererIdPrefix=a;sourceArnPrefix=b&limit=1` and `GET /v1/discoverers?discovererIdPrefix=a&sourceArnPrefix=b;sourceArnPrefix=&limit=1`.
5. Note both requests build the identical `selection` string `discovererIdPrefix=a;sourceArnPrefix=b;sourceArnPrefix=` at list_discoverers.rs:161-165, so a `NextToken` minted for the first listing is accepted by the second instead of being refused with `invalid_next_token()` as state.rs:3243-3246 states it must be.
