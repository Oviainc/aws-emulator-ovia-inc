Issue-Source: trace-sweep-divergence
Issue-Trace: asglf-lt-contents-refusals
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asglf-lt-contents-refusals asglf-activity-instance-type-invalid
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: ec2|CreateLaunchTemplate|2xx->4xx|$.CreateLaunchTemplateResponse
Issue-Key: ec2|CreateLaunchTemplateVersion|2xx->4xx|$.CreateLaunchTemplateVersionResponse
Issue-Key: ec2|CreateLaunchTemplateVersion|2xx->4xx|status
Issue-Related: 20261106T135659Z-ec2-createlaunchtemplate-refuses-an-imageid-given-in-the.md
Issue-Commit: 5e1f0b507a128238d75b745ce1d1dcae4dc5a977

# EC2 launch-template writes refuse an unknown instance type instead of returning it as a ValidationWarning

> _Note: this issue was identified against the code base at commit `5e1f0b507a128238d75b745ce1d1dcae4dc5a977`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLaunchTemplate

## Description
Two net-new root causes with one shared fault: EC2's launch-template write path treats a non-existent instance type as a hard refusal, where AWS creates the template/version and reports the problem in the response's warning element. Each cause must be closed separately.

Cause 1 of 2 - CreateLaunchTemplate refuses LaunchTemplateData with an unknown instanceType.
AWS: Smithy: CreateLaunchTemplate output members LaunchTemplate (xmlName launchTemplate) and Warning (xmlName warning) targeting ValidationWarning, whose Errors member (xmlName errorSet) carries "The error codes and error messages that are returned for the parameters or parameter combinations that are not valid when a new launch template or new version of a launch template is created"; the operation models no errors for this case. So AWS answers HTTP 200, creates the template (defaultVersionNumber 1, latestVersionNumber 1) and returns warning.errorSet.item with code 'InvalidInstanceType' and the message whose fixed text is "The following supplied instance types do not exist: [" + the instance types from the request + "]".
Emulator: HTTP 400 with no CreateLaunchTemplateResponse at all - the template is never created.
Why: AWS's contract is that an unresolvable instance type is advisory: callers (including Auto Scaling trajectories and CloudFormation) legitimately create a template and read warning.errorSet. Refusing instead aborts the whole workflow and no later step can reference the template, and a client that reads the warning element gets nothing to read.
Fix: the CreateLaunchTemplate handler's LaunchTemplateData validation - emit the ValidationWarning errorSet in the 200 response instead of refusing - plus the serializer for the output's warning member.
Traces: asglf-lt-contents-refusals#task#14.

Cause 2 of 2 - CreateLaunchTemplateVersion refuses LaunchTemplateData with an unknown instanceType.
AWS: Smithy: CreateLaunchTemplateVersion output members LaunchTemplateVersion (xmlName launchTemplateVersion) and Warning (xmlName warning, ValidationWarning); no modelled errors for this case. AWS answers HTTP 200, creates the new version (versionNumber incremented, defaultVersion false, createdBy the calling principal's ARN, operator.managed false) and reports warning.errorSet.item code 'InvalidInstanceType' with the same "The following supplied instance types do not exist: [...]" template.
Emulator: HTTP 400 and no CreateLaunchTemplateVersionResponse, so the version is never created.
Why: the group/template update sequence that depends on the new version cannot proceed, and the advisory warning AWS documents is unobservable.
Fix: the CreateLaunchTemplateVersion handler's LaunchTemplateData validation and warning serialization. Note the separate open stub finding for CreateLaunchTemplateVersion (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_launch_template_version.rs answering 501 on other fixtures); here the operation does answer, and answers 400 where AWS answers 200 with a warning.
Traces: asglf-activity-instance-type-invalid#task#8.

## Evidence
Cause 1 (asglf-lt-contents-refusals#task#14, CreateLaunchTemplate): status expected 200, got 400; CreateLaunchTemplateResponse missing - expected launchTemplate {defaultVersionNumber '1', latestVersionNumber '1', createdBy 'arn:aws:iam::000000000000:user/emulator', operator {managed 'false'}, tagSet...} plus warning {errorSet {item {code 'InvalidInstanceType', message 'The following supplied instance types do not exist: [t3.bogus]'}}}.
Cause 2 (asglf-activity-instance-type-invalid#task#8, CreateLaunchTemplateVersion): status expected 200, got 400; CreateLaunchTemplateVersionResponse missing - expected launchTemplateVersion {versionNumber '2', defaultVersion 'false', launchTemplateData {instanceType 't3.bogus', imageId ...}, operator {managed 'false'}} plus warning {errorSet {item {code 'InvalidInstanceType', message 'The following supplied instance types do not exist: [t3.bogus]'}}}. (The ids, names and timestamps in the recorded bodies are per-run values, not requirements.)

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus and inspect asglf-lt-contents-refusals#task#14 (CreateLaunchTemplate whose LaunchTemplateData.InstanceType names a non-existent type) and asglf-activity-instance-type-invalid#task#8 (CreateLaunchTemplateVersion with the same kind of LaunchTemplateData). Both must answer HTTP 200 with the resource created and a warning.errorSet carrying InvalidInstanceType.
