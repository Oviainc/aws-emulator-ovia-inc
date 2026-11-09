Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.cloudwatch-insight-rule-cfn-deploy-delete-lifecycle cloudformation__cloudwatch.cloudwatch-insight-rule-cfn-invalid-rule-body-rejected cloudformation__cloudwatch.cloudwatch-log-alarm-cfn-deploy-delete-lifecycle cloudformation__cloudwatch.cloudwatch-otel-enrichment-cfn-singleton-lifecycle cloudformation__cloudwatch.deploy-cloudformation-alarm-mute-rule-malformed-schedule-rejected-rollback cloudformation__cloudwatch.deploy-cloudformation-alarm-mute-rule-provisioned-and-deleted cloudformation__cloudwatch.deploy-cloudformation-anomaly-detector-provisioned-and-deleted
Issue-Key: cloudformation__cloudwatch|CreateStack|ValidationError
Issue-Key: cloudformation__cloudwatch.deploy-cloudformation-anomaly-detector-provisioned-and-deleted|run#15|AssertionError|assert re.fullmatch(r"Detector-[A-Za-z0-9]{12}", outputs["DetectorRef"]), (
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudformation__cloudwatch: 2 net-new causes — four AWS::CloudWatch::* resource types are unknown to the template processor, and AnomalyDetector's Ref is the full logical path instead of a Detector-<suffix> id

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 2: CreateStack refuses any template containing AWS::CloudWatch::InsightRule, AWS::CloudWatch::LogAlarm, AWS::CloudWatch::OTelEnrichment or AWS::CloudWatch::AlarmMuteRule as an unrecognized resource type.
AWS: CloudFormation resolves every one of these against its resource-type registry and provisions the CloudWatch resource; a template using them deploys to CREATE_COMPLETE. A genuinely unknown type is what gets 'Template format error: Unrecognized resource types: [...]' — these four are not unknown to AWS. Smithy (cloudformation 2010-05-15): CreateStack models ValidationError/400 for malformed templates, which is the envelope the emulator is (wrongly) using here.
Emulator: CreateStack answers 400 ValidationError 'Template format error: Unrecognized resource types: [<type>]' for each of the four types, so the stack is never created and every downstream lifecycle, rejection and delete assertion is unreachable. The message's fixed text is 'Template format error: Unrecognized resource types: ' and its variable part is the bracketed list of type names taken from the submitted template.
Why: none of these CloudWatch resources can be managed as infrastructure-as-code. A customer's stack fails at submit time rather than deploying, and the rollback paths that depend on a resource failing for a REAL reason (a malformed schedule, an invalid rule body) can never be exercised.
Fix: the CloudFormation template resource-type registry/dispatch component, plus the per-type provisioning handlers for these four CloudWatch types.
Failing checks: cloudformation__cloudwatch.cloudwatch-insight-rule-cfn-deploy-delete-lifecycle, cloudformation__cloudwatch.cloudwatch-insight-rule-cfn-invalid-rule-body-rejected, cloudformation__cloudwatch.cloudwatch-log-alarm-cfn-deploy-delete-lifecycle, cloudformation__cloudwatch.cloudwatch-otel-enrichment-cfn-singleton-lifecycle, cloudformation__cloudwatch.deploy-cloudformation-alarm-mute-rule-malformed-schedule-rejected-rollback, cloudformation__cloudwatch.deploy-cloudformation-alarm-mute-rule-provisioned-and-deleted.

Cause 2 of 2: Ref on an AWS::CloudWatch::AnomalyDetector returns the full stack-qualified logical path instead of the detector's own physical id.
AWS: the physical id CloudFormation assigns an anomaly detector — and therefore the value Ref resolves to and GetAtt/outputs carry — is of the form 'Detector-<12-character suffix>'. Recorded behavior only (the resource's Ref value is not expressed in the Smithy API model).
Emulator: Ref resolves to '<stack-name>-<logical-id>-<12 digits>' (the generated-name template the emulator uses for every resource), e.g. the recorded run produced a value shaped '<stack>-Detector-<digits>'. The requirement is the SHAPE 'Detector-<12-char suffix>', not the recorded value.
Why: a template that passes Ref of the detector into another resource or an output hands downstream callers a string that is not an anomaly-detector id, so DeleteAnomalyDetector/DescribeAnomalyDetectors on it cannot resolve the resource.
Fix: the AWS::CloudWatch::AnomalyDetector provisioning handler's physical-id minting (it must not fall through to the generic '<stack>-<logical>-<n>' name generator).
Failing check: cloudformation__cloudwatch.deploy-cloudformation-anomaly-detector-provisioned-and-deleted.

## Evidence
Cause 1 — Expected: CreateStack accepts the template and the stack reaches CREATE_COMPLETE with the CloudWatch resource provisioned.
Actual: 'botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CloudWatch::InsightRule]' (and identically for [AWS::CloudWatch::LogAlarm], [AWS::CloudWatch::OTelEnrichment], [AWS::CloudWatch::AlarmMuteRule]).

Cause 2 — Expected: Ref -> a 'Detector-<12-character suffix>' physical id.
Actual: 'AssertionError: Ref should be a Detector-<12-character suffix> physical id, got \'sim2real-behav-adstk-4cbb95e1c62f-Detector-355378357298\'' (host/stack/digits are per-run; the divergence is the shape).

## Steps to Reproduce
Cause 1: CreateStack with a template whose only resource is of type AWS::CloudWatch::InsightRule (repeat with AWS::CloudWatch::LogAlarm, AWS::CloudWatch::OTelEnrichment, AWS::CloudWatch::AlarmMuteRule). Observe 400 ValidationError 'Template format error: Unrecognized resource types: [...]' instead of a created stack.
Cause 2: CreateStack with an AWS::CloudWatch::AnomalyDetector resource and an Output of Ref on it; after CREATE_COMPLETE read DescribeStacks Outputs and compare the value against the 'Detector-<12-char suffix>' form.
