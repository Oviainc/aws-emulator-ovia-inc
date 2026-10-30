Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: savingsplans.delete-queued-savings-plan-not-found-rejected
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans.delete-queued-savings-plan-not-found-rejected|run#8|AssertionError|assert err["Message"] == f"SavingsPlan can't be found. Savings Plan Id: {bogus_id}.", f"unexpected delete_bogus message: {err['Message']!r}"
Issue-Commit: 2a04615191c549bdab1e2f00e25668c6ae6e3d20

# DeleteQueuedSavingsPlan's ResourceNotFoundException message is the crate's own sentence, not AWS's "SavingsPlan can't be found. Savings Plan Id: <id>."

> _Note: this issue was identified against the code base at commit `2a04615191c549bdab1e2f00e25668c6ae6e3d20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteQueuedSavingsPlan

## Description
One net-new root cause in the service-com-amazonaws-savingsplans-awssavingsplan crate, found by behavioral replay.

Cause 1 of 1: DeleteQueuedSavingsPlan — the 404 ResourceNotFoundException body carries the emulator's own lowercase sentence instead of AWS's message for an unknown/foreign savingsPlanId. Failing check: savingsplans.delete-queued-savings-plan-not-found-rejected.

AWS: Smithy (services/service-com-amazonaws-savingsplans-awssavingsplan/smithy-model/com-amazonaws-savingsplans-awssavingsplan.json): DeleteQueuedSavingsPlan input savingsPlanId @required (target SavingsPlanId); modeled errors include ResourceNotFoundException with httpError 404. The model does not state message text, so the text is recorded behavior only: for a well-formed savingsPlanId (a UUID) that the caller does not own, AWS answers HTTP 404 with Code=ResourceNotFoundException and the message template "SavingsPlan can't be found. Savings Plan Id: <savingsPlanId>." — fixed text is "SavingsPlan can't be found. Savings Plan Id: " followed by the one variable part, the savingsPlanId taken verbatim from the request, then a terminating period. The classification (404 / ResourceNotFoundException) already matches; only the message diverges.

Emulator: returns the correct Code=ResourceNotFoundException, but the message is "no Savings Plan with id '<savingsPlanId>'" — different fixed wording, lowercase, the id wrapped in single quotes and no terminating period. The replayed assertion failed with: unexpected delete_bogus message: "no Savings Plan with id '7e5cd8da-c641-4786-aa69-9bcc16899b0b'" (the UUID is a per-run value and is not part of the requirement; the template is).

Why: clients and tests that make a delete idempotent, or that log/branch on the service's not-found text (common where only the message distinguishes "foreign id" from other 404 paths), never see the sentence AWS sends, so an expected not-found is surfaced as an unknown failure. The same wrong-sentence pattern makes the emulator's errors unusable as a drop-in for recorded AWS error bodies.

Fix: the DeleteQueuedSavingsPlan handler's plan-lookup failure path in the savingsplans crate — construct the ResourceNotFoundException with AWS's sentence, interpolating only the request's savingsPlanId. Note: open issue 20261029T203239Z-savingsplans-handlers-diverge-from-recorded-aws-answers-in.md carries a DeleteQueuedSavingsPlan message key, but its Cause 7 is the 400 ValidationException wrong-state message ("Savings Plan is not in Queued State."); it does NOT cover this 404 not-found message path, which is a separate construction site.

Traces: savingsplans.delete-queued-savings-plan-not-found-rejected (the same check confirms the surrounding lifecycle works: creating a queued plan, deleting it and reaching queued-deleted all pass, so the divergence is isolated to the foreign-id 404 message).

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 20/21 passed; sole failure [mismatch] savingsplans.delete-queued-savings-plan-not-found-rejected.

Assertion in check_savingsplans_delete_queued_savings_plan_not_found_rejected.py line 32:
  assert err["Message"] == f"SavingsPlan can't be found. Savings Plan Id: {bogus_id}."
AssertionError: unexpected delete_bogus message: "no Savings Plan with id '7e5cd8da-c641-4786-aa69-9bcc16899b0b'"

Expected (AWS): HTTP 404, Code=ResourceNotFoundException, Message="SavingsPlan can't be found. Savings Plan Id: <savingsPlanId>." — fixed text "SavingsPlan can't be found. Savings Plan Id: " + the savingsPlanId from the request + ".".
Actual (emulator): HTTP 404, Code=ResourceNotFoundException (matches), Message="no Savings Plan with id '<savingsPlanId>'".

The preceding assertion on err["Code"] == "ResourceNotFoundException" passed, and the subsequent real delete of the check's own queued plan plus the poll to 'queued-deleted' are reached only after this assertion, so only the message text is at fault.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. CreateSavingsPlan from a 1-year Compute / No Upfront / USD offering with commitment "0.001" and a purchaseTime ~200 days in the future; poll DescribeSavingsPlans until state == 'queued'.
3. Call DeleteQueuedSavingsPlan with a freshly generated UUID that is not a plan the caller owns.
4. Observe the error body: Code is ResourceNotFoundException (correct) but Message is "no Savings Plan with id '<uuid>'" instead of "SavingsPlan can't be found. Savings Plan Id: <uuid>.".
5. Re-replay behavioral check savingsplans.delete-queued-savings-plan-not-found-rejected; it must pass end-to-end (including the follow-on real delete reaching 'queued-deleted').
