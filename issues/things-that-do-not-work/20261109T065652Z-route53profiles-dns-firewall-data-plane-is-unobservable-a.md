Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53profiles.dns-firewall-resolution-native-protocol-vpc-only
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles.dns-firewall-resolution-native-protocol-vpc-only|dig#8|AssertionError|assert "FunctionError" not in resp, f"the in-VPC querier failed: {body!r}"
Issue-Commit: 16763728aa87eb81fa6e42d119d21e429fdd84e3

# route53profiles DNS Firewall data plane is unobservable: a VPC-attached Lambda's UDP datagram to the Amazon-provided resolver 169.254.169.253 is refused ENETUNREACH (1 cause)

> _Note: this issue was identified against the code base at commit `16763728aa87eb81fa6e42d119d21e429fdd84e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateResourceToProfile

## Description
Cause 1 of 1: a Lambda function created with VpcConfig naming a subnet of the VPC under test cannot send a UDP datagram to the Amazon-provided DNS resolver address, so no DNS answer can ever be observed from inside the VPC a Route 53 Profile is associated with. The whole data-plane half of Route 53 Profiles — whether a DNS Firewall rule group wired into a Profile and associated to a VPC actually makes the VPC's Resolver answer NXDOMAIN for a blocked domain while a control domain keeps resolving — is therefore unmeasurable. Failing check: route53profiles.dns-firewall-resolution-native-protocol-vpc-only.

AWS: recorded behavior only — in-VPC DNS reachability is not described by any Smithy shape (smithy_query on route53profiles models only the control plane: AssociateResourceToProfile / GetProfileResourceAssociation / AssociateProfile and their Status members). AWS makes the Amazon-provided DNS resolver reachable over UDP/53 at the link-local address 169.254.169.253 (and at the VPC CIDR's .2 address) from any execution environment attached to the VPC via an ENI, which is how a Lambda with VpcConfig queries it. Once a Profile's FIREWALL_RULE_GROUP resource association and its VPC association are both COMPLETE, a query from inside that VPC for a domain on a BLOCK/NXDOMAIN rule's domain list is answered rcode NXDOMAIN with no answer records, while a domain not on the list is still answered normally.

Emulator: the handler's `socket.sendto(pkt, ('169.254.169.253', 53))` raises `OSError: [Errno 101] a Lambda function in this emulator opens no connection: the only address its datagrams reach is the X-Ray daemon named by AWS_XRAY_DAEMON_ADDRESS` (ENETUNREACH, raised in the sandbox's own socket shim at `socket.py` `_reachable`), and the Invoke response carries FunctionError with that errorMessage/errorType `OSError`. The refusal text is the emulator's own announcement from inside the sandbox — it declares that the only datagram destination it will route is the X-Ray daemon address — so this is the emulator's behavior, not the runner's. The check aborts on its very first baseline query (`the in-VPC querier failed: ...`), before any firewall rule exists, so nothing about the Profile's effect on resolution is exercised.

Why: a client cannot model, test or verify any VPC-scoped effect of a Route 53 Profile against the emulator. Profiles exist to push Resolver configuration (DNS Firewall rule groups, resolver rules, query-log configs) onto VPCs, and the only observable proof that the push took effect is a DNS answer from inside the VPC — no API reports it. With in-VPC UDP egress refused, every Profile resource association reads COMPLETE in the control plane while having no observable effect at all, so a client that passes against the emulator learns nothing about whether its blocks apply, and a regression in Resolver/Firewall evaluation is undetectable.

Fix: the Lambda execution-environment networking for VpcConfig-attached functions, plus the VPC Resolver data plane it must reach. The sandbox's datagram routing must not be restricted to the X-Ray daemon address: a datagram to 169.254.169.253:53 (and to the VPC CIDR's .2 resolver) from a VPC-attached function must be delivered to an emulated VPC Resolver, and that Resolver must apply the DNS Firewall rule groups reachable through the Profiles associated with that VPC — answering NXDOMAIN with zero answer records for a domain on a BLOCK/NXDOMAIN rule's domain list, and resolving names not on it. This is the same sandbox-networking root cause as the already-open issue 20261108T074808Z-ec2-lambda-route53-a-vpcconfig-attached-lambda-has (and the sandbox-has-no-network cause in 20261108T074808Z-lambda-2-net-new-causes-an-out-of), which do not carry this check's replay key; this one additionally requires the Profile-driven Firewall evaluation on the answer path.

Traces: route53profiles.dns-firewall-resolution-native-protocol-vpc-only (fails in its `dig` helper on the pre-rule baseline query).

## Evidence
Expected (AWS): after CreateFirewallDomainList + CreateFirewallRule(Action=BLOCK, BlockResponse=NXDOMAIN) + AssociateResourceToProfile(ResourceType FIREWALL_RULE_GROUP) + AssociateProfile(ResourceId=<vpc>) all settle COMPLETE, a UDP/53 query issued from a VpcConfig-attached Lambda in that VPC to 169.254.169.253 is answered: before the rule exists both the blocked and the control domain return answer records; after it, the blocked domain returns rcode NXDOMAIN with an empty answer section and the control domain still returns records.

Actual (emulator): the first in-VPC query already fails. Invoke returns FunctionError with `{'errorMessage': '[Errno 101] a Lambda function in this emulator opens no connection: the only address its datagrams reach is the X-Ray daemon named by AWS_XRAY_DAEMON_ADDRESS', 'errorType': 'OSError', 'stackTrace': [... 'index.py', line 45, in _one\n    s.sendto(pkt, (server, 53))', ... 'socket.py', line 340, in _reachable\n    raise error(_ENETUNREACH, _NO_ROUTE)']}`, and the check reports `AssertionError: the in-VPC querier failed: {...}`.

The fixed text of the emulator's refusal is `a Lambda function in this emulator opens no connection: the only address its datagrams reach is the X-Ray daemon named by AWS_XRAY_DAEMON_ADDRESS` with errno 101 (ENETUNREACH) and errorType `OSError`; AWS raises no such error at all — the datagram is delivered and a DNS response comes back. The resolver address 169.254.169.253, the port 53, the domains example.com./example.net. and the requestId above are this run's values; the requirement is that any UDP datagram from a VPC-attached function to the VPC's Amazon-provided resolver is routed and answered, with the rcode determined by the Firewall rules the associated Profile carries.

## Steps to Reproduce
1. Create a VPC and a subnet in it (ec2 CreateVpc/CreateSubnet) and a Route 53 Profile (route53profiles CreateProfile).
2. Create a Lambda function with VpcConfig naming that subnet, whose handler opens an AF_INET/SOCK_DGRAM socket and sendto()s a DNS query packet to ('169.254.169.253', 53), then recvfrom()s the reply and parses the rcode and answer records.
3. Invoke it for a publicly resolvable domain and a control domain. Observe the invocation returns FunctionError `OSError: [Errno 101] a Lambda function in this emulator opens no connection: the only address its datagrams reach is the X-Ray daemon named by AWS_XRAY_DAEMON_ADDRESS` instead of a DNS answer.
4. (Blocked by step 3 in the emulator) Create a firewall domain list holding the first domain, a firewall rule group with a Priority=1 Action=BLOCK BlockResponse=NXDOMAIN rule, AssociateResourceToProfile the rule group ARN to the Profile and AssociateProfile the Profile to the VPC, poll both to COMPLETE, and re-query: the blocked domain must answer NXDOMAIN with no records while the control domain still resolves.
5. Replay route53profiles.dns-firewall-resolution-native-protocol-vpc-only.
