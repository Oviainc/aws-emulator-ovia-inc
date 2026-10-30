Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: network-firewall.disassociate-last-subnet-rejected
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall.disassociate-last-subnet-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 57d63351ca97f1a697a2b67a3b53a7b29fcd1d25

# DisassociateSubnets removes a firewall's last subnet mapping instead of refusing with InvalidRequestException

> _Note: this issue was identified against the code base at commit `57d63351ca97f1a697a2b67a3b53a7b29fcd1d25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateSubnets

## Description
ONE net-new root cause, service network-firewall.

Cause 1 of 1: DisassociateSubnets does not enforce the at-least-one-subnet-mapping invariant, so a call that would strip a firewall's only remaining mapping is accepted and the mapping is destroyed.

Context: this is NOT the gap the open issue 20261023T132856Z-network-firewall-does-not-track-policy-rule-group.md describes in its "Cause 2 of 4". That cause was that DisassociateSubnets answered the scaffold error code 'NotImplemented'. The handler is now served (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/disassociate_subnets.rs carries a real implementation), and the check's failure has moved: the call no longer errors, it SUCCEEDS with HTTP 200. The missing guard that the old cause mentioned in passing is still absent and is now the only reason the check fails, so it is filed here as its own root cause with its own replay key.

AWS: Smithy (com.amazonaws.networkfirewall#NetworkFirewall_20201112, awsJson1_0) models DisassociateSubnets with input SubnetIds (required, AzSubnets) + FirewallArn/FirewallName/UpdateToken, output FirewallArn/FirewallName/SubnetMappings/UpdateToken, and the client errors InvalidOperationException, InvalidRequestException, InvalidTokenException, ResourceNotFoundException, ThrottlingException (all HTTP 400, member Message -> ErrorMessage). InvalidRequestException is documented "The operation failed because of a problem with your request. Examples include: You specified an unsupported parameter name or value...". A firewall must retain at least one subnet mapping: when the SubnetIds of the request would leave the firewall with an empty SubnetMappings set, AWS REFUSES the call with HTTP 400 / InvalidRequestException, nothing is mutated, and DescribeFirewall still reports the mapping. Message (recorded behavior only; the Smithy model does not state the text): fixed text "availability zone subnet(s) cannot be removed as firewall must have at least one subnet mapping, parameter: " followed by the variable part — the offending subnet id(s) taken from the request's SubnetIds, rendered as a bracketed list, e.g. "[<subnet-id>]". Only the bracketed ids vary per request; the sentence before them is fixed. Positive control from the same recording: once a second subnet in another AZ is associated, the identical DisassociateSubnets of the first subnet is ACCEPTED (200) and DescribeFirewall reports only the remaining mapping — so the refusal is conditioned purely on emptying the mapping set, not on disassociation being unsupported.

Emulator: the call is accepted. DisassociateSubnets on a firewall whose only mapping is the named subnet answers HTTP 200 with SubnetMappings: [] and a freshly re-minted UpdateToken, i.e. the firewall is left with no subnet mapping and no endpoint at all. FirewallRecord::disassociate_subnets (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/state.rs, around line 1631) checks only three preconditions — an empty SubnetIds list (SubnetAssociationBreach::NoSubnets), SubnetChangeProtection (Breach::Protected) and a repeated subnet id (Breach::RepeatedSubnet) — and has no check on the size of the surviving mapping set, so the handler never has a breach to map onto InvalidRequestException for this case.

Why: AWS's guard exists so a firewall can never be left without an endpoint in any Availability Zone. Against the emulator a client (or an IaC/automation run that shrinks mappings one AZ at a time) silently drives a firewall into a state real AWS refuses to create: a firewall with zero mappings that routes nothing and that AWS would never have produced. Code that depends on the 400 to stop before the last AZ — or a test that asserts the invariant is enforced — gets a 200, and the destructive effect is already applied by the time it looks.

Fix: the at-least-one-mapping precondition belongs in the firewall record's subnet-mapping mutation in the service's state module (the disassociate path alongside the existing NoSubnets / Protected / RepeatedSubnet breaches), with a new breach arm that the DisassociateSubnets handler's breach mapper turns into InvalidRequestException (HTTP 400) carrying the message above; nothing may be written on that refusal, matching the handler's existing "nothing is written on a refusal" contract. File and line citations are under Traces.

Traces: network-firewall.disassociate-last-subnet-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the built emulator. Per-service: network-firewall 17/22 (77%), 5 failed. Generated firewall/subnet names, the account id 000000000000, the region, ARNs, UpdateToken uuids and timestamps below are environment-specific and are NOT part of any requirement.

network-firewall.disassociate-last-subnet-rejected [mismatch]:
  Setup: CreateVpc 10.88.0.0/16, one subnet 10.88.1.0/24 in the first available AZ, CreateFirewallPolicy (StatelessDefaultActions/StatelessFragmentDefaultActions = [aws:forward_to_sfe]), CreateFirewall with SubnetMappings=[{SubnetId: <that one subnet>}] — all answer 200.
  Call: nfw.disassociate_subnets(FirewallArn=<arn>, SubnetIds=[<the firewall's only subnet>])
  Expected (AWS): HTTP 400, Error.Code 'InvalidRequestException', Error.Message 'availability zone subnet(s) cannot be removed as firewall must have at least one subnet mapping, parameter: [<subnet-id>]' (fixed sentence + the request's subnet id), and DescribeFirewall still reporting that SubnetMapping.
  Actual (emulator): AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'FirewallArn': 'arn:aws:network-firewall:us-west-2:000000000000:firewall/<name>', 'FirewallName': '<name>', 'SubnetMappings': [], 'UpdateToken': '<uuid>', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', ...}}} — the mapping set is emptied.
  The check's later positive control (associate a second subnet in another AZ, then disassociate the first, which must answer 200) is never reached because the rejection assertion fails first; it must pass after the fix.

Code consulted in the checkout: services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/disassociate_subnets.rs (handler is implemented; its breach mapper disassociate_breach_error has arms only for Protected -> InvalidOperationException and NoSubnets / RepeatedSubnet / ZoneCapacity -> InvalidRequestException) and services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/state.rs line ~1631 FirewallRecord::disassociate_subnets (no check on the surviving mapping count).

Smithy consulted (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json): DisassociateSubnets (SubnetIds required -> AzSubnets; errors InvalidOperationException/InvalidRequestException/InvalidTokenException/ResourceNotFoundException/ThrottlingException, all client HTTP 400; output SubnetMappings), InvalidRequestException (structure, client, HTTP 400, member Message -> ErrorMessage).

## Steps to Reproduce
1. aws ec2 create-vpc --cidr-block 10.88.0.0/16 ; aws ec2 create-subnet --vpc-id <vpc> --cidr-block 10.88.1.0/24 --availability-zone <az1>.
2. aws network-firewall create-firewall-policy --firewall-policy-name p1 --firewall-policy '{"StatelessDefaultActions":["aws:forward_to_sfe"],"StatelessFragmentDefaultActions":["aws:forward_to_sfe"]}'
3. aws network-firewall create-firewall --firewall-name fw1 --firewall-policy-arn <p1-arn> --vpc-id <vpc> --subnet-mappings SubnetId=<subnet1>  -> 200, exactly one SubnetMapping.
4. aws network-firewall disassociate-subnets --firewall-arn <fw1-arn> --subnet-ids <subnet1>
   Emulator: HTTP 200 with SubnetMappings: [].
   AWS: HTTP 400 InvalidRequestException 'availability zone subnet(s) cannot be removed as firewall must have at least one subnet mapping, parameter: [<subnet1>]'.
5. aws network-firewall describe-firewall --firewall-arn <fw1-arn> -> after the fix this must still report SubnetMappings containing <subnet1> (nothing written on the refusal).
6. Positive control to re-verify after the fix: aws ec2 create-subnet --vpc-id <vpc> --cidr-block 10.88.2.0/24 --availability-zone <az2>; aws network-firewall associate-subnets --firewall-arn <fw1-arn> --subnet-mappings SubnetId=<subnet2>; then repeat step 4 -> must answer 200 and describe-firewall must report only <subnet2>.
