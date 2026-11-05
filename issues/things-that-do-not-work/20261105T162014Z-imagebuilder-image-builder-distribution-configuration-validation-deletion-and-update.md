Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-create-distribution-configuration-rejects-a-region-entry-with-no-outputs
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-create-distribution-configuration-rejects-a-region-entry-with-no-outputs imagebuilder-create-distribution-configuration-rejects-an-unknown-region imagebuilder-ibp-traj-distribution-config-mistakes imagebuilder-distribution-configuration-accepts-an-empty-distribution-list imagebuilder-ibp-traj-cleanup-in-dependency-order imagebuilder-distribution-config-multi-region imagebuilder-update-distribution-configuration-persists-new-region imagebuilder-image-pipeline-round-trips-its-three-configurations-before-any-run
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|CreateDistributionConfiguration|4xx->2xx|$.message
Issue-Key: imagebuilder|CreateDistributionConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateDistributionConfiguration|4xx->2xx|status
Issue-Key: imagebuilder|CreateDistributionConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|CreateDistributionConfiguration|2xx->4xx|$.clientToken
Issue-Key: imagebuilder|CreateDistributionConfiguration|2xx->4xx|$.distributionConfigurationArn
Issue-Key: imagebuilder|CreateDistributionConfiguration|2xx->4xx|status
Issue-Key: imagebuilder|GetDistributionConfiguration|4xx->2xx|$.__type
Issue-Key: imagebuilder|GetDistributionConfiguration|4xx->2xx|$.message
Issue-Key: imagebuilder|GetDistributionConfiguration|4xx->2xx|status
Issue-Key: imagebuilder|GetDistributionConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: imagebuilder|GetDistributionConfiguration|2xx->2xx|$.distributionConfiguration.distributions[*]
Issue-Key: imagebuilder|GetImagePipeline|2xx->2xx|$.imagePipeline.consecutiveFailures
Issue-Related: 20261105T162014Z-imagebuilder-image-builder-refusal-error-types-and-messages-are.md
Issue-Commit: 4f520f1edeaf87046d1e59f82118bd1eb9149075

# Image Builder distribution-configuration validation, deletion and update state diverge from AWS

> _Note: this issue was identified against the code base at commit `4f520f1edeaf87046d1e59f82118bd1eb9149075`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistributionConfiguration

## Description
This bundle carries SEVEN distinct causes in the Image Builder distribution-configuration lifecycle (plus one image-pipeline read-shape cause). All must be closed before this issue is submitted. The shared fault: the distribution-configuration path neither validates the distributions list nor keeps its stored state in step with creates, deletes and updates, so reads answer from stale or never-validated state. (Related but NOT covering: 20261102T085510Z-imagebuilder-image-builder-accepts-creates-and-deletes-that-aws.md and 20261102T085510Z-imagebuilder-image-builder-delete-and-recreate-lifecycle-leaves-stored.md — neither carries the fixture steps listed below.)

Cause 1 of 7: CreateDistributionConfiguration accepts a region entry with no outputs.
AWS: refuses with 400 and the message "The value supplied for parameter 'distributions[0]' is not valid. One or more outputs should be provided for each region in a distribution configuration." Fixed text is "The value supplied for parameter '" ... "' is not valid. One or more outputs should be provided for each region in a distribution configuration."; the variable part is the indexed member path of the offending Distribution entry, taken from the request. A Distribution entry must carry at least one output (amiDistributionConfiguration, containerDistributionConfiguration, fastLaunchConfigurations, launchTemplateConfigurations, s3ExportConfiguration, ...). Smithy: CreateDistributionConfiguration.distributions -> DistributionList (required); modeled 400 errors include InvalidParameterCombinationException and InvalidRequestException. Hypothesis: the "value supplied for parameter ... is not valid" wording is InvalidParameterValueException's message class (the recorded errortype header was present but its value not asserted).
Emulator: answers 200 and creates the distribution configuration.
Why: a client gets a successfully-created distribution configuration that cannot distribute anything; the failure surfaces much later (at image distribution) instead of at create time, and the invalid config is now persisted.
Fix: CreateDistributionConfiguration's input validation in the imagebuilder service crate — per-entry output presence check over the distributions list.
Traces: imagebuilder-create-distribution-configuration-rejects-a-region-entry-with-no-outputs#task#1 (also imagebuilder-create-distribution-configuration-rejects-an-unknown-region#task#1, imagebuilder-ibp-traj-distribution-config-mistakes#task#1).

