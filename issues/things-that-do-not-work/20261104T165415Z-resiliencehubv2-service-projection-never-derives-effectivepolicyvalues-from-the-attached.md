Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehubv2.deleting-referenced-policy-clears-service-effective-policy-values
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2.deleting-referenced-policy-clears-service-effective-policy-values|run#17|KeyError|effective = service["effectivePolicyValues"]
Issue-Commit: 82bf3da4596cf523466b02fd09cd706f8136ec0c

# Service projection never derives effectivePolicyValues from the attached policy, so CreateService/GetService omit the member entirely

> _Note: this issue was identified against the code base at commit `82bf3da4596cf523466b02fd09cd706f8136ec0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
ONE root cause, affecting every reader of the resiliencehubv2 `Service` shape.

=== Cause 1 of 1: `Service$effectivePolicyValues` is hard-coded absent instead of being derived from the referenced policy (failing check: resiliencehubv2.deleting-referenced-policy-clears-service-effective-policy-values) ===

AWS: Smithy `Service$effectivePolicyValues` -> `EffectivePolicyValues`, documented "The effective policy values for the service." `EffectivePolicyValues` is a structure of per-target sources: `availabilitySlo` (SloSource: `value` Double "The availability SLO percentage value", `policyName` EntityName, `source` PolicyValueSource), `multiAzRto`/`multiAzRpo`/`multiRegionRto`/`multiRegionRpo`/`dataRecoveryTimeBetweenBackups` (TargetSource: `value` Integer in minutes, `policyName`, `source`), and `multiAzDrApproach`/`multiRegionDrApproach` (DisasterRecoverySource: `value` String, `policyName`, `source`). `CreateService` (POST /v2/create-service) and the @readonly `GetService` (GET /v2/get-service) both return `service` (Service, required). The recorded behavior this check is grounded in: when a service is created with `policyArn` naming an existing Policy, the 200 `service` carries `effectivePolicyValues` whose per-target entries hold the values resolved from that policy — e.g. a policy whose `availabilitySlo.target` is T yields `effectivePolicyValues.availabilitySlo.value == T` (the target value comes from the request/stored policy, not from any one recording). Once the referenced policy is deleted, the member is still PRESENT and resolves to the empty object `{}` (nothing is derivable any more) while `service.policyArn` keeps pointing at the now-dangling ARN. The member is therefore always present on the wire; only its contents vary. Hypothesis (neither model nor recording states it): `policyName`/`source` identify which policy, possibly cross-account, each value came from.

Emulator: the member is absent from every `Service` body. `services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/state.rs` `ServiceRecord::to_service()` sets `effective_policy_values: None` unconditionally, with a comment that members "no operation in this slice computes" are left absent; the record stores only `policy_arn` and never resolves it against the policy store, even though both resources live behind the same single state lock. `CreateService` (src/ops/create_service.rs `handle`, which returns `record.to_service()`) and `GetService` answer from that one projection, so neither ever emits the member. Replay: the botocore-parsed CreateService response has no `effectivePolicyValues` key at all — `KeyError: 'effectivePolicyValues'` at the first read of it.

Why: a client that attaches a resilience policy to a service and then reads the service back to learn the targets actually in force gets a null/absent member and cannot distinguish "no effective targets" from "this emulator does not compute them". Any logic keyed on the effective availability SLO, RTO/RPO or DR approach (dashboards, drift checks, gating a test or assessment on configured targets) takes the wrong branch or raises on the missing key, and the AWS invariant that deleting the referenced policy CLEARS the derived values to `{}` while the `policyArn` pointer survives is unobservable — the before and after states of a policy deletion look identical.

Fix: the `Service` response projection and the service state that feeds it — `ServiceRecord::to_service` (and the shared view helpers beside it in src/state.rs) must resolve the record's `policy_arn` against the policy store under the same lock and build `EffectivePolicyValues` from the referenced policy's `availabilitySlo`, `multiAz`, `multiRegion` and `dataRecovery` blocks (each as the matching SloSource/TargetSource/DisasterRecoverySource with `policyName` set to the policy's name), emitting a present-but-empty object when the service has no `policyArn` or when that ARN no longer resolves, rather than eliding the member. `ServiceSummary`'s projection beside it elides `achievability`/`organizationId` the same way and is not covered by this issue.

Related open issues, NOT duplicates of this cause: "Policy lifecycle responses diverge from AWS: empty tags dropped, multiAz not reflected, deleted policy still served 200" (20261104T144946Z-...) owns the Policy-side divergences, including DeletePolicy answering 501 — this check's later assertions (that GetService clears effectivePolicyValues to `{}` after the policy is deleted, and that GetPolicy then answers 404 "Policy not found: <resource-id>") cannot be reached until both that issue and this one are closed, so re-replay this check after fixing the projection. "CreateService never validates the permissionModel.invokerRoleName IAM role" (20261029T191749Z-...) owns the invoker-role validation gap, not the response projection.

Traces: failing behavioral check resiliencehubv2.deleting-referenced-policy-clears-service-effective-policy-values.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (bootstrap_signal=false; 1/7 checks passed).

Failing check (verdict: error), service heading `### resiliencehubv2`:
  resiliencehubv2.deleting-referenced-policy-clears-service-effective-policy-values
  File "check_resiliencehubv2_deleting_referenced_policy_clears_service_effective_policy_values.py", line 104, in run
    effective = service["effectivePolicyValues"]
  KeyError: 'effectivePolicyValues'

