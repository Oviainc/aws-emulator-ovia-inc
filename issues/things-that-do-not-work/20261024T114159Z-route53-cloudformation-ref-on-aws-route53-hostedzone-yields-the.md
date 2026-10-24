Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.deploy-hosted-zone-cfn-resolves-record route53.deploy-hosted-zone-recordset-resolves-via-cfn
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route-53|TestDNSAnswer|ValidationException
Issue-Key: route-53|GetHostedZone|ValidationException
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# CloudFormation Ref on AWS::Route53::HostedZone yields the zone NAME, not the hosted zone ID, so every CFN-deployed Route 53 stack is unusable downstream

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource provider for `AWS::Route53::HostedZone` returns the zone's DNS name as the resource's Ref value (physical id) instead of the hosted zone ID.

AWS: `Ref` on an AWS::Route53::HostedZone resource returns the hosted zone ID (the `/hostedzone/`-less id form, e.g. `Z1D633PJN98FT9`) — which is precisely why templates write `HostedZoneId: !Ref HostedZone` on an AWS::Route53::RecordSet and export it as a stack output. The route53 Smithy model fixes the shape that value must have: `GetHostedZone.Id` and `TestDNSAnswer.HostedZoneId` both target `ResourceId` with `@length(min:0, max:32)`, so a DNS name is not a legal value for either. Recorded behavior of both checks: the stack's `Ref`-valued output is passed straight to GetHostedZone / TestDNSAnswer and must be accepted.

Emulator: the stack reaches CREATE_COMPLETE, but the output whose Value is `{"Ref": "<hosted zone resource>"}` carries the zone name — e.g. `sim2real-r53-cfn-7b729b325d05.example.` and `sim2real-cfn-zone4b72b8fa359a.com.`. Feeding that back to route53 then fails the modeled length constraint: HTTP 400 `ValidationException` "1 validation error detected: Value '<zone name>' at 'id' failed to satisfy constraint: Member must have length less than or equal to 32" (GetHostedZone), and the same against 'hostedZoneId' for TestDNSAnswer.

Why: every Route 53 stack that references its own zone is broken end to end. Downstream resources in the same template (`AWS::Route53::RecordSet` with `HostedZoneId: !Ref Zone`) are wired to a value that is not a hosted zone id, and any consumer of the stack output — another stack, a script, an SDK call — gets a 400 instead of the zone. CREATE_COMPLETE makes the stack look healthy, so the failure only appears at the first use of the output.

Fix: the CloudFormation resource provider for AWS::Route53::HostedZone (the Ref/physical-resource-id it reports, and the value propagated to AWS::Route53::RecordSet's HostedZoneId) in the cloudformation service crate — services/service-com-amazonaws-cloudformation-cloudformation/src. It must record the id returned by CreateHostedZone (bare, <=32 chars) as the resource's Ref value, not the zone name.

Traces: route53.deploy-hosted-zone-cfn-resolves-record, route53.deploy-hosted-zone-recordset-resolves-via-cfn.

## Evidence
Template: Resources.Zone = AWS::Route53::HostedZone {Name: <zone>}; Resources.Record = AWS::Route53::RecordSet {HostedZoneId: {"Ref": "Zone"}}; Outputs.ZoneId.Value = {"Ref": "Zone"}.
Expected (AWS): the output is a hosted zone ID (<=32 chars) accepted by GetHostedZone/TestDNSAnswer.
Actual (emulator):
- botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the GetHostedZone operation: 1 validation error detected: Value 'sim2real-r53-cfn-7b729b325d05.example.' at 'id' failed to satisfy constraint: Member must have length less than or equal to 32 (route53.deploy-hosted-zone-recordset-resolves-via-cfn)
- botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the TestDNSAnswer operation: 1 validation error detected: Value 'sim2real-cfn-zone4b72b8fa359a.com.' at 'hostedZoneId' failed to satisfy constraint: Member must have length less than or equal to 32 (route53.deploy-hosted-zone-cfn-resolves-record)

## Steps to Reproduce
1. CreateStack with a template containing an AWS::Route53::HostedZone named "Zone", an AWS::Route53::RecordSet whose HostedZoneId is {"Ref": "Zone"}, and an Output whose Value is {"Ref": "Zone"}.
2. Wait for CREATE_COMPLETE and read the output via DescribeStacks.
3. Observe the output value is the zone DNS name; pass it to route53.get_hosted_zone(Id=...) and observe HTTP 400 ValidationException on the 32-char ResourceId constraint instead of the zone.