Cause 2 of 7: CreateDistributionConfiguration does not require '{{ imagebuilder:buildDate }}' in the AMI name.
AWS: refuses with 400 and the message "The value supplied for parameter 'distributions[0].amiDistributionConfiguration.name' is not valid. The EC2 AMI name must contain '{{ imagebuilder:buildDate }}' to ensure that the name will be unique." Fixed text is everything but the indexed member path, which comes from the request; the required substring '{{ imagebuilder:buildDate }}' is itself fixed text of the rule.
Emulator: does not apply the rule at all. In this step it instead answers 400 "A distribution configuration named <name> already exists in this Amazon Web Services Region." with x-amzn-errortype ResourceAlreadyExistsException — the wrong refusal, reached only because an earlier request that AWS rejected was persisted (see cause 3).
Why: a client is told its name collides when the real defect is the missing buildDate template, so it renames and retries forever; and AMI names that AWS guarantees unique are accepted non-unique.
Fix: CreateDistributionConfiguration's validation of amiDistributionConfiguration.name, evaluated before the name-conflict check.
Traces: imagebuilder-ibp-traj-distribution-config-mistakes#task#3.

Cause 3 of 7: a distribution configuration AWS refused is still persisted, so the next valid create is refused as a duplicate.
AWS: a create that fails validation stores nothing, so a subsequent valid CreateDistributionConfiguration with the same name succeeds with 200 and returns clientToken and distributionConfigurationArn (Smithy: CreateDistributionConfigurationResponse.distributionConfigurationArn, clientToken; http code 200).
Emulator: answers 400 with no clientToken and no distributionConfigurationArn — the rejected (or unvalidated) earlier create left a record under that name, which the duplicate-name check then trips over.
Why: the name is permanently burned for the client; the resource it needs can never be created, and the error blames a conflict that does not exist in AWS.
Fix: the CreateDistributionConfiguration handler's state write — validate fully before inserting, and do not insert on any refusal path.
Traces: imagebuilder-create-distribution-configuration-rejects-an-unknown-region#task#3 (also imagebuilder-ibp-traj-distribution-config-mistakes#task#4).

Cause 4 of 7: GetDistributionConfiguration answers 200 for a deleted distribution configuration.
AWS: after DeleteDistributionConfiguration, GetDistributionConfiguration returns 404 with __type ResourceNotFoundException and the message "The resource ARN '<distributionConfigurationArn>' was not found." Fixed text is "The resource ARN '" ... "' was not found."; the variable part is the requested ARN, taken from the request. Smithy: ResourceNotFoundException (client, httpError 404, "At least one of the resources referenced by your request does not exist.").
Emulator: answers 200 with a body and no __type/message — the deleted configuration is still readable.
Why: a client's existence check and its delete-confirmation loop both read the resource as live after a successful delete, so teardown and idempotent-recreate flows never converge.
Fix: DeleteDistributionConfiguration's state removal and/or GetDistributionConfiguration's lookup-miss branch in the imagebuilder service crate.
Traces: imagebuilder-create-distribution-configuration-rejects-a-region-entry-with-no-outputs#teardown#6 (also #teardown#7, imagebuilder-distribution-configuration-accepts-an-empty-distribution-list#teardown#4, imagebuilder-ibp-traj-cleanup-in-dependency-order#task#19).

Cause 5 of 7: the same not-found read on the multi-region fixture, where AWS uses the required-resource message form.
AWS: 404 with x-amzn-errortype ResourceNotFoundException and the message "The following required resource 'DistributionConfiguration' cannot be found: '<distributionConfigurationArn>'." Fixed text is "The following required resource 'DistributionConfiguration' cannot be found: '" ... "'."; the ARN comes from the request. Both this and cause 4's form must be produced on their respective paths.
Emulator: answers 200 with no errortype header and no message.
Why: as cause 4; additionally a client matching on the required-resource message form sees neither status nor text it expects.
Fix: GetDistributionConfiguration's lookup-miss branch — emit the message form AWS uses for this read path.
Traces: imagebuilder-distribution-config-multi-region#teardown#6.

Cause 6 of 7: UpdateDistributionConfiguration does not persist an added region.
AWS: UpdateDistributionConfiguration replaces the distributions list with the one supplied (Smithy: UpdateDistributionConfiguration.distributions -> DistributionList, required), so a following GetDistributionConfiguration returns every entry of the new list, including the newly added region with its amiDistributionConfiguration.
Emulator: GetDistributionConfiguration returns only the first entry; distributions[1] is absent from the response body.
Why: the update silently does nothing for added regions — the client believes a second-region distribution is configured when the emulator's state still holds the original single-region list.
Fix: the UpdateDistributionConfiguration handler's state write (it must replace the stored distributions list), or GetDistributionConfiguration's serializer if the list is stored but not emitted.
Traces: imagebuilder-update-distribution-configuration-persists-new-region#task#4.

