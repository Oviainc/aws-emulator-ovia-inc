Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.nat-gateway-subnet-delete-dependency-violation-rejected ec2.op-contract-delete-nat-gateway ec2.op-contract-disassociate-nat-gateway-address ec2.op-contract-get-network-insights-access-scope-content ec2.op-contract-modify-network-interface-attribute ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.nat-gateway-subnet-delete-dependency-violation-rejected|_error#24|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ec2.op-contract-delete-nat-gateway|run#24|AssertionError|assert err == ("NatGatewayMalformed", f"Invalid Nat Gateway {bogus_id}"), f"unexpected error: {err}"
Issue-Key: ec2.op-contract-disassociate-nat-gateway-address|run#17|AssertionError|assert code == "DryRunOperation", code
Issue-Key: ec2.op-contract-get-network-insights-access-scope-content|run#12|AssertionError|assert code == "InvalidParameterValue", f"unexpected error code: {code}"
Issue-Key: ec2.op-contract-modify-network-interface-attribute|run#12|AssertionError|assert message == "Fields for multiple attribute types specified: description, sourceDestCheck", message
Issue-Key: ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently|run#24|AssertionError|assert default_route == igw_id, (
Issue-Commit: e871fc30d8e65c54afe98441bab15a179188eb75

# ec2: 6 net-new behavioral root causes — DeleteSubnet ignores an in-use NAT gateway, DeleteNatGateway/GetNetworkInsightsAccessScopeContent misclassify a bogus id, DisassociateNatGatewayAddress DryRun answers NotImplemented, ModifyNetworkInterfaceAttribute's collision message is emulator-authored, and a CDK VPC's public subnet carries no default route

> _Note: this issue was identified against the code base at commit `e871fc30d8e65c54afe98441bab15a179188eb75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSubnet, DeleteNatGateway, DisassociateNatGatewayAddress, GetNetworkInsightsAccessScopeContent, ModifyNetworkInterfaceAttribute, CreateRoute/DescribeRouteTables

## Description
Six independent EC2 root causes, all grounded in failing behavioral checks the suite report lists under `### ec2` and that no open issue owns (the report marked neither OWNED nor ALREADY FILED for any of them). Each cause must be closed before this issue is submitted.

=== Cause 1 of 6: DeleteSubnet deletes a subnet that still hosts an available NAT gateway ===
AWS: DeleteSubnet refuses a subnet that still carries a dependency. An `available` NAT gateway keeps a service-managed network interface in its subnet, so the delete is refused with HTTP 400, error code `DependencyViolation` and the message `The subnet '<subnet-id>' has dependencies and cannot be deleted.` — the fixed text is `The subnet '` … `' has dependencies and cannot be deleted.` and the only variable part is the subnet id taken from the request. The subnet must still be readable by DescribeSubnets afterwards, and the same DeleteSubnet succeeds (HTTP 200) once the NAT gateway reaches `deleted` (not merely `deleting`). Smithy: DeleteSubnet input `SubnetId` (required) + `DryRun`, output `Unit`; the ec2Query model declares no errors for this operation, so the code/message above are recorded behavior only.
Emulator: answers HTTP 200 with an empty DeleteSubnetResponse and removes the subnet while the NAT gateway is still `available`. The replay shows the call SUCCEEDED where a rejection was expected (`ResponseMetadata.HTTPStatusCode: 200`).
Why: a client that relies on DependencyViolation to detect "still in use" instead destroys live network infrastructure — teardown-ordering logic, dependency walkers and CloudFormation-style reverse-delete sequencing all mis-sequence, and the NAT gateway is left anchored to a subnet that no longer exists.
Fix: the DeleteSubnet handler's pre-delete dependency check (NAT gateways, network interfaces, instances resident in the subnet) must run and refuse before any state mutation.
Traces: ec2.nat-gateway-subnet-delete-dependency-violation-rejected

