Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b3076e7c0c2ebb8fac474fe10cf99e18802cbe88

# AssociateFeed appends outputs carrying unresolvable dictionary and fixture references

_Filed: 2026-10-24T18:03:43Z_
_Operation: AssociateFeed | Class: validation_

> _Note: this issue was identified against the code base at commit `b3076e7c0c2ebb8fac474fe10cf99e18802cbe88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateFeed` appends the request's `outputs` to the stored feed after checking only three things about them: no duplicate name within the request, no collision with an output the feed already holds, and the per-feed output quota. The CONTENT of each appended output's `outputConfig` is never resolved against the rest of the store, so an association can introduce:

1. A `subtitling` output whose `dictionary` names a `DictionaryId` that no dictionary in the Region has (or one in a different Region). The feed is then persisted holding a dangling dictionary reference, and `GetDictionary` for that id answers 404 while `GetFeed` reports an output pointing at it.
2. A `clipping` output whose `dataSourceConfiguration.fixtureId` names a fixture the data source does not hold — an id for which `GetFixture` itself answers a refusal.

Expected: the appended outputs' references are resolved before the feed is mutated, and an unresolvable `dictionary`/`fixtureId` earns the modeled `ValidationException` (HTTP 400) with the feed left exactly as it was.
Actual: HTTP 200, the feed goes ACTIVE, and the dangling reference is persisted and reported by `GetFeed`/`UpdateFeed` from then on.

The emulator already maintains this referential relation in the opposite direction, which is what makes the asymmetry observable: `state.feeds_referencing` derives `GetDictionary$references` from the outputs of stored feeds and `DeleteDictionary` refuses (409) while any live feed references the dictionary. So an AssociateFeed-introduced reference to a non-existent dictionary creates a feed/dictionary graph the service's own integrity rules do not admit — and if that dictionary id is later minted by `CreateDictionary`, the new dictionary immediately reports `REFERENCED` by a feed that never legitimately referenced it, and cannot be deleted.

## Evidence

Spec: `smithy_query(query="shape",shape="SubtitlingConfig")` → `dictionary` is "The ID of a custom dictionary to improve transcription accuracy... **Use the CreateDictionary operation to create a dictionary**"; `smithy_query(query="shape",shape="DataSourceConfiguration")` → `fixtureId` (required) is "The ID of the fixture whose event data you want Elemental Inference to map onto this clipping output". `ValidationException` (HTTP 400) is modeled for AssociateFeed. The service's own integrity rule is modeled too: `GetDictionaryResponse$references` plus DeleteDictionary's documented refusal ("You cannot delete a dictionary that is referenced by a feed").
Handler: `src/ops/associate_feed.rs` `handle()` — the only pre-write checks are `resembles_an_arn` on `associatedResourceName` (~line 213), `first_duplicate_output_name` (~line 228), the feed-status precondition (~line 280), the name collision with a held output (~line 314) and `MAX_OUTPUTS_PER_FEED` (~line 335). `input.outputs` is then mapped straight into `crate::state::Output::new(...)` (~line 370) with no lookup into `state.dictionaries` or `state.fixtures`.
Tests: `tests/all/feed_association.rs` asserts the duplicate-name conflict (line ~579) and the status preconditions, but no test sends an appended output whose `subtitling.dictionary` or `clipping.dataSourceConfiguration.fixtureId` resolves to nothing. (The sibling gap on CreateFeed is tracked separately as `20261024T012105Z-elementalinference-createfeed-validation`; AssociateFeed is the other writer of the output list and is unenforced in the same way.)

## Steps to Reproduce

1. POST /v1/feed with one cropping output → note the feed id (status AVAILABLE)
2. POST /v1/feed/{id}/associate with {"associatedResourceName":"source-media","outputs":[{"name":"subs","status":"ENABLED","outputConfig":{"subtitling":{"language":"eng","dictionary":"doesnotexist1234567"}}}],"dryRun":false} → observe HTTP 200 (expected: HTTP 400 ValidationException)
3. GET /v1/dictionary/doesnotexist1234567 → observe HTTP 404, so the feed now references a dictionary that does not exist
4. GET /v1/feed/{id} → observe the appended output with its dangling `dictionary` reference and `fromAssociation: true`
5. POST /v1/feed/{id2}/associate with an output whose outputConfig is {"clipping":{"dataSourceConfiguration":{"fixtureId":"no-such-fixture"}}} → observe HTTP 200 although GET /v1/fixtures/no-such-fixture refuses that id
