Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 19809ff85dacb8a5011e45ecfee3e76273a4f2a9

# Feed and dictionary stores are not Region-scoped: a resource created through one Region's endpoint is listed, read and mutated through every other

_Filed: 2026-10-24T09:25:50Z_
_Operation: ListFeeds | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `19809ff85dacb8a5011e45ecfee3e76273a4f2a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Elemental Inference is a regional service: each Region's endpoint owns its own feeds and dictionaries, and a feed created in us-west-2 does not exist for a caller signing for us-east-1 (GetFeed/GetDictionary answer ResourceNotFoundException there and ListFeeds/ListDictionaries do not report it).

The emulator keeps ONE global store (`ComAmazonawsElementalinferenceElementalinferenceState.feeds` / `.dictionaries`) and no handler other than the ARN-based tagging path looks at the request's Region at all. `CreateFeed`/`CreateDictionary` do read the SigV4 credential scope — but only to mint the ARN (`crate::state::request_region` -> `feed_arn`/`dictionary_arn`). Consequences a client can observe:

* Create a feed with a request signed for us-west-2, then call `GET /v1/feeds` (or `GET /v1/feed/{id}`) with a request signed for us-east-1: the feed is listed/returned, and the body carries an ARN whose Region field is `us-west-2` — a cross-Region ARN no real Region's endpoint ever hands back.
* The same for dictionaries (`ListDictionaries`, `GetDictionary`, `ExportDictionaryEntries`), and for the mutators `UpdateFeed`/`DeleteFeed`/`AssociateFeed`/`DisassociateFeed`/`UpdateDictionary`/`DeleteDictionary`, all of which resolve by bare id.
* The per-Region quotas (`MAX_FEEDS`, `MAX_DICTIONARIES`) are consumed globally, so feeds created in one Region exhaust another Region's quota and produce ServiceQuotaExceededException (402) for a Region that holds nothing.
* The tagging path is inconsistent with the rest: `state.tagged_resource_mut` matches the WHOLE ARN including its Region field, so a us-east-1 caller that can see and delete the feed cannot tag it with its own Region's ARN (404) — one Region sees the resource for reads and writes but not for tagging.

Expected: the store is partitioned by the request's Region, so a resource is visible only through the endpoint that created it, and every id-based read/mutation in another Region answers ResourceNotFoundException. Actual: one shared store, Region used only for ARN text.

## Evidence

Spec/grounding: regional isolation of control-plane resources is well-established AWS behavior, and the model's own ARN patterns carry a Region field (`resourceArn` pattern `^arn:aws[a-z\-]*:elemental-inference[a-z\-]*:[a-z0-9\-]+:[0-9]{12}:(feed|dictionary)/[a-zA-Z0-9]{1,19}$`), which the emulator fills from the request's SigV4 credential scope — so the service clearly models per-Region resources.
Handler: `src/state.rs` holds `feeds: BTreeMap<String, Feed>` / `dictionaries: BTreeMap<String, Dictionary>` keyed by id only, with no Region dimension; `src/ops/list_feeds.rs` and `src/ops/list_dictionaries.rs` iterate the whole map (`state.feeds.iter()`), and `src/ops/get_feed.rs`, `get_dictionary.rs`, `update_feed.rs`, `delete_feed.rs`, `associate_feed.rs`, `disassociate_feed.rs`, `update_dictionary.rs`, `delete_dictionary.rs`, `export_dictionary_entries.rs` all look up `state.feeds/dictionaries.get(&input.id)` without consulting `crate::state::request_region(ctx.parts())` (several of them ignore `ctx` entirely: `let _ = ctx;`). Only `create_feed.rs`/`create_dictionary.rs` read the Region, and only to build the ARN.
Tests: `tests/all/harness.rs` signs every request for one Region (`REGION = "us-west-2"`), so no test exercises two Regions; `tests/all/resource_tagging.rs:320` ("the right resource in another region") only asserts that a foreign-Region ARN is refused by TagResource — the listing/read path is unasserted.

## Steps to Reproduce

1. POST /v1/feed signed for us-west-2 with one output -> note the feed id and its ARN (region field us-west-2)
2. GET /v1/feeds signed for us-east-1 -> observe the us-west-2 feed in the page (expected: an empty list for that Region)
3. GET /v1/feed/{id} signed for us-east-1 -> observe HTTP 200 with an ARN whose Region is us-west-2 (expected: 404 ResourceNotFoundException)
4. DELETE /v1/feed/{id} signed for us-east-1 -> observe HTTP 202 accepting the delete of another Region's feed
5. Repeat with POST /v1/dictionary + GET /v1/dictionaries / GET /v1/dictionary/{id} across two Regions for the same result
