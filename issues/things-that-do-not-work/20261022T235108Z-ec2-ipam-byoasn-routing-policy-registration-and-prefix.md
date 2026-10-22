Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-associate-ipam-byoasn ec2.op-contract-disassociate-ipam-byoasn ec2.op-contract-provision-ipam-byoasn-rejected ec2.op-contract-batch-modify-ipam-routing-policy-registrations ec2.op-contract-delete-ipam-routing-policy-registration ec2.op-contract-get-ipam-routing-policy-registrations ec2.op-contract-get-ipam-routing-policy-registration-deltas ec2.op-contract-delete-ipam-internet-registry-association ec2.op-contract-enableIpamInternetRegistryAssociation ec2.op-contract-get-ipam-prefix-list-resolver-rules ec2.op-contract-get-ipam-prefix-list-resolver-versions ec2.op-contract-get-ipam-prefix-list-resolver-version-entries ec2.op-contract-get-ipam-route-origin-authorizations ec2.op-contract-get-associated-ipv6-pool-cidrs ec2.ipam-family-error-semantics
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-associate-ipam-byoasn|run#5|AssertionError|assert dry_run_code == "DryRunOperation", f"unexpected dry-run code: {dry_run_code}"
Issue-Key: ec2.op-contract-disassociate-ipam-byoasn|run#2|AssertionError|assert code == "DryRunOperation", (asn, cidr, code)
Issue-Key: ec2.op-contract-provision-ipam-byoasn-rejected|run#3|AssertionError|assert boundary1 == "InvalidIpamId.NotFound", f"expected InvalidIpamId.NotFound, got {boundary1}"
Issue-Key: ec2.op-contract-batch-modify-ipam-routing-policy-registrations|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code}"
Issue-Key: ec2.op-contract-delete-ipam-routing-policy-registration|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-get-ipam-routing-policy-registrations|run#4|AssertionError|assert dry_run_code == "DryRunOperation", (
Issue-Key: ec2.op-contract-get-ipam-routing-policy-registration-deltas|run#4|AssertionError|assert dry_run_code == "DryRunOperation", (
Issue-Key: ec2.op-contract-delete-ipam-internet-registry-association|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-enableIpamInternetRegistryAssociation|run#5|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code}"
Issue-Key: ec2.op-contract-get-ipam-prefix-list-resolver-rules|run#4|AssertionError|assert dry_run_code == "DryRunOperation", (
Issue-Key: ec2.op-contract-get-ipam-prefix-list-resolver-versions|run#4|AssertionError|assert dry_run_code == "DryRunOperation", (
Issue-Key: ec2.op-contract-get-ipam-prefix-list-resolver-version-entries|run#4|AssertionError|assert dry_run_code == "DryRunOperation", (
Issue-Key: ec2.op-contract-get-ipam-route-origin-authorizations|run#4|AssertionError|assert dry_run_code == "DryRunOperation", (
Issue-Key: ec2.op-contract-get-associated-ipv6-pool-cidrs|run#3|AssertionError|assert code == "DryRunOperation", code
Issue-Key: ec2.ipam-family-error-semantics|run#6|AssertionError|assert not problems, "unexpected IPAM error semantics:\n" + "\n".join(problems)
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# EC2 IPAM BYOASN, routing-policy-registration and prefix-list-resolver operations answer 501 NotImplemented instead of DryRunOperation / their not-found codes (10 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ProvisionIpamByoasn

## Description
Ten unimplemented EC2 IPAM-surface operations, each its own cause. All ten must be closed before submitting. Common shape: these operations model a DryRun member and no error shapes (Smithy com-amazonaws-ec2-amazonec2; EC2 error codes are ec2Query wire codes, not modeled shapes). AWS answers an authorized DryRun=true call with 400 DryRunOperation 'Request would have succeeded, but DryRun flag is set.', and an unknown-resource call with the operation's own code (e.g. InvalidIpamId.NotFound, InvalidIpamPoolId.NotFound). The emulator answers NotImplemented ('operation <Op> is scaffolded but not implemented in this emulator') for all of them. Why, for all ten: the whole IPAM/BYOASN control surface is unusable — a client can neither probe permission with DryRun nor get a not-found boundary, so no IPAM automation can be developed or negatively tested. Fix, for all ten: implement the named handler under services/service-com-amazonaws-ec2-amazonec2/src/ops/.

Cause 1 of 10: AssociateIpamByoasn — 'unexpected dry-run code: NotImplemented'. Trace: ec2.op-contract-associate-ipam-byoasn.
Cause 2 of 10: DisassociateIpamByoasn — NotImplemented for the (64512, 203.0.113.0/24) probe. Trace: ec2.op-contract-disassociate-ipam-byoasn.
Cause 3 of 10: ProvisionIpamByoasn — expected InvalidIpamId.NotFound, got NotImplemented. Trace: ec2.op-contract-provision-ipam-byoasn-rejected.
Cause 4 of 10: BatchModifyIpamRoutingPolicyRegistrations — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-batch-modify-ipam-routing-policy-registrations.
Cause 5 of 10: the IPAM routing-policy-registration surface (DeleteIpamRoutingPolicyRegistration, GetIpamRoutingPolicyRegistrations, GetIpamRoutingPolicyRegistrationDeltas) — all three expected DryRunOperation, all three got NotImplemented. Traces: ec2.op-contract-delete-ipam-routing-policy-registration, ec2.op-contract-get-ipam-routing-policy-registrations, ec2.op-contract-get-ipam-routing-policy-registration-deltas.
Cause 6 of 10: DeleteIpamInternetRegistryAssociation — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-delete-ipam-internet-registry-association.
Cause 7 of 10: EnableIpamInternetRegistryAssociation — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-enableIpamInternetRegistryAssociation.
Cause 8 of 10: the IPAM prefix-list-resolver read surface (GetIpamPrefixListResolverRules, GetIpamPrefixListResolverVersions, GetIpamPrefixListResolverVersionEntries) — all three expected DryRunOperation for an authorized DryRun call, all three got NotImplemented. Traces: ec2.op-contract-get-ipam-prefix-list-resolver-rules, ec2.op-contract-get-ipam-prefix-list-resolver-versions, ec2.op-contract-get-ipam-prefix-list-resolver-version-entries.
Cause 9 of 10: GetIpamRouteOriginAuthorizations — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-get-ipam-route-origin-authorizations.
Cause 10 of 10: GetAssociatedIpv6PoolCidrs — NotImplemented where AWS answers the pool-not-found code. Trace: ec2.op-contract-get-associated-ipv6-pool-cidrs.

The aggregate family check ec2.ipam-family-error-semantics fails over this same surface and is listed in trace_ids so it is re-replayed with the fix.

## Evidence
Representative replay output: AssertionError: unexpected dry-run code: NotImplemented; AssertionError: ('64512', '203.0.113.0/24', 'NotImplemented'); AssertionError: expected InvalidIpamId.NotFound, got NotImplemented; AssertionError: expected DryRunOperation, got NotImplemented; AssertionError: expected DryRunOperation for an authorized DryRun call, got 'NotImplemented'; AssertionError: NotImplemented (GetAssociatedIpv6PoolCidrs). Every NotImplemented body reads 'operation <Op> is scaffolded but not implemented in this emulator'. Expected: HTTP 400 DryRunOperation 'Request would have succeeded, but DryRun flag is set.' for the DryRun arms; the operation's own NotFound code otherwise.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repro per cause: call the named operation with DryRun=True and an authorized caller (expect DryRunOperation), then without DryRun against a nonexistent ipam-/pool-/registration id (expect that operation's NotFound code); observe NotImplemented in both arms.
