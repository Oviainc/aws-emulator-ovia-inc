Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# An unpublished deploymentPatternVersionName is refused with the deployment-pattern not-found message instead of the version's own

_Filed: 2026-10-28T11:49:57Z_
_Operation: UpdateDeployment | Class: error-response_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` states a rule for its not-found messages: a deployment pattern VERSION is its own resource, so a pattern that exists at a version the catalogue does not publish must be refused with the version's own message (`DEPLOYMENT_PATTERN_VERSION_NOT_FOUND_MESSAGE`) rather than with the pattern's. `GetDeploymentPatternVersion` follows that rule for exactly this condition. `UpdateDeployment` breaks it: when the caller names a `deploymentPatternVersionName` the catalogue does not publish for the deployment's pattern — the pattern itself having been resolved successfully one statement earlier — it answers the PATTERN's message, which tells the caller the deployment pattern does not exist when the pattern does exist and only the version does not.

## Evidence

The rule, services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:183-191:
```
/// The message a deployment-pattern-VERSION lookup that finds nothing carries.
///
/// A version of a deployment pattern is its own resource (its own name, description and
/// read operations), so a pattern that exists at a version the catalogue does not
/// publish is refused with its own message rather than with the pattern's.
pub const DEPLOYMENT_PATTERN_VERSION_NOT_FOUND_MESSAGE: &str =
    "The specified deployment pattern version does not exist or insufficient permissions.";
```
The sibling that applies it, src/ops/get_deployment_pattern_version.rs:189-200:
```
        let Some(summary) = pattern.version_summary(&input.deployment_pattern_version_name) else {
            return Err(... ResourceNotFoundException { message: Some(
                crate::state::DEPLOYMENT_PATTERN_VERSION_NOT_FOUND_MESSAGE.to_string()), } ...
```
The handler that breaks it, src/ops/update_deployment.rs:264-275 (the pattern was already resolved at :232-243, so this branch means "the pattern exists, the version does not"):
```
        if input
            .deployment_pattern_version_name
            .as_ref()
            .is_some_and(|requested| pattern.version(requested).is_none())
        {
            return Err(UpdateDeploymentErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE.to_string()),
                },
            )
            .into());
        }
```
Test coverage: tests/all/deployment_update.rs:838-859 currently pins the pattern message for this row ("deploymentPatternVersionName", "The specified deployment pattern does not exist or insufficient permissions.") with only the generic rationale "a catalogue lookup carries its fixed message" — which both constants satisfy — so the expectation needs updating to the version constant the crate's own doc mandates.

## Steps to Reproduce

1. CreateDeployment for workload SAP / pattern SapHanaSingle and wait for it to reach COMPLETED.
2. POST /updateDeployment with {"deploymentId": "<id>", "specifications": {<the conforming map>}, "deploymentPatternVersionName": "9.9.9"} — a well-formed version the catalogue does not publish for an existing pattern.
3. Observe 404 ResourceNotFoundException with message "The specified deployment pattern does not exist or insufficient permissions."
4. Expected per state.rs:183-191 and the GetDeploymentPatternVersion sibling: "The specified deployment pattern version does not exist or insufficient permissions."
5. Contrast: POST /getDeploymentPatternVersion with the same workload, pattern and version 9.9.9 returns the version message for the identical condition.
