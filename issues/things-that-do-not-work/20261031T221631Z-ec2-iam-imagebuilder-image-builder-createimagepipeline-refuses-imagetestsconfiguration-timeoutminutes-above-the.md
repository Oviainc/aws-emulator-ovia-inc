Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes
Issue-Key: ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes|run#9|AssertionError|assert (code, message) == ("InvalidParameterValueException", expected), (
Issue-Commit: 86d4487805fae2c305c5dd9de933831005de30fd

# Image Builder CreateImagePipeline refuses imageTestsConfiguration.timeoutMinutes above the ceiling with a generic ValidationException instead of InvalidParameterValueException

> _Note: this issue was identified against the code base at commit `86d4487805fae2c305c5dd9de933831005de30fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateImagePipeline

## Description
One net-new cause. The check id's leading token is the composite group ec2__iam__imagebuilder; the diverging operation belongs to imagebuilder (the ec2/iam calls in the check are only setup and they succeeded). No open issue carries this check: the only imagebuilder issues on file are the bootstrap issue and the CreateInfrastructureConfiguration stub, both resolved, and the report flagged neither this check as blocked by a stub nor as owned.

Cause 1 of 1: a parameter-range refusal is answered with the emulator's generic Smithy-constraint error class and wording rather than Image Builder's own.
AWS: Image Builder answers an out-of-range imageTestsConfiguration.timeoutMinutes with HTTP 400, code InvalidParameterValueException and message "The value supplied for parameter 'imageTestsConfiguration.timeoutMinutes' is not valid. imageTestsConfiguration.timeoutMinutes must be smaller than or equal to 1440" — fixed text "The value supplied for parameter '" <member path> "' is not valid. " <member path> " must be smaller than or equal to " <the modeled maximum>, with the member path and the limit being the only variable parts (the offending value itself is not echoed). Smithy (com-amazonaws-imagebuilder-imagebuilder): ImageTestsConfiguration.timeoutMinutes -> ImageTestsTimeoutMinutes with @range min 60 max 1440, and the service models InvalidParameterValueException (client error, httpError 400, member message) whose doc is 'The value that you provided for the specified parameter is invalid'; it is Image Builder's parameter-refusal class even though CreateImagePipeline's own error list enumerates only ClientException/InvalidRequestException/etc. The ceiling 1440 itself is accepted and round-trips.
Emulator: HTTP 400 with code ValidationException and message "1 validation error detected: Value '1441' at 'imageTestsConfiguration.timeoutMinutes' failed to satisfy constraint: Member must have value less than or equal to 1440" — the framework's generic constraint report, an error class Image Builder does not use on this surface, with wording that echoes the submitted value and a different sentence template.
Why: Image Builder answers every parameter refusal with that one code, so clients and the AWS SDKs/CLI branch on InvalidParameterValueException; a ValidationException matches no handler, surfaces as an unmodeled/unknown error, and any client that parses the message to report which parameter and which limit was violated reads the wrong template.
Fix: the request validation in the CreateImagePipeline handler (services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/create_image_pipeline.rs) must raise the service's modeled InvalidParameterValueException with Image Builder's message template for the imageTestsConfiguration.timeoutMinutes range, instead of letting the generic Smithy @range constraint reporter answer.
Traces: ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes.

## Evidence
AssertionError: timeoutMinutes 1441 should be refused InvalidParameterValueException "The value supplied for parameter 'imageTestsConfiguration.timeoutMinutes' is not valid. imageTestsConfiguration.timeoutMinutes must be smaller than or equal to 1440", got 'ValidationException' "1 validation error detected: Value '1441' at 'imageTestsConfiguration.timeoutMinutes' failed to satisfy constraint: Member must have value less than or equal to 1440". The call was imagebuilder:CreateImagePipeline(name=<unique>, imageRecipeArn=<recipe>, infrastructureConfigurationArn=<infra>, imageTestsConfiguration={'imageTestsEnabled': True, 'timeoutMinutes': 1441}); all setup (component, recipe, instance profile, infrastructure configuration, VPC/subnet/security group) succeeded, so only the refusal class and wording diverge.

## Steps to Reproduce
1. Clone and boot the emulator; replay ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-a-test-timeout-above-1440-minutes.
2. Minimal repro: create an imagebuilder component, image recipe and infrastructure configuration, then call imagebuilder:CreateImagePipeline with imageTestsConfiguration={'imageTestsEnabled': true, 'timeoutMinutes': 1441}. Expect HTTP 400 / InvalidParameterValueException with the message template above; observe HTTP 400 / ValidationException with the generic constraint text.
3. Confirm the positive control still holds: the same call with timeoutMinutes=1440 is accepted and GetImagePipeline echoes {'imageTestsEnabled': true, 'timeoutMinutes': 1440}.
