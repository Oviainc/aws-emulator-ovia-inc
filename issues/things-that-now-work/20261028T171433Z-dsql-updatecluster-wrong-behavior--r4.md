Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 68e4a9e742a6835063083b9abc795c05ac8a482e

# An idempotency replay whose record lacks the member it answered serves the LIVE status instead of refusing, against the state module's own stated rule

_Filed: 2026-10-28T17:14:33Z_
_Operation: UpdateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `68e4a9e742a6835063083b9abc795c05ac8a482e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `@idempotencyToken` handler of this crate serves a recognised retry from `RecordedCall`, and the state module states exactly what a handler must do when the record does not carry a member the original call answered: refuse, never answer a re-derived value. Five of the seven replay paths honour that rule and answer the modelled `InternalServerException`. Two do not:

* `UpdateCluster`'s replay reads `recorded.answered(STATUS_MEMBER)` inside an `if let Some(..)`, so a record without `status` silently leaves `answer.status` at the value `updated_cluster_output(existing)` read off the LIVE cluster — the very value the two lines of comment directly above it say is not this request's result ("the clock settles the cluster back to `ACTIVE` and a later update moves it again, neither of which is this request's result"). The retry then answers 200 with a status the first call never produced instead of refusing.
* `CreateStream`'s replay has the identical shape and the identical defect for the stream's `status`.

The effect is a 200 carrying a value the handler's own comment declares wrong, where the sibling paths refuse — i.e. the two copies have already drifted from the rule the state module spells out, and nothing stops a future change to what a handler records from turning this latent path into a served wrong answer.

## Evidence

The rule, stated by the state module that owns the record (`src/state.rs:546-551`, `RecordedCall::answered`):

> /// What the call answered for `member`, or `None` when it recorded no value
> /// for it — which is the case for every member the handler renders from
> /// [`Self::resource`]'s stored record instead ([`Self::answering`] is the
> /// only writer). `None` is never a value to serve: a handler that recorded a
> /// member has to find it, so one that does not must refuse rather than
> /// answer an empty one.

The two handlers that break it:

`src/ops/update_cluster.rs:272-282`:
```rust
crate::state::TokenVerdict::Replay(recorded) => {
    if let Some(existing) = state.clusters.get(&recorded.resource) {
        let mut answer = updated_cluster_output(existing);
        // The status this call answered: the clock settles the
        // cluster back to `ACTIVE` and a later update moves it
        // again, neither of which is this request's result.
        if let Some(status) = recorded.answered(crate::state::STATUS_MEMBER) {
            answer.status = status.to_string();
        }
        return Ok(answer);
    }
}
```

`src/ops/create_stream.rs:275-283`:
```rust
let mut answer = created_stream_output(&input.cluster_identifier, existing);
// The status this call answered: the clock settles the
// stream to `ACTIVE` afterwards, which is not what this
// request produced.
if let Some(status) = recorded.answered(crate::state::STATUS_MEMBER) {
    answer.status = status.to_string();
}
return Ok(answer);
```

The siblings that honour the rule, each refusing instead of answering:

* `src/ops/create_cluster.rs:308-322`: "// A served create always keeps what it answered, so / // there is nothing to serve here and nothing this / // request may mint in its place." → `return Err(CreateClusterErrorModeled::InternalServerException(InternalServerException { message: "the recorded result of this request is unavailable" .. }))`
* `src/ops/put_cluster_policy.rs:211-224`: `let Some(version) = recorded.answered(crate::state::POLICY_VERSION_MEMBER) else { /* "A recorded write always keeps the version it issued, so there is nothing to serve here" */ return Err(..InternalServerException..) };`
* `src/ops/delete_cluster_policy.rs:191-204`: same `else { return Err(..InternalServerException..) }`
* `src/ops/delete_cluster.rs:211-223` and `src/ops/delete_stream.rs:205-219`: `recorded_delete_output(..).ok_or_else(|| .. InternalServerException { message: "the recorded result of this request is unavailable" .. })`

Test coverage: `tests/all/idempotent_replay_result.rs:394` (`a_minting_call_retry_answers_the_status_the_first_call_answered`) drives the `CreateCluster` / `UpdateCluster` / `CreateStream` replays only with a complete record, so the branch above is unpinned for both handlers, while the refusal the siblings give is the crate's stated contract.

## Steps to Reproduce

1. Read `src/state.rs:546-551`: `RecordedCall::answered` documents that `None` "is never a value to serve: a handler that recorded a member has to find it, so one that does not must refuse rather than answer an empty one".
2. Read `src/ops/update_cluster.rs:272-282` and `src/ops/create_stream.rs:275-283`: both wrap `recorded.answered(STATUS_MEMBER)` in `if let Some(..)` and fall through to `return Ok(answer)` with the status read off the live record.
3. Read `src/ops/create_cluster.rs:308-322`, `src/ops/put_cluster_policy.rs:211-224`, `src/ops/delete_cluster_policy.rs:191-204`, `src/ops/delete_cluster.rs:211-223`, `src/ops/delete_stream.rs:205-219`: each uses `let .. else` / `ok_or_else` to answer `InternalServerException` for exactly the same case.
4. Reproduce the served wrong answer by handing the `UpdateCluster` ledger a `RecordedCall::new(identifier)` with no `status` answer (as `RecordedCall::new` alone builds) under a token, then retrying the same request after the clock has settled the cluster: the retry answers 200 with the cluster's CURRENT status rather than refusing.
