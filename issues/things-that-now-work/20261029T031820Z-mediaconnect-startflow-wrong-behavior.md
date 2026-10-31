Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c0c2239955f7987f5942ca86a100c431d5defb9f

# StartFlow overwrites a stop the clock still owes, silently abandoning an accepted StopFlow (and StopFlow does the mirror)

_Filed: 2026-10-29T03:18:20Z_
_Operation: StartFlow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c0c2239955f7987f5942ca86a100c431d5defb9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartFlow only treats `ACTIVE` and `STARTING` as "already started"; for every other status it calls `begin_transition(STARTING, ACTIVE, now)`, which REPLACES `StoredFlow::pending` outright. A flow that is in `STOPPING` because StopFlow was accepted (202) a moment earlier therefore has its owed `STANDBY` discarded and is driven to `ACTIVE` instead: the stop the caller was told had been accepted never happens, and nothing reports that it was cancelled. StopFlow is the exact mirror — it flips a flow in `STARTING` to `STOPPING` and drops the owed `ACTIVE`.

Every sibling start/stop handler in this same crate refuses exactly this case: the router input/output start and stop handlers check `is_transitioning()` (which is `pending.is_some() || is_deleting()`) and answer the modelled ConflictException, with the stated reason that "a start MOVES the ... state, so it cannot be accepted for ... one the clock already owes a DIFFERENT state". The flow handlers skip that check, although the flow lifecycle uses the identical `pending` / `begin_transition` machinery and the crate already has a refusal envelope for a flow state precondition (`state_precondition_message`, used by DeleteFlow).

## Evidence

The flow handler has no in-flight check — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/start_flow.rs:172-183:
```
        // Starting a flow is not instantaneous: it reports `STARTING` and the clock owes
        // `ACTIVE` one transition span later. A flow that is already running, or already
        // on its way there, keeps the status it has.
        if flow.status != crate::state::STATUS_ACTIVE
            && flow.status != crate::state::STATUS_STARTING
        {
            flow.begin_transition(
                crate::state::STATUS_STARTING,
                crate::state::STATUS_ACTIVE,
                now,
            );
        }
```
and the mirror in src/ops/stop_flow.rs:175-183 (`if flow.status != STANDBY && flow.status != STOPPING { flow.begin_transition(STOPPING, STANDBY, now); }`).

`begin_transition` overwrites the owed change — src/state.rs:4003-4009 (router spelling, identical to the flow's):
```
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now: SystemTime) {
        self.state = transient.to_string();
        self.pending = Some(PendingStatus { status: settled.to_string(), due_at: now + TRANSITION_SPAN });
    }
```

The sibling handlers of this crate DO apply the check — src/ops/start_router_input.rs:194-199 and :230-233:
```
        // A start MOVES the input's state, so it cannot be accepted for an input that is on
        // its way out, or one the clock already owes a DIFFERENT state (a create still
        // settling, a restart still recovering): the input is leaving or between two states,
        // which is the conflict `ConflictException` describes
...
        if !already_started {
            if router_input.is_transitioning() {
                return Err(conflict(router_input).into());
            }
```
The same pair appears in src/ops/stop_router_input.rs:223-226, src/ops/start_router_output.rs:234-237 and src/ops/stop_router_output.rs:224-227.

The crate already words a flow state precondition as a BadRequestException — src/ops/delete_flow.rs:192-203 uses `crate::state::state_precondition_message("Flow", crate::state::STATUS_STANDBY, "DELETE")`.

Test coverage: tests/all/flow_lifecycle.rs:540-582 only walks start-then-settle-then-stop-then-settle, and tests/all/lifecycle_preconditions.rs:236-249 only asserts DeleteFlow is refused during `STARTING`/`STOPPING`. No test pins what a start issued while a stop is in flight does, so nothing in the crate justifies the divergence.

## Steps to Reproduce

1. CreateFlow, then POST /v1/flows/start/{FlowArn}; wait out the transition span so the flow reports ACTIVE.
2. POST /v1/flows/stop/{FlowArn} — 202, status STOPPING; the clock now owes STANDBY.
3. Immediately POST /v1/flows/start/{FlowArn} — observe 202 with status STARTING, where the sibling StartRouterInput/StartRouterOutput answer 409 ConflictException for a resource the clock already owes a different state.
4. Wait out the transition span and DescribeFlow: the flow is ACTIVE. The StopFlow that was accepted with 202 has been silently abandoned — no caller is told, and a following DeleteFlow is refused because the flow is not STANDBY.
5. Symmetrically: start a flow, then POST /v1/flows/stop/{FlowArn} while it is STARTING — the owed ACTIVE is dropped the same way.
