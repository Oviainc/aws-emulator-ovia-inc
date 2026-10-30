Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dlm.create-policy-missing-required-schedule-rejected dlm.update-policy-type-immutable-mutation-rejected
Issue-Shape-Id: com.amazonaws.dlm#dlm_20180112
Issue-Key: dlm.create-policy-missing-required-schedule-rejected|run#9|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: dlm.update-policy-type-immutable-mutation-rejected|run#9|AssertionError|assert err.get("Message") == expected_message, (
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# DLM policy validation refusals use their own message wording instead of AWS's InvalidRequestException templates (missing required parameter, non-updatable parameter)

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLifecyclePolicy, UpdateLifecyclePolicy

## Description
Two net-new root causes in the dlm crate's policy validation. Both refuse the right call with the right error code (InvalidRequestException, HTTP 400) but compose a message that is not the message AWS words for that error class, so the status/code are right and the body is wrong.

Cause 1 of 2: CreateLifecyclePolicy — missing-required-parameter message. Failing check: dlm.create-policy-missing-required-schedule-rejected.
AWS: Smithy (services/service-com-amazonaws-dlm-dlm-20180112): CreateLifecyclePolicy errors InvalidRequestException (errorKind client, httpError 400); InvalidRequestException models members Message, Code, RequiredParameters ("The request omitted one or more required parameters") and MutuallyExclusiveParameters; PolicyDetails.Schedules is @length min 1 max 4 and documented "[Custom snapshot and AMI policies only]" as the policy-defined actions a custom snapshot/AMI policy must carry. The model does not state the message text, so for the text: recorded behavior only — AWS answers a custom EBS_SNAPSHOT_MANAGEMENT policy submitted with no Schedules with InvalidRequestException and the message 'The following required parameter(s) are missing: {Schedules}'. Fixed text of that template: "The following required parameter(s) are missing: " followed by a brace-wrapped, comma-separated list; the variable part is the set of omitted required member names taken from the request (here the single name Schedules), named by the member's own name, not a dotted path.
Emulator: returns InvalidRequestException with message 'The parameter PolicyDetails.Schedules is required for this policy configuration.' — different fixed text, and it names the member by a dotted path (PolicyDetails.Schedules) rather than the bare member name AWS lists.
Why: a client that classifies or surfaces DLM validation failures by the documented missing-parameters wording (and any tooling that parses the brace-delimited name list out of the message to tell the user which parameters to supply, mirroring the RequiredParameters member) finds neither the template nor the parameter list, so a recoverable "you forgot Schedules" refusal is reported as an opaque failure.
Fix: the CreateLifecyclePolicy handler's request validation in the dlm service crate — the place that detects an absent PolicyDetails.Schedules and builds the InvalidRequestException — must emit AWS's missing-required-parameter template (and populate RequiredParameters with the same names).
Traces: dlm.create-policy-missing-required-schedule-rejected (positive control in the same check — a policy WITH a schedule — passes, so only the refusal body diverges).

