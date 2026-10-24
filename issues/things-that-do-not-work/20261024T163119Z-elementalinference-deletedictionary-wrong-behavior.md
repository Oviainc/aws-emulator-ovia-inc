Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0831f46a299ca00fdb6555dd0d1d0166298aa2f3

# The modeled DELETED dictionary status is unreachable: a deleted dictionary vanishes from the store, so a repeated DeleteDictionary answers 404 instead of reporting DELETED

_Filed: 2026-10-24T16:31:19Z_
_Operation: DeleteDictionary | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0831f46a299ca00fdb6555dd0d1d0166298aa2f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteDictionaryResponse$status` is documented as "The status of the dictionary after deletion", and `DELETED` is one of the five values `DictionaryStatus` declares (AVAILABLE | CREATING | DELETED | DELETING | REFERENCED). The emulator can never produce it.

`DeleteDictionary` parks the record in `DELETING` with a ~400 ms transition due, and the settle pass then REMOVES the record from the store outright. From that instant the dictionary simply does not exist.

Observable consequences:
* `DeleteDictionary` is modeled `@idempotent`, and it answers 202 ("accepted", which invites a client to poll the delete to completion). Inside the transition window a repeat correctly answers 202 with `status: DELETING`; once the window has passed, the very same idempotent call answers HTTP 404 `ResourceNotFoundException` instead of a success carrying `status: DELETED`.
* No operation of this service ever reports `DictionaryStatus.DELETED` — not `DeleteDictionary`, not `GetDictionary`, not `ListDictionaries` — so a client state machine that waits for the terminal `DELETED` status never sees it and has to treat a 404 as the terminal state.

Expected: once deletion of the dictionary has succeeded, the service reports it as `DELETED` (the delete answers success with `status: DELETED` rather than refusing). Actual: the record is dropped and every later call naming it is a 404.

This is the dictionary counterpart of the already-filed DeleteFeed/FeedStatus.DELETED gap; it is a distinct operation, a distinct enum and a distinct store, and the dictionary handler and the dictionary branch of the settle pass are separate code.

## Evidence

Spec: `smithy_query(query="doc",shape="DeleteDictionaryResponse",member="status")` -> "The status of the dictionary after deletion"; `smithy_query(query="shape",shape="DictionaryStatus")` -> values include `DELETED`; OPERATIONS.md "DeleteDictionary" shows `DELETE /v1/dictionary/{id} -> 202` with `@idempotent` and a required `status` output member.
Handler: `src/ops/delete_dictionary.rs` `handle()` only ever calls `dictionary.enter_transient(crate::state::DictionaryStatus::Deleting, now)` (or reuses the already-due instant) and reports `dictionary.reported_status(&references)`, which can return only CREATING/AVAILABLE/REFERENCED/DELETING; `src/state.rs` `settle_due_transitions()` drops the entry from `state.dictionaries` when the due instant passes (`DictionaryStatus::Deleting => false` in the `retain`), and the local `DictionaryStatus` enum declares no `Deleted` variant at all ("The `DictionaryStatus` values this slice of the service produces").
Tests: `tests/all/dictionary_lifecycle.rs` asserts the CREATING->AVAILABLE settle and that "an id no live dictionary has is a 404 ResourceNotFoundException"; `tests/all/dictionary_mutation.rs` asserts the DELETING window and the later 404 — i.e. the suite pins the current behavior, and no test exercises a DELETED report.

## Steps to Reproduce

1. POST /v1/dictionary with {"name":"d1","language":"eng"} -> note the dictionary id
2. DELETE /v1/dictionary/{id} -> HTTP 202 with status DELETING
3. Wait past the emulator's transition span (~400 ms)
4. DELETE /v1/dictionary/{id} again (an idempotent retry of the same accepted delete) -> observe HTTP 404 ResourceNotFoundException instead of a success carrying status DELETED
5. GET /v1/dictionary/{id} -> 404; the modeled DictionaryStatus value DELETED is never observable from any operation of the service
