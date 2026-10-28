Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.deploy-hosted-zone-cfn-record-resolves
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route-53|TestDNSAnswer|NoSuchHostedZone
Issue-Commit: cb933d25eac9e1275ee3ecb224ec471b338d603c

# CloudFormation Ref on AWS::Route53::HostedZone yields the zone NAME, so TestDNSAnswer on a CFN-deployed zone is refused with NoSuchHostedZone

> _Note: this issue was identified against the code base at commit `cb933d25eac9e1275ee3ecb224ec471b338d603c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource provider for AWS::Route53::HostedZone uses the zone's DNS name as the resource's Ref value (physical id) instead of the hosted zone ID, so a stack output of {"Ref": "<zone resource>"} hands downstream Route 53 calls a value that is not a hosted zone id.

NOTE ON OVERLAP (this is a NET-NEW failing check, not a duplicate filing): open issues 20261025T023940Z-route53-cloudformation-ref-on-aws-route53-hostedzone-still-yields.md and 20261024T114159Z-route53-cloudformation-ref-on-aws-route53-hostedzone-yields-the.md describe the same root cause, but their replay keys are bound to the InvalidInput/ValidationException refusal (a zone name longer than the 32-char ResourceId limit) and neither claims route53.deploy-hosted-zone-cfn-record-resolves. In this check the generated zone name happens to be 30 characters, so it passes the length constraint and the call reaches lookup, producing a DIFFERENT refusal (404 NoSuchHostedZone) that no open issue carries. If those issues are still open, close all three together.

AWS: Ref on an AWS::Route53::HostedZone resource returns the hosted zone ID (bare form, e.g. Z1D633PJN98FT9) — which is why templates write `HostedZoneId: {"Ref": "Zone"}` on an AWS::Route53::RecordSet and export the same value as a stack Output. The route53 Smithy model fixes the shape of that value: TestDNSAnswer.HostedZoneId targets com.amazonaws.route53#ResourceId, a string with @length(min:0,max:32) (httpQuery 'hostedzoneid', required); TestDNSAnswer's modeled errors are InvalidInput (400) and NoSuchHostedZone (404). A DNS name is therefore never a legal value. Recorded behavior: the Ref-valued output is passed straight to TestDNSAnswer and resolves the record created by the sibling AWS::Route53::RecordSet (ResponseCode NOERROR, RecordData exactly the record's value).
Emulator: the stack reaches CREATE_COMPLETE, and the Output whose Value is {"Ref": "Zone"} carries the zone DNS NAME. Feeding that value back to route53 yields HTTP 404 NoSuchHostedZone with message 'No hosted zone found with ID: <value passed as HostedZoneId>' — fixed text 'No hosted zone found with ID: ', variable part the value taken from the request (in this run 'sim2real-r53-7adc983cf6fd.com.'). When the generated name exceeds 32 characters the same defect instead surfaces as 400 InvalidInput on the ResourceId length constraint, so the observable refusal depends only on the length of the zone name.
Why: every CloudFormation stack that references its own Route 53 zone is broken end to end. The sibling AWS::Route53::RecordSet is wired to a non-id, and any consumer of the stack output (another stack, a script, an SDK call) gets a 4xx instead of the zone or the DNS answer. CREATE_COMPLETE makes the stack look healthy, so the breakage only surfaces at first use of the output.
Fix: the AWS::Route53::HostedZone resource provider in the cloudformation service crate (services/service-com-amazonaws-cloudformation-cloudformation/src) — record the id returned by CreateHostedZone (bare, <= 32 chars) as the resource's physical id / Ref value and propagate it to AWS::Route53::RecordSet's HostedZoneId.
Traces: route53.deploy-hosted-zone-cfn-record-resolves.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline.

Check route53.deploy-hosted-zone-cfn-record-resolves (verdict: error). Template: Resources.Zone = AWS::Route53::HostedZone {Name: {"Ref": "ZoneName"}}; Resources.RecordSet = AWS::Route53::RecordSet {HostedZoneId: {"Ref": "Zone"}, Name: www.<zone>, Type: A, TTL: 300, ResourceRecords: [192.0.2.123]}; Outputs.ZoneId.Value = {"Ref": "Zone"}.

Expected (AWS): DescribeStacks output ZoneId is a hosted zone ID (<= 32 chars); route53.test_dns_answer(HostedZoneId=<that id>, RecordName=www.<zone>, RecordType='A') returns ResponseCode 'NOERROR' and RecordData ['192.0.2.123'].

Actual (emulator): the stack reaches CREATE_COMPLETE and the output is the zone DNS name, so the subsequent call fails:
botocore.errorfactory.NoSuchHostedZone: An error occurred (NoSuchHostedZone) when calling the TestDNSAnswer operation: No hosted zone found with ID: sim2real-r53-7adc983cf6fd.com.
(HTTP 404, code NoSuchHostedZone; the id in the message is the Ref value from the request, i.e. the zone name the provider published.)

Model evidence (smithy_query on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): TestDNSAnswer.HostedZoneId -> ResourceId @length(min:0,max:32), required, httpQuery 'hostedzoneid'; errors InvalidInput(400), NoSuchHostedZone(404).

## Steps to Reproduce
1. CreateStack with a template containing an AWS::Route53::HostedZone resource 'Zone' (Name: <a zone name of 30 characters or fewer, e.g. sim2real-r53-<hex>.com.>), an AWS::Route53::RecordSet whose HostedZoneId is {"Ref": "Zone"} (Type A, TTL 300, ResourceRecords [192.0.2.123]), and an Output ZoneId whose Value is {"Ref": "Zone"}.
2. Wait for CREATE_COMPLETE and read Outputs.ZoneId via DescribeStacks — observe the zone DNS name instead of a hosted zone id.
3. Call route53.test_dns_answer(HostedZoneId=<that output>, RecordName=www.<zone>, RecordType='A') and observe HTTP 404 NoSuchHostedZone ('No hosted zone found with ID: <zone name>') instead of NOERROR with RecordData ['192.0.2.123'].
