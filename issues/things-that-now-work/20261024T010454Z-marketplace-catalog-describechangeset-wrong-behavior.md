Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# No change set ever reaches FAILED, so Status=FAILED, FailureCode, FailureDescription and ErrorDetailList are unreachable

_Filed: 2026-10-24T01:04:54Z_
_Operation: DescribeChangeSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every change set the emulator accepts runs PREPARING → APPLYING → SUCCEEDED on a fixed 250 ms/250 ms schedule; nothing ever transitions to `FAILED`. Consequently `DescribeChangeSet` can never report `Status: FAILED`, `FailureCode` (CLIENT_ERROR | SERVER_FAULT), `FailureDescription`, or a `ChangeSummary.ErrorDetailList` entry — all modeled output members. Real AWS accepts many malformed/semantically-invalid change requests synchronously (200 with a ChangeSetId) and then fails them asynchronously, and a seller's workflow polls DescribeChangeSet until it sees SUCCEEDED or FAILED and then reads `ErrorDetailList` to find out why. In this emulator the failure branch of that workflow is unobservable: content problems are either refused up-front as 422 at StartChangeSet or applied silently.

## Evidence

Spec: `DescribeChangeSetResponse` models `Status` (ChangeStatus includes FAILED), `FailureCode` ("Returned if the change set is in FAILED status"), `FailureDescription`, and `ChangeSummary$ErrorDetailList`.
Handler/state: `src/ops/start_change_set.rs` sets `failure_code: None, failure_description: None` with the comment "Nothing this emulator applies fails, so no change set reaches FAILED and none reports a failure"; the `schedule` only ever contains APPLYING then SUCCEEDED. `src/state.rs` `ChangeRecord::to_summary()` sets `error_detail_list: None` with the same reasoning, and `settle()`/`apply_changes()` have no failure path.
Tests: `tests/all/change_set_lifecycle.rs` line 426 only asserts that a RUNNING change set reports no FailureCode; no test exercises a FAILED change set because none can exist.

## Steps to Reproduce

1. POST /StartChangeSet with a change whose details are structurally accepted but semantically invalid for the change type (e.g. ChangeType=AddDeliveryOptions with details missing required members)
2. Poll GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> until it stops changing
3. Observe Status=SUCCEEDED with no FailureCode/FailureDescription and ChangeSet[].ErrorDetailList absent; on AWS such a change set settles to FAILED with FailureCode=CLIENT_ERROR and per-change ErrorDetailList entries
