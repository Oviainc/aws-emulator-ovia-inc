Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.disassociate-route-server-endpoint-still-in-use-blocked ec2.tgwc-vpc-route-to-unattached-transit-gateway-rejected ec2.vscan-ingress-from-an-unknown-prefix-list-rejected-not-found ec2.op-contract-release-hosts ec2.op-contract-create-route-duplicate-rejected ec2.route-creation-duplicate-destination-rejected ec2.asvpce-streaming-vpce-gateway-type-rejected ec2.asvpce-streaming-vpce-custom-policy-rejected ec2.asgssh-reachability-path-blocked-by-security-group-until-authorized ec2.op-contract-delete-network-insights-analysis ec2.op-contract-describe-spot-fleet-requests ec2.op-contract-describeSpotFleetInstances ec2.ipam-routing-policy-registration-round-trip
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.disassociate-route-server-endpoint-still-in-use-blocked|_error#23|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: ec2.tgwc-vpc-route-to-unattached-transit-gateway-rejected|_client_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ec2.vscan-ingress-from-an-unknown-prefix-list-rejected-not-found|run#7|AssertionError|raise AssertionError("a batch authorize naming an unknown prefix list unexpectedly succeeded")
Issue-Key: ec2.op-contract-release-hosts|run#26|AssertionError|assert resp["Successful"] == [], resp
Issue-Key: ec2|CreateRoute|RouteAlreadyExists
Issue-Key: ec2.asvpce-streaming-vpce-gateway-type-rejected|run#8|AssertionError|assert dry_code == "InvalidParameter", (
Issue-Key: ec2.asvpce-streaming-vpce-custom-policy-rejected|run#4|AssertionError|assert svc["VpcEndpointPolicySupported"] is False, (
Issue-Key: ec2.asgssh-reachability-path-blocked-by-security-group-until-authorized|run#10|AssertionError|assert (blocked["Status"], blocked.get("NetworkPathFound")) == ("succeeded", False), (
Issue-Key: ec2.op-contract-delete-network-insights-analysis|run#17|AssertionError|assert finished["Status"] == "succeeded", f"analysis did not succeed: {finished}"
Issue-Key: ec2.op-contract-describe-spot-fleet-requests|_latest_amazon_linux_ami#7|AssertionError|raise AssertionError("no Amazon Linux 2 AMI found to build a Spot Fleet launch spec")
Issue-Key: ec2.op-contract-describeSpotFleetInstances|_latest_amazon_linux_ami#5|AssertionError|raise AssertionError("no Amazon Linux 2 AMI found to build a Spot Fleet launch spec")
Issue-Key: ec2.ipam-routing-policy-registration-round-trip|run#12|AssertionError|assert created["Rir"] == "RIPE", f"expected Rir 'RIPE', got {created['Rir']!r}"
Issue-Commit: c832bd0d619782e17fabc455ea6e3058ac5fd237

# EC2 accepts ten requests AWS refuses or answers differently: missing dependency checks, non-idempotent CreateRoute, DryRun that skips validation, wrong catalogue facts

> _Note: this issue was identified against the code base at commit `c832bd0d619782e17fabc455ea6e3058ac5fd237`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
This issue bundles 10 net-new root causes in the EC2 emulator. Smithy: the ec2 model binds awsQuery and models no operation errors (errors: [] on CreateRoute, AssociateAddress, ReleaseHosts, DescribeNetworkInterfaces), so the codes/messages below are recorded behavior only.

Cause 1 of 10: DeleteRouteServerEndpoint succeeds while the endpoint is still in use.
AWS: an endpoint whose route server is still associated with the VPC cannot be deleted; the call is refused (IncorrectState) and the endpoint stays available.
Emulator: returns 200 with the endpoint moved to State 'deleting'.
Why: a client learns no teardown ordering and destroys a resource that should be protected, leaving dangling state.
Fix: the DeleteRouteServerEndpoint handler's in-use dependency check.
Traces: ec2.disassociate-route-server-endpoint-still-in-use-blocked.