=== Cause 2 of 6: DeleteNatGateway reports a never-issued NAT gateway id as not-found instead of malformed ===
AWS: a NAT gateway id that has the right surface shape (`nat-` + 17 hex characters) but was never issued by the service is rejected with error code `NatGatewayMalformed` and the message `Invalid Nat Gateway <nat-gateway-id>` — fixed text `Invalid Nat Gateway ` followed by the id copied from the request. AWS validates the identifier beyond a regex (both low-entropy and genuinely random synthetic suffixes are refused the same way) and answers this malformed-id class BEFORE any existence check, so a not-found class is never reached for an id the service never allocated. Smithy: DeleteNatGateway input `NatGatewayId` (required) + `DryRun`; no modeled errors, so the code/message are recorded behavior only.
Emulator: answers error code `NatGatewayNotFound` with the message `The natgateway ID '<nat-gateway-id>' does not exist`.
Why: a client cannot distinguish "this id is not an AWS id" from "the gateway is already gone". Cleanup and retry loops that intentionally swallow a not-found also swallow a typo or a cross-account/cross-region id, so a wrong target is silently accepted.
Fix: the DeleteNatGateway handler's identifier validation, ahead of the state lookup, must classify an id the store never issued as NatGatewayMalformed.
Traces: ec2.op-contract-delete-nat-gateway

