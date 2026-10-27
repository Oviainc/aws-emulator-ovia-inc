Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5030ab47f2620aff89acb3d55898eb8626aa6921

# UpdateReward accepts a Rewards list whose length does not match the trajectory's captured turn count

_Filed: 2026-10-24T07:59:08Z_
_Operation: UpdateReward | Class: validation_

> _Note: this issue was identified against the code base at commit `5030ab47f2620aff89acb3d55898eb8626aa6921`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateRewardRequest.Rewards` is documented as "The list of reward values to assign to this trajectory. Provide one reward value per turn in the trajectory." — the per-turn cardinality is part of the modelled contract, and the service does validate the contents of this list (the emulator's own comment records that the real service answers 400 `ValidationException` "Rewards list cannot be empty" for `Rewards: []`, i.e. list content is server-checked rather than passed through).

Expected: a reward submission whose score count disagrees with the number of turns captured on the trajectory is refused as invalid (HTTP 400 `ValidationException`), since the scores cannot be attributed to turns; a submission with exactly one score per captured turn succeeds and transitions the trajectory to reward-received.

Actual: `src/ops/update_reward.rs` only rejects the EMPTY list. Any non-empty length is accepted verbatim and written to `trajectory.rewards` — a trajectory with 3 captured turns accepts `Rewards: [0.5]`, and a trajectory with 1 turn accepts `Rewards: [0.1, 0.2, 0.3, 0.4]`. The handler never reads `trajectory.turns` at all (it only inspects `completion_status` and `trajectory.rewards`), so the turn count the store already holds is never compared against the submission. The mis-sized list then becomes the trajectory's recorded reward state (status flips to `reward-received`) and, because a later call with different scores is refused 409, the wrong-sized submission is permanently sealed in.

## Evidence

Spec: `smithy_query(query="shape", shape="UpdateRewardRequest")` — member `Rewards` targets `DoubleList`, `@required`, doc "The list of reward values to assign to this trajectory. Provide one reward value per turn in the trajectory."; `UpdateReward` doc "Updates the reward values for a trajectory and transitions it to reward-received status... Call this operation after CompleteRollout"; `ValidationException` is modelled for this operation at httpError 400. `DoubleList` declares no `@length`, so the cardinality rule exists only in the documented semantics and must be enforced by the handler.
Handler: `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/update_reward.rs` — the only `Rewards` check is `if input.rewards.is_empty() { return Err(validation(EMPTY_REWARDS_MESSAGE...)) }` (lines ~197-199); the trajectory branch at lines ~232-241 stores `Some(input.rewards.clone())` without consulting `trajectory.turns`.
State: `src/state.rs` already holds the per-turn record needed for the check (`Trajectory.turns`, each `Turn` carrying its `index`), so the comparison is available at the point of the write.
Tests: `tests/all/trajectory_lifecycle.rs::update_reward_records_scores_only_on_a_ready_trajectory` and `tests/all/modelled_rejections.rs` assert the not-found / not-sealed / failed-status / empty-list paths only — no test submits a reward list of a length different from the number of captured turns, so this is uncovered rather than deliberately pinned.

## Steps to Reproduce

1. Make a job ARN known to the runtime (one of the two seeded ARNs, e.g. `arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/agentic-rft-rollout-1`).
2. POST /sample three times with `X-Amzn-SageMaker-Job-Arn: <that ARN>`, `X-Amzn-SageMaker-Trajectory-Id: traj-reward-count` and a small JSON chat payload — three turns are captured on the trajectory.
3. POST /complete-rollout with the same JobArn header and body `{"TrajectoryId":"traj-reward-count","Status":"ready"}` → 200, trajectory sealed.
4. POST /update-reward with the same JobArn header and body `{"TrajectoryId":"traj-reward-count","Rewards":[0.5]}` — one score for three turns.
5. Observe HTTP 200 and the single score recorded as the trajectory's rewards (status `reward-received`), where the documented one-reward-per-turn contract calls for a 400 `ValidationException`.
6. Repeat on a one-turn trajectory with `{"TrajectoryId":"traj-extra","Rewards":[0.1,0.2,0.3,0.4]}` — also 200, showing neither too-few nor too-many scores are checked.
