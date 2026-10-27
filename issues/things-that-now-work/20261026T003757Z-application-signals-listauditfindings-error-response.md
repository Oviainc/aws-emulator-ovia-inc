Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 232a76c0adfd6241e7fe920f3ef518cb863b87b4

# The shared page cursor's NextToken refusal echoes the caller's token, breaking the crate's own no-echo rule for ValidationException messages

_Filed: 2026-10-26T00:37:57Z_
_Operation: ListAuditFindings | Class: error-response_

> _Note: this issue was identified against the code base at commit `232a76c0adfd6241e7fe920f3ef518cb863b87b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::page_of` words the refusal of a `NextToken` this service did not issue by interpolating the caller's token into the message. The crate's single constraint-violation builder documents the opposite rule — a `ValidationException` message states the member and the requirement and NOTHING of the rejected value — and every other `NextToken` refusal in this crate (four sibling handlers) follows it.

So the same failure is worded two different ways depending on which read the caller hit, and the reads that go through `page_of` copy caller input into every client log that records the failure, which is exactly what the stated rule exists to prevent.

The `page_of` wording affects `ListAuditFindings`, `ListEntityEvents`, `ListServices`, `ListServiceOperations`, `ListServiceDependencies`, `ListServiceDependents`, `ListServiceStates` and `ListServiceLevelObjectiveExclusionWindows` — every read of this service whose cursor is an offset.

## Evidence

The rule the crate states for a `ValidationException` message — `services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:719-722`:

    /// The clause names the member path and the requirement and NOTHING of the value that was
    /// rejected: `ValidationException` carries only `message`, so this text is the whole
    /// machine-readable detail a client gets, and the rejected value is caller input that would
    /// otherwise be copied into every client log recording the failure.

The shared cursor breaks it — `src/state.rs:1374-1378`:

        let offset = match token {
            None => 0,
            Some(token) => decode_cursor(tag, token)
                .and_then(|cursor| cursor.parse::<usize>().ok())
                .ok_or_else(|| format!("NextToken `{token}` is not a token this service issued"))?,

Four sibling refusals of the very same failure carry no echo:

- `src/ops/list_service_level_objectives.rs:195-197`: `return Err(crate::scaffolded::Scaffolded::validation("NextToken is not a token this service issued"))`
- `src/ops/list_grouping_attribute_definitions.rs:169-173`: `"NextToken is not a token this service issued"`
- `src/ops/list_instrumentation_configurations.rs:215-218`: `"NextToken is not a token this service issued for this service, environment and instrumentation type"`
- `src/ops/get_instrumentation_configuration_status.rs:248-251`: `"NextToken is not a token this service issued for this configuration, status and time range"`

The suite already pins the no-echo rule for the other refusal this service builds itself — `tests/all/max_results_range.rs:229-232`:

            assert!(
                !message.contains(&max_results.to_string()),
                "case {case:?}: the message must not echo the rejected value, got {message:?}"
            );

No test asserts the echoed `page_of` wording (`grep "is not a token"` over `tests/` matches only a case label at `tests/all/instrumentation_configuration_status.rs:973`).

## Steps to Reproduce

1. Call `POST /auditFindings?StartTime=<t0>&EndTime=<t1>` with body `{"AuditTargets":[{"Type":"service","Data":{"Service":{"Type":"Service","Name":"checkout","Environment":"prod"}}}],"NextToken":"not-a-token"}`.
2. Observe the 400 body: `message` is "NextToken `not-a-token` is not a token this service issued" — it contains the rejected value.
3. Call `POST /slos?NextToken=not-a-token` (`ListServiceLevelObjectives`), the same failure on a sibling read.
4. Observe the 400 body: `message` is "NextToken is not a token this service issued" — no echo.
5. Expected: one wording for one failure, with no echo of caller input, as `src/state.rs:719-722` states and `tests/all/max_results_range.rs:229-232` pins for the sibling refusal this service builds.