=== Cause 3 of 6: DisassociateNatGatewayAddress with DryRun=true answers NotImplemented ===
AWS: EC2's universal dry-run contract — for a request the caller is authorized to make, `DryRun=true` changes nothing and is answered HTTP 412 with error code `DryRunOperation` and the exact message `Request would have succeeded, but DryRun flag is set.` (entirely fixed text, no variable part). The association must still be present afterwards. Smithy: DisassociateNatGatewayAddress input carries `DryRun` (Boolean) alongside `NatGatewayId` (required), `AssociationIds` (required) and `MaxDrainDurationSeconds` (range 1–4000).
Emulator: the DryRun request is refused with error code `NotImplemented`.
Why: a client that preflights a mutation reads NotImplemented as "this API does not exist here" and either aborts or skips the preflight and mutates blind; it can never confirm authorization without side effects.
Fix: the DisassociateNatGatewayAddress handler must evaluate the DryRun branch (412 DryRunOperation with AWS's fixed message) before executing the disassociation. The open issue 20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md records the same contract for OTHER operations; it does not carry this operation or this check's replay key, so this cause is net-new here.
Traces: ec2.op-contract-disassociate-nat-gateway-address

=== Cause 4 of 6: GetNetworkInsightsAccessScopeContent returns a NotFound class for a nonexistent scope id ===
AWS: this operation rejects a syntactically valid but nonexistent `NetworkInsightsAccessScopeId` as a parameter-validation failure: error code `InvalidParameterValue` with the message `NetworkInsightsAccessScopeId (<scope-id>) is not valid` — fixed text `NetworkInsightsAccessScopeId (` + the id from the request + `) is not valid`. It is explicitly NOT a not-found-class error for this operation. Smithy: GetNetworkInsightsAccessScopeContent input `NetworkInsightsAccessScopeId` (required) + `DryRun`, output `NetworkInsightsAccessScopeContent`; no modeled errors, so the code/message are recorded behavior only.
Emulator: answers error code `InvalidNetworkInsightsAccessScopeId.NotFound`.
Why: clients branch on the error class — `InvalidParameterValue` is a caller-input bug to surface, a `*.NotFound` is a benign "it is gone" that cleanup paths swallow. The emulator inverts that decision for this operation.
Fix: the GetNetworkInsightsAccessScopeContent handler's lookup-failure mapping must emit InvalidParameterValue with AWS's message template rather than the generic per-resource NotFound code.
Traces: ec2.op-contract-get-network-insights-access-scope-content

=== Cause 5 of 6: ModifyNetworkInterfaceAttribute's multi-attribute refusal message is emulator-authored ===
AWS: ModifyNetworkInterfaceAttribute accepts exactly one attribute per call. A request naming two (e.g. `Description` and `SourceDestCheck`) is refused `InvalidParameterCombination` with the message `Fields for multiple attribute types specified: description, sourceDestCheck` — the fixed text is `Fields for multiple attribute types specified: ` and the variable part is the comma-separated list of the attribute members actually present in the request, named by their EC2 wire names (lower-camel, e.g. `description`, `sourceDestCheck`, as the Smithy model's xmlName/ec2QueryName for each member gives them). The refused call must have no effect: an independent DescribeNetworkInterfaces still shows the previously-set description. Smithy: ModifyNetworkInterfaceAttribute input members `Description` (xmlName `description`), `SourceDestCheck` (xmlName `sourceDestCheck`), `Groups`, `Attachment`, `EnaSrdSpecification`, `ConnectionTrackingSpecification`, `AssociatePublicIpAddress`, `EnablePrimaryIpv6`, `AssociatedSubnetIds`, `DryRun`; no modeled errors, so code/message are recorded behavior only.
Emulator: the code is right (`InvalidParameterCombination`) but the message is emulator-authored prose, `Only one attribute can be specified at a time`, which names none of the colliding fields.
Why: operators and tooling read the message to learn WHICH fields collided; with a constant sentence the caller must guess, and any assertion or log-scrape on AWS's wording fails.
Fix: the ModifyNetworkInterfaceAttribute validation's message template — build the field list from the attribute members present in the request instead of emitting a fixed sentence.
Traces: ec2.op-contract-modify-network-interface-attribute

=== Cause 6 of 6: a CDK-deployed VPC's public subnets carry no 0.0.0.0/0 route to the attached internet gateway ===
AWS: a CDK VPC with PUBLIC and PRIVATE_ISOLATED subnet groups across two AZs deploys so that each public subnet's associated route table carries a `0.0.0.0/0` route whose `GatewayId` is the internet gateway attached to that VPC, while isolated subnets carry no default route at all, and the two groups sit on different route tables. Recorded behavior only (a CloudFormation-driven composite of AWS::EC2::RouteTable / AWS::EC2::SubnetRouteTableAssociation / AWS::EC2::Route and the EC2 read paths).
Emulator: the stack reaches CREATE_COMPLETE, DescribeInternetGateways shows the gateway attached to the VPC, and the public subnet IS associated with a route table (the association lookup succeeds), but DescribeRouteTables for that table reports no `0.0.0.0/0` route carrying a GatewayId — the derived default-route target is absent. Replay: `public subnet <subnet-id> must route 0.0.0.0/0 to the Internet Gateway <igw-id>, found target None`.
Why: every "is this subnet public?" derivation a client performs (CDK/console topology views, egress audits, reachability reasoning) concludes the opposite of the deployed intent, and a stack that reports CREATE_COMPLETE has no working egress path.
Hypothesis: either the stack's AWS::EC2::Route resource creates no route row in the associated table, or DescribeRouteTables omits the GatewayId member when serializing an internet-gateway route.
Fix: the AWS::EC2::Route provisioning path (the CreateRoute handler reached from stack create) and the DescribeRouteTables route serializer — the route must appear in the associated table with GatewayId naming the internet gateway. The open issue 20261104T045949Z-cloudformation-ec2-a-cdk-cloudformation-deployed-vpc-has.md records a neighbouring CDK/CloudFormation VPC route-table defect under the `cloudformation__ec2` service and does not carry this check's replay key.
Traces: ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently

Related open issue (different causes, named so a fixer does not double-file): 20261106T072955Z-ec2-nat-gateway-create-and-describe-paths-report.md covers NAT gateway CREATE/DESCRIBE address status, validation message and attachedApplianceSet — none of the three NAT causes above (DeleteSubnet dependency, DeleteNatGateway malformed id, DisassociateNatGatewayAddress DryRun) are in it.

## Evidence
Cause 1 — ec2.nat-gateway-subnet-delete-dependency-violation-rejected [mismatch]
  Expected: ClientError DependencyViolation, Message "The subnet '<subnet-id>' has dependencies and cannot be deleted.", subnet still present.
  Actual:   AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — DeleteSubnet 200'd while the NAT gateway was 'available'.

Cause 2 — ec2.op-contract-delete-nat-gateway [mismatch]
  Expected: ('NatGatewayMalformed', 'Invalid Nat Gateway <nat-id>')
  Actual:   ('NatGatewayNotFound', "The natgateway ID 'nat-3221229d2526fa328' does not exist")  (the id is a per-run value filling the template, not a requirement)

Cause 3 — ec2.op-contract-disassociate-nat-gateway-address [mismatch]
  Expected: code 'DryRunOperation', message 'Request would have succeeded, but DryRun flag is set.', association unchanged.
  Actual:   AssertionError: NotImplemented  (assert code == "DryRunOperation" saw code 'NotImplemented')

Cause 4 — ec2.op-contract-get-network-insights-access-scope-content [mismatch]
  Expected: code 'InvalidParameterValue', message 'NetworkInsightsAccessScopeId (<bogus-id>) is not valid'
  Actual:   AssertionError: unexpected error code: InvalidNetworkInsightsAccessScopeId.NotFound

Cause 5 — ec2.op-contract-modify-network-interface-attribute [mismatch]
  Expected: code 'InvalidParameterCombination' (matched) AND message 'Fields for multiple attribute types specified: description, sourceDestCheck'
  Actual:   AssertionError: Only one attribute can be specified at a time

Cause 6 — ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently [mismatch]
  Expected: each public subnet's route table holds a 0.0.0.0/0 route with GatewayId = the VPC's attached internet gateway; isolated subnets hold none.
  Actual:   AssertionError: public subnet <subnet-id> must route 0.0.0.0/0 to the Internet Gateway <igw-id>, found target None  (ids are per-run values)

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these six checks (all listed under `### ec2`):
1. ec2.nat-gateway-subnet-delete-dependency-violation-rejected — create a VPC, attach an IGW, create a subnet, allocate an EIP, CreateNatGateway in that subnet, wait for State=available, then DeleteSubnet on that subnet: expect 400 DependencyViolation "The subnet '<id>' has dependencies and cannot be deleted." and the subnet still readable; DeleteNatGateway, wait for State=deleted, then DeleteSubnet must succeed 200.
2. ec2.op-contract-delete-nat-gateway — after a full create→available→delete→deleted lifecycle, call DeleteNatGateway with `nat-` + 17 random hex chars: expect NatGatewayMalformed / "Invalid Nat Gateway <id>".
3. ec2.op-contract-disassociate-nat-gateway-address — create a public NAT gateway, AssociateNatGatewayAddress with a second EIP, wait for Status=succeeded, then DisassociateNatGatewayAddress(NatGatewayId, AssociationIds=[secondary], DryRun=True): expect 412 DryRunOperation / "Request would have succeeded, but DryRun flag is set." with the association untouched.
4. ec2.op-contract-get-network-insights-access-scope-content — CreateNetworkInsightsAccessScope, read its content back, then call GetNetworkInsightsAccessScopeContent with the same id shape but an all-`f` suffix: expect InvalidParameterValue / "NetworkInsightsAccessScopeId (<id>) is not valid".
5. ec2.op-contract-modify-network-interface-attribute — CreateNetworkInterface, ModifyNetworkInterfaceAttribute with Description only (observable via DescribeNetworkInterfaces), then one call passing both Description and SourceDestCheck: expect InvalidParameterCombination / "Fields for multiple attribute types specified: description, sourceDestCheck" and no change to the stored description.
6. ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently — cdk synth the cdk_vpc_subnets app, CreateStack, wait for CREATE_COMPLETE, then for each PublicSubnetIds output resolve the associated route table via DescribeRouteTables(Filters=[association.subnet-id]) and assert its 0.0.0.0/0 route's GatewayId equals the VPC's attached internet gateway; isolated subnets must carry no 0.0.0.0/0 route.
