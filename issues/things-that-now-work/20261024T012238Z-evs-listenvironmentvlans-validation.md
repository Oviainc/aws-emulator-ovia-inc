Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# A list against an environment that is still initializing answers 200 instead of ValidationException

_Filed: 2026-10-24T01:22:38Z_
_Operation: ListEnvironmentVlans | Class: validation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service's own ValidationException documents a second trigger beyond malformed input: "You will see this exception if invalid inputs are provided for any of the Amazon EVS environment operations, or if a list operation is performed on an environment resource that is still initializing." An environment is still initializing while its `environmentState` is CREATING.

Expected: ListEnvironmentVlans against an environment whose state is CREATING is refused with ValidationException (HTTP 400) rather than reporting the resources of a half-built environment.

Actual: the handler resolves the environment, materialises due transitions and then pages its VLANs unconditionally — there is no state test anywhere in the crate (grep for "initializing" finds only the copied doc blocks). A ListEnvironmentVlans issued inside the 500 ms `TRANSITION_DELAY` window that follows CreateEnvironment therefore returns HTTP 200 with all ten VLANs in state CREATING. The same rule applies to this operation's siblings that address an environment resource — ListEnvironmentHosts, ListEnvironmentConnectors and ListVmEntitlements, which answer 200 with an empty page in the same window — so a fix should be applied consistently across them.

## Evidence

Spec: `ValidationException` shape doc (quoted above), which the model attaches to ListEnvironmentVlans/ListEnvironmentHosts/ListEnvironmentConnectors/ListVmEntitlements as a declared error; `EnvironmentState` CREATING is the initializing state and `crate::state::TRANSITION_DELAY` (src/state.rs:127) keeps an environment in it for 500 ms after the create. Handler: src/ops/list_environment_vlans.rs lines 197-230 — `environment_in_region` lookup, then `environment.vlans.iter()`; no `environment.state` test (compare src/ops/create_environment_host.rs lines 245-254, which does gate on the environment being deployed). Tests: tests/all/environment_lifecycle.rs and tests/all/hosts.rs list VLANs/hosts only after sleeping past the transition; no test asserts the behaviour of a list issued while the environment is CREATING.

## Steps to Reproduce

1. CreateEnvironment with a valid request and note the environmentId (the response reports environmentState=CREATING).
2. Immediately (within the 500 ms transition window, before GetEnvironment reports CREATED) call ListEnvironmentVlans for that environmentId.
3. Observe HTTP 200 with ten VLANs instead of a ValidationException for a list performed on an environment that is still initializing.
4. Repeat with ListEnvironmentHosts / ListEnvironmentConnectors — both answer 200 with an empty page in the same window.
