Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.deploy-hosted-zone-cfn-resolves-record route53.deploy-hosted-zone-recordset-resolves-via-cfn
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route-53|TestDNSAnswer|InvalidInput
Issue-Key: route-53|GetHostedZone|InvalidInput
Issue-Commit: df6081fa6700f84ddafc058f223e27db3aea4084

# CloudFormation Ref on AWS::Route53::HostedZone still yields the zone NAME, not the hosted zone ID (now refused as InvalidInput)

> _Note: this issue was identified against the code base at commit `df6081fa6700f84ddafc058f223e27db3aea4084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource provider for AWS::Route53::HostedZone reports the zone's DNS name as the resource's Ref value (physical id) instead of the hosted zone ID, so any template that references its own zone hands downstream route53 calls a value that is not a hosted zone id.

NOTE ON OVERLAP: open issue 20261024T114159Z-route53-cloudformation-ref-on-aws-route53-hostedzone-yields-the.md describes this same root cause and lists these same two checks, but it did not claim them in this replay — its replay keys name the error code ValidationException, whereas the emulator now refuses these calls with InvalidInput. This filing re-grounds the cause in the current failing checks and the current error code; if that issue is still open, close them together.

AWS: Ref on an AWS::Route53::HostedZone resource returns the hosted zone ID (the bare form, e.g. Z1D633PJN98FT9), which is why templates write `HostedZoneId: !Ref HostedZone` on an AWS::Route53::RecordSet and export it as a stack output. The route53 Smithy model fixes the shape of that value: GetHostedZone.Id and TestDNSAnswer.HostedZoneId both target com.amazonaws.route53#ResourceId with @length(min:0, max:32), so a DNS name can never be a legal value for either. Recorded behavior of both checks: the stack's Ref-valued output is passed straight to GetHostedZone / TestDNSAnswer and is accepted.
Emulator: the stack reaches CREATE_COMPLETE, but the output whose Value is {"Ref": "<zone resource>"} carries the zone DNS name. Feeding it back to route53 then fails the modeled length constraint with HTTP 400 code InvalidInput: '1 validation error detected: Value <zone name> at 'id' failed to satisfy constraint: Member must have length less than or equal to 32' (GetHostedZone) and the same against 'hostedZoneId' (TestDNSAnswer).
Why: every Route 53 stack that references its own zone is broken end to end. Sibling resources in the same template (AWS::Route53::RecordSet with HostedZoneId: !Ref Zone) are wired to a non-id, and any consumer of the stack output — another stack, a script, an SDK call — gets a 400 instead of the zone. CREATE_COMPLETE makes the stack look healthy, so the failure only surfaces at first use of the output.
Fix: the AWS::Route53::HostedZone resource provider in the cloudformation service crate (services/service-com-amazonaws-cloudformation-cloudformation/src) — record the id returned by CreateHostedZone (bare, <= 32 chars) as the resource's physical id / Ref value, and propagate that to AWS::Route53::RecordSet's HostedZoneId.
Traces: route53.deploy-hosted-zone-cfn-resolves-record, route53.deploy-hosted-zone-recordset-resolves-via-cfn.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline.

Template (both checks, JSON and YAML forms): Resources.Zone = AWS::Route53::HostedZone {Name: <zone>}; Resources.Record = AWS::Route53::RecordSet {HostedZoneId: !Ref Zone, Type: A, TTL: 60, ResourceRecords: [203.0.113.x]}; Outputs.<id>.Value = !Ref Zone.
Expected (AWS): the output is a hosted zone ID (<= 32 chars) accepted by GetHostedZone / TestDNSAnswer.
Actual (emulator):
- botocore.errorfactory.InvalidInput: An error occurred (InvalidInput) when calling the TestDNSAnswer operation: 1 validation error detected: Value 'sim2real-cfn-zone77ebe7c7dd07.com.' at 'hostedZoneId' failed to satisfy constraint: Member must have length less than or equal to 32 (route53.deploy-hosted-zone-cfn-resolves-record)
- botocore.errorfactory.InvalidInput: An error occurred (InvalidInput) when calling the GetHostedZone operation: 1 validation error detected: Value 'sim2real-r53-cfn-7637a29174b8.example.' at 'id' failed to satisfy constraint: Member must have length less than or equal to 32 (route53.deploy-hosted-zone-recordset-resolves-via-cfn)

Model evidence (smithy_query on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): ResourceId is a string with @length(min:0,max:32); GetHostedZone.Id and TestDNSAnswer.HostedZoneId target it (both @httpLabel, required).

## Steps to Reproduce
1. CreateStack with a template containing an AWS::Route53::HostedZone named 'Zone', an AWS::Route53::RecordSet whose HostedZoneId is {"Ref": "Zone"}, and an Output whose Value is {"Ref": "Zone"}.
2. Wait for CREATE_COMPLETE and read the output via DescribeStacks — the value is the zone DNS name, not a hosted zone id.
3. Pass that value to route53.get_hosted_zone(Id=...) or route53.test_dns_answer(HostedZoneId=...) and observe HTTP 400 InvalidInput on the 32-char ResourceId constraint instead of the zone / the DNS answer.