The check had already succeeded in: CreatePolicy with `availabilitySlo={"target": 99.9}`, and CreateService with `policyArn=<that policy's ARN>` plus a valid `permissionModel.invokerRoleName`. It asserted `service["policyArn"] == policy_arn` (passed), then read `service["effectivePolicyValues"]` and found no such member. Expected (per the check and the recorded AWS behavior) `effectivePolicyValues.availabilitySlo.value` equal to the policy's availability target (99.9 is this run's value for that target, not part of the requirement); got no member at all.

Still-unreached assertions in the same check, blocked behind this one:
  - GetService after DeletePolicy must still report the dangling `policyArn`.
  - GetService after DeletePolicy must report `effectivePolicyValues == {}` (present and empty).
  - GetPolicy on the deleted ARN must be ResourceNotFoundException with message "Policy not found: " + the ARN's resource segment.

Model evidence (services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/smithy-model/com-amazonaws-resiliencehubv2-ngrhservicecore.json):
  - Service$effectivePolicyValues -> EffectivePolicyValues, doc "The effective policy values for the service."
  - EffectivePolicyValues members: availabilitySlo (SloSource), multiAzRto/multiAzRpo (TargetSource), multiAzDrApproach (DisasterRecoverySource), multiRegionRto/multiRegionRpo (TargetSource), multiRegionDrApproach (DisasterRecoverySource), dataRecoveryTimeBetweenBackups (TargetSource).
  - SloSource: value (Double), policyName (EntityName), source (PolicyValueSource enum). TargetSource: value (Integer, minutes), policyName, source. DisasterRecoverySource: value (String), policyName, source.
  - CreateService: POST /v2/create-service, input policyArn (Arn), output service (Service, required). GetService: GET /v2/get-service, @readonly, output service (Service, required).
  - Policy members feeding the derivation: availabilitySlo (AvailabilitySlo), multiAz (MultiAzTargets), multiRegion (MultiRegionTargets), dataRecovery (DataRecoveryTargets), name (EntityName, required).

Emulator code evidence:
  - src/state.rs, ServiceRecord::to_service(): `effective_policy_values: None,` — hard-coded, with the doc comment "Members no operation in this slice computes (effectivePolicyValues, achievability, estimatedAssessmentCost, resourceDiscovery, rerunAssessment, organizationId) are left absent rather than filled with a value nothing derived."
  - src/ops/create_service.rs handle(): builds a ServiceRecord that stores `policy_arn: input.policy_arn` and returns `record.to_service()`; it never reads the policy store, so nothing is ever derived from the referenced policy.
  - The policy records live in the same state struct (src/state/policies.rs, re-exported from src/state.rs), under one lock, so the resolution needs no new plumbing.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. POST /v2/create-policy with a valid `name` and `availabilitySlo={"target": <T>}`; note the returned `policy.policyArn`.
3. Create an IAM role whose trust policy allows resiliencehub.amazonaws.com, then POST /v2/create-service with a valid `name`, `regions=[<region>]`, `permissionModel={"invokerRoleName": <role>}` and `policyArn=<the policy ARN>`.
4. Inspect the 200 body's `service`: `policyArn` is echoed, but there is no `effectivePolicyValues` member (botocore raises KeyError on it). AWS returns the member with `availabilitySlo.value == <T>` and `availabilitySlo.policyName` naming the policy.
5. GET /v2/get-service?serviceArn=<the service ARN>: the 200 body's `service` likewise has no `effectivePolicyValues`.
6. POST /v2/delete-policy for that policy ARN, then GET /v2/get-service again: AWS keeps the dangling `policyArn` and reports `effectivePolicyValues` as a present, empty object `{}`; the emulator still reports no member, so the deletion's effect on the service is unobservable. (DeletePolicy itself currently answers 501 — owned by the separately filed policy-lifecycle issue.)
7. Re-replay behavioral check resiliencehubv2.deleting-referenced-policy-clears-service-effective-policy-values to confirm.
