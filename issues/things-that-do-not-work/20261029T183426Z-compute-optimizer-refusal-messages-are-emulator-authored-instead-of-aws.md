Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-ec2-instance-recommendations-and-metrics
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-ec2-instance-recommendations-and-metrics compute-optimizer-recommendation-preferences-error-paths
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer|GetEC2InstanceRecommendations|4xx->4xx|$.message
Issue-Key: compute-optimizer|PutRecommendationPreferences|4xx->4xx|$.message
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# Refusal messages are emulator-authored instead of AWS's wording on GetEC2InstanceRecommendations and PutRecommendationPreferences

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEC2InstanceRecommendations

## Description
TWO causes, one shared fault: the refusal is raised on the right path with the right HTTP status and the right error code, but the message body is not the sentence AWS emits. In both cases AWS's message is 100% FIXED text — it names no member and echoes no request value — while the emulator substitutes text of its own.

=== Cause 1 of 2: GetEC2InstanceRecommendations malformed instance ARN message ===
AWS: Smithy: GetEC2InstanceRecommendations input member instanceArns -> InstanceArns (list of InstanceArn); modeled error InvalidParameterValueException (client, httpError 400). Recorded behavior: an instanceArns entry that is not a well-formed EC2 instance ARN is refused HTTP 400 with the message "Invalid instance arn." — the whole sentence is fixed text, including the lower-case spelling "arn"; the rejected value is NOT echoed and the member name is NOT mentioned.
Emulator: HTTP 400 and the same error code (status and code both match), but the message is "Invalid instance ARN." — AWS's sentence with "arn" re-cased to "ARN".
Why: the message is a requirement of this error class. A client, SDK test suite or support tool that matches the documented sentence exactly (or hashes/compares it) does not recognise the emulator's refusal, and a fidelity test that asserts the message fails on a service whose behaviour is otherwise correct. It is a one-token difference, which is exactly the kind of divergence an emulator should not introduce since the text is fully determined and carries no per-run value.
Fix: the GetEC2InstanceRecommendations handler's instanceArns validation in the compute-optimizer crate — the literal message string at that refusal site must be "Invalid instance arn."
Traces: compute-optimizer-ec2-instance-recommendations-and-metrics#task#5

=== Cause 2 of 2: PutRecommendationPreferences AutoScalingGroup-outside-ResourceArn-scope message ===
AWS: Smithy: PutRecommendationPreferences input members resourceType (ResourceType enum, required) and scope (Scope); ScopeName = Organization | AccountId | ResourceArn; modeled error InvalidParameterValueException (client, httpError 400). Recorded behavior: resourceType=AutoScalingGroup combined with a scope name of "Organization" or "AccountId" is refused HTTP 400 with the message 'You cannot create a recommendation preference for an Auto Scaling Group with the "Organization" or "AccountId" scope name. Specify "ResourceArn" as the scope name, the Auto Scaling group ARN ID as the scope value, and try again.' Every part of that sentence pair is FIXED text, including the literal double-quoted scope-name tokens; nothing is taken from the request.
Emulator: HTTP 400 and the correct error code, but the message is "The value supplied for parameter scope is not valid: a AutoScalingGroup recommendation preference can only be created at the resource level, with a scope name of ResourceArn." — a generic "value supplied for parameter <member> is not valid" template that names the member `scope`, which AWS's message never does, and drops AWS's remediation sentence.
Why: a console or CLI wrapper surfaces AWS's remediation guidance ("Specify \"ResourceArn\" as the scope name, the Auto Scaling group ARN ID as the scope value") to the operator; against the emulator the operator is shown different guidance, and a client matching the documented text treats the refusal as an unknown error. The refusal itself is correct — no preference is created — so only the wording diverges.
Fix: the PutRecommendationPreferences handler's scope/resourceType compatibility check in the compute-optimizer crate: raise the ASG-specific fixed sentence instead of routing through the shared "value supplied for parameter" message builder.
Traces: compute-optimizer-recommendation-preferences-error-paths#task#2 (the same cluster also covers compute-optimizer-recommendation-preferences-error-paths#task#3, the Organization-scope spelling of the same refusal)

Relation to open issues: the open behavioral issue 20261029T172203Z-compute-optimizer-four-client-error-messages-are-emulator.md describes the SAME generic-message-builder fault for Cause 2 (its Cause 3, grounded in check compute-optimizer.put-recommendation-preferences-asg-account-scope-rejected) and for three other refusal paths, but it carries no wire replay key for this corpus, so this wire divergence was reported unowned by the replay and is filed here with its fixture binding; a fixer closing that issue should re-replay the steps named above. Cause 1 is not covered by it or by any other open issue.

## Evidence
Both clusters replayed against a running emulator; status and error code matched in both, only the message text diverged.

Cause 1 — GetEC2InstanceRecommendations [compute-optimizer-ec2-instance-recommendations-and-metrics#task#5], expected 400, got 400:
  $.message: value_mismatch
    expected 'Invalid instance arn.'   (100% fixed text; note lower-case 'arn')
    actual   'Invalid instance ARN.'

Cause 2 — PutRecommendationPreferences [compute-optimizer-recommendation-preferences-error-paths#task#2, cluster also covers #task#3], expected 400, got 400:
  $.message: value_mismatch
    expected 'You cannot create a recommendation preference for an Auto Scaling Group with the "Organization" or "AccountId" scope name. Specify "ResourceArn" as the scope name, the Auto Scaling group ARN ID as the scope value, and try again.'   (100% fixed text, including the quoted scope-name tokens)
    actual   'The value supplied for parameter scope is not valid: a AutoScalingGroup recommendation preference can only be created at the resource level, with a scope name of ResourceArn.'
  The cluster's two members are the AccountId-scope and Organization-scope spellings of the request; both get the same emulator template, confirming one shared message site.

Model evidence (smithy_query on services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/smithy-model/com-amazonaws-computeoptimizer-computeoptimizerservice.json): GetEC2InstanceRecommendations and PutRecommendationPreferences both declare InvalidParameterValueException (client, httpError 400) among their errors; PutRecommendationPreferences input = {resourceType (required, ResourceType), scope (Scope), enhancedInfrastructureMetrics, inferredWorkloadTypes, externalMetricsPreference, lookBackPeriod, utilizationPreferences, preferredResources, savingsEstimationMode}. Neither message is derived from a modeled constraint, so the recorded text is the requirement.

## Steps to Reproduce
Boot the emulator and opt the account in (UpdateEnrollmentStatus status=Active).

Cause 1: call GetEC2InstanceRecommendations(instanceArns=["not-an-arn"]). The call is refused 400 with the right error code; read Error.Message — it is 'Invalid instance ARN.' where AWS sends 'Invalid instance arn.'

Cause 2: call PutRecommendationPreferences(resourceType="AutoScalingGroup", scope={"name": "AccountId", "value": <caller account id>}, lookBackPeriod="DAYS_32"), then repeat with scope={"name": "Organization", "value": <org id>}. Both are refused 400 with the right error code; read Error.Message — it is the generic 'The value supplied for parameter scope is not valid: ...' template rather than AWS's two-sentence 'You cannot create a recommendation preference for an Auto Scaling Group with the "Organization" or "AccountId" scope name. Specify "ResourceArn" ...' text. The same call with resourceType="Ec2Instance" succeeds, confirming the refusal path is reached correctly.

Replay: re-run fixture compute-optimizer-ec2-instance-recommendations-and-metrics step task#5 (representative) and compute-optimizer-recommendation-preferences-error-paths step task#2.
