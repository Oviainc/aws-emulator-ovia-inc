Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# The uplinkEchoConfig typed-reference rule is written twice, once in CreateConfig and once in UpdateConfig

_Filed: 2026-10-27T23:34:38Z_
_Operation: CreateConfig | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "an `uplinkEchoConfig`'s `antennaUplinkConfigArn` must resolve to a config of capability type `antenna-uplink`, an unresolvable ARN answering 434 `ResourceNotFoundException` and a resolvable ARN of another type answering 431 `InvalidParameterException`" is implemented twice in this crate, in two verbatim copies: the `ECHOED_TYPE` constant that names the admissible capability, the `if let ConfigTypeData::uplinkEchoConfig(..)` branch that decides when the rule applies, and the three-arm match on `typed_reference_fault` that maps each fault to its error shape, message helper and `parameter_name` literal. Only the innermost resolution (`typed_reference_fault`) is shared; the rule around it is not, so the two copies can drift — a change to which capability an echo config may name, or to which of the two faults answers which error, has to be made in both handlers or the create and the update stop agreeing.

This is the one place in the crate where a rule common to a create and its update is NOT lifted into `crate::state`: the dataflow-endpoint-group creates share `check_endpoint_ports` and `claim_endpoint_name` for exactly this reason, the config creates/updates share `validate_config_data`, every tag-taking operation shares `validate_tags`, and all three window-filtered listings share `validate_search_window`. The fix is the same shape: one helper in `crate::state` that answers the fault (or the ready-made `InvalidParameterException`/not-found message pair) and lets each handler lift it into its own error enum, leaving every response byte and every existing test unchanged.

## Evidence

Copy 1 — `src/ops/create_config.rs:221-258`:
```
        if let ConfigTypeData::uplinkEchoConfig(echo) = &stored.config_data {
            // The one capability type an `uplinkEchoConfig`'s reference may name — the
            // member's own name and the `antenna-uplink` segment of a `ConfigArn` are
            // what say so.
            const ECHOED_TYPE: &str = "antenna-uplink";

            match guard.typed_reference_fault(&echo.antenna_uplink_config_arn, ECHOED_TYPE) {
                None => {}
                Some(crate::state::TypedReferenceFault::Absent) => {
                    return Err(CreateConfigErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: Some(crate::state::typed_reference_absent_message(
                                ECHOED_TYPE,
                                &echo.antenna_uplink_config_arn,
                            )),
                        },
                    )
                    .into());
                }
                Some(crate::state::TypedReferenceFault::WrongType) => {
                    return Err(CreateConfigErrorModeled::InvalidParameterException(
                        InvalidParameterException {
                            message: Some(crate::state::typed_reference_wrong_type_message(
                                ECHOED_TYPE,
                            )),
                            parameter_name: Some(
                                "configData.uplinkEchoConfig.antennaUplinkConfigArn".to_string(),
                            ),
                        },
                    )
                    .into());
                }
            }
        }
```
Copy 2 — `src/ops/update_config.rs:226-259`, identical but for the error-enum name, and preceded by a comment that says in so many words that it is one rule in two places (`src/ops/update_config.rs:222-225`):
```
        // An `uplinkEchoConfig` echoes a specific ANTENNA UPLINK config, named by ARN:
        // the update is held to the same reference rule, and answers the same two texts,
        // as the create, so an echo config cannot be pointed at a config that does not
        // exist (a missing resource) or at one of another type (a bad parameter).
        if let ConfigTypeData::uplinkEchoConfig(echo) = &input.config_data {
            // The one capability type an `uplinkEchoConfig`'s reference may name — the
            // member's own name and the `antenna-uplink` segment of a `ConfigArn` are
            // what say so.
            const ECHOED_TYPE: &str = "antenna-uplink";

            match guard.typed_reference_fault(&echo.antenna_uplink_config_arn, ECHOED_TYPE) {
```
The crate's own stated standard for exactly this situation — `src/ops/create_dataflow_endpoint_group_v2.rs:179-183`:
```
            // Every address the endpoint carries is a socket Ground Station connects a
            // stream to, so each port it names has to be one a transport can address, and
            // the name it is selected by has to be one no other endpoint of this group
            // took. Both rules belong to every create of a dataflow endpoint group, so both
            // are held in `crate::state` rather than in one of its handlers.
```
and `src/state.rs:2438-2439` on `check_endpoint_ports`: "Written here rather than in a handler because every create of a dataflow endpoint group holds its endpoints' addresses to the same rule."

Test coverage that pins both copies' behaviour (so a shared helper must leave them unchanged): `tests/all/config_defaults_and_references.rs` drives the create path (`case` rows around :602-685, asserting `typed_reference_wrong_type_message("antenna-uplink")` and 434 for an absent ARN) and then the update path at :687-747 — `// And the update path is held to the same two answers as the create.` with `assert_eq!(retyped.status, 431, ...)`, `assert_eq!(retyped.string("message"), typed_reference_wrong_type_message("antenna-uplink"), ...)` and `assert_eq!(dangling.status, 434, ...)`. Also `tests/all/lifecycle_writes.rs:533-556` (`// An \`uplinkEchoConfig\` update is held to the create's reference rule`), `tests/all/not_found_messages.rs:229-265` and `tests/all/scheduling_loop.rs:829-880`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-groundstation-groundstation/src/ops/create_config.rs` and read lines 214-259.
2. Open `services/service-com-amazonaws-groundstation-groundstation/src/ops/update_config.rs` and read lines 222-259 — the same `ECHOED_TYPE` constant, the same union-member branch, the same three-arm `typed_reference_fault` match, the same two message helpers and the same `"configData.uplinkEchoConfig.antennaUplinkConfigArn"` parameter name.
3. Compare with how the crate handles the other rules shared by two writers: `crate::state::check_endpoint_ports` and `crate::state::claim_endpoint_name` (called from both dataflow-endpoint-group creates), `validate_config_data` (both config writers), `validate_tags` (every tag-taking operation), `validate_search_window` (all three window-filtered listings) — each lives once in `src/state.rs` and is lifted into the caller's error enum at the call site.
4. Change the admissible capability or either fault's error mapping in one copy only: the create and the update now answer differently for the same `uplinkEchoConfig`, and `tests/all/config_defaults_and_references.rs:687-747` is the test that would catch it only for whichever copy it exercises.
