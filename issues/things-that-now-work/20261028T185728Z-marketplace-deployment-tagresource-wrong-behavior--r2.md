Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# Tagging sequence samples the clock OUTSIDE the store lock, contradicting its own "ONE clock sample taken under the lock" rule

_Filed: 2026-10-28T18:57:28Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::resolve_tagged_parameter` is the single sequence every tagging operation runs (`ListTagsForResource`, `TagResource`, `UntagResource` all enter it through `with_tagged_parameter` / `write_tagged_parameter`). Its own documentation states three times that the clock instant used for the expiry purge and for the write stamp is sampled UNDER the store lock, so that one call's purge and its write are served at one instant. The code samples `SystemTime::now()` on the line BEFORE `state.lock()`, so the sample is taken outside the lock and a request can block arbitrarily long on the mutex between sampling and acting.

Consequence inside this crate's own stated invariant: two concurrent tagging writes against the same record can stamp `updated_at` out of order relative to the order their writes actually landed. Request A samples T1 and blocks on the lock; request B samples T2 > T1, acquires the lock, merges its tags and stamps T2; A then acquires the lock, merges onto B's content and stamps T1 < T2. The record's stored content is the one A produced, but the "last written" instant it reports is older than that content — exactly what `DeploymentParameterRecord::mark_written` says cannot happen. The same prologue is copied in `put_deployment_parameter.rs` (sample, then lock), so a `PutDeploymentParameter` in-place rewrite races the tagging writes the same way.

No operation of this model reports `updated_at` on the wire, so there is no response-visible symptom today; the defect is that the sequence does not hold the invariant its documentation asserts, and the fix (take the sample after acquiring the lock) changes no response and no existing test, which are all single-threaded.

## Evidence

The documented rule, `services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs:1075-1078` (`with_tagged_parameter`, step 4 of the sequence):

    /// 4. materialise the deletion the clock has already made due, from ONE clock sample
    ///    taken under the lock
    ///    ([`...State::purge_expired`]),
    ///    so a parameter past its `expirationDate` is gone before the ARN is resolved;

Restated for the write stamp, `state.rs:1124-1126` (`write_tagged_parameter`):

    /// The instant is the sample the sequence already took under the lock for the
    /// expiry it materialises, so the whole call — the deletion the clock made due and
    /// the write this record records — is served at ONE instant.

And again at `state.rs:1152-1154` (`resolve_tagged_parameter`):

    /// [`write_tagged_parameter`] are the reporting and the writing view of: `resolved`
    /// additionally sees the instant the sequence sampled under the lock, which is what
    /// lets a write stamp the record it changed with the instant its own call was
    /// served at.

The code under those comments, `state.rs:1172-1174` — the sample is taken BEFORE `state.lock()`:

    let now = SystemTime::now();
    let mut store = state.lock();
    store.purge_expired(now);

The invariant that this breaks, `state.rs:437-443` (`DeploymentParameterRecord::mark_written`):

    /// [`Self::updated_at`] is the record's "last written" instant, so EVERY
    /// operation that writes the stored record refreshes it from the clock sample
    /// its own call was served at — the content a write leaves and the instant the
    /// record reports for it are one write, so no stored record can report a
    /// "last written" instant older than its own content.

The same sample-then-lock order is duplicated for the write path of the fourth operation, `src/ops/put_deployment_parameter.rs:264-274`:

        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        ...
        store.purge_expired(now);

Test coverage: `tests/all/tag_write_last_written.rs` pins the stamping rule, but only for calls issued one at a time — e.g. `every_tag_write_refreshes_the_records_last_written_instant` asserts `updated_at >= before && updated_at <= after` around a single awaited call (lines 191-204), so no test exercises two writes contending for the lock and none would change if the sample moved under the lock.

## Steps to Reproduce

1. Read `services/.../src/state.rs:1072-1078` and `1124-1126`: the sequence documents the expiry purge and the write stamp as using ONE clock sample 'taken under the lock'.
2. Read `state.rs:1172-1174`: `let now = SystemTime::now();` executes before `let mut store = state.lock();`, so the sample is taken outside the lock.
3. Create a deployment parameter via `PutDeploymentParameter` and note its `resourceArn`.
4. Issue two concurrent `TagResource` calls against that ARN (and/or one `TagResource` plus one `PutDeploymentParameter` in-place rewrite). The handler that samples the clock FIRST but acquires the lock SECOND stores its content last while stamping the earlier instant.
5. Read the record's `updated_at` out of the store (as `tests/all/tag_write_last_written.rs::written_instants` does): it can be older than the instant at which the content it now holds was actually written, which `state.rs:437-443` declares impossible.
