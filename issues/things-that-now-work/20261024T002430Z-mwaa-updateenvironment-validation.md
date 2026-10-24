Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# UpdateEnvironment accepts spec-invalid input: S3 path without its object version, min>max sizing, unsupported AirflowVersion/EnvironmentClass

_Filed: 2026-10-24T00:24:30Z_
_Operation: UpdateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateEnvironment merges whatever the request carries into the stored spec with no business-rule validation, so requests the real service refuses with HTTP 400 `ValidationException` are accepted with HTTP 200 and the (impossible) configuration becomes visible to every later GetEnvironment. Symptoms of this one class:

1. S3 path/version pairing. The model's own docs state the rule: `PluginsS3Path` — "If specified, then the plugins.zip version is required"; `RequirementsS3Path` — "If specified, then a file version is required"; `StartupScriptS3ObjectVersion` — "You must specify the version ID … every time you update the script". A PATCH naming `PluginsS3Path` (or `RequirementsS3Path`) without the matching `…S3ObjectVersion` must be refused; the handler stores the path and leaves the stale/absent version.
2. Relational sizing rules. `MinWorkers` > `MaxWorkers` and `MinWebservers` > `MaxWebservers` are refused by AWS ("MinWorkers must be less than or equal to MaxWorkers."), including when only one side is in the patch and the other comes from the stored record. The handler copies each member independently (lines 294-305) with no cross-member or stored-value comparison. (The already-open issue 20261023T180451Z covers this rule for CreateEnvironment only; on update the comparison must additionally be made against the stored value.)
3. Unsupported enumerated values. `AirflowVersion` is pattern-only in the model but the doc fixes the value set ("Valid values: 2.7.2, 2.8.1, 2.9.2, 2.10.1, 2.10.3, 2.11.0, and 3.0.6"), and `EnvironmentClass` likewise ("Valid values: mw1.micro, mw1.small, mw1.medium, mw1.large, mw1.xlarge, mw1.2xlarge"). `AirflowVersion='9.9.9'` or `EnvironmentClass='mw1.enormous'` is accepted and stored.
4. Update while the environment is not in a terminal state. The handler calls `begin_update` unconditionally, so a PATCH against an environment still `CREATING` (or already `UPDATING`) is accepted and silently overwrites the pending transition, where AWS refuses an update of an environment that is not AVAILABLE.

The same rules in 1 and 3 apply to CreateEnvironment's handler (src/ops/create_environment.rs:290-341 stores the same members unchecked); the fix belongs in the shared validation both operations need.

## Evidence

Spec grounding: Smithy docs for UpdateEnvironment.PluginsS3Path / RequirementsS3Path / StartupScriptS3ObjectVersion state the version is required when the path is specified; AirflowVersion and EnvironmentClass docs enumerate the valid values; UpdateEnvironment declares ValidationException (HTTP 400). Handler: src/ops/update_environment.rs:271-347 is a straight member-by-member merge (`if let Some(value) = input.x { spec.x = value }`) with no validation of any kind before `stored.begin_update(...)`. No test in services/service-com-amazonaws-mwaa-amazonmwaa/tests/all asserts any of these refusals — tests/all/environment_lifecycle.rs only round-trips patches that are internally consistent, and tests/all/rejection_contract.rs covers only framework-level constraint violations (length/pattern/range).

## Steps to Reproduce

1. Boot the emulator; create an environment 'PatchEnv' with the required members only and wait for it to report AVAILABLE.
2. mwaa.update_environment(Name='PatchEnv', PluginsS3Path='plugins.zip') with no PluginsS3ObjectVersion -> expected HTTP 400 ValidationException (the path requires its version); observed HTTP 200 with the Arn, and GetEnvironment then reports PluginsS3Path with no PluginsS3ObjectVersion.
3. mwaa.update_environment(Name='PatchEnv', MinWorkers=5, MaxWorkers=1) -> expected 400 ValidationException 'MinWorkers must be less than or equal to MaxWorkers.'; observed 200 and both values stored.
4. mwaa.update_environment(Name='PatchEnv', AirflowVersion='9.9.9') and mwaa.update_environment(Name='PatchEnv', EnvironmentClass='mw1.enormous') -> expected 400 ValidationException; observed 200 and GetEnvironment reports the bogus values.
5. Immediately after a create (while Status is still CREATING) call mwaa.update_environment(Name='<new env>', MaxWorkers=4) -> expected 400 ValidationException for an environment that is not AVAILABLE; observed 200 and the status jumps to UPDATING.
