Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__route53.route53-private-hosted-zone-name-resolves-only-from-inside-the-associated-vpc ec2__lambda__route53.route53-private-zone-name-rejected-with-nxdomain-after-the-vpc-is-disassociated
Issue-Key: ec2__lambda__route53.route53-private-hosted-zone-name-resolves-only-from-inside-the-associated-vpc|_prove_in_vpc#16|AssertionError|assert reachable, (
Issue-Key: ec2__lambda__route53.route53-private-zone-name-rejected-with-nxdomain-after-the-vpc-is-disassociated|_prove_in_vpc#18|AssertionError|assert reachable, (
Issue-Commit: 3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e

# ec2__lambda__route53: a VpcConfig-attached Lambda has no VPC network — it cannot reach the VPC's .2 resolver, so private hosted zone resolution is entirely unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a function created with VpcConfig naming a subnet of a VPC is not placed in that VPC's network. From inside the handler, the VPC's reserved resolver address (the VPC CIDR base + 2) is unreachable and the sandbox has no source address in the VPC CIDR, so neither the positive nor the negative half of private-DNS behaviour can be measured. Failing checks: ec2__lambda__route53.route53-private-hosted-zone-name-resolves-only-from-inside-the-associated-vpc, ec2__lambda__route53.route53-private-zone-name-rejected-with-nxdomain-after-the-vpc-is-disassociated.

AWS: recorded behavior only — VPC networking is not described by a Smithy shape. AWS attaches a VpcConfig-configured function to an ENI in the named subnet: the execution environment has an address inside the VPC CIDR and can send DNS to the VPC's Amazon-provided resolver at the second address of the VPC CIDR (172.31.0.2 for 172.31.0.0/16). A Route 53 private hosted zone associated with that VPC resolves from inside it and only from inside it; after DisassociateVPCFromHostedZone the same name answers NXDOMAIN.

Emulator: the handler has no network at all. The replay's in-VPC vantage point reports `reply: {'ok': False, 'stage': 'connect', 'error': 'OSError: a Lambda function in this emulator has no host network to reach'}` and `route: {'src_ip': None, 'src_ip_error': 'OSError: errno=None a Lambda function in this emulator has no host network to reach'}`, and the check aborts with `AssertionError: the in-VPC vantage point cannot reach the VPC resolver 172.31.0.2 ... So this execution environment is not in that VPC and NOTHING about the private zone was measured`. The refusal text is the emulator's own announcement from inside the sandbox, so this is the emulator's behaviour, not the runner's.

Why: a client cannot model any VPC-scoped behaviour of a Lambda — private hosted zones, VPC endpoints reached by name, in-VPC service addresses, or the security boundary that a VPC-only name is supposed to provide. A function that AWS would confine to the VPC is in the emulator confined to nothing, and a name that should be private is neither resolvable nor provably private.

Fix: the Lambda execution-environment networking for VpcConfig-attached functions — the sandbox must have an address in the configured subnet's CIDR and must route DNS to the VPC resolver at the VPC CIDR's .2 address, which in turn must answer from the private hosted zones associated with that VPC (and NXDOMAIN once the VPC is disassociated).

Traces: both checks listed above.

## Evidence
Expected (AWS): a function created with VpcConfig for a subnet of VPC 172.31.0.0/16 has a source address inside 172.31.0.0/16 and can connect to the VPC resolver at 172.31.0.2; a private hosted zone associated with that VPC resolves from there, and answers NXDOMAIN after the VPC is disassociated.

Actual (emulator): `AssertionError: the in-VPC vantage point cannot reach the VPC resolver 172.31.0.2, an RFC 1918 address belonging to the VPC 172.31.0.0/16 that was passed in VpcConfig. So this execution environment is not in that VPC and NOTHING about the private zone was measured: {'resolver': '172.31.0.2', 'reply': {'ok': False, 'stage': 'connect', 'errno': None, 'error': 'OSError: a Lambda function in this emulator has no host network to reach'}, 'route': {'src_ip': None, 'src_ip_error': 'OSError: errno=None a Lambda function in this emulator has no host network to reach'}}`. The CIDR/resolver values above are the VPC the request created; the requirement is that the sandbox's address lies in the configured subnet's CIDR and that the CIDR's .2 resolver is reachable.

## Steps to Reproduce
1. Create a VPC and subnet, and a Route 53 private hosted zone associated with that VPC holding an A record.
2. Create a Lambda with VpcConfig naming that subnet; from the handler, read its own source address and send a DNS query for the private name to the VPC CIDR's .2 address.
3. Observe the handler cannot connect at all ('a Lambda function in this emulator has no host network to reach') and has no source IP, instead of an in-CIDR address and an answer from the private zone.
4. Replay ec2__lambda__route53.route53-private-hosted-zone-name-resolves-only-from-inside-the-associated-vpc and ec2__lambda__route53.route53-private-zone-name-rejected-with-nxdomain-after-the-vpc-is-disassociated.
