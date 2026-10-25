Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elementalinference.delete-feed-removes-it-regardless-of-status elementalinference.delete-dictionary-referenced-by-feed-rejected
Issue-Shape-Id: com.amazonaws.elementalinference#ElementalInference
Issue-Key: elementalinference.delete-feed-removes-it-regardless-of-status|_error_code#12|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: elementalinference.delete-dictionary-referenced-by-feed-rejected|_error_code#22|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: ef659389d11311dc27a7ee58cb593d22d3f663cf

# A completed deletion is kept as a reportable tombstone, so a repeat DeleteFeed/DeleteDictionary answers 202 DELETED where AWS refuses 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `ef659389d11311dc27a7ee58cb593d22d3f663cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFeed

## Description
This issue bundles 2 root causes, both in the "a completed deletion stays reportable" design added to this service's state. A fixer must close BOTH before submitting.

Background: the resolved issue `20261024T092550Z-elementalinference-deletefeed-wrong-behavior.md` and the still-open `20261024T163119Z-elementalinference-deletedictionary-wrong-behavior.md` both argue that a repeated delete must answer success carrying `status: DELETED` and that answering 404 is the defect. The replayed recordings contradict that premise directly: after the deletion has completed, the SAME idempotent delete call is refused by AWS with HTTP 404 `ResourceNotFoundException`. The open dictionary issue should be closed as not-a-defect rather than implemented, and the feed fix needs to be reverted on this point.

Cause 1 of 2: DeleteFeed on a feed whose deletion already completed. Failing check: elementalinference.delete-feed-removes-it-regardless-of-status
AWS:      Smithy: `DeleteFeed` = `DELETE /v1/feed/{id}` -> 202, `@idempotent`, and it models `ResourceNotFoundException` (httpError 404, documented "The resource specified in the action doesn't exist."). Recorded behavior: the feed is deleted, an independent `GetFeed` then answers 404 `ResourceNotFoundException` (message "Resource not found" — the same not-found message rule this service already applies to any id it holds nothing for), and a SECOND `DeleteFeed` on the same id is ALSO refused 404 `ResourceNotFoundException`. AWS keeps no post-deletion record that a later call naming the id can report.
Emulator: the second DeleteFeed answers HTTP 202 with a success body `{"arn": ..., "id": ..., "status": "DELETED"}`. The deleted feed is retired into a tombstone record that stays in the store and is resolvable only by the delete path, so the delete accepts an id every other operation (correctly) treats as absent.
Why:      a client cannot distinguish "this id exists and I just deleted it" from "this id does not exist". Teardown/reconciliation logic that deletes an id and treats 404 as "already gone" (and success as "it was still there") loops or double-counts; a client that polls DeleteFeed for the terminal condition never sees the refusal AWS uses to signal it. The emulator is also internally inconsistent — GetFeed says 404 for the same id in the same instant.
Fix:      DeleteFeed's handler and the feed branch of the state store: a completed deletion must leave nothing a later call can resolve, so the delete path must stop resolving retired records and must answer the modelled 404 for them.
Traces:   elementalinference.delete-feed-removes-it-regardless-of-status (replay shows the re-delete returning 202 with status DELETED instead of raising ResourceNotFoundException).

Cause 2 of 2: DeleteDictionary on a dictionary whose deletion already completed. Failing check: elementalinference.delete-dictionary-referenced-by-feed-rejected
AWS:      Smithy: `DeleteDictionary` = `DELETE /v1/dictionary/{id}` -> 202, `@idempotent`, models `ResourceNotFoundException` (httpError 404). Recorded behavior: once the dictionary's reference is drained and the delete succeeds, `GetDictionary` answers 404 `ResourceNotFoundException` ("Resource not found") and a repeat `DeleteDictionary` on the same id is ALSO refused 404 `ResourceNotFoundException`.
Emulator: the repeat DeleteDictionary answers HTTP 202 with `{"arn": ..., "id": ..., "status": "DELETED"}`, from a separate dictionary tombstone record and a separate handler/state branch from Cause 1 — which is why this is a second work item and not the same line of code.
Why:      same client consequence as Cause 1, and it breaks the check's wider assertion chain: the recorded contract is that a deleted dictionary id is dead for every operation, including the delete itself.
Fix:      DeleteDictionary's handler and the dictionary branch of the state store (the dictionary counterpart of Cause 1's retire/resolve path).
Traces:   elementalinference.delete-dictionary-referenced-by-feed-rejected (the ConflictException-while-referenced leg passes; the failure is the final re-delete of the already-deleted dictionary returning 202/DELETED).

