Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.launch-under-http-tokens-enforced-requires-required
Issue-Key: ec2__ssm.launch-under-http-tokens-enforced-requires-required|_client_error#5|AssertionError|raise AssertionError(
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# RunInstances ignores the IMDSv2-enforcement condition — a launch with MetadataOptions HttpTokens=optional succeeds where AWS refuses it

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Cause 1 of 1: RunInstances does not evaluate the caller's metadata-options condition, so a launch that must be IMDSv2-enforced is accepted with HttpTokens=optional and an instance is actually created.

AWS: when the caller is restricted to IMDSv2-enforced launches (the documented `ec2:MetadataHttpTokens` condition set to `required`), RunInstances with MetadataOptions.HttpTokens=`optional` is refused 403 `UnauthorizedOperation` with the message `You are not authorized to perform this operation.`; no reservation and no instance are created. Only HttpTokens=`required` is accepted. Smithy (ec2): RunInstances input MetadataOptions -> InstanceMetadataOptionsRequest with member HttpTokens (enum optional|required); the service's common errors include UnauthorizedOperation. Fixed text of the refusal message as quoted, no variable parts.

Emulator: the refused call SUCCEEDS — RunInstances returns 200 with a full Reservation (ReservationId, one Instance with its ENI, volume and security group), so an instance is launched with IMDSv1 still permitted.

Why: this is the primary guardrail against instance-credential theft via IMDSv1 (SSRF). A client that enforces IMDSv2 by condition believes every launch is hardened, while the emulator lets an IMDSv1-capable instance through — the guardrail is silently absent and nothing in the response signals it. A negative test of the guardrail also passes vacuously.

Fix: the RunInstances handler's authorization/validation step in the EC2 service — the metadata-options condition must be evaluated against MetadataOptions.HttpTokens before the reservation is created, and refused with UnauthorizedOperation when the request does not satisfy it.

Traces: check ec2__ssm.launch-under-http-tokens-enforced-requires-required.

Related but not covering: open issue 20261107T075601Z-ec2-ssm-windows-amis-report-platform-windows-instead.md mentions launch-validation gaps for other properties and owns other checks under this heading; this check is not carried by it.

## Evidence
Expected: RunInstances(..., MetadataOptions={'HttpTokens': 'optional'}) under the IMDSv2-enforcing condition is refused 403 UnauthorizedOperation `You are not authorized to perform this operation.`, with no instance created; the same call with HttpTokens='required' succeeds.

Actual: `AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'ReservationId': 'r-...', 'OwnerId': '...', 'Instances': [{... 'NetworkInterfaces': [...], 'RootDeviceName': '/dev/xvd...'}]}` — a 200 Reservation with a live instance. The ids, IPs, MAC, timestamps and token in that payload are per-run values; the divergence is that a 200 was returned at all instead of the 403 refusal.

## Steps to Reproduce
1. Arrange the caller so IMDSv2 enforcement applies (the ec2:MetadataHttpTokens=required condition on ec2:RunInstances).
2. Call ec2 RunInstances(ImageId=..., InstanceType='t3.micro', SubnetId=..., MetadataOptions={'HttpTokens':'optional'}).
3. The call returns 200 with a Reservation and a running instance; expected 403 UnauthorizedOperation "You are not authorized to perform this operation." and no instance.
