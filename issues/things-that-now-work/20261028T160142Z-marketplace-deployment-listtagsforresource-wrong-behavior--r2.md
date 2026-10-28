Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# Take the resolution sequence's clock sample under the store lock, as its own doc states

_Filed: 2026-10-28T16:01:42Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::resolve_tagged_parameter` is the ONE sequence every operation that addresses a deployment parameter by `resourceArn` runs (`ListTagsForResource`, `TagResource`, `UntagResource`). Its own step list documents step 4 as materialising the due deletion "from ONE clock sample taken under the lock", and the handlers repeat the promise that follows from it (ListTagsForResource: it "must not report a resource the clock has already taken"; `write_tagged_parameter`: "The instant is the sample the sequence already took under the lock, so the whole call ... is served at ONE instant").

The code takes the sample BEFORE acquiring the lock. Two consequences follow, both contradicting text in the same module:

1. Expiry: a caller that waits on the lock purges with a `now` sampled before it got in, so a record whose `expirationDate` fell inside that window is NOT purged and is resolved and reported — against the module-level promise that "no caller ever sees a parameter past its expiration date" and against ListTagsForResource's own "must not report a resource the clock has already taken".
2. Last-written instant: `write_tagged_parameter` stamps `DeploymentParameterRecord::mark_written(now)` from that same pre-lock sample, so two concurrent tag writes can land out of order — the writer that sampled earlier can acquire the lock later and stamp a record whose content it just wrote with an instant older than the previous writer's. `mark_written`'s doc states the opposite invariant: "no stored record can report a 'last written' instant older than its own content".

The fix is an ordering one inside the shared helper (sample after `state.lock()`); it changes no response body, no status and no existing test, since `updated_at` is observed only through the store and every existing suite is sequential.

## Evidence

Doc side — services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs:1075-1078 (step list of `resolve_tagged_parameter`):

    /// 4. materialise the deletion the clock has already made due, from ONE clock sample
    ///    taken under the lock
    ///    ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::purge_expired`]),
    ///    so a parameter past its `expirationDate` is gone before the ARN is resolved;

src/state.rs:1124-1126 (`write_tagged_parameter`):

    /// The instant is the sample the sequence already took under the lock, so the whole call —
    /// the deletion the clock made due and the write this record records — is served at ONE instant.

src/state.rs:437-441 (`mark_written`): "EVERY operation that writes the stored record refreshes it from the clock sample its own call was served at — the content a write leaves and the instant the record reports for it are one write, so no stored record can report a \"last written\" instant older than its own content."

src/state.rs:10-14 (module doc): "every operation that resolves a deployment parameter first materialises the deletion the clock has made due ... so no caller ever sees a parameter past its expiration date."

Code side — src/state.rs:1172-1174, inside `resolve_tagged_parameter`, the sample is taken BEFORE the lock:

    let now = SystemTime::now();
    let mut store = state.lock();
    store.purge_expired(now);

Handler side — src/ops/list_tags_for_resource.rs:178-182: "A parameter past its `expirationDate` is scheduled for deletion, and that deletion was owed at the sequence's own clock sample whether or not anybody called — so this reporting operation may apply it, and must not report a resource the clock has already taken." The same helper is reached from src/ops/tag_resource.rs:211 and src/ops/untag_resource.rs:182.

Test coverage: tests/all/deployment_parameter_expiry.rs and tests/all/tagging_resolution_sequence.rs pin the expiry/resolution answers, and tests/all/tag_write_last_written.rs pins `updated_at`/`created_at`; all are sequential, so none exercises the pre-lock window and none would change if the sample moved under the lock.

## Steps to Reproduce

1. Read services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs:1075-1078 — the sequence documents step 4 as using "ONE clock sample taken under the lock".
2. Read src/state.rs:1172-1174 — `let now = SystemTime::now();` executes before `let mut store = state.lock();`, so the sample is taken outside the lock.
3. Hold the store lock from another caller (e.g. a concurrent TagResource on the same engine) long enough for a record's `expirationDate` to pass after a ListTagsForResource has sampled `now` but before it acquires the lock: `purge_expired(now)` judges with the stale instant, the expired record is not dropped, and its tags are reported with HTTP 200 instead of the 404 the expiry rule promises.
4. With two concurrent TagResource calls on one record, let the caller that sampled the earlier instant acquire the lock second: `write_tagged_parameter` stamps `updated_at` with an instant older than the previous write's, contradicting the invariant stated at src/state.rs:437-441.