## Evidence
Cause 1 — check elementalinference.delete-feed-removes-it-regardless-of-status:
Expected (recorded AWS): DeleteFeed on an already-deleted feed id -> HTTP 404, Error.Code `ResourceNotFoundException`, message "Resource not found" (example text; the rule is this service's established not-found message for an unknown id).
Actual (emulator): HTTP 202, body `{'arn': 'arn:aws:elemental-inference:us-west-2:000000000000:feed/<id>', 'id': '<id>', 'status': 'DELETED'}`; harness raised `AssertionError: expected a rejection, but the call SUCCEEDED and returned {...'status': 'DELETED'}`. Note the immediately preceding GetFeed on the same id DID answer 404 ResourceNotFoundException, so the two paths disagree.
Code: `services/service-com-amazonaws-elementalinference-elementalinference/src/ops/delete_feed.rs` lines ~183-202 resolve the id via `state.feed_including_deleted_mut(&region, &input.id)` and return early with `status: DELETED` when `feed.is_deleted()`; `src/state.rs` `Feed::retire()` (~line 687) parks the record as `FeedStatus::Deleted` instead of removing it, and `settle_due_transitions()` (~line 1238) calls `retire(now)` for `FeedStatus::Deleting`.

Cause 2 — check elementalinference.delete-dictionary-referenced-by-feed-rejected:
Expected (recorded AWS): DeleteDictionary on an already-deleted dictionary id -> HTTP 404, Error.Code `ResourceNotFoundException`, message "Resource not found".
Actual (emulator): HTTP 202, body `{'arn': 'arn:aws:elemental-inference:us-west-2:000000000000:dictionary/<id>', 'id': '<id>', 'status': 'DELETED'}`; same `AssertionError: expected a rejection, but the call SUCCEEDED`. The preceding GetDictionary on that id answered 404 ResourceNotFoundException.
Code: `src/state.rs` `Dictionary::is_deleted()` / `Dictionary::retire()` (~lines 474-487) and the dictionary arm of `settle_due_transitions()`/`prune_completed_deletions()`; `src/ops/delete_dictionary.rs` resolves and reports the retired record.

Spec used: smithy_query(query="operation", operation="DeleteFeed,DeleteDictionary") — both are `@idempotent`, both bind HTTP 202 on success, and both declare `ResourceNotFoundException` with httpError 404.

## Steps to Reproduce
Cause 1:
1. POST /v1/feed with one subtitling output -> note the feed id; poll GET /v1/feed/{id} until status AVAILABLE.
2. DELETE /v1/feed/{id} -> 202.
3. Poll GET /v1/feed/{id} until it answers 404 ResourceNotFoundException ("Resource not found").
4. DELETE /v1/feed/{id} again -> observe HTTP 202 with status DELETED; AWS answers HTTP 404 ResourceNotFoundException.

Cause 2:
1. POST /v1/dictionary {"name":"d1","language":"eng","entries":"[{\"content\": \"ace\"}]"} -> note the dictionary id.
2. DELETE /v1/dictionary/{id} -> 202; poll GET /v1/dictionary/{id} until 404 ResourceNotFoundException.
3. DELETE /v1/dictionary/{id} again -> observe HTTP 202 with status DELETED; AWS answers HTTP 404 ResourceNotFoundException.