Cause 2 of 2: UpdateLifecyclePolicy — non-updatable-parameter message. Failing check: dlm.update-policy-type-immutable-mutation-rejected.
AWS: Smithy: UpdateLifecyclePolicy errors InvalidRequestException (client, 400); PolicyDetails.PolicyType is an enum (PolicyTypeValues) and is fixed at creation. Message text: recorded behavior only — an update that supplies a PolicyDetails.PolicyType different from the stored one is refused with InvalidRequestException and the message 'The following parameter(s) cannot be updated: PolicyType'. Fixed text: "The following parameter(s) cannot be updated: " followed by a comma-separated list; the variable part is the set of immutable parameter names the request tried to change, taken from the request (here PolicyType), again by bare member name.
Emulator: returns InvalidRequestException with message "The parameter PolicyDetails.PolicyType cannot be updated. The policy's policy type is EBS_SNAPSHOT_MANAGEMENT and IMAGE_MANAGEMENT was requested." — a different template that also appends the stored and requested enum values, which AWS's message for this class does not carry.
Why: same client impact as Cause 1 — the refusal cannot be recognised by its documented wording, and the appended state/request values make the message unstable across accounts and runs, so any client keying off it mis-handles a plain immutability refusal.
Fix: the UpdateLifecyclePolicy handler's immutability check in the dlm service crate must emit AWS's non-updatable-parameter template and list the offending parameter names only.
Traces: dlm.update-policy-type-immutable-mutation-rejected (the check's other assertions — PolicyType and Description left untouched, and the Description-only update succeeding — pass, so only the refusal body diverges).

Neither cause is covered by an open dlm issue: the existing open issues on these operations concern default-policy ResourceTypes/PolicyType storage (20261029T033510Z-dlm-createlifecyclepolicy-validation--r7.md, 20261029T162741Z-dlm-createlifecyclepolicy-parameter-ignored--r2.md), iam:PassRole ordering (20261029T064757Z-dlm-createlifecyclepolicy-error-response--r2.md), the "but not both" mutual-exclusion comparison (20261028T155030Z-dlm-updatelifecyclepolicy-wrong-behavior--r4.md) and ArchiveRule/CreateRule enforcement (20261029T170711Z-dlm-updatelifecyclepolicy-validation--r4.md) — none of them is the wording of these two refusals.

## Evidence
dlm.create-policy-missing-required-schedule-rejected:
  CreateLifecyclePolicy(ExecutionRoleArn=<role>, Description=<desc>, State=ENABLED, PolicyDetails={PolicyType: EBS_SNAPSHOT_MANAGEMENT, ResourceTypes: [VOLUME], TargetTags: [...]})  # Schedules omitted
  expected: Code=InvalidRequestException, Message='The following required parameter(s) are missing: {Schedules}'
  actual:   Code=InvalidRequestException, Message='The parameter PolicyDetails.Schedules is required for this policy configuration.'
  AssertionError: expected message 'The following required parameter(s) are missing: {Schedules}', got 'The parameter PolicyDetails.Schedules is required for this policy configuration.'

dlm.update-policy-type-immutable-mutation-rejected:
  UpdateLifecyclePolicy(PolicyId=<policy>, PolicyDetails={PolicyType: IMAGE_MANAGEMENT, ...}) on a policy created as EBS_SNAPSHOT_MANAGEMENT
  expected: Code=InvalidRequestException, Message='The following parameter(s) cannot be updated: PolicyType'
  actual:   Code=InvalidRequestException, Message="The parameter PolicyDetails.PolicyType cannot be updated. The policy's policy type is EBS_SNAPSHOT_MANAGEMENT and IMAGE_MANAGEMENT was requested."
  AssertionError: expected message 'The following parameter(s) cannot be updated: PolicyType', got "The parameter PolicyDetails.PolicyType cannot be updated. ..."

In both cases the HTTP status (400) and error code (InvalidRequestException) match AWS; only the message body diverges.

## Steps to Reproduce
Cause 1:
1. Create an IAM role DLM can assume and use its ARN as ExecutionRoleArn.
2. Call CreateLifecyclePolicy with State=ENABLED and PolicyDetails={PolicyType: EBS_SNAPSHOT_MANAGEMENT, ResourceTypes: [VOLUME], TargetTags: [{Key: <k>, Value: <v>}]}, omitting Schedules entirely.
3. Observe the ClientError: code is InvalidRequestException (correct) but Message is not 'The following required parameter(s) are missing: {Schedules}'.

Cause 2:
1. Create a custom EBS_SNAPSHOT_MANAGEMENT policy with one schedule.
2. Call UpdateLifecyclePolicy(PolicyId=<that policy>, PolicyDetails={PolicyType: IMAGE_MANAGEMENT, ResourceTypes: [INSTANCE], TargetTags: [...], Schedules: [...]}).
3. Observe the ClientError: code is InvalidRequestException (correct) but Message is not 'The following parameter(s) cannot be updated: PolicyType'.

Replay: run_behavioral_suite and re-run the two check ids in trace_ids.
