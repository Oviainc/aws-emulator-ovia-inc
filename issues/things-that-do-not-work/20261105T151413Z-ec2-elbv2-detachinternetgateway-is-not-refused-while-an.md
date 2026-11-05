Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.internet-gateway-detach-from-a-vpc-serving-an-internet-facing-alb-rejected
Issue-Key: ec2__elbv2.internet-gateway-detach-from-a-vpc-serving-an-internet-facing-alb-rejected|_error#17|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: ef7f816f5820fcb580018a7c921bd985d34fbc86

# ec2__elbv2: DetachInternetGateway is not refused while an internet-facing ALB holds mapped public addresses in the VPC (1 cause)

> _Note: this issue was identified against the code base at commit `ef7f816f5820fcb580018a7c921bd985d34fbc86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetachInternetGateway

## Description
Cause 1 of 1 — DetachInternetGateway succeeds on a VPC that still has mapped public addresses (an active internet-facing ALB's ENIs), instead of being refused DependencyViolation.

AWS: Smithy (com.amazonaws.ec2#DetachInternetGateway) documents the operation as "Detaches an internet gateway from a VPC, disabling connectivity between the internet and the VPC. The VPC must not contain any running instances with Elastic IP addresses or public IPv4 addresses." The operation models no typed errors (EC2 query protocol, errors are common/untyped), so the refusal itself is recorded behavior: while an active internet-facing Application Load Balancer holds public addresses in the VPC, DetachInternetGateway is refused with error code `DependencyViolation` and the message `Network <vpc-id> has some mapped public address(es). Please unmap those public address(es) before detaching the gateway.` — everything except `<vpc-id>` is the error class's fixed text; `<vpc-id>` is taken from the request. The refused call is a no-op: DescribeInternetGateways still reports the attachment as `(<vpc-id>, available)`. After DeleteLoadBalancer, once the ALB's ENIs and their public addresses are released, the identical detach succeeds and the gateway reads back attached to nothing.

Emulator: the detach SUCCEEDS (HTTP 200, no error) while the ALB is `active` and internet-facing — `AssertionError: the call succeeded; real AWS rejects it, so there is no error code` raised by the check's `_error()` helper, which only runs because no ClientError was raised. The emulator never evaluates the VPC's public-address dependents, so no `DependencyViolation` is produced and the attachment is torn down under a live load balancer.

Why: a client that relies on AWS's dependency guard to protect a serving VPC gets the opposite answer. Teardown automation that treats a successful detach as "no public dependents remain" will detach the gateway while an internet-facing ALB is still serving, and negative tests that assert the refusal cannot pass; conversely, retry loops that wait for DependencyViolation to clear (the documented way to sequence teardown) observe a state the emulator never reports.

Fix: the DetachInternetGateway handler's dependency validation (services/service-com-amazonaws-ec2-amazonec2/src/ops/detach_internet_gateway.rs) must consult the VPC's mapped public addresses — including the ENIs/public addresses held by active internet-facing ELBv2 load balancers in that VPC — and refuse with `DependencyViolation` and the message above, leaving the attachment unchanged, until those addresses are released.

Traces: ec2__elbv2.internet-gateway-detach-from-a-vpc-serving-an-internet-facing-alb-rejected. No open issue carries this cause: the existing internet-gateway issues (20261104T022117Z-ec2-internet-gateway-ids-break-the-tag-plane.md, 20261103T200742Z-ec2-a-stack-created-vpc-has-no-internet-gateway.md) concern tagging, describe-after-delete and stack-created attachments, not the detach dependency guard.

## Evidence
Failing check under `### ec2__elbv2`, not marked ALREADY FILED or OWNED:

- [mismatch] ec2__elbv2.internet-gateway-detach-from-a-vpc-serving-an-internet-facing-alb-rejected — Error: File "check_ec2__elbv2_internet_gateway_detach_from_a_vpc_serving_an_internet_facing_alb_rejected.py", line 49, in run / got = _error(detach) / line 108, in _error / raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code") / AssertionError: the call succeeded; real AWS rejects it, so there is no error code

Expected (the check's assertion): `ec2.detach_internet_gateway(InternetGatewayId=<igw>, VpcId=<vpc>)` raises ClientError with (Code, Message) == ("DependencyViolation", "Network <vpc> has some mapped public address(es). Please unmap those public address(es) before detaching the gateway."), and DescribeInternetGateways still reports Attachments == [(<vpc>, "available")]. After DeleteLoadBalancer the same call eventually succeeds and Attachments == [].

Actual: the detach returns success on the first call, with the ALB reported `active` and `Scheme == internet-facing`, so there is no error code at all.

## Steps to Reproduce
1. Create an isolated VPC (10.0.0.0/16) with subnets in two AZs, a security group, and an internet gateway; AttachInternetGateway it to the VPC.
2. CreateLoadBalancer an `application` load balancer with `Scheme=internet-facing` on those subnets and poll DescribeLoadBalancers until `State.Code == active`.
3. Call `ec2 detach-internet-gateway --internet-gateway-id <igw> --vpc-id <vpc>`: expect error code `DependencyViolation` with the message `Network <vpc> has some mapped public address(es). Please unmap those public address(es) before detaching the gateway.`, and expect DescribeInternetGateways to still show the attachment `available`.
4. DeleteLoadBalancer, then retry the identical detach until it succeeds and the gateway reports no attachments.
Replay: ec2__elbv2.internet-gateway-detach-from-a-vpc-serving-an-internet-facing-alb-rejected
