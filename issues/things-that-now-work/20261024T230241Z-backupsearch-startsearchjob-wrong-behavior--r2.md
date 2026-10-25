Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# Owed-status due instants add a Duration to SystemTime unguarded, while the same handler guards the identical addition with checked_add

_Filed: 2026-10-24T23:02:41Z_
_Operation: StartSearchJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate computes four "due instant" values by adding a `Duration` to the request's `SystemTime`. One of those additions — the `ClientToken` validity window — is deliberately computed FALLIBLY with `checked_add`, and `src/state.rs` documents exactly why: `SystemTime + Duration` panics when the sum is not representable, and aborting the handler is the wrong answer. The other three use the panicking `+` operator on the very same `now` value, two of them on the lines immediately following the guarded call in `start_search_job.rs`.

So the crate states a rule about this operation ("the window's end is computed fallibly ... rather than aborting the handler") and then breaks it three lines later for the owed-status queue, and again in `stop_search_job.rs` and `start_search_result_export_job.rs`. Where `ClientToken::minted` degrades gracefully, the owed-status computation aborts the request with an unmodelled panic instead of a modelled answer. There is no behavioural impact under a sane clock — hence LOW — but the inconsistency is exactly the one the crate's own comment says must not exist, and the guarded and unguarded forms sit side by side where they can drift further.

## Evidence

The guarded side — services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs:212-222:
```
    /// Adding a duration to a `SystemTime` panics when the sum is not representable, so
    /// the window's end is computed fallibly and an unrepresentable one is taken as a
    /// window that has ALREADY closed: a clock far enough ahead makes the next call under
    /// the token a new request rather than aborting the handler.
    pub fn minted(value: String, completed: SystemTime) -> Self {
        Self {
            value,
            expires_at: completed
                .checked_add(CLIENT_TOKEN_VALIDITY)
                .unwrap_or(completed),
        }
    }
```

The unguarded siblings, all adding to the same `now: SystemTime` the handler took at entry — src/ops/start_search_job.rs:350-362 (the `checked_add` call at :353 and the bare `+` at :356 and :358 are in the SAME struct literal):
```
            client_token: input
                .client_token
                .clone()
                .map(|token| crate::state::ClientToken::minted(token, now)),
            request_fingerprint: fingerprint,
            owed_statuses: VecDeque::from([
                (now + crate::state::CREATING_DELAY, crate::state::RUNNING, None),
                (
                    now + crate::state::TRANSITION_DELAY,
                    owed_status,
                    owed_message,
                ),
            ]),
```
src/ops/stop_search_job.rs:205-209: `job.owe_only(\n            now + crate::state::TRANSITION_DELAY,\n            crate::state::STOPPED,\n            None,\n        );`
src/ops/start_search_result_export_job.rs:406-410: `next_status_due: Some((\n                now + crate::state::TRANSITION_DELAY,\n                owed_status,\n                owed_message,\n            )),`

Test coverage: the owed-status machinery is exercised by tests/all/search_job_status_phases.rs and tests/all/search_job_outcome_and_paging.rs (CREATING -> RUNNING -> terminal) and the token window by tests/all/client_token_window.rs; none of them exercises the non-representable sum, so making the three additions fallible the way `ClientToken::minted` already is leaves every response and every assertion unchanged.

## Steps to Reproduce

1. Read src/state.rs:212-222 and note that `ClientToken::minted` documents that `SystemTime + Duration` panics on a non-representable sum and therefore uses `checked_add(..).unwrap_or(completed)`.
2. Read src/ops/start_search_job.rs:350-362: the same handler calls `ClientToken::minted(token, now)` (guarded) and, in the same struct literal, builds `owed_statuses` with `now + crate::state::CREATING_DELAY` and `now + crate::state::TRANSITION_DELAY` (unguarded).
3. Read src/ops/stop_search_job.rs:205-209 and src/ops/start_search_result_export_job.rs:406-410 for the same unguarded `now + TRANSITION_DELAY`.
4. Observe that for a `now` near the representable maximum of `SystemTime`, the token window degrades to "already closed" as documented, while the three owed-status additions panic and the request is aborted instead of being answered with a modelled response.