Cause 7 of 7: GetImagePipeline emits consecutiveFailures for a pipeline that has never run.
AWS: for a freshly created pipeline with no run, the imagePipeline object in the response omits consecutiveFailures entirely. Smithy: ImagePipeline.consecutiveFailures -> ConsecutiveFailures (integer, range min 0), an optional member Image Builder populates from tracked scheduled-execution failures; recorded behavior shows it absent before any run.
Emulator: includes consecutiveFailures: 0 in the imagePipeline object.
Why: a client that distinguishes "never ran" (member absent) from "ran and has zero consecutive failures" (member 0) reads the pipeline as having executed; round-trip comparisons of the pipeline object also fail on the extra key.
Fix: the GetImagePipeline response serializer / the ImagePipeline projection in the imagebuilder service crate — leave the member unset until a scheduled execution has been tracked.
Traces: imagebuilder-image-pipeline-round-trips-its-three-configurations-before-any-run#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — imagebuilder-create-distribution-configuration-rejects-a-region-entry-with-no-outputs#task#1, CreateDistributionConfiguration: status expected 400, got 200; $.message missing (expected "The value supplied for parameter 'distributions[0]' is not valid. One or more outputs should be provided for each region in a distribution configuration.", got None); $headers.x-amzn-errortype missing. Cluster also covers imagebuilder-create-distribution-configuration-rejects-an-unknown-region#task#1 and imagebuilder-ibp-traj-distribution-config-mistakes#task#1.

Cause 2 — imagebuilder-ibp-traj-distribution-config-mistakes#task#3, CreateDistributionConfiguration, 400 vs 400: $.message value_mismatch (expected "The value supplied for parameter 'distributions[0].amiDistributionConfiguration.name' is not valid. The EC2 AMI name must contain '{{ imagebuilder:buildDate }}' to ensure that the name will be unique.", got 'A distribution configuration named <name> already exists in this Amazon Web Services Region.'); $headers.x-amzn-errortype shape_mismatch (got 'ResourceAlreadyExistsException').

Cause 3 — imagebuilder-create-distribution-configuration-rejects-an-unknown-region#task#3, CreateDistributionConfiguration: status expected 200, got 400; $.clientToken missing; $.distributionConfigurationArn missing (expected 'arn:aws:imagebuilder:<region>:<account>:distribution-configuration/<name>'). Cluster also covers imagebuilder-ibp-traj-distribution-config-mistakes#task#4.

Cause 4 — imagebuilder-create-distribution-configuration-rejects-a-region-entry-with-no-outputs#teardown#6, GetDistributionConfiguration: status expected 404, got 200; $.__type missing (expected 'ResourceNotFoundException'); $.message missing (expected "The resource ARN '<arn>' was not found."). Cluster also covers #teardown#7, imagebuilder-distribution-configuration-accepts-an-empty-distribution-list#teardown#4 and imagebuilder-ibp-traj-cleanup-in-dependency-order#task#19.

Cause 5 — imagebuilder-distribution-config-multi-region#teardown#6, GetDistributionConfiguration: status expected 404, got 200; $.message missing (expected "The following required resource 'DistributionConfiguration' cannot be found: '<arn>'."); $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException').

Cause 6 — imagebuilder-update-distribution-configuration-persists-new-region#task#4, GetDistributionConfiguration, 200 vs 200: $.distributionConfiguration.distributions[1] missing (expected a second entry {'region': <added region>, 'amiDistributionConfiguration': {'name': '<ami-name>-{{ imagebuilder:buildDate }}'}}, got None).

Cause 7 — imagebuilder-image-pipeline-round-trips-its-three-configurations-before-any-run#task#9, GetImagePipeline, 200 vs 200: $.imagePipeline.consecutiveFailures extra (expected None, got 0).

## Steps to Reproduce
1. Build the emulator from this checkout and boot it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture imagebuilder-create-distribution-configuration-rejects-a-region-entry-with-no-outputs, step task#1 — CreateDistributionConfiguration whose distributions[0] has a region but no output member; expect 400, observe 200.
4. Cause 2: fixture imagebuilder-ibp-traj-distribution-config-mistakes, step task#3 — CreateDistributionConfiguration with amiDistributionConfiguration.name lacking '{{ imagebuilder:buildDate }}'.
5. Cause 3: fixture imagebuilder-create-distribution-configuration-rejects-an-unknown-region, step task#3 — after a create AWS refuses, re-create the same name with a valid body; expect 200 with an ARN, observe 400.
6. Cause 4: same fixture-family teardown — imagebuilder-create-distribution-configuration-rejects-a-region-entry-with-no-outputs, step teardown#6: DeleteDistributionConfiguration then GetDistributionConfiguration on the same ARN; expect 404 ResourceNotFoundException.
7. Cause 5: fixture imagebuilder-distribution-config-multi-region, step teardown#6 — same delete-then-get, required-resource message form.
8. Cause 6: fixture imagebuilder-update-distribution-configuration-persists-new-region, step task#4 — UpdateDistributionConfiguration adding a second region, then GetDistributionConfiguration; expect two entries in distributions.
9. Cause 7: fixture imagebuilder-image-pipeline-round-trips-its-three-configurations-before-any-run, step task#9 — GetImagePipeline on a pipeline that has never run; consecutiveFailures must be absent.
10. Re-replay every fixture step listed in Traces to confirm all seven causes are closed.
