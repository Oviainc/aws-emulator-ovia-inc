Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# Environment health checks are never computed, so `checks` is always absent and `environmentStatus` is a blanket PASSED

_Filed: 2026-10-24T02:21:27Z_
_Operation: GetEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Environment$checks` is "A check on the environment to identify environment health and validate VMware VCF licensing compliance", and the `Check$type` doc enumerates the checks the service performs, several of which are computable from facts this emulator already holds:

* `KEY_REUSE` — "Verifies that the VCF license key is not used by another Amazon EVS environment" (every environment's `licenseInfo` is stored, so reuse across the store is decidable).
* `KEY_COVERAGE` — "Verifies that the VCF license key allocates sufficient vCPU cores for all deployed hosts" (the stored host list and `licenseInfo` decide it).
* `HOST_COUNT` — "Verifies that the environment meets the minimum host count" (the stored `hosts` map decides it; Amazon EVS environments require at least four hosts, and `CreateEnvironmentRequest$hosts` is modelled `len 4..4`).

Expected: a settled environment reports a non-empty `checks` list with a per-check `type`/`result` (and `impairedSince` when impaired), and `environmentStatus` is the roll-up of those results — so an environment carrying fewer than the minimum number of hosts, or one whose VCF license key is also used by another environment of the account, reports a FAILED check and `environmentStatus` FAILED.

Actual: `checks` is initialised to the empty vector at create and nothing ever appends to it, so `Environment$checks` is omitted from every response (CreateEnvironment, GetEnvironment, DeleteEnvironment), and `StoredEnvironment::environment_status()` degenerates to "UNKNOWN while not CREATED, PASSED otherwise" — a self-deployed environment with zero hosts, or two environments sharing one license key, both report PASSED with no checks at all. The same roll-up is reported as `EnvironmentSummary$environmentStatus` by ListEnvironments and CreateEnvironmentHost, so the hollow value is visible there too.

## Evidence

Spec: `Check` shape doc ("Amazon EVS performs the following checks: KEY_REUSE … KEY_COVERAGE … HOST_COUNT …"), `Environment$checks` and `Environment$environmentStatus` (`smithy_query(query="shape", shape="Check,CheckType,CheckResult")`), and `CreateEnvironmentRequest$hosts` modelled `len 4..4`. Handler/state: src/ops/create_environment.rs line 411 (`checks: Vec::new()`), src/state.rs lines 848-849 (`pub checks: Vec<Check>`) with no writer anywhere (grep for `checks` in src/ finds only the declaration, the roll-up and the serialiser), src/state.rs lines 977-993 (`environment_status`) and line 1016 (`checks: (!self.checks.is_empty()).then(...)`). Tests: tests/all/environment_lifecycle.rs lines 330 and 371 pin the current roll-up (UNKNOWN while CREATING, then the default for a settled environment); no test asserts a `checks` entry, and nothing asserts a FAILED check for an under-populated or license-colliding environment.

## Steps to Reproduce

1. Register a VPC and service access subnet in the EC2 store, then CreateEnvironment with vcfVersion=SELF_DEPLOYED, termsAccepted=true and ten non-overlapping initialVlans.
2. Wait for the environment to reach CREATED and call GetEnvironment.
3. Observe `environment.checks` is absent entirely, even though the environment has zero hosts (minimum host count not met).
4. Observe `environment.environmentStatus` is PASSED rather than the roll-up of a failed HOST_COUNT check.
5. Create a second environment carrying the same licenseInfo key as a first one and GetEnvironment both — neither reports a KEY_REUSE check.
