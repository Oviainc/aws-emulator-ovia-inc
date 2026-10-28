Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# A nextToken issued by a sibling list operation is honoured as a resume point and silently answers an empty page

_Filed: 2026-10-28T13:37:43Z_
_Operation: ListConnectors | Class: pagination_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four paginated operations of this crate (ListConnectors, ListCustomPlugins, ListWorkerConfigurations, ListConnectorOperations) share ONE token codec in `crate::state` whose opaque token carries a single service-wide marker and NO discriminator for the listing that issued it. `decode_page_token` therefore accepts a token minted by any of the four, and `page` then uses that foreign key as a resume point against a completely unrelated key space (`page` compares raw strings: `if resume_after.as_deref().is_some_and(|from| key.as_str() < from) { continue; }`, src/state.rs:2279).

Because every ARN this service mints is `arn:aws:kafkaconnect:<region>:000000000000:<kind>/<name>/<id>` and the kinds sort `connector/` < `custom-plugin/` < `worker-configuration/`, feeding a `ListCustomPlugins` (or `ListWorkerConfigurations`) token to `ListConnectors` makes EVERY stored connector compare strictly less than the resume key, so all of them are skipped and the call answers HTTP 200 with `"connectors": []` and no `nextToken` — a silent, truncated success that hides all state instead of the `BadRequestException` / `Invalid parameter nextToken.` this crate answers for a token that is not a resume point for the listing. The same happens for `ListConnectorOperations`, whose keys are `{nanos:039}/{arn}` and so begin with a digit: any ARN-keyed token (digits sort below `a`) hides every operation of the connector. In the other direction (a `ListConnectors` token handed to `ListCustomPlugins`/`ListWorkerConfigurations`) nothing is skipped at all, so the caller silently gets page one again and re-reads items it has already consumed.

This contradicts the crate's own stated rule for the marker and the rule its tests spell out for an unissued token ("not an empty page and not a silently honoured resume point"): a token that is not a resume point for THIS listing is silently honoured as one. The refusal message, status and code for genuinely malformed tokens need not change.

## Evidence

The rule the marker exists to enforce — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs:26-28:
```
/// The marker every `nextToken` this service issues carries, so a token it did
/// not issue is refused rather than honoured as a resume point.
const PAGE_TOKEN_MARKER: &str = "kafkaconnect:v1:";
```
One codec for all four listings, with no per-listing discriminator — src/state.rs:2298-2311:
```
fn encode_page_token(key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_MARKER}{key}"))
}
...
fn decode_page_token(token: &str) -> Result<String, BadPageToken> {
    let bytes = hex::decode(token).map_err(|_| BadPageToken)?;
    let decoded = String::from_utf8(bytes).map_err(|_| BadPageToken)?;
    decoded
        .strip_prefix(PAGE_TOKEN_MARKER)
        .map(ToString::to_string)
        .ok_or(BadPageToken)
}
```
The resume comparison that drops every item of an unrelated key space — src/state.rs:2274-2287:
```
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
...
    for (key, item) in keyed {
        if resume_after.as_deref().is_some_and(|from| key.as_str() < from) {
            continue;
        }
```
The four handlers that all hand their own `next_token` to that one helper, each keyed by a different ARN kind or key form:
- src/ops/list_connectors.rs:182-196 — `.map(|connector| (connector.arn.clone(), connector))` then `crate::state::page(keyed, input.max_results, input.next_token.as_deref())`
- src/ops/list_custom_plugins.rs:178-192 — `(plugin.arn.clone(), plugin)` then the same `page(..)`
- src/ops/list_worker_configurations.rs:183-199 — `(worker.arn.clone(), worker)` then the same `page(..)`
- src/ops/list_connector_operations.rs:206-218 — `let keyed = state.connector_operations_of(&input.connector_arn);` whose key is `format!("{nanos:039}/{}", self.arn)` (src/state.rs:1149-1157)

The ARN shape that makes the key spaces sort apart — src/ops/create_connector.rs:413-417 (`format!("connector/{}/{}", ..)`), src/ops/create_custom_plugin.rs:292-296 (`format!("custom-plugin/{}/{}", ..)`), src/ops/create_worker_configuration.rs:258-262 (`format!("worker-configuration/{}/{}", ..)`), all through `::engine::emulator_arn`, i.e. `arn:aws:{service}:{region}:000000000000:{resource}` (vendor/server/src/lib.rs:162-164).

The rule this crate's own tests state, which the foreign-token path breaks — tests/all/list_worker_configurations.rs:417-420:
```
    // A token this service never issued is a bad request, not an empty page and
    // not a silently honoured resume point. Both forms are refused: one that is
    // not even the token's encoding, and one that is — hex that decodes to a
    // plausible key — but carries none of this service's marker.
```
and tests/all/connector_operations.rs:622-624 (same sentence). Test coverage: the existing `forged` rows only cover tokens WITHOUT the marker (`"not-a-token"`, `"6161"`, bare hex of a plausible key — list_worker_configurations.rs:421-427, connector_operations.rs:625-629) and the happy-path paging rows re-feed each listing its OWN token (custom_plugin_and_connector_lifecycle.rs:985, list_worker_configurations.rs:317, connector_operations.rs:532), so nothing pins what a sibling listing's token does.

## Steps to Reproduce

1. Boot the emulator with the kafkaconnect service mounted and, signing for us-east-1, create at least one connector (POST /v1/connectors) and at least two custom plugins (POST /v1/custom-plugins).
2. GET /v1/custom-plugins?maxResults=1 and copy the returned `nextToken` T (it is hex of "kafkaconnect:v1:arn:aws:kafkaconnect:us-east-1:000000000000:custom-plugin/<name>/<id>").
3. GET /v1/connectors?nextToken=T — observe HTTP 200 with `"connectors": []` and no `nextToken`: every connector key (`...:000000000000:connector/...`) sorts before the custom-plugin resume key, so src/state.rs:2279 skips all of them. Expected instead: the 400 `BadRequestException` / `Invalid parameter nextToken.` this crate answers for a token that is not a resume point for this listing (tests/all/list_worker_configurations.rs:417-420).
4. Reverse it: GET /v1/connectors?maxResults=1, then GET /v1/custom-plugins?nextToken=<that token> — observe HTTP 200 returning page one again (nothing is skipped), so a client following tokens re-reads items it already consumed.
5. Restart or update a connector so it has operations, then GET /v1/connectors/<connectorArn>/operations?nextToken=<any ListConnectors token> — observe HTTP 200 with an empty `connectorOperations` list, because operation keys begin with a digit (`{nanos:039}/<arn>`) and therefore all sort below the ARN resume key.
