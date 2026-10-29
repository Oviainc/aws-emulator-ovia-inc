Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# The status-only recorded replay answer is implemented twice, once in UpdateCluster and once in CreateStream

_Filed: 2026-10-29T06:46:32Z_
_Operation: UpdateCluster | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, and three times applies, the rule that the answer a served `@idempotencyToken` call keeps for its retry — the encoding, the read-back, and the refusal of a record that does not hold a member the original call answered — is ONE implementation in `crate::state`, parameterised by the output each handler builds: `RecordedDelete` for the two deletes, `RecordedPolicyWrite` for the two policy writers, `RecordedClusterCreation` for `CreateCluster`.

The remaining pair — `UpdateCluster` and `CreateStream`, whose record holds exactly one member, the `status` the call answered — is the one case that was never factored. Each handler hand-rolls the same three steps against `RecordedCall`: write `RecordedCall::new(<resource>).answering(STATUS_MEMBER, <answered status>)`, read it back with `recorded.answered(STATUS_MEMBER)`, and refuse with `crate::state::recorded_result_unavailable()` when the record holds no value. Two copies of one rule, which can drift exactly as the already-fixed copies of the delete codec and the policy-version codec could: reverting either copy to the live resource's status is the defect `20261028T171433Z-dsql-updatecluster-wrong-behavior--r4` ("An idempotency replay whose record lacks the member it answered serves the LIVE status instead of refusing") already had to be fixed once, and nothing today forces the other handler to decide that case the same way.

The fix is the one the crate has already made three times: a `state`-side codec for the status-only record (e.g. `RecordedStatus { status }` with `into_record(resource)` / `from_record`) that both handlers call, each keeping only the shape of its own output. It produces byte-identical records (`RecordedCall::new(..).answering(STATUS_MEMBER, ..)`) and the identical refusal, so every response and every existing test stays unchanged.

## Evidence

The rule the state module states for itself — `services/service-com-amazonaws-dsql-dsql/src/state.rs:28-37`:

"The answer a served delete keeps for its retry ([`RecordedDelete`]) lives here for the same reason: both deletes record and read back the same members, so the encoding and the refusal of an incomplete record are one implementation, parameterised by the output each handler builds. The answer a served policy write keeps ([`RecordedPolicyWrite`]) lives here on the same terms — both policy writers record and read back the same version member — and so does the answer a served `CreateCluster` keeps ([`RecordedClusterCreation`]), as does the refusal every replay path gives for a record that does not hold a member the original call answered ([`recorded_result_unavailable`]), so no two of them can decide that case differently."

Copy 1, read side — `src/ops/update_cluster.rs:274-288`:
```rust
                        let mut answer = updated_cluster_output(existing);
                        // The status this call answered: the clock settles the
                        // cluster back to `ACTIVE` and a later update moves it
                        // again, neither of which is this request's result. A
                        // served update always records that status, so a record
                        // without it is refused rather than answered with the
                        // live cluster's.
                        let Some(status) = recorded.answered(crate::state::STATUS_MEMBER) else {
                            return Err(UpdateClusterErrorModeled::InternalServerException(
                                crate::state::recorded_result_unavailable(),
                            )
                            .into());
                        };
                        answer.status = status.to_string();
                        return Ok(answer);
```
and its write side — `src/ops/update_cluster.rs:365-374`:
```rust
        if let Some(token) = input.client_token {
            state
                .tokens(crate::state::TokenScope::UpdateCluster)
                .record(
                    token,
                    fingerprint,
                    crate::state::RecordedCall::new(&input.identifier)
                        .answering(crate::state::STATUS_MEMBER, &updated.status),
                );
```

Copy 2, read side — `src/ops/create_stream.rs:275-289`:
```rust
                        let mut answer =
                            created_stream_output(&input.cluster_identifier, existing);
                        // The status this call answered: the clock settles the
                        // stream to `ACTIVE` afterwards, which is not what this
                        // request produced. A served create always records that
                        // status, so a record without it is refused rather than
                        // answered with the live stream's.
                        let Some(status) = recorded.answered(crate::state::STATUS_MEMBER) else {
                            return Err(CreateStreamErrorModeled::InternalServerException(
                                crate::state::recorded_result_unavailable(),
                            )
                            .into());
                        };
                        answer.status = status.to_string();
                        return Ok(answer);
```
and its write side — `src/ops/create_stream.rs:367-376`:
```rust
        if let Some(token) = input.client_token {
            state
                .tokens(crate::state::TokenScope::CreateStream)
                .record(
                    token,
                    fingerprint,
                    crate::state::RecordedCall::new(&stream_identifier)
                        .answering(crate::state::STATUS_MEMBER, &created.status),
                );
```

The three sibling codecs that already live in one place — `src/state.rs:2448-2468` (`impl RecordedDelete { fn into_record(self) -> RecordedCall { RecordedCall::new(&self.resource).answering(STATUS_MEMBER, &self.status)... } fn from_record(recorded: &RecordedCall) -> Option<Self>`), `src/state.rs:2490-2506` (`impl RecordedPolicyWrite { fn into_record(self, resource: &str) ... fn from_record(...) -> Option<Self>`), `src/state.rs:2530-2568` (`impl RecordedClusterCreation { fn into_record(self, resource: &str) ... fn from_record(...) -> Option<Self>`).

Test coverage that pins both copies' behaviour and must stay green unchanged: `tests/all/incomplete_replay_record.rs:406-481` (`an_incomplete_replay_record_is_refused_by_every_replay_path`, whose table includes `Replay::UpdateCluster` and `Replay::CreateStream` and builds the expected record as `RecordedCall::new(target.resource()).answering(which.recorded_member(), which.recorded_value())` at lines 284-296) and `tests/all/idempotent_replay_result.rs:391-395` (`a_minting_call_retry_answers_the_status_the_first_call_answered`).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-dsql-dsql/src/state.rs:28-37` and read the stated rule: the answer a served call keeps for its retry — and the refusal of a record missing a member — is one implementation per shape in `state`, "so no two of them can decide that case differently".
2. Confirm the three codecs that follow it: `RecordedDelete` (state.rs:2448-2468), `RecordedPolicyWrite` (state.rs:2490-2506), `RecordedClusterCreation` (state.rs:2530-2568).
3. Open `src/ops/update_cluster.rs:274-288` and `src/ops/update_cluster.rs:365-374`: the status-only record is written and read back inline, not through a `state` codec.
4. Open `src/ops/create_stream.rs:275-289` and `src/ops/create_stream.rs:367-376`: the same three steps again, the second copy of the same rule.
5. Note that each copy independently decides what a record with no `status` means; `20261028T171433Z-dsql-updatecluster-wrong-behavior--r4` is that decision having already been wrong in one of them.
6. Fold both into one `state`-side codec (e.g. `RecordedStatus`) producing the identical `RecordedCall` and the identical `recorded_result_unavailable()` refusal, then re-run `cargo test -p service-com-amazonaws-dsql-dsql` — `tests/all/incomplete_replay_record.rs` and `tests/all/idempotent_replay_result.rs` must pass unchanged.
