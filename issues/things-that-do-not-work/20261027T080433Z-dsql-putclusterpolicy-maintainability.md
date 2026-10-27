Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 01d4c565076fad2daa34afee2942eef023c65029

# The recorded policy-version replay answer is implemented twice, once in PutClusterPolicy and once in DeleteClusterPolicy

_Filed: 2026-10-27T08:04:33Z_
_Operation: PutClusterPolicy | Class: maintainability_

> _Note: this issue was identified against the code base at commit `01d4c565076fad2daa34afee2942eef023c65029`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both policy writers read back the same recorded answer on a `clientToken` replay — the `policyVersion` the first call issued — and both refuse an incomplete record with the same `InternalServerException` and the same sentence. The rule (read `POLICY_VERSION_MEMBER` off the `RecordedCall`; a record missing it is refused rather than answered with an empty member; otherwise answer it) is written out twice, so the two copies can drift: one could start answering an empty version, or word the refusal differently, while the other does not.

This crate already centralised exactly this rule for its other pair of replay-answering handlers — `crate::state::RecordedDelete::{into_record, from_record}` — and `src/state.rs` states that as the module's organising principle. The same codec (a `RecordedPolicyWrite`-style `from_record` in `state.rs` returning the recorded version, each handler mapping `None` to its own operation's `InternalServerException`, as the two deletes already do with `RecordedDelete::from_record`) removes the second copy without changing any response: both handlers answer the identical body today, and the existing tests continue to pin it.

## Evidence

Copy 1 — `src/ops/put_cluster_policy.rs:204-228`:
```
                crate::state::TokenVerdict::Replay(recorded) => {
...
                    let Some(version) = recorded.answered(crate::state::POLICY_VERSION_MEMBER)
                    else {
                        // A recorded write always keeps the version it issued,
                        // so there is nothing to serve here and nothing this
                        // request may attach in its place.
                        return Err(PutClusterPolicyErrorModeled::InternalServerException(
                            InternalServerException {
                                message: "the recorded result of this request is unavailable"
                                    .to_string(),
                                retry_after_seconds: None,
                            },
                        )
                        .into());
                    };
                    return Ok(PutClusterPolicyOutput {
                        policy_version: version.to_string(),
                    });
```

Copy 2 — `src/ops/delete_cluster_policy.rs:186-208`:
```
                crate::state::TokenVerdict::Replay(recorded) => {
...
                    let Some(version) = recorded.answered(crate::state::POLICY_VERSION_MEMBER)
                    else {
                        // A recorded delete always keeps the version it removed,
                        // so there is nothing to serve here and nothing this
                        // request may remove in its place.
                        return Err(DeleteClusterPolicyErrorModeled::InternalServerException(
                            InternalServerException {
                                message: "the recorded result of this request is unavailable"
                                    .to_string(),
                                retry_after_seconds: None,
                            },
                        )
                        .into());
                    };
                    return Ok(DeleteClusterPolicyOutput {
                        policy_version: version.to_string(),
                    });
```

The crate's own stated principle for the parallel case — `src/state.rs:26-30`:
```
//! builds. The answer a served delete keeps for its retry
//! ([`RecordedDelete`]) lives here for the same reason: both deletes record and
//! read back the same members, so the encoding and the refusal of an incomplete
//! record are one implementation, parameterised by the output each handler
//! builds.
```
and the shared codec it refers to, `src/state.rs:2162-2183` (`RecordedDelete::into_record` / `from_record`).

Tests that pin the responses this must leave unchanged: `tests/all/cluster_policy.rs:10-11` ("a policy write's `clientToken` serves ONE request — a replay of that request answers from it, a different request under the same token is refused"), `tests/all/idempotent_replay_result.rs:216` (`a PutClusterPolicy retry under a clientToken the service already [served]`) and its `DeleteClusterPolicy` rows, and `tests/all/policy_version_conflict_and_lockout.rs`.

## Steps to Reproduce

1. Open `src/ops/put_cluster_policy.rs:204-228` and `src/ops/delete_cluster_policy.rs:186-208` side by side: the same read of `POLICY_VERSION_MEMBER`, the same `InternalServerException` with the same sentence, and the same single-member output.
2. Compare with `src/state.rs:2162-2183`, where the identical rule for the two DELETE handlers is one shared codec, and with `src/state.rs:26-30`, which states that principle.
3. Change the refusal sentence in one of the two policy handlers: no test fails for the other, and the two replay paths now word the same condition differently.
