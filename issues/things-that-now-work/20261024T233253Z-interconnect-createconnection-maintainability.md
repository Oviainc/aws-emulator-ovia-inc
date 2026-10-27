Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# The "bandwidth must be in the Environment's available set" rule is implemented twice, once per writer

_Filed: 2026-10-24T23:32:53Z_
_Operation: CreateConnection | Class: maintainability_

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that a connection's bandwidth has to be one the Environment currently offers (`Environment$bandwidths.available`) is coded independently in two places in this crate: once in `CreateConnection` and once in `UpdateConnection`. Both do the same three things — fetch the environment's `bandwidths.available`, test membership, and build the identical `InterconnectValidationException` message rendering the offered list — with different expressions (`Vec::contains` on a clone vs `iter().any()` on a borrowed slice) and a duplicated format string. `UpdateConnection`'s own comment says it is applying the rule "exactly as on create", which is precisely the coupling that can silently drift: a change to the membership test, the fallback when the environment has no `available` list, or the message on one side leaves the other behind, and the two paths then disagree about which bandwidths a connection may hold.

The crate already centralises its other shared rules in `src/state.rs` (`tag_write_violation`, `environment_not_found_message`, `connection_not_updatable_message`, `is_updatable_state`), and `tag_write_violation`'s own doc states the reason: "Both of this service's tag writers … judge their input here, so one rule decides what a stored tag set can be however it was reached". The bandwidth rule is the one writer-shared rule that was not given that treatment. Folding it into a single state.rs helper leaves every response byte-identical and both existing tests passing.

## Evidence

Copy 1 — services/service-com-amazonaws-interconnect-interconnect/src/ops/create_connection.rs:238-254:
```
// `Environment$bandwidths.available` is "the list of currently available
// bandwidths": a bandwidth outside it cannot be allocated …
let available = environment.bandwidths.available.clone().unwrap_or_default();
if !available.contains(&input.bandwidth) {
    return Err(CreateConnectionErrorModeled::InterconnectValidationException(
        InterconnectValidationException {
            message: format!(
                "bandwidth '{}' is not available on environment '{}'; available bandwidths are [{}]",
                input.bandwidth, environment.environment_id, available.join(", ")
```
Copy 2 — services/service-com-amazonaws-interconnect-interconnect/src/ops/update_connection.rs:197-215:
```
// A new bandwidth is allocated on the SAME environment, so it is held to
// that environment's currently available set, exactly as on create.
if let Some(bandwidth) = input.bandwidth.as_deref() {
    let available = store.environments.get(&environment_id)
        .and_then(|environment| environment.bandwidths.available.as_deref())
        .unwrap_or_default();
    if !available.iter().any(|offered| offered == bandwidth) {
        return Err(UpdateConnectionErrorModeled::InterconnectValidationException(
            InterconnectValidationException {
                message: format!(
                    "bandwidth '{bandwidth}' is not available on environment '{environment_id}'; available bandwidths are [{}]",
                    available.join(", ")
```
The single-helper precedent in the same crate — src/state.rs:572-581 (`tag_write_violation`): "Both of this service's tag writers (tag-on-create and the tagging operation) judge their input here, so one rule decides what a stored tag set can be however it was reached".

Existing tests that pin both copies' behaviour (and must stay unchanged): tests/all/connection_lifecycle.rs `create_connection_refuses_what_the_environment_does_not_offer` (line 746) and `update_connection_refuses_an_unknown_or_unofferable_change` (lines 1289-1319, which asserts HTTP 400 for every `supported` but not `available` bandwidth plus "40Gbps", and that the record is left alone).

## Steps to Reproduce

1. Read services/service-com-amazonaws-interconnect-interconnect/src/ops/create_connection.rs:238-254 and src/ops/update_connection.rs:197-215: the same availability rule and the same message text are written twice.
2. Note src/state.rs houses every other rule the crate shares between writers (tag_write_violation at line 581, is_updatable_state at line 615, connection_not_updatable_message at line 623) — there is no bandwidth equivalent.
3. Change one copy (e.g. the `unwrap_or_default()` fallback for an environment with no `available` list, or the message's rendering) and observe that nothing forces the other copy to follow: create and update would then disagree about which bandwidths a connection may hold.
4. A single `state.rs` helper taking (environment, bandwidth) and returning the same Option<String> message keeps both responses and both tests above identical.
