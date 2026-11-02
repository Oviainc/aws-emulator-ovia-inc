Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__sqs.cdk-default-deploy-queue-with-dlq-redrives-and-carries-cdk-metadata cloudformation__s3__sqs.deploy-cfn-change-set-with-stage-outside-allowedvalues-rejected cloudformation__s3__sqs.deploy-cfn-stack-update-to-unmapped-stage-rejected cloudformation__s3__sqs.deploy-cfn-template-body-and-url-together-rejected
Issue-Key: cloudformation__s3__sqs.cdk-default-deploy-queue-with-dlq-redrives-and-carries-cdk-metadata|run#12|AssertionError|assert rc == 0, f"`cdk deploy` exited {rc}; output tail {out[-800:]!r}"
Issue-Key: cloudformation__s3__sqs.deploy-cfn-change-set-with-stage-outside-allowedvalues-rejected|run#23|AssertionError|assert got == expected, f"CreateChangeSet with Stage=delta: expected {expected!r}, got {got!r}"
Issue-Key: cloudformation__s3__sqs.deploy-cfn-stack-update-to-unmapped-stage-rejected|run#21|AssertionError|assert (code, message) == expected, (
Issue-Key: cloudformation__s3__sqs.deploy-cfn-template-body-and-url-together-rejected|run#20|AssertionError|assert message == _BOTH_TEMPLATE_MEMBERS_MESSAGE, (
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# cloudformation__s3__sqs: 3 net-new causes — AWS::CDK::Metadata is rejected as an unrecognized resource type, Parameter AllowedValues are never enforced, and the TemplateBody+TemplateURL refusal message diverges

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new root causes in CloudFormation's template validation / parameter validation surface.

Cause 1 of 3: the AWS::CDK::Metadata resource type is refused, so no default-synthesized CDK stack can be deployed.
AWS: AWS::CDK::Metadata is a real (if inert) CloudFormation resource type that every default `cdk synth` emits; CreateStack/CreateChangeSet accept it and provision it as a no-op resource. Smithy (cloudformation CreateChangeSet errors) models no error for a known type — the refusal class here is ValidationError 'Template format error: Unrecognized resource types: [...]', which AWS reserves for types it does not know, so a template carrying only AWS::CDK::Metadata plus SQS resources must be accepted; recorded behavior only.
Emulator: `cdk deploy` fails at change-set creation with 'Template format error: Unrecognized resource types: [AWS::CDK::Metadata]'.
Why: the default CDK toolchain cannot deploy anything at all against the emulator; every CDK-based scenario stops at synthesis-to-change-set.
Fix: register AWS::CDK::Metadata as a known, no-op resource type in the CloudFormation resource-type registry / template validator.
Traces: cloudformation__s3__sqs.cdk-default-deploy-queue-with-dlq-redrives-and-carries-cdk-metadata.

Cause 2 of 3: a Parameter value outside its AllowedValues is accepted and then fails later inside template processing, with the wrong refusal.
AWS: parameter constraints are validated before the template is processed. A value outside AllowedValues is refused ValidationError with the fixed text "Parameter '<ParameterName>' must be one of AllowedValues" (the variable part is the parameter's logical name from the request), identically on CreateChangeSet and UpdateStack. Smithy (cloudformation CreateChangeSet/UpdateStack) models no dedicated error shape; the code is ValidationError, HTTP 400 — recorded behavior only for the message text.
Emulator: the parameter constraint is never checked, so the bad value reaches Fn::FindInMap and the call is refused with ValidationError 'Template error: Unable to get mapping for StageConfig::<value>::<key>' — the same code but the wrong message, and the failure is attributed to the mapping rather than the parameter. Recorded on both legs ('... StageConfig::delta::Tier' for CreateChangeSet and '... StageConfig::delta::VisibilityTimeout' for UpdateStack), where only the mapping key differs.
Why: a client validating its own parameter contracts cannot tell a disallowed parameter from a broken Mappings block, and the error message names an internal mapping key instead of the parameter the caller got wrong.
Fix: enforce Parameters constraints (AllowedValues, AllowedPattern, Min/MaxLength, Min/MaxValue) in CloudFormation's parameter-validation step, before template processing.
Traces: cloudformation__s3__sqs.deploy-cfn-change-set-with-stage-outside-allowedvalues-rejected, cloudformation__s3__sqs.deploy-cfn-stack-update-to-unmapped-stage-rejected.

Cause 3 of 3: the both-template-sources refusal message is not AWS's text.
AWS: CreateStack with both TemplateBody and TemplateURL is refused ValidationError with the fixed message 'You cannot specify both Template URL and Template Body.' (no variable parts).
Emulator: same code, message 'You cannot specify both TemplateBody and TemplateURL' — different fixed text.
Why: clients and test suites that assert on the documented refusal text see a mismatch, and error-message matching in deployment tooling breaks.
Fix: the message template in CreateStack's input validation.
Traces: cloudformation__s3__sqs.deploy-cfn-template-body-and-url-together-rejected.

## Evidence
Cause 1 — Actual: 'AssertionError: `cdk deploy` exited 1; output tail ... creating CloudFormation changeset...\nTemplate format error: Unrecognized resource types: [AWS::CDK::Metadata]'. Expected: deploy succeeds and the stack carries the metadata resource.
Cause 2 — Actual: ('ValidationError', 'Template error: Unable to get mapping for StageConfig::delta::Tier') on CreateChangeSet and ('ValidationError', 'Template error: Unable to get mapping for StageConfig::delta::VisibilityTimeout') on UpdateStack. Expected both: ('ValidationError', "Parameter 'Stage' must be one of AllowedValues").
Cause 3 — Actual message 'You cannot specify both TemplateBody and TemplateURL'; expected 'You cannot specify both Template URL and Template Body.'

## Steps to Reproduce
Cause 1: `cdk deploy` a default-synthesized stack (queue + DLQ) — the template contains AWS::CDK::Metadata. Cause 2: CreateChangeSet (then UpdateStack) with a Parameter declared with AllowedValues and a value outside the list that is also used as a Fn::FindInMap key; read the ClientError code and message. Cause 3: CreateStack passing both TemplateBody and TemplateURL. Replay checks cloudformation__s3__sqs.cdk-default-deploy-queue-with-dlq-redrives-and-carries-cdk-metadata, cloudformation__s3__sqs.deploy-cfn-change-set-with-stage-outside-allowedvalues-rejected, cloudformation__s3__sqs.deploy-cfn-stack-update-to-unmapped-stage-rejected, cloudformation__s3__sqs.deploy-cfn-template-body-and-url-together-rejected.