Cause 2 of 10: CreateRoute toward a transit gateway that is not attached to the VPC succeeds.
AWS: the route is refused — a TGW target must have an attachment in the route table's VPC.
Emulator: the call succeeds, so the route table holds a route that could never forward.
Why: the misconfiguration surfaces only as silently dropped traffic instead of at the API.
Fix: the CreateRoute handler's transit-gateway attachment validation.
Traces: ec2.tgwc-vpc-route-to-unattached-transit-gateway-rejected.

Cause 3 of 10: AuthorizeSecurityGroupIngress naming an unknown prefix list succeeds.
AWS: a permission whose PrefixListIds names a nonexistent list is refused (InvalidPrefixListId.NotFound) and NO rule in the batch is applied.
Emulator: the batch authorize succeeds.
Why: the group ends up with rules the caller believes reference a prefix list that does not exist; validation is deferred to nothing.
Fix: prefix-list resolution in the authorize handler's validation, applied before any rule is stored.
Traces: ec2.vscan-ingress-from-an-unknown-prefix-list-rejected-not-found.

Cause 4 of 10: ReleaseHosts releases a host it must refuse.
AWS: ReleaseHosts reports a host it cannot release under Unsuccessful (with its per-item error) and leaves Successful empty (Smithy: ReleaseHostsResult has Successful and Unsuccessful lists).
Emulator: returns Successful=['h-...'], Unsuccessful=[].
Why: a caller believes dedicated-host capacity was released while instances still depend on it; per-item error reporting is never exercised.
Fix: the ReleaseHosts handler — evaluate host state/instances per item and populate Unsuccessful.
Traces: ec2.op-contract-release-hosts.

Cause 5 of 10: CreateRoute is not idempotent for an identical destination+target.
AWS: re-submitting CreateRoute with the same DestinationCidrBlock AND the same target is an idempotent no-op success (Return=true, one route in the table); only the same CIDR toward a DIFFERENT target is refused RouteAlreadyExists.
Emulator: the identical re-submission is refused RouteAlreadyExists ("The route identified by <cidr> already exists."), raising an error mid-scenario.
Why: any converging deployer (CloudFormation, Terraform, a retry after a timeout) that re-asserts its routes fails instead of converging.
Fix: the CreateRoute handler's duplicate detection — compare target as well as destination.
Traces: ec2.op-contract-create-route-duplicate-rejected, ec2.route-creation-duplicate-destination-rejected.

Cause 6 of 10: DryRun short-circuits validation, so an invalid request reports DryRunOperation.
AWS: DryRun answers DryRunOperation only when the request would otherwise succeed; a request that would be refused answers the real refusal — here InvalidParameter for a Gateway-type endpoint against an interface-only service.
Emulator: answers DryRunOperation ("Request would have succeeded, but DryRun flag is set.") for a request the non-dry run refuses InvalidParameter.
Why: DryRun is used precisely to pre-validate; a client that pre-flights gets a green light and then fails on the real call.
Fix: the CreateVpcEndpoint handler — run parameter validation before the DryRun short-circuit.
Traces: ec2.asvpce-streaming-vpce-gateway-type-rejected.

Cause 7 of 10: the VPC endpoint service catalogue reports wrong facts for the AppStream streaming service.
AWS: DescribeVpcEndpointServices for com.amazonaws.<region>.appstream.streaming reports VpcEndpointPolicySupported=false, so a custom endpoint policy on it is refused.
Emulator: the catalogue entry carries VpcEndpointPolicySupported=true (with a PrivateDnsName set), so the refusal is unreachable.
Why: a client reads the catalogue to decide whether it may attach a policy and is told yes for a service that forbids it.
Fix: the DescribeVpcEndpointServices service catalogue data for the streaming service.
Traces: ec2.asvpce-streaming-vpce-custom-policy-rejected.

