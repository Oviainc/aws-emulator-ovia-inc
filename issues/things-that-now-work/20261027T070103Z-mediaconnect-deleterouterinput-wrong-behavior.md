Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# DeleteRouterInput leaves the pending state change in place, so a deleted router input settles back into ACTIVE/STANDBY instead of staying DELETING

_Filed: 2026-10-27T07:01:03Z_
_Operation: DeleteRouterInput | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StoredRouterInput::begin_deletion` writes `state = DELETING` and `removal_due_at`, but it does NOT clear `self.pending` — the state change an earlier StartRouterInput / RestartRouterInput / UpdateRouterInput left owed. `settle_due_transitions` calls `settle(now)` on every router input unconditionally (before the removal retain), and `settle` overwrites `state` with `pending.status` as soon as that pending instant comes due. Because the pending transition was started BEFORE the delete, its `due_at` is earlier than `removal_due_at`, so there is a window in which the record the delete already accepted reports a RUNNING state: GetRouterInput, BatchGetRouterInput and ListRouterInputs answer `ACTIVE` (or `STANDBY`/`RECOVERING`, whatever was owed) for an input whose DeleteRouterInput response said `DELETING`, and the DELETING state the delete reported is lost.

The crate's two sibling deletions do the opposite, each with a comment saying exactly why: DeleteRouterOutput clears `pending` inline, and `StoredRouterNetworkInterface::begin_deletion` clears `pending` and moves `updated_at`. The router-input path also skips the `updated_at = now` both siblings perform, so the record's `UpdatedAt` does not move for an accepted delete.

## Evidence

Router input deletion does not cancel the owed transition — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:3874-3880:
```
    /// Enter `DELETING` now and owe the record's removal once [`TRANSITION_SPAN`] has
    /// passed — the deletion counterpart of [`Self::begin_transition`], which owes a STATE
    /// instead.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.state = ROUTER_INPUT_STATE_DELETING.to_string();
        self.removal_due_at = Some(now + TRANSITION_SPAN);
    }
```
The sibling router OUTPUT delete cancels it, and says why — src/ops/delete_router_output.rs:202-209:
```
        if !router_output.is_deleting() {
            router_output.state = crate::state::ROUTER_OUTPUT_STATE_DELETING.to_string();
            // The delete supersedes whatever state change was owed before it: a router
            // output on its way out does not settle into `STANDBY` on the way.
            router_output.pending = None;
            router_output.removal_due_at = Some(now + crate::state::TRANSITION_SPAN);
            router_output.updated_at = now;
        }
```
The sibling network interface does the same — src/state.rs:4657-4666:
```
    /// ... The delete supersedes whatever state change was owed before it: an interface
    /// on its way out does not settle into `ACTIVE` on the way.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.state = ROUTER_NETWORK_INTERFACE_STATE_DELETING.to_string();
        self.pending = None;
        self.removal_due_at = Some(now + TRANSITION_SPAN);
        self.updated_at = now;
    }
```
The owed state is still materialised for a deleting input — src/state.rs:5280-5282 and 5299-5300:
```
        for router_input in self.router_inputs.values_mut() {
            router_input.settle(now);
        }
...
        self.router_inputs
            .retain(|_, router_input| !router_input.is_removal_due(now));
```
The delete path that calls it — src/ops/delete_router_input.rs:199-201:
```
        if !router_input.is_deleting() {
            router_input.begin_deletion(now);
        }
```
A pending transition is what StartRouterInput leaves behind — src/ops/start_router_input.rs:238-244 (`begin_transition(STARTING, ACTIVE, now)`), with `TRANSITION_SPAN = Duration::from_millis(500)` (src/state.rs:336).

Test coverage: the router-input deletion suite seeds every record with `pending: None` (tests/all/router_input_deletion.rs:118), and tests/all/get_router_input.rs:111-189 has `owe_transition` / `owe_removal` helpers but no case that combines them — so no existing test pins the current behaviour.

## Steps to Reproduce

1. CreateRouterInput, then StartRouterInput on the returned ARN: the record is now `STARTING` with `pending = {status: ACTIVE, due_at: now + 500 ms}` (src/ops/start_router_input.rs:238).
2. Within that 500 ms window call DeleteRouterInput on the same ARN: it answers 200 with `state: DELETING` and sets `removal_due_at = delete_instant + 500 ms`, leaving `pending` untouched.
3. Wait until the START's `due_at` has passed but the delete's `removal_due_at` has not (e.g. delete at t0+100 ms, read at t0+550 ms), then call GetRouterInput on the ARN.
4. Observe `routerInput.state == "ACTIVE"` for an input whose delete was already accepted (and `UpdatedAt` unchanged by the delete); the same read for a router OUTPUT or a router NETWORK INTERFACE deleted the same way keeps reporting `DELETING`, because those two paths clear `pending`.
