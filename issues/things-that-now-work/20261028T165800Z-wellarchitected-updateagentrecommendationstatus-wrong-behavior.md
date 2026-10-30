Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f965a258785d300a5b0cd6c5f88de592a3c6328

# A repeated UpdateAgentRecommendationStatus re-stamps lastModifiedAt, against the handler's own @idempotent no-op rule and the sibling feedback PUT

_Filed: 2026-10-28T16:58:00Z_
_Operation: UpdateAgentRecommendationStatus | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f965a258785d300a5b0cd6c5f88de592a3c6328`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateAgentRecommendationStatus` is modelled `@idempotent` and carries NO client token, so the only thing that can make a retry indistinguishable from the request it retries is the handler refusing to write when the request asks for the state the recommendation already holds. The handler's own closing comment claims exactly that ("re-sending the same status leaves the recommendation in the state it already reached"), but the write above it is unconditional: `record.status`, `record.update_reason`, `record.last_modified_by` and `record.last_modified_at = Some(now)` are assigned on every call, including one that carries the status (and reason) the record already holds.

Consequence: a caller that retries the identical status update — the retry an `@idempotent` operation exists to make safe — leaves the recommendation reporting a DIFFERENT `lastModifiedAt` than the first call left, so a later `GetAgentRecommendation` / `ListAgentRecommendations` cannot tell the two apart from one real second update. The record did not change in any member a caller asked about, yet the member that reports "when this recommendation was last modified" moved.

The crate already owns this rule and applies it one file over: `PutAgentRecommendationFeedback` — the other `@idempotent`, token-less, ARN-addressed write on the SAME `AgentRecommendationRecord` — computes `already_held` and skips the write ("the instant it was accepted at included") for precisely this reason. `UpdateAgentRecommendationStatus` states the rule and skips the guard.

## Evidence

Side 1 — the handler states the rule and then breaks it. `src/ops/update_agent_recommendation_status.rs:196-213`:

```
        record.status = input.status;
        record.update_reason = input.update_reason;
        // "The identifier of the user or system that last modified this recommendation":
        // every request this emulator serves is attributed to its one account, exactly as
        // the generation that produced the recommendation is.
        record.last_modified_by = Some(::engine::emulator_account_id().to_string());
        record.last_modified_at = Some(now);

        // The response carries no members (`UpdateAgentRecommendationStatusResponse` is an
        // empty structure), so the recorded status is observed through the operations that
        // report the recommendation. `@idempotent`: re-sending the same status leaves the
        // recommendation in the state it already reached.
```

Side 2 — the sibling write of the same record, with the guard the comment above promises. `src/ops/put_agent_recommendation_feedback.rs:223-243`:

```
        // `@idempotent`: a resubmission of the feedback this recommendation already holds
        // leaves it exactly as it stood, the instant it was accepted at included, so a
        // retried request is indistinguishable from the one it retries. Only feedback that
        // differs in a member the caller submitted is a new submission.
        let already_held = record.feedback.as_ref().is_some_and(|held| {
            held.feedback_type == input.r#type
                && held.category == input.feedback_category
                && held.comments == input.comments
        });
        if !already_held {
            record.feedback = Some(crate::state::AgentRecommendationFeedback { ... });
        }
```

The stamped member is observable — `src/state.rs:5656-5659` (`AgentRecommendationRecord::to_summary`): `last_modified_by: self.last_modified_by.clone(), last_modified_at: self.last_modified_at.map(::aws_smithy_types::DateTime::from),` and `src/ops/get_agent_recommendation.rs:319-322`: `last_modified_by: record.last_modified_by.clone(), last_modified_at: record.last_modified_at.map(...)`.

Both operations are modelled the same way, so the divergence is not the model's: `smithy_query(query="operation", operation="UpdateAgentRecommendationStatus")` -> input `{recommendationArn, status, updateReason}`, `"idempotent": true`, no client token; `PutAgentRecommendationFeedback` -> `{recommendationArn, type, feedbackCategory, comments}`, `"idempotent": true`, no client token.

No test pins the divergent behaviour. `tests/all/agent_recommendations.rs:520-524` only asserts that AN update records the member at all: `item.get("lastModifiedAt").is_some_and(|at| !at.is_null()), "case {case:?}: an updated recommendation reports when it was modified"` — each row there is a single update of a distinct recommendation (`tests/all/agent_recommendations.rs:471-485`). The no-op rule for the feedback sibling IS pinned: `tests/all/agent_recommendation_feedback.rs:16-17` — "3. `@idempotent`: resubmitting the feedback a recommendation already holds leaves it exactly as it stood, the instant it was accepted at included."

## Steps to Reproduce

1. Create an agent profile with an aggregation configuration (`POST /api/v1/agent-profiles`) and start a recommendation generation for it (`POST /api/v1/agent-profiles/{profileArn}/recommendation-generations`); let the clock pass the generation's completion instant
2. `GET /api/v1/agent-profiles/{profileArn}/recommendations` to obtain a `recommendationArn`
3. `PATCH /api/v1/agent-recommendations/{recommendationArn}/status` with `{"status":"IN_PROGRESS","updateReason":"picked up"}` — answers 200
4. Read the recommendation back (`GET /api/v1/agent-recommendations/{recommendationArn}`) and note `lastModifiedAt`
5. Re-send the IDENTICAL PATCH (the retry an `@idempotent` operation sanctions), then read the recommendation back again
6. Observe `lastModifiedAt` has moved although no member of the recommendation differs — the opposite of the rule stated at src/ops/update_agent_recommendation_status.rs:210-213, and of the guard the sibling at src/ops/put_agent_recommendation_feedback.rs:227-243 applies to the same record