Cause 8 of 10: Reachability Analyzer performs no path tracing, so every analysis fails.
AWS: StartNetworkInsightsAnalysis completes with Status 'succeeded' and NetworkPathFound true/false according to the security groups, NACLs and routes on the path.
Emulator: the analysis finishes Status 'failed' with StatusMessage "This emulator does not perform Reachability Analyzer path tracing, so no reachability result was produced for this path." and no NetworkPathFound.
Why: every connectivity diagnosis built on Reachability Analyzer is unusable, and the failure is reported as an analysis error rather than a result.
Fix: the network-insights analysis handler/state — derive the path result from SG/NACL/route state.
Traces: ec2.asgssh-reachability-path-blocked-by-security-group-until-authorized, ec2.op-contract-delete-network-insights-analysis.

Cause 9 of 10: the AMI catalogue contains no Amazon Linux 2 image.
AWS: DescribeImages with the Amazon Linux 2 owner/name filters returns the public AL2 AMIs in every commercial region.
Emulator: the filtered DescribeImages returns nothing, so a Spot Fleet launch spec cannot even be built and the Spot Fleet read paths are never reached.
Why: any client that resolves "the latest Amazon Linux 2 AMI" — the most common launch idiom — has no image to launch.
Fix: the DescribeImages AMI catalogue seed data.
Traces: ec2.op-contract-describe-spot-fleet-requests, ec2.op-contract-describeSpotFleetInstances.

Cause 10 of 10: the IPAM routing-policy registration lowercases Rir on read-back.
AWS: the Rir value is echoed as submitted/canonicalised in upper case ('RIPE'); the registration round-trips its RIR identity.
Emulator: returns 'ripe'.
Why: a client comparing the RIR against its own constant mismatches, and enum-valued members become case-unstable.
Fix: the IPAM routing-policy registration handler/serializer — preserve the canonical RIR casing.
Traces: ec2.ipam-routing-policy-registration-round-trip.

## Evidence
Replay output, expected vs actual:
C1: "expected a ClientError, but the call succeeded and returned {'RouteServerEndpoint': {... 'State': 'deleting' ...}, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}".
C2: "the call succeeded; real AWS rejects it, so there is no error".
C3: "a batch authorize naming an unknown prefix list unexpectedly succeeded".
C4: "{'Successful': ['h-00000000000000045'], 'Unsuccessful': []}" — expected Successful == [].
C5: uncaught "An error occurred (RouteAlreadyExists) when calling the CreateRoute operation: The route identified by 0.0.0.0/0 already exists." on the identical destination+target re-submission the check asserts is an idempotent no-op (and the same for 10.99.0.0/24).
C6: "DryRun on a Gateway request for the Interface-only streaming service returned 'DryRunOperation' ('Request would have succeeded, but DryRun flag is set.'); expected the same InvalidParameter the real request raises".
C7: catalogue entry shows 'VpcEndpointPolicySupported': True for ServiceName 'com.amazonaws.us-west-2.appstream.streaming'; expected False.
C8: analysis reads ('failed', None, 'This emulator does not perform Reachability Analyzer path tracing, so no reachability result was produced for this path.'); expected ('succeeded', False) for a path no SG rule admits.
C9: "no Amazon Linux 2 AMI found to build a Spot Fleet launch spec".
C10: "expected Rir 'RIPE', got 'ripe'".

## Steps to Reproduce
Replay the checks in trace_ids. Minimal reproductions: CreateRouteServer + endpoint + AssociateRouteServer, then DeleteRouteServerEndpoint; CreateRoute(TransitGatewayId=<tgw with no attachment in this VPC>); AuthorizeSecurityGroupIngress with IpPermissions[].PrefixListIds=[{'PrefixListId':'pl-0123456789abcdef0'}]; AllocateHosts, RunInstances onto the host, ReleaseHosts; CreateRoute(rtb, 0.0.0.0/0, GatewayId=igw) twice with identical arguments; CreateVpcEndpoint(VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.appstream.streaming', DryRun=True); DescribeVpcEndpointServices for that service name and read VpcEndpointPolicySupported; CreateNetworkInsightsPath + StartNetworkInsightsAnalysis and poll Status; DescribeImages(Owners=['amazon'], Filters=[{'Name':'name','Values':['amzn2-ami-hvm-*']}]); create an IPAM routing-policy registration with Rir='RIPE' and read it back.
