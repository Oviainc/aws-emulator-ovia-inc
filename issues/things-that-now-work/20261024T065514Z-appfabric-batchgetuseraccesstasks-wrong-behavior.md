Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# BatchGetUserAccessTasks silently collapses repeated taskIdList entries, returning fewer results than ids

_Filed: 2026-10-24T06:55:14Z_
_Operation: BatchGetUserAccessTasks | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

BatchGetUserAccessTasks takes a taskIdList (1..50 UUIDs) and reports one UserAccessResultItem per polled task; a batch read on AWS answers positionally for the ids it was given. The emulator's handler keeps a `polled` HashSet and `continue`s on any id it has already seen, so a request naming the same taskId twice (a normal outcome of a client concatenating poll batches) comes back with ONE item instead of two and the caller cannot line results up with the ids it submitted.

Expected: an item for every entry in taskIdList, in request order, repeats included. Actual: userAccessResultsList is shorter than taskIdList whenever an id repeats.

## Evidence

Spec: BatchGetUserAccessTasks input `taskIdList` (list<UUID>, @length 1..50, required) / output `userAccessResultsList` (list<UserAccessResultItem>) — OPERATIONS.md and smithy_query(shape=TaskIdList); the model declares no dedup or set semantics (the list is not @uniqueItems).
Handler: src/ops/batch_get_user_access_tasks.rs lines 188-201 — `if !polled.insert(task_id.as_str()) { continue; }` before the per-task lookup.
Test coverage: tests/all/user_access_tasks.rs polls with distinct ids only (the batch call at line 129); no test asserts the deduping behavior.

## Steps to Reproduce

1. POST /appbundles -> note appBundle.arn; create and connect an app authorization under it
2. POST /useraccess/start with {appBundleIdentifier, email} -> note userAccessTasksList[0].taskId
3. POST /useraccess/batchget with {appBundleIdentifier, taskIdList: [taskId, taskId]}
4. Observe userAccessResultsList has 1 item for 2 submitted ids
