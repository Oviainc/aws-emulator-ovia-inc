Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 99c19747b30de4fba88ebdfb7af58fcb882c179f

# Resolve workload-then-pattern through one helper instead of four copies of the same two-step 404 rule

_Filed: 2026-10-29T10:11:51Z_
_Operation: GetWorkloadDeploymentPattern | Class: maintainability_

> _Note: this issue was identified against the code base at commit `99c19747b30de4fba88ebdfb7af58fcb882c179f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "resolve the workload first and answer `WORKLOAD_NOT_FOUND_MESSAGE` when it is absent, then resolve the pattern inside it and answer `DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE`" is written out verbatim in four handlers of this crate (plus two partial variants). `crate::state` already owns every other shared rule of this service (`specification_warnings`, `filter_name_refusal`, `paginate`, the not-found message constants), but this resolution — including the kind-precedence that `tests/all/not_found_resource_kind.rs` pins — exists only as copies, so the copies can drift (one of them already differs: `update_deployment.rs` folds the two steps into one `and_then` and reports the PATTERN message when the workload is missing). A single helper in `crate::state` returning the resolved `&DeploymentPattern` or the message of the kind that is missing would leave every response byte and every existing test unchanged.

## Evidence

Copy 1 — `src/ops/create_deployment.rs:228-243`:
```
        let Some(workload) = store.workloads.get(&input.workload_name) else {
            return Err(CreateDeploymentErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException { message: Some(crate::state::WORKLOAD_NOT_FOUND_MESSAGE.to_string()) },
...
        let Some(pattern) = workload.patterns.get(&input.deployment_pattern_name) else {
            ... message: Some(crate::state::DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE.to_string()),
```
Copy 2 — `src/ops/get_workload_deployment_pattern.rs:163-184`: `let Some(workload) = store.workloads.get(&input.workload_name) else { ... WORKLOAD_NOT_FOUND_MESSAGE ... }; let Some(pattern) = workload.patterns.get(&input.deployment_pattern_name) else { ... DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE ... };`
Copy 3 — `src/ops/get_deployment_pattern_version.rs:167-188`: the identical pair before the version lookup.
Copy 4 — `src/ops/list_deployment_pattern_versions.rs:213-234`: the identical pair inside the `store.lock()` block.
Drifted variant — `src/ops/update_deployment.rs:232-243`:
```
        let Some(pattern) = store
            .workloads
            .get(&workload_name)
            .and_then(|workload| workload.patterns.get(&pattern_name))
        else {
            ... message: Some(crate::state::DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE.to_string()),
```
The crate states the intent the copies work against in `src/state.rs:503-507`: "`patterns` holds the full per-pattern record ... so the operations that report a pattern and the one that deploys it read the SAME entry."
Tests that pin the behaviour a shared helper must preserve: `tests/all/not_found_resource_kind.rs:147` (`a_not_found_carries_the_message_of_the_kind_of_resource_that_is_missing`, with rows for `/getWorkloadDeploymentPattern` at lines 243 and 292) and `tests/all/not_found_contract.rs`.

## Steps to Reproduce

1. Open src/ops/create_deployment.rs, src/ops/get_workload_deployment_pattern.rs, src/ops/get_deployment_pattern_version.rs and src/ops/list_deployment_pattern_versions.rs side by side
2. Observe the same two-step lookup with the same two message constants and the same kind precedence written four times, and a fifth, differing formulation in src/ops/update_deployment.rs:232-243
3. Run the existing tests (tests/all/not_found_resource_kind.rs, tests/all/not_found_contract.rs) — they pass against the copies and must keep passing against a single state.rs helper
