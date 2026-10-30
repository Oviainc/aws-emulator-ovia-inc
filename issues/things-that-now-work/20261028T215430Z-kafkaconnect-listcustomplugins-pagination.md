Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# A nextToken minted by a namePrefix-narrowed listing is honoured by a wider listing and silently drops resources

_Filed: 2026-10-28T21:54:30Z_
_Operation: ListCustomPlugins | Class: pagination_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three regional list handlers (ListCustomPlugins, ListWorkerConfigurations, ListConnectors) narrow their key space with a caller-supplied name prefix BEFORE paging, but they label the page token with `Listing::regional(Self::NAME, &region)` — a scope that carries only the operation name and the region, never the prefix. So a `nextToken` minted while a prefix was in force decodes successfully for a later call of the same operation in the same region that passes a DIFFERENT prefix, or none at all.

The resume key inside the token is the ARN of the first item of the narrowed listing's next page. When it is replayed against the un-narrowed listing, `page` skips every resource whose ARN sorts before that key (`if resume_after.as_deref().is_some_and(|from| key.as_str() < from) { continue; }`), so resources the caller asked for are dropped from the answer with a 200 and no indication — exactly the "silently truncates a page" outcome the `Listing` design says it exists to prevent, and exactly what the sibling `ListConnectorOperations` avoids by folding its narrowing (the parent connector ARN) INTO the scope via `Listing::of_resource`.

The same defect is present verbatim in `list_connectors.rs` (connectorNamePrefix) and `list_worker_configurations.rs` (namePrefix); it is one rule, copied three times, so one fix covers all three.

## Evidence

The documented contract of the scope the handlers pass:

- `services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs:2265-2267`:
  "    /// The key space that operation paged: what distinguishes two listings of\n    /// the same operation from each other.\n    scope: &'a str,"
- `state.rs:2271-2273`:
  "    /// A listing of every resource of one kind in ONE region: two regions hold\n    /// independent resources, so each region's listing has its own key space.\n    pub fn regional(operation: &'a str, region: &'a str) -> Self {"
- `state.rs:2256-2260` states the harm the mechanism exists to prevent: "So a token carries the listing that minted it and [`page`] refuses one minted anywhere else, instead of comparing a key from a foreign key space against its own keys — which silently truncates a page (every key sorting past the foreign one is dropped) or silently replays one (none of them does)."

The handler narrows the key space and then labels it as the whole region anyway:

- `src/ops/list_custom_plugins.rs:171`: "        let prefix = input.name_prefix.unwrap_or_default();"
- `src/ops/list_custom_plugins.rs:178-182`: "        let keyed: Vec<(String, &crate::state::CustomPluginRecord)> = state\n            .custom_plugins_in(&region)\n            .filter(|plugin| plugin.name.starts_with(&prefix))\n            .map(|plugin| (plugin.arn.clone(), plugin))\n            .collect();"
- `src/ops/list_custom_plugins.rs:187-192`: "        let Ok(page) = crate::state::page(\n            keyed,\n            input.max_results,\n            input.next_token.as_deref(),\n            crate::state::Listing::regional(Self::NAME, &region),\n        ) else {"

The skip that drops the items, `state.rs:2341`: "        if resume_after.as_deref().is_some_and(|from| key.as_str() < from) {"

The sibling that DOES bind its narrowing into the scope — `src/ops/list_connector_operations.rs:216`: "            crate::state::Listing::of_resource(Self::NAME, &input.connector_arn)," with `state.rs:2280-2282`: "    /// A listing of what is stored against one parent resource, named by its\n    /// ARN — which carries the region too, so it scopes the listing by itself."

The two identical copies in the siblings: `src/ops/list_connectors.rs:174` + `:184` + `:195`, and `src/ops/list_worker_configurations.rs:175` + `:185` + `:196`.

Test coverage shows this dimension is untested: `tests/all/page_token_belongs_to_one_listing.rs:308-341` builds its listing fixture by varying only the OPERATION and the REGION (`ListConnectors in {region}`, `ListCustomPlugins in {region}`, `ListWorkerConfigurations in {region}`, `ListConnectorOperations of {connector_arn}`) — no prefix is ever varied; and `tests/all/list_worker_configurations.rs:315` re-sends the prefix on every page ("/v1/worker-configurations?namePrefix=page-&maxResults={page_size}"), so a token is never replayed under a different prefix.

## Steps to Reproduce

1. Create three custom plugins in us-east-1, named so that at least two share a prefix the third does not: "zz-one", "zz-two", and "aa-other".
2. GET /v1/custom-plugins?namePrefix=zz-&maxResults=1 — the answer carries one zz plugin and a nextToken whose decoded resume key is the ARN of the other zz plugin.
3. GET /v1/custom-plugins?maxResults=100&nextToken=<that token> with NO namePrefix.
4. Observe 200 with a page that silently omits every plugin whose ARN sorts before the resume key ("aa-other" and the first zz plugin whenever their ARNs sort earlier), even though the caller asked for every plugin in the Region and the answer carries no nextToken to indicate anything was skipped.
5. Contrast with replaying a ListWorkerConfigurations token against ListCustomPlugins, which `page` refuses as BadPageToken -> BadRequestException because the operation name IS part of the scope.
