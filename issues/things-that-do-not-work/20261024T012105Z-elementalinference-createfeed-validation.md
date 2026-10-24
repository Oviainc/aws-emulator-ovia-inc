Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# CreateFeed accepts an outputs array that the documented rules forbid: empty list, duplicate output names, and references that resolve to nothing

_Filed: 2026-10-24T01:21:05Z_
_Operation: CreateFeed | Class: validation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `outputs` payload of CreateFeed is stored exactly as submitted, with no content checks at all.

1. EMPTY LIST. `CreateFeedRequest$outputs` is documented "You must specify at least one output, but you can later add outputs using AssociateFeed, or add, modify, and delete outputs using UpdateFeed." The list shape `CreateOutputList` carries no `@length` trait, so the framework's boundary validator cannot refuse an empty array and the handler does not either. Expected: HTTP 400 `ValidationException`. Actual: HTTP 202 and a persisted feed whose `outputs` is `[]`, which `GetFeed`/`ListFeeds` then report — a feed the real service would never have created.

2. DUPLICATE OUTPUT NAMES. Two outputs with the same `name` in one CreateFeed request are both stored, so the feed holds two outputs that cannot be told apart. The emulator itself treats this as illegal everywhere else: `AssociateFeed` refuses both an intra-request duplicate and a collision with a stored output with `ConflictException` (`associate_feed.rs`, `first_duplicate_name` and the "already has an output named" check), on the grounds that "You can't modify an existing output (for example to change its name). Instead, use UpdateFeed". CreateFeed (and UpdateFeed) leave the same invariant unenforced, so a feed created with duplicates is in a state no AssociateFeed call could have produced.

3. UNRESOLVED REFERENCES. `SubtitlingConfig$dictionary` is "The ID of a custom dictionary ... Use the CreateDictionary operation to create a dictionary" and `DataSourceConfiguration$fixtureId` is "The ID of the fixture whose event data you want Elemental Inference to map onto this clipping output". Neither is resolved against the dictionary store or the fixture catalog, so CreateFeed happily persists an output pointing at a dictionary id that no dictionary has, or at a fixture id the data source does not hold (an id for which `GetFixture` answers a refusal). The service clearly maintains this referential relation in the other direction — `DeleteDictionary` refuses while a feed references the dictionary and `GetDictionary$references` reports the referencing feeds — so a feed carrying a dangling reference is a state the real service's integrity rules do not admit.

## Evidence

Spec: `smithy_query(query="doc",shape="CreateFeedRequest",member="outputs")` → "You must specify at least one output..."; `CreateOutputList` has no `@length` (so no boundary refusal); `SubtitlingConfig$dictionary` and `DataSourceConfiguration$fixtureId` docs quoted above; `ValidationException` (HTTP 400) is modeled for CreateFeed.
Handler: `src/ops/create_feed.rs` `handle()` — the only pre-store checks are the cross-account `accessRoleArn` denial and the `MAX_FEEDS` quota; `input.outputs` is mapped straight into `crate::state::Output::new(...)` with no emptiness, uniqueness or reference check (lines ~246-281).
Tests: no test asserts a refusal for an empty `outputs`. `tests/all/error_contract.rs:314-325` sends `"outputs": []` only inside a case that expects the `name` pattern refusal, and `tests/all/refusals_from_state.rs:48` comments that the documentation requires "at least one output" while its helper always supplies one — i.e. the rule is known but unenforced and untested. `tests/all/feed_association.rs:579` asserts the duplicate-name conflict for AssociateFeed only.

## Steps to Reproduce

1. POST /v1/feed with {"name": "no-outputs-feed", "outputs": []} — observe HTTP 202 and a feed id instead of HTTP 400 ValidationException
2. GET /v1/feed/{id} — observe `outputs: []` on a persisted feed
3. POST /v1/feed with two entries in `outputs` that share one `name` — observe HTTP 202 instead of a refusal (the same payload shape is refused with 409 ConflictException when sent to POST /v1/feed/{id}/associate)
4. POST /v1/feed with one output whose outputConfig is {"subtitling": {"language": "eng", "dictionary": "doesnotexist1234567"}} — observe HTTP 202; GET /v1/dictionary/doesnotexist1234567 answers 404, so the feed now references a dictionary that does not exist
5. POST /v1/feed with one output whose outputConfig is {"clipping": {"dataSourceConfiguration": {"fixtureId": "no-such-fixture"}}} — observe HTTP 202 although GET /v1/fixtures/no-such-fixture refuses that id
