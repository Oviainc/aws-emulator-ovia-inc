Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b3076e7c0c2ebb8fac474fe10cf99e18802cbe88

# TagResource ignores the Region the request was signed for, so one Region's endpoint tags another Region's resource

_Filed: 2026-10-24T18:03:43Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b3076e7c0c2ebb8fac474fe10cf99e18802cbe88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other resource-reaching operation of this emulator is now Region-scoped: `GetFeed`, `UpdateFeed`, `DeleteFeed`, `AssociateFeed`, `DisassociateFeed`, `GetFeedPolicy`, `PutFeedPolicy`, `GetDictionary`, `UpdateDictionary`, `DeleteDictionary`, `ExportDictionaryEntries`, `ListFeeds` and `ListDictionaries` all call `crate::state::request_region(ctx.parts())` and resolve the resource WITHIN that Region (`state.feed(&region, id)` / `state.dictionary(&region, id)`), so an id belonging to another Region's endpoint answers 404.

`TagResource` is the one path that does not. Its handler discards the request context entirely (`let _ = ctx;`) and resolves the resource by ARN alone through `state.tagged_resource_mut(&input.resource_arn)`, which matches the stored record's full ARN text but takes no Region argument. Consequence: a request signed for Region B that carries the CORRECT ARN of a resource created in Region A succeeds (HTTP 204) and mutates Region A's record — a write Region B's endpoint does not own. Region A's `GetFeed`/`GetDictionary` then reports a tag that was written through a different Region's endpoint.

Expected: `TagResource` associates tags with "the resource identified by the specified resourceArn IN THE CURRENT REGION" (the operation's own documentation), so an ARN whose Region field is not the Region the request was signed for names no resource of that endpoint and earns the modeled `ResourceNotFoundException` (HTTP 404) — the same answer the id-based paths give for another Region's resource.
Actual: HTTP 204 and a cross-Region tag write; the ARN's Region field is never compared with the request's credential scope.

## Evidence

Spec: `smithy_query(query="doc",shape="TagResource")` → "Associates the specified tags to the resource identified by the specified resourceArn **in the current region**." The modeled `resourceArn` pattern carries a Region field (`^arn:aws[a-z\-]*:elemental-inference[a-z\-]*:[a-z0-9\-]+:[0-9]{12}:(feed|dictionary)/[a-zA-Z0-9]{1,19}$`), and `ResourceNotFoundException` (HTTP 404) is modeled for TagResource. Regional ownership of control-plane resources is well-established AWS behavior and is already the rule the rest of this crate enforces.
Handler: `src/ops/tag_resource.rs` `handle()` — line ~195 `let _ = ctx;` (the Region is never read), line ~222 `state.tagged_resource_mut(&input.resource_arn)`. `src/state.rs` `tagged_resource_mut(&mut self, resource_arn: &str)` (~line 941) takes no `region` parameter and only filters on `feed.arn == resource_arn` / `dictionary.arn == resource_arn`; its own doc comment quotes the "in the current region" sentence it does not implement.
Tests: `tests/all/region_isolation.rs:500` (`tagging_resolves_a_resource_in_the_same_region_the_id_paths_do`) only covers the foreign-Region signer presenting a REWRITTEN foreign-Region ARN (line 533 `arn.replacen(arn_region(&arn), foreign, 1)`), which 404s because the ARN text differs. The case of a foreign-Region signer presenting the owner Region's real ARN is unasserted. `tests/all/resource_tagging.rs:306` likewise only varies the ARN text, not the signing Region. Note: the previously fixed issue `20261024T092550Z-elementalinference-listfeeds-wrong-behavior` Region-scoped the id-based paths and explicitly flagged the ARN path as inconsistent; this is the residual half of that inconsistency, now in the opposite direction.

## Steps to Reproduce

1. POST /v1/feed signed for us-west-2 with one output → note the feed id and its ARN (Region field us-west-2)
2. POST /v1/tags/arn:aws:elemental-inference:us-west-2:<account>:feed/<id> with body {"tags":{"written-by":"us-east-1"}} but sign the request for us-east-1 → observe HTTP 204 (expected: HTTP 404 ResourceNotFoundException, since that feed is not in the current Region)
3. GET /v1/feed/<id> signed for us-west-2 → observe the tag written through the us-east-1 endpoint in `tags`
4. Repeat with a dictionary ARN (POST /v1/dictionary signed for us-west-2, then POST /v1/tags/<us-west-2 dictionary ARN> signed for eu-west-1) for the same result
5. Contrast: DELETE /v1/feed/<id> signed for us-east-1 correctly answers 404, so the two paths disagree about which Region owns the feed
