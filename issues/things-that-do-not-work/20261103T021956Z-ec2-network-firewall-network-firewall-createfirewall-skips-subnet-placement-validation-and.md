Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__network-firewall.nfwr-firewall-duplicate-name-rejected ec2__network-firewall.nfwr-firewall-subnet-from-other-vpc-rejected ec2__network-firewall.nfwr-firewall-two-subnets-one-zone-rejected ec2__network-firewall.e2e-nfwr-verify-ec-traffic-routes-through-network-firewall-lifecycle
Issue-Key: ec2__network-firewall.nfwr-firewall-duplicate-name-rejected|run#10|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: ec2__network-firewall.nfwr-firewall-subnet-from-other-vpc-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__network-firewall.nfwr-firewall-two-subnets-one-zone-rejected|_client_error#8|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it, so there is no error: {result!r}")
Issue-Key: ec2__network-firewall|CreateStack|ValidationError
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# Network Firewall CreateFirewall skips subnet-placement validation and uses the wrong exception class for a duplicate name (3 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewall

## Description
Three net-new root causes in Network Firewall's CreateFirewall validation, replayed under the `ec2__network-firewall` suite heading.

Cause 1 of 3: a duplicate firewall name is refused with the wrong exception class.
AWS: recorded behavior only. CreateFirewall with a FirewallName already in use in the account/region is refused InvalidRequestException (HTTP 400) - the class AWS uses for a request that is structurally wrong for the current state.
Emulator: answers InvalidOperationException instead.
Why: the two classes are modelled separately and SDK retry/error-mapping branches on them; a client that treats InvalidRequestException as "the name is taken, pick another" never reaches that branch, and one that treats InvalidOperationException as retryable loops.
Fix: the duplicate-name check in the CreateFirewall handler's validation.
Traces: ec2__network-firewall.nfwr-firewall-duplicate-name-rejected

Cause 2 of 3: CreateFirewall accepts a SubnetMapping whose subnet is in another VPC.
AWS: recorded behavior only. Every subnet in SubnetMappings must belong to the firewall's VpcId; a subnet from a different VPC is refused.
Emulator: returns HTTP 200 and provisions the firewall - the replay shows a Firewall with VpcId of one VPC and SubnetMappings naming a subnet of another, with FirewallStatus.Status='PROVISIONING' and a SyncStates attachment and endpoint minted for it.
Why: the firewall endpoint cannot be attached on AWS, so the emulator provisions and reports as healthy an inspection path that does not exist. Routing tests that send traffic through the firewall endpoint pass locally and the deployment fails in production.
Fix: the SubnetMappings/VpcId consistency check in the CreateFirewall handler, before state is written and SyncStates are synthesised.
Traces: ec2__network-firewall.nfwr-firewall-subnet-from-other-vpc-rejected

Cause 3 of 3: CreateFirewall accepts two subnets in the same Availability Zone.
AWS: recorded behavior only. SubnetMappings may name at most one subnet per Availability Zone; two subnets in one AZ are refused.
Emulator: returns HTTP 200 and creates BOTH attachments, synthesising a SyncStates entry per subnet (the replay shows us-west-2a and us-west-2b entries derived from two subnets that the check placed in a single zone).
Why: one endpoint per AZ is the invariant route tables are built against; two endpoints in one zone makes the route-to-endpoint mapping ambiguous, and any derivation of "the firewall endpoint for this AZ" silently picks one.
Fix: the per-AZ uniqueness check over SubnetMappings in the CreateFirewall handler.
Traces: ec2__network-firewall.nfwr-firewall-two-subnets-one-zone-rejected

The group's end-to-end check ec2__network-firewall.e2e-nfwr-verify-ec-traffic-routes-through-network-firewall-lifecycle also fails unmarked in this replay; it exercises the same CreateFirewall-to-routing path and should be re-replayed with the three above.

## Evidence
Replayed failing checks under `### ec2__network-firewall` (none marked ALREADY FILED / OWNED by):

1. ec2__network-firewall.nfwr-firewall-duplicate-name-rejected -> AssertionError: a duplicate firewall name returned 'InvalidOperationException', expected InvalidRequestException
2. ec2__network-firewall.nfwr-firewall-subnet-from-other-vpc-rejected -> AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'Firewall': {'FirewallName': '...-bad', 'FirewallArn': 'arn:aws:network-firewall:us-west-2:000000000000:firewall/...', 'FirewallPolicyArn': '...', 'VpcId': 'vpc-00000000000000024', 'SubnetMappings': [{'SubnetId': 'subnet-00000000000000033'}], ...}, 'FirewallStatus': {'Status': 'PROVISIONING', 'ConfigurationSyncStateSummary': 'IN_SYNC', 'SyncStates': {'us-west-2a': {'Attachment': {'SubnetId': 'subnet-00000000000000033', 'EndpointId': 'vpce-...', 'Status': 'CREATING'}, ...}}}, ... 'HTTPStatusCode': 200 ...}
3. ec2__network-firewall.nfwr-firewall-two-subnets-one-zone-rejected -> AssertionError: the call succeeded; real AWS rejects it, so there is no error: {'Firewall': {... 'SubnetMappings': [{'SubnetId': 'subnet-0000000000000002b'}, {'SubnetId': 'subnet-0000000000000002f'}] ...}, 'FirewallStatus': {'Status': 'PROVISIONING', 'SyncStates': {'us-west-2b': {...}, 'us-west-2a': {...}}}, ...}
4. ec2__network-firewall.e2e-nfwr-verify-ec-traffic-routes-through-network-firewall-lifecycle -> fails unmarked in the same group (errored verdict), exercising the same create-then-route path.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the checks appear under `### ec2__network-firewall`, unmarked.
2. Standalone repros (one firewall policy plus two VPCs with subnets):
   - CreateFirewall(FirewallName=N, FirewallPolicyArn=P, VpcId=A, SubnetMappings=[{SubnetId:<subnet in A>}]) then repeat with the same FirewallName: the second must fail InvalidRequestException.
   - CreateFirewall(VpcId=A, SubnetMappings=[{SubnetId:<subnet in VPC B>}]): must be refused; no Firewall or SyncStates entry may be returned.
   - CreateFirewall(VpcId=A, SubnetMappings=[{SubnetId:<subnet1 in AZ a>},{SubnetId:<subnet2 in AZ a>}]): must be refused.
