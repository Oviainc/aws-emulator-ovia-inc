Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# DeleteEnvironment removes the environment synchronously, so the DELETING status is never observable

_Filed: 2026-10-24T00:24:30Z_
_Operation: DeleteEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteEnvironment drops the record (and its ARN index entry) inside the call, so the environment vanishes in the same instant the 200 is returned.

Expected (real MWAA): deletion is asynchronous. The call returns 200 immediately, but the environment remains readable with `Status: "DELETING"` ("The request to delete the environment is in progress" — the EnvironmentStatus doc) and still appears in ListEnvironments until teardown finishes; only then does GetEnvironment answer 404 ResourceNotFoundException. A client that polls GetEnvironment/ListEnvironments to wait for teardown observes DELETING at least once.

Actual (emulator): `store.remove(&input.name)` is performed in the handler, so the very next GetEnvironment answers 404 and ListEnvironments no longer names the environment. `EnvironmentStatus` values DELETING and DELETED are unreachable in this emulator, and a client's "wait until deleted" loop never sees the transient state (it may also mis-handle the immediate 404 as "never existed"). This is the mirror image of create/update, which the emulator already models as a clock-owed transition via `PendingTransition` / `settle` — deletion should go through the same mechanism (enter DELETING, then disappear when the transition comes due).

## Evidence

Spec: EnvironmentStatus enumerates CREATING, …, DELETING ("The request to delete the environment is in progress"), DELETED; DeleteEnvironment is modeled as returning an empty 200 with the environment teardown happening out of band. Handler: src/ops/delete_environment.rs:145-155 takes the lock, calls `store.settle(now)` and then `store.remove(&input.name)` immediately. State: src/state.rs already carries `PendingTransition`/`settle` and a `TRANSITION_SPAN` used for CREATING->AVAILABLE and UPDATING->AVAILABLE, but nothing ever sets a DELETING status. No test asserts a DELETING observation; tests/all/environment_lifecycle.rs asserts the record is gone right after delete.

## Steps to Reproduce

1. Boot the emulator; create environment 'DelEnv' and let it reach AVAILABLE.
2. mwaa.delete_environment(Name='DelEnv') -> 200.
3. Immediately call mwaa.get_environment(Name='DelEnv') and mwaa.list_environments().
4. Expected: GetEnvironment returns the environment with Status='DELETING' (and the name still listed) for the teardown window, and only afterwards 404 ResourceNotFoundException.
5. Observed: GetEnvironment answers 404 ResourceNotFoundException at once and ListEnvironments no longer contains 'DelEnv'; no DELETING status is ever reported.
