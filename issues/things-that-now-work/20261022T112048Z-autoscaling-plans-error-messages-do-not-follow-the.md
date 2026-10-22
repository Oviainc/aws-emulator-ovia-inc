Issue-Source: trace-sweep-divergence
Issue-Trace: autoscalingplans-absent-plan-read-and-write-envelopes
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscalingplans-absent-plan-read-and-write-envelopes
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans|DeleteScalingPlan|4xx->4xx|$.Message
Issue-Key: autoscaling-plans|UpdateScalingPlan|4xx->4xx|$.Message
Issue-Key: autoscaling-plans|DescribeScalingPlans|4xx->4xx|$.Message
Issue-Commit: 6f223cb07d8f19a9e48b2b4f7aac2665ac919e98

# autoscaling-plans error messages do not follow the AWS wording built from the request

> _Note: this issue was identified against the code base at commit `6f223cb07d8f19a9e48b2b4f7aac2665ac919e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteScalingPlan

## Description
Three autoscaling-plans error paths return the correct HTTP status and the correct error code but phrase the message differently from AWS. Each of these messages is produced by a known rule - the not-found messages are built from the request's ScalingPlanName and ScalingPlanVersion, and the mutual-exclusion message is the fixed text of a known parameter-conflict rule - so the wording is part of the contract rather than incidental text. This bundle has 3 causes; all three must be closed.

Cause 1 of 3: DeleteScalingPlan not-found message wording.
AWS: DeleteScalingPlan models ObjectNotFoundException (client error, httpError 400, doc "The specified object could not be found.", single Message member) (Smithy: DeleteScalingPlan errors; shape ObjectNotFoundException). The recorded 400 response's message is built from the request parameters in the form "No scaling plan found for scaling plan name: <ScalingPlanName>, version: <ScalingPlanVersion>".
Emulator: same HTTP 400 and same error code, but the message reads "No scaling plan found with name '<ScalingPlanName>' and version <ScalingPlanVersion>" - the request values are correct, the template is not.
Why: a client that matches on the error text - log scrapers, assertion suites and the many tools that discriminate not-found variants by message because ObjectNotFoundException carries no other discriminator - behaves differently against the emulator than against AWS.
Fix: the DeleteScalingPlan handler's ObjectNotFoundException construction: format the message as AWS does, substituting the requested name and version into "No scaling plan found for scaling plan name: {name}, version: {version}".
Traces: autoscalingplans-absent-plan-read-and-write-envelopes#task#3.

Cause 2 of 3: UpdateScalingPlan not-found message wording - the same defect on the other write path.
AWS: UpdateScalingPlan models the same ObjectNotFoundException (client, httpError 400) (Smithy: UpdateScalingPlan errors). The recorded 400 message uses the same request-derived form "No scaling plan found for scaling plan name: <ScalingPlanName>, version: <ScalingPlanVersion>".
Emulator: HTTP 400 with the correct error code but the message "No scaling plan found with name '<ScalingPlanName>' and version <ScalingPlanVersion>".
Why: as Cause 1 - the two write paths diverge from AWS identically, so a client cannot recognize either not-found response by its text.
Fix: the UpdateScalingPlan handler's ObjectNotFoundException construction; share one message formatter with DeleteScalingPlan so both paths stay aligned.
Traces: autoscalingplans-absent-plan-read-and-write-envelopes#task#4.

Cause 3 of 3: DescribeScalingPlans parameter-conflict message wording.
AWS: DescribeScalingPlans takes both ScalingPlanNames and ApplicationSources as optional input members and models ValidationException (client, httpError 400) (Smithy: DescribeScalingPlans input ScalingPlanNames/ScalingPlanVersion/ApplicationSources; errors include ValidationException). Supplying both is rejected, and the recorded 400 message for that rule is the fixed text "Application sources cannot be provided with scaling plan names or version".
Emulator: it does enforce the rule and returns HTTP 400 with the right error code, but phrases it "ScalingPlanNames and ApplicationSources cannot both be specified".
Why: the conflict is detected correctly, so the impact is limited to clients and tests that read or match the message; they see text AWS never emits.
Fix: the DescribeScalingPlans input validation's ValidationException message, which should carry AWS's wording for the ApplicationSources/ScalingPlanNames conflict.
Traces: autoscalingplans-absent-plan-read-and-write-envelopes#task#5.

## Evidence
All three steps are from fixture autoscalingplans-absent-plan-read-and-write-envelopes; in every case the HTTP status (400) and the error code matched and only $.Message diverged.

Cause 1 - step task#3, DeleteScalingPlan: $.Message value_mismatch - expected 'No scaling plan found for scaling plan name: <name>, version: <version>' (recorded instance: 'No scaling plan found for scaling plan name: sim2real-probe-asp-absent-c82e5b40, version: 1'), got "No scaling plan found with name 'sim2real-probe-asp-absent-c82e5b40' and version 1".

Cause 2 - step task#4, UpdateScalingPlan: $.Message value_mismatch - same expected template, same actual template as Cause 1.

Cause 3 - step task#5, DescribeScalingPlans with both ScalingPlanNames and ApplicationSources: $.Message value_mismatch - expected 'Application sources cannot be provided with scaling plan names or version', got 'ScalingPlanNames and ApplicationSources cannot both be specified'.

Model evidence: Smithy ObjectNotFoundException (httpError 400, members {Message}) is bound to DeleteScalingPlan and UpdateScalingPlan; ValidationException (httpError 400, members {Message}) is bound to DescribeScalingPlans.

## Steps to Reproduce
1. Replay fixture autoscalingplans-absent-plan-read-and-write-envelopes against the emulator.
2. Step task#3: DeleteScalingPlan for a scaling plan name that was never created, version 1. Status 400 and the error code are right; compare $.Message against 'No scaling plan found for scaling plan name: <name>, version: <version>'.
3. Step task#4: UpdateScalingPlan for the same absent name/version. Same message divergence.
4. Step task#5: DescribeScalingPlans with ScalingPlanNames and ApplicationSources both supplied. Status 400 is right; compare $.Message against 'Application sources cannot be provided with scaling plan names or version'.
