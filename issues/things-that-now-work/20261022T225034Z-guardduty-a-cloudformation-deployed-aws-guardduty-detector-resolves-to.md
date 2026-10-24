Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.deploy-detector-cfn-behaves guardduty.deploy-guardduty-detector-cfn-active-behavior
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetDetector|BadRequestException
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# A CloudFormation-deployed AWS::GuardDuty::Detector resolves to a physical id GetDetector rejects as an invalid detectorId

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDetector

## Description
Cause 1 of 1 — the physical resource id CloudFormation assigns to an AWS::GuardDuty::Detector is not a detector id the GuardDuty service recognizes.
AWS: Smithy: GetDetector is GET /detector/{DetectorId} with DetectorId a required @httpLabel (@length 1-300) and errors BadRequestException (httpError 400) and InternalServerErrorException (500); its output requires Status (DetectorStatus) and ServiceRole. CloudFormation's AWS::GuardDuty::Detector returns the detector's id for `Ref`, so `!Ref Detector` yields a value that GetDetector resolves to a real, ENABLED detector once the stack reaches CREATE_COMPLETE. Recorded behavior: after the stack completes, GetDetector on the Ref'd value returns 200 with Status ENABLED and the FindingPublishingFrequency the template set.
Emulator: the stack reaches completion, but GetDetector on the Ref'd/Outputs value fails with HTTP 400 BadRequestException "The request is rejected because the parameter detectorId has an invalid value." — i.e. CloudFormation either never created a GuardDuty detector for the resource or assigned it a physical id that is not a detector id (a logical name or synthesized arn rather than the id GuardDuty's state is keyed by). Both templates that exercise this fail identically, with and without a Tags property and for different FindingPublishingFrequency values, so it is not template-specific.
Why: every infrastructure-as-code path to GuardDuty is broken. A client that deploys a detector by CloudFormation and then addresses it through the GuardDuty API (GetDetector, UpdateDetector, CreateIPSet/CreateFilter under /detector/{DetectorId}/...) gets a 400 on every call, and a stack that reports CREATE_COMPLETE while the resource is unusable makes the emulator report success for a deployment that produced nothing usable.
Fix: the CloudFormation resource provider for AWS::GuardDuty::Detector — it must create the detector through the GuardDuty service state and record the resulting DetectorId as the resource's physical id, so that Ref/Outputs return the same id GuardDuty's GetDetector resolves; the GuardDuty detector state keyed by that id must also report Status ENABLED and the template's FindingPublishingFrequency.
Traces: guardduty.deploy-detector-cfn-behaves, guardduty.deploy-guardduty-detector-cfn-active-behavior.

## Evidence
Both checks fail at the first GuardDuty call after the stack deploys:
  botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the GetDetector operation: The request is rejected because the parameter detectorId has an invalid value.

guardduty.deploy-detector-cfn-behaves template: AWS::GuardDuty::Detector with Enable: true, FindingPublishingFrequency: SIX_HOURS, Tags; Outputs.DetectorId = !Ref Detector.
guardduty.deploy-guardduty-detector-cfn-active-behavior template: AWS::GuardDuty::Detector with Enable: true, FindingPublishingFrequency: FIFTEEN_MINUTES; the check asserts the deployed detector is genuinely ACTIVE via GetDetector.

Expected per model (services/service-com-amazonaws-guardduty-guarddutyapiservice/smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json): GetDetector -> 200 with required Status (ENABLED) and ServiceRole, plus FindingPublishingFrequency as configured. Observed: 400 BadRequestException on the id CloudFormation handed back.

## Steps to Reproduce
1. Boot the emulator.
2. CreateStack with a template containing:
   Resources:
     Detector:
       Type: AWS::GuardDuty::Detector
       Properties: { Enable: true, FindingPublishingFrequency: SIX_HOURS }
   Outputs:
     DetectorId: { Value: !Ref Detector }
3. Wait for stack create to complete and read the DetectorId output (or DescribeStackResources for the physical id).
4. Call guardduty GetDetector(DetectorId=<that value>).
5. Observe HTTP 400 BadRequestException "The request is rejected because the parameter detectorId has an invalid value." instead of a 200 whose Status is ENABLED.
Or replay guardduty.deploy-detector-cfn-behaves and guardduty.deploy-guardduty-detector-cfn-active-behavior.
