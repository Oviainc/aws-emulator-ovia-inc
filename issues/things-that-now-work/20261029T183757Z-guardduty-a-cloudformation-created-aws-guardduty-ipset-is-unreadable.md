Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.deploy-ipset-cfn-becomes-active-and-usable guardduty.deploy-ipset-cfn-behaves
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetIPSet|BadRequestException
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# A CloudFormation-created AWS::GuardDuty::IPSet is unreadable: GetIPSet rejects the stack resource's physical id

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIPSet

## Description
This issue carries 1 root cause in the guardduty crate (reached through CloudFormation).

Cause 1 of 1: the physical resource id CloudFormation reports for an AWS::GuardDuty::IPSet is not an id GuardDuty itself will accept.
AWS: Smithy GetIPSet takes DetectorId and IpSetId as httpLabels and returns Name/Format/Location/Status (errors BadRequestException 400, InternalServerErrorException 500). Recorded behavior: after a stack containing AWS::GuardDuty::Detector plus AWS::GuardDuty::IPSet reaches CREATE_COMPLETE, the IPSet's physical resource id (from DescribeStackResource) is the IpSetId: it appears in ListIPSets for the stack's detector, GetIPSet on it answers 200 with the templated Name, and its Status reaches ACTIVE.
Emulator: GetIPSet on that physical id is refused with HTTP 400 BadRequestException 'The request is rejected because the parameter ipSetId has an invalid value.' — the stack created a resource whose id the GuardDuty surface does not recognize, so neither of the two deploy checks can read or poll it.
Why: every CloudFormation-driven workflow for GuardDuty trusted/threat IP lists breaks at the first read-back: a client that deploys a stack and then verifies (or drives) the IPSet through the GuardDuty API gets a hard refusal for a resource the stack reports as created, and the IPSet is effectively invisible even though the stack succeeded. Hypothesis: the CFN resource provider returns an identifier of a different form (or registers the IPSet under a detector other than the stack's) than the id the GuardDuty state keys IPSets by.
Fix: the AWS::GuardDuty::IPSet resource provider's create path and the physical-id it hands back, and/or the IpSetId lookup in the GetIPSet handler it must agree with (services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/get_ip_set.rs and the cloudformation resource mapping for AWS::GuardDuty::IPSet).
Traces: guardduty.deploy-ipset-cfn-becomes-active-and-usable, guardduty.deploy-ipset-cfn-behaves.

## Evidence
Both checks fail identically on the first read of the stack resource:
'botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the GetIPSet operation: The request is rejected because the parameter ipSetId has an invalid value.'
The id passed is the physical resource id of the stack's GDIPSet resource (template: AWS::GuardDuty::Detector + AWS::GuardDuty::IPSet with Activate: true, Format: TXT, Location: !Ref IpSetLocation, DetectorId: !Ref GDDetector). On real AWS the same id is accepted by GetIPSet, appears in ListIPSets, and reaches Status ACTIVE.

## Steps to Reproduce
1. In an account/region with no GuardDuty detector, put a TXT list object in an S3 bucket.
2. Deploy a CloudFormation stack with AWS::GuardDuty::Detector (Enable: true) and AWS::GuardDuty::IPSet (Activate: true, Format: TXT, Location: the object URL, DetectorId: !Ref of the detector); wait for CREATE_COMPLETE.
3. Read the physical resource ids of both resources via DescribeStackResource.
4. Call ListIPSets(DetectorId) and GetIPSet(DetectorId, IpSetId=<the IPSet physical id>) — the emulator refuses with BadRequestException 'The request is rejected because the parameter ipSetId has an invalid value.'
5. Equivalent to running guardduty.deploy-ipset-cfn-behaves and guardduty.deploy-ipset-cfn-becomes-active-and-usable.
