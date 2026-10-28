Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-local-gateway-route-rejected-unknown-route-table ec2.create-local-gateway-route-table-rejected-unknown-gateway ec2.create-local-gateway-route-table-vif-group-association-rejected ec2.create-local-gateway-route-table-vpc-association-rejected ec2.op-contract-delete-coip-pool ec2.op-contract-get-coip-pool-usage ec2.local-gateway-family-error-semantics ec2.client-vpn-and-coip-error-semantics
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-local-gateway-route-rejected-unknown-route-table|run#3|AssertionError|assert code == "InvalidLocalGatewayRouteTableID.NotFound", (
Issue-Key: ec2.create-local-gateway-route-table-rejected-unknown-gateway|run#3|AssertionError|assert code == "InvalidLocalGatewayID.NotFound", (
Issue-Key: ec2.create-local-gateway-route-table-vif-group-association-rejected|run#3|AssertionError|assert code == "InvalidLocalGatewayRouteTableID.NotFound", (
Issue-Key: ec2.create-local-gateway-route-table-vpc-association-rejected|run#5|AssertionError|assert code == "InvalidLocalGatewayRouteTableID.NotFound", (
Issue-Key: ec2.op-contract-delete-coip-pool|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-get-coip-pool-usage|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.local-gateway-family-error-semantics|run#7|AssertionError|assert not problems, "unexpected local-gateway error semantics:\n" + "\n".join(problems)
Issue-Key: ec2.client-vpn-and-coip-error-semantics|run#5|AssertionError|assert not problems, "unexpected client-VPN/COIP error semantics:\n" + "\n".join(problems)
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# EC2 local-gateway route-table and COIP-pool operations answer 501 NotImplemented instead of their not-found / DryRun codes (6 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLocalGatewayRouteTable

## Description
Six unimplemented EC2 operations on the local-gateway / customer-owned-IP surface, each its own cause. All six must be closed before submitting. Common shape: these operations model a DryRun member and no error shapes (Smithy com-amazonaws-ec2-amazonec2; EC2 error codes are ec2Query wire codes, documented not modeled). AWS refuses a well-formed but nonexistent local-gateway id with InvalidLocalGatewayID.NotFound, a nonexistent local-gateway route table with InvalidLocalGatewayRouteTableID.NotFound, and an authorized DryRun=true call with 400 DryRunOperation 'Request would have succeeded, but DryRun flag is set.'. The emulator answers NotImplemented ('operation <Op> is scaffolded but not implemented in this emulator'). Why, for all six: the Outpost local-gateway and COIP surfaces are unusable and their not-found boundaries unobservable, so a client cannot tell a missing resource from a broken service. Fix, for all six: implement the named handler under services/service-com-amazonaws-ec2-amazonec2/src/ops/.

Cause 1 of 6: CreateLocalGatewayRoute — expected InvalidLocalGatewayRouteTableID.NotFound, got NotImplemented. Trace: ec2.create-local-gateway-route-rejected-unknown-route-table.
Cause 2 of 6: CreateLocalGatewayRouteTable — expected InvalidLocalGatewayID.NotFound for a nonexistent local gateway, got NotImplemented. Trace: ec2.create-local-gateway-route-table-rejected-unknown-gateway.
Cause 3 of 6: CreateLocalGatewayRouteTableVirtualInterfaceGroupAssociation — expected InvalidLocalGatewayRouteTableID.NotFound, got NotImplemented. Trace: ec2.create-local-gateway-route-table-vif-group-association-rejected.
Cause 4 of 6: CreateLocalGatewayRouteTableVpcAssociation — expected InvalidLocalGatewayRouteTableID.NotFound, got NotImplemented. Trace: ec2.create-local-gateway-route-table-vpc-association-rejected.
Cause 5 of 6: DeleteCoipPool — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-delete-coip-pool.
Cause 6 of 6: GetCoipPoolUsage — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-get-coip-pool-usage.

The aggregate family checks ec2.local-gateway-family-error-semantics and ec2.client-vpn-and-coip-error-semantics fail over this same surface and are listed in trace_ids so they are re-replayed with the fix.

## Evidence
Replay output: AssertionError: expected InvalidLocalGatewayRouteTableID.NotFound, got 'NotImplemented' (3 checks); AssertionError: expected InvalidLocalGatewayID.NotFound for a nonexistent local gateway, got 'NotImplemented'; AssertionError: expected DryRunOperation, got 'NotImplemented' (DeleteCoipPool, GetCoipPoolUsage). Every NotImplemented body reads 'operation <Op> is scaffolded but not implemented in this emulator'. Expected: HTTP 400 with the named ec2Query error code, or DryRunOperation 'Request would have succeeded, but DryRun flag is set.' under DryRun.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repro: ec2:CreateLocalGatewayRouteTable with LocalGatewayId='lgw-0123456789abcdef0' (expect InvalidLocalGatewayID.NotFound) and ec2:DeleteCoipPool with DryRun=True (expect DryRunOperation); observe NotImplemented.
