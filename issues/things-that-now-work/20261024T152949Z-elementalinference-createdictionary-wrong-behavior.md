Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# CreateDictionary is not idempotent: a replayed identical create mints a second dictionary with a new id

_Filed: 2026-10-24T15:29:49Z_
_Operation: CreateDictionary | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateDictionary is modeled `@idempotent`, so the effect on the service of several identical requests must be the effect of one. The handler instead mints a fresh `DictionaryId` on every call and inserts another record, with no lookup of what the account already holds and no uniqueness rule on `name`.

So an SDK retry of a create whose response was lost (or a client that simply re-runs its setup) silently doubles the account's dictionaries: two records with the same `name` and `language`, two different ids and ARNs, both reported by ListDictionaries, and the first one orphaned because the caller only learns the second id. Each one also counts against `MAX_DICTIONARIES`.

Expected (either reading of the modeled contract): a replayed identical CreateDictionary answers with the dictionary that already exists — same `id`, same `arn`, one stored record — or, if the real service keys creation on the name, it refuses the duplicate with the modeled `ConflictException` (HTTP 409). Actual: HTTP 202 and a brand-new dictionary each time, and the modeled `ConflictException` is unreachable from this operation.

The identical pattern is in `create_feed.rs` (also `@idempotent`, also modeling ConflictException, also minting a new FeedId per call); a fix should cover both. CreateFeed's `wrong-behavior` slot is already taken by the output-config defaults issue, so it is reported here.

## Evidence

Spec: `smithy_query(query="operation",operation="CreateDictionary")` — `idempotent: true`, errors include `ConflictException` (HTTP 409) which no code path in the handler can produce; `OPERATIONS.md` lists the same traits for CreateFeed.
Handler: `src/ops/create_dictionary.rs` `handle()` lines ~239-273 — `state.mint_dictionary_id()` then `state.dictionaries.insert(...)` unconditionally; the only pre-insert state check is the `MAX_DICTIONARIES` quota. There is no index by name and no record of previously served requests anywhere in `src/state.rs` (`ComAmazonawsElementalinferenceElementalinferenceState` holds only `feeds`, `dictionaries`, `fixtures`, id entropy and the transition clock).
Tests: `tests/all/dictionary_lifecycle.rs:103 create_dictionary_persists_a_dictionary_get_dictionary_reports_member_for_member` creates with distinct names only and asserts every minted id is distinct; no test submits the same CreateDictionary body twice, so neither the replay nor the ConflictException path is asserted.

## Steps to Reproduce

1. POST /v1/dictionary with {"name": "brandterms", "language": "eng", "entries": "Hawks"} -> HTTP 202 with id A
2. Replay the exact same POST body (as an SDK retry after a lost response) -> observe HTTP 202 with a DIFFERENT id B instead of the idempotent reply naming id A (or a 409 ConflictException)
3. GET /v1/dictionaries -> observe two dictionaries both named "brandterms", so the retry doubled the account's state
