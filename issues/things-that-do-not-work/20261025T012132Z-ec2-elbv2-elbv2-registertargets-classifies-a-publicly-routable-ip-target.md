Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.ecsnt-ip-target-outside-the-vpc-rejected ec2__elbv2.ip-target-from-a-public-range-rejected ec2__elbv2.ip-target-in-the-vpc-cidr-but-outside-every-subnet-rejected
Issue-Key: ec2__elbv2.ecsnt-ip-target-outside-the-vpc-rejected|run#6|AssertionError|assert refused == {"192.168.5.5": ("ValidationError", "You must specify a valid Availability Zone or select 'all' for all "
Issue-Key: ec2__elbv2.ip-target-from-a-public-range-rejected|run#7|AssertionError|assert message == (
Issue-Key: ec2__elbv2.ip-target-in-the-vpc-cidr-but-outside-every-subnet-rejected|run#8|AssertionError|assert message == (
Issue-Commit: 706a0d41e7cf89b05226ca56bdcce9b8afe63db0

# ELBv2 RegisterTargets classifies a publicly-routable IP target as a missing-AvailabilityZone error, so an unrecoverable refusal is reported as a recoverable one

> _Note: this issue was identified against the code base at commit `706a0d41e7cf89b05226ca56bdcce9b8afe63db0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets

## Description
One root cause, grounded in 3 failing behavioral checks whose check-id leading token is the composite 'ec2__elbv2'.

Note on neighbouring open issues: 20261023T025808Z-ec2-elbv2-the-whole-ec2-elbv2-ip-target-surface-is.md lists these same three check ids, but its root cause is a DIFFERENT one — ELBv2 CreateTargetGroup answering 400 InvalidAction at version 2015-12-01, so no ip target group could be created at all. In this replay CreateTargetGroup succeeds and the checks run all the way into target registration; what now diverges is inside the RegisterTargets ip-address validation, which that issue never reached. 20261024T080337Z-elbv2-modifytargetgroup-skips-two-server-side-health-check.md covers ModifyTargetGroup health-check validation, not target registration.

Cause 1 of 1: RegisterTargets evaluates the "AvailabilityZone is required for an address outside the VPC" rule BEFORE the "the address must be private" rule, so a publicly-routable ip target is refused with the missing-AvailabilityZone class instead of the not-allowed-address class.

AWS: Smithy com.amazonaws.elasticloadbalancingv2#RegisterTargets(TargetGroupArn required, Targets required: list of TargetDescription{Id, Port @range 1..65535, AvailabilityZone}) models InvalidTargetException (awsQueryError InvalidTarget, HTTP 400), TargetGroupNotFound, TooManyRegistrationsForTargetId, TooManyTargets. TargetDescription.AvailabilityZone is documented: "If the target type is ip and the IP address is in a subnet of the VPC for the target group, the Availability Zone is automatically detected and this parameter is optional. If the IP address is outside the VPC, this parameter is required." The three rejection classes themselves are recorded behavior only (the model does not name ValidationError); the recordings show all three answered with Error.Code 'ValidationError' at HTTP 400 and show AWS picking the class from the ADDRESS first:
  - a publicly-routable address (8.8.8.8, 192.0.2.10) is refused for being public, with or without AvailabilityZone — example message "The IP address '192.0.2.10' is not allowed, as it must be an address from a VPC subnet, RFC1918, or RFC6598". This class is UNRECOVERABLE: retrying with AvailabilityZone='all' is still refused.
  - a private address outside the target group's VPC (192.168.5.5) and no AvailabilityZone is refused for the missing zone — example message "You must specify a valid Availability Zone or select 'all' for all enabled Availability zones, since the IP address '192.168.5.5' is outside the VPC". This class IS recoverable: the identical call with AvailabilityZone='all' succeeds (the recording shows it accepted).
  - an address inside the VPC CIDR but in no subnet is refused for that, with no AvailabilityZone remedy — example message "The IP address '10.x.10.20' is within the VPC, but is not within a VPC subnet so it can't be registered".
The requirement is the rule, not the recorded text: each class must be selected by the address, and the message must name the offending address from the request together with that class's constraint.

Emulator: the address class is decided only after a zone-presence test. Registering 8.8.8.8 with no AvailabilityZone into an ip target group is answered ('ValidationError', "You must specify a valid Availability Zone or select 'all' for all enabled Availability Zones for the load balancer") — the same answer it gives for the private out-of-VPC address 192.168.5.5, and it also drops the "...since the IP address X is outside the VPC" clause that identifies that class. The public-address branch does exist (192.0.2.10 WITH AvailabilityZone='all' is refused 'ValidationError', "The IP address '192.0.2.10' is not valid: you can't specify publicly routable IP addresses"), it is simply never reached when the zone is absent. The in-CIDR/out-of-subnet branch likewise reports a differently-shaped message that leaks an internal VPC id ("The IP address '10.154.10.20' is in the CIDR of VPC vpc-0000000000000001e but is not in any of its subnets").

Why: a client that reads the refusal to decide what to do next is told the wrong thing. The missing-AvailabilityZone class means "supply a zone and retry", so an ECS/registration controller, a Terraform-style reconciler or an operator diagnosing 'no healthy targets' will retry the public address with AvailabilityZone='all' — which on AWS is permanently refused. Conversely the three distinct diagnoses AWS gives for the three address classes (public, private-outside-VPC, in-CIDR-but-no-subnet) collapse into one on the emulator, so the exact fault that a troubleshooting runbook keys on is unobservable. All three classes are HTTP 400 'ValidationError', so the status and code are already right; what diverges is which rule fires and therefore which class is reported.

Fix: the ip-target address classification in the ELBv2 RegisterTargets handler's validation in the elasticloadbalancingv2 service crate — test the address against the private ranges (VPC subnet / RFC1918 / RFC6598) and against the target group's VPC CIDR and subnet set BEFORE testing for the presence of TargetDescription.AvailabilityZone, and emit that class's refusal naming the address from the request.

Traces: ec2__elbv2.ecsnt-ip-target-outside-the-vpc-rejected is the direct example (8.8.8.8 with no zone answered as missing-zone). ec2__elbv2.ip-target-from-a-public-range-rejected and ec2__elbv2.ip-target-in-the-vpc-cidr-but-outside-every-subnet-rejected exercise the other two branches of the same classifier and assert the per-class refusal text it emits; they are listed so the fixer re-replays the whole classifier.

## Evidence
ec2__elbv2.ecsnt-ip-target-outside-the-vpc-rejected — AssertionError: refusals: {'192.168.5.5': ('ValidationError', "You must specify a valid Availability Zone or select 'all' for all enabled Availability Zones for the load balancer"), '8.8.8.8': ('ValidationError', "You must specify a valid Availability Zone or select 'all' for all enabled Availability Zones for the load balancer")}.
Expected (recorded AWS): {'192.168.5.5': ('ValidationError', "You must specify a valid Availability Zone or select 'all' for all enabled Availability zones, since the IP address '192.168.5.5' is outside the VPC"), '8.8.8.8': ('ValidationError', "The IP address '8.8.8.8' is not allowed, as it must be an address from a VPC subnet, RFC1918, or RFC6598")} — i.e. the public address must be refused for being public, not for a missing zone.

ec2__elbv2.ip-target-from-a-public-range-rejected — the same classifier, reached with AvailabilityZone='all', DOES take the public branch: AssertionError: public-range refusal message: "The IP address '192.0.2.10' is not valid: you can't specify publicly routable IP addresses" (recorded AWS example: "The IP address '192.0.2.10' is not allowed, as it must be an address from a VPC subnet, RFC1918, or RFC6598"). This is what proves the branch exists but is ordered after the zone test.

ec2__elbv2.ip-target-in-the-vpc-cidr-but-outside-every-subnet-rejected — AssertionError: out-of-subnet refusal message: "The IP address '10.154.10.20' is in the CIDR of VPC vpc-0000000000000001e but is not in any of its subnets" (recorded AWS example: "The IP address '10.154.10.20' is within the VPC, but is not within a VPC subnet so it can't be registered").

Smithy citation: RegisterTargets(TargetGroupArn required, Targets required) errors InvalidTargetException {awsQueryError InvalidTarget, httpResponseCode 400}, TargetGroupNotFoundException, TooManyRegistrationsForTargetIdException, TooManyTargetsException; TargetDescription.AvailabilityZone doc: "If the IP address is outside the VPC, this parameter is required." The ValidationError code and the per-class messages are recorded behavior only.

## Steps to Reproduce
1. Boot the emulator.
2. ec2.create_vpc(CidrBlock='10.0.0.0/16') and create at least one subnet in it (e.g. 10.0.0.0/20).
3. elbv2.create_target_group(Name='sim2real-ip-tg', Protocol='HTTP', Port=80, VpcId=<vpc>, TargetType='ip').
4. elbv2.register_targets(TargetGroupArn=<tg>, Targets=[{'Id': '8.8.8.8', 'Port': 80}])  # publicly routable, NO AvailabilityZone.
   Observed: ValidationError "You must specify a valid Availability Zone or select 'all' for all enabled Availability Zones for the load balancer".
   Expected: ValidationError refusing the address itself because it is not from a VPC subnet / RFC1918 / RFC6598 — the refusal must NOT suggest that supplying an AvailabilityZone would help.
5. elbv2.register_targets(TargetGroupArn=<tg>, Targets=[{'Id': '8.8.8.8', 'Port': 80, 'AvailabilityZone': 'all'}]) — still refused, and the message now takes the public-address branch, showing the two rules are evaluated in the wrong order.
6. elbv2.register_targets(TargetGroupArn=<tg>, Targets=[{'Id': '192.168.5.5', 'Port': 80}]) — private, outside the VPC, no zone: expected refusal names the address and says it is outside the VPC; observed message omits that clause.
7. elbv2.register_targets(TargetGroupArn=<tg>, Targets=[{'Id': '10.0.200.20', 'Port': 443}]) — inside the VPC CIDR, in no subnet: expected refusal says the address is within the VPC but not within a VPC subnet; observed message reports an internal VPC id instead.
8. Re-replay the 3 check ids in trace_ids.
