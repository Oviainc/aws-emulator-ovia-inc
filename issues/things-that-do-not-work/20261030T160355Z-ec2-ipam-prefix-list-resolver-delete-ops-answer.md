Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-delete-ipam-prefix-list-resolver ec2.op-contract-delete-ipam-prefix-list-resolver-target ec2.op-contract-describe-ipam-prefix-list-resolvers ec2.op-contract-describe-ipam-prefix-list-resolver-targets ec2.op-contract-describe-ipam-prefix-list-resolver-targets-superseded-stub
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-delete-ipam-prefix-list-resolver|run#12|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code}"
Issue-Key: ec2.op-contract-delete-ipam-prefix-list-resolver-target|run#20|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code}"
Issue-Key: ec2.op-contract-describe-ipam-prefix-list-resolvers|run#10|AssertionError|assert message == (
Issue-Key: ec2.op-contract-describe-ipam-prefix-list-resolver-targets|run#31|AssertionError|assert message == (
Issue-Key: ec2.op-contract-describe-ipam-prefix-list-resolver-targets-superseded-stub|run#11|AssertionError|assert message == (
Issue-Commit: 9b1913dfb1a265094321f560fd11d2f4ba8c2a4f

# EC2 IPAM prefix-list-resolver delete ops answer NotImplemented instead of DryRunOperation, and the Describe ops' NotFound messages are emulator-authored prose (4 causes)

> _Note: this issue was identified against the code base at commit `9b1913dfb1a265094321f560fd11d2f4ba8c2a4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIpamPrefixListResolver

## Description
Four net-new root causes on the EC2 IPAM prefix-list-resolver surface. All four must be closed before submitting.

Note on neighbouring open issues: 20261022T235108Z-ec2-ipam-byoasn-routing-policy-registration-and-prefix.md covers the GetIpamPrefixListResolver* READ family (Rules/Versions/VersionEntries) and 20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md covers a different set of DryRun operations. Neither names the five check ids in trace_ids here, and neither names the Delete* or Describe* prefix-list-resolver operations below, so those issues do not cover these causes.

Cause 1 of 4: DeleteIpamPrefixListResolver refuses an authorized DryRun probe with NotImplemented.
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) models DeleteIpamPrefixListResolver with input members DryRun (Boolean) and the required IpamPrefixListResolverId, output IpamPrefixListResolver, and no modeled error shapes (EC2 is ec2Query, so error codes are wire codes rather than modeled shapes). For an authorized caller with DryRun=true, EC2 answers HTTP 400 with error code DryRunOperation and the fixed message "Request would have succeeded, but DryRun flag is set." — that message is entirely fixed text with no variable parts.
Emulator: answers error code NotImplemented with the body "operation DeleteIpamPrefixListResolver is scaffolded but not implemented in this emulator"; the replay asserts `expected DryRunOperation, got NotImplemented`.
Why: a client cannot probe its permission to delete a resolver, and cannot distinguish DryRunOperation from UnauthorizedOperation; the real delete leg of any IPAM prefix-list-resolver teardown is also unavailable, so resolvers created by a test can never be removed.
Fix: the operation's handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_prefix_list_resolver.rs) — implement the DryRun short-circuit before any state mutation, then the real delete.
Traces: ec2.op-contract-delete-ipam-prefix-list-resolver.

Cause 2 of 4: DeleteIpamPrefixListResolverTarget refuses an authorized DryRun probe with NotImplemented.
AWS: Smithy models DeleteIpamPrefixListResolverTarget with input members DryRun (Boolean) and the required IpamPrefixListResolverTargetId, output IpamPrefixListResolverTarget, no modeled errors. An authorized DryRun=true call is answered HTTP 400 DryRunOperation "Request would have succeeded, but DryRun flag is set." (fixed text, no variable parts).
Emulator: answers NotImplemented ("operation DeleteIpamPrefixListResolverTarget is scaffolded but not implemented in this emulator"); the replay asserts `expected DryRunOperation, got NotImplemented`.
Why: same as Cause 1 for the target sub-resource — no permission probe, no detach/delete path, so a resolver cannot be unwound from its targets.
Fix: the operation's handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_prefix_list_resolver_target.rs) — DryRun short-circuit first, then the real delete.
Traces: ec2.op-contract-delete-ipam-prefix-list-resolver-target.

Cause 3 of 4: the InvalidIpamPrefixListResolverId.NotFound message is emulator-authored prose instead of AWS's wording.
AWS: recorded behavior only for the message text (EC2 ec2Query error messages are not in the Smithy model). A well-formed but nonexistent resolver id passed to DescribeIpamPrefixListResolvers (IpamPrefixListResolverIds) or to DescribeIpamPrefixListResolverTargets (IpamPrefixListResolverId, which is a scoping LOOKUP, not a narrowing filter) is refused with HTTP 400, error code InvalidIpamPrefixListResolverId.NotFound and the message "The specified IPAM Prefix List Resolver does not exist." Every word of that message is fixed text: the message carries NO variable part — in particular it does not interpolate the requested id.
Emulator: returns the right code but the message "The IPAM prefix list resolver (ipam-plr-0000000000000000) does not exist." — different fixed wording AND an interpolated id that AWS does not include. (ipam-plr-0000000000000000 is just the id the check happened to pass; the defect is the template, not that value.)
Why: clients and test suites that match on the documented message text — and log scrapers / error-mapping layers keyed to it — do not recognise the refusal, and a client that parses the id out of the message learns a field AWS never puts there.
Fix: the not-found refusal in these operations' handlers/validation (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_ipam_prefix_list_resolvers.rs and describe_ipam_prefix_list_resolver_targets.rs) — emit the fixed AWS message for this code rather than formatting the requested id into prose.
Traces: ec2.op-contract-describe-ipam-prefix-list-resolvers, ec2.op-contract-describe-ipam-prefix-list-resolver-targets.

Cause 4 of 4: the InvalidIpamPrefixListResolverTargetId.NotFound message is emulator-authored prose instead of AWS's wording.
AWS: recorded behavior only for the message text. DescribeIpamPrefixListResolverTargets given a well-formed but nonexistent id in IpamPrefixListResolverTargetIds is refused with HTTP 400, error code InvalidIpamPrefixListResolverTargetId.NotFound and the all-fixed message "The specified IPAM Prefix List Resolver Target does not exist." — again no variable part, and no interpolated id.
Emulator: returns the right code but the message "The IPAM prefix list resolver target (ipam-plrt-0000000000000000) does not exist." (the ipam-plrt-… value is the check's probe id, not part of the requirement).
Why: as Cause 3 — the refusal is unrecognisable to anything matching AWS's message for this error class.
Fix: the target-id not-found refusal in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_ipam_prefix_list_resolver_targets.rs — emit the fixed AWS message.
Traces: ec2.op-contract-describe-ipam-prefix-list-resolver-targets-superseded-stub.

## Evidence
Replay output, verbatim:
- ec2.op-contract-delete-ipam-prefix-list-resolver — AssertionError: expected DryRunOperation, got NotImplemented (check_ec2_op_contract_delete_ipam_prefix_list_resolver.py line 81). Expected: HTTP 400, code DryRunOperation, message "Request would have succeeded, but DryRun flag is set." Actual: code NotImplemented, body "operation DeleteIpamPrefixListResolver is scaffolded but not implemented in this emulator".
- ec2.op-contract-delete-ipam-prefix-list-resolver-target — AssertionError: expected DryRunOperation, got NotImplemented (line 100). Same expected/actual shape for DeleteIpamPrefixListResolverTarget.
- ec2.op-contract-describe-ipam-prefix-list-resolvers — AssertionError: unexpected InvalidIpamPrefixListResolverId.NotFound message: 'The IPAM prefix list resolver (ipam-plr-0000000000000000) does not exist.' Expected message: "The specified IPAM Prefix List Resolver does not exist." Code matched (InvalidIpamPrefixListResolverId.NotFound); only the message diverged.
- ec2.op-contract-describe-ipam-prefix-list-resolver-targets — AssertionError: unexpected InvalidIpamPrefixListResolverId.NotFound message: 'The IPAM prefix list resolver (ipam-plr-0000000000000000) does not exist.' Expected: "The specified IPAM Prefix List Resolver does not exist."
- ec2.op-contract-describe-ipam-prefix-list-resolver-targets-superseded-stub — AssertionError: unexpected InvalidIpamPrefixListResolverTargetId.NotFound message: 'The IPAM prefix list resolver target (ipam-plrt-0000000000000000) does not exist.' Expected: "The specified IPAM Prefix List Resolver Target does not exist."

Fixed vs variable text: for both NotFound codes the AWS message is 100% fixed text — the emulator both rewords it and adds a parenthesised id taken from the request, which AWS does not emit. For DryRunOperation the message is likewise 100% fixed text.

Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): DeleteIpamPrefixListResolver{DryRun, IpamPrefixListResolverId(required)} -> {IpamPrefixListResolver}; DeleteIpamPrefixListResolverTarget{DryRun, IpamPrefixListResolverTargetId(required)} -> {IpamPrefixListResolverTarget}; DescribeIpamPrefixListResolvers{DryRun, Filters, MaxResults(5..1000), NextToken, IpamPrefixListResolverIds} -> {IpamPrefixListResolvers, NextToken}; DescribeIpamPrefixListResolverTargets{DryRun, Filters, MaxResults(5..1000), NextToken, IpamPrefixListResolverTargetIds, IpamPrefixListResolverId} -> {IpamPrefixListResolverTargets, NextToken}. All four model zero error shapes (ec2Query wire codes).

## Steps to Reproduce
Replay the five check ids in trace_ids against the emulator. Minimal repro per cause:
1. ec2.delete_ipam_prefix_list_resolver(IpamPrefixListResolverId=<any id>, DryRun=True) as an authorized caller -> expect 400 DryRunOperation "Request would have succeeded, but DryRun flag is set."; observe NotImplemented.
2. ec2.delete_ipam_prefix_list_resolver_target(IpamPrefixListResolverTargetId=<any id>, DryRun=True) -> same expectation; observe NotImplemented.
3. ec2.describe_ipam_prefix_list_resolvers(IpamPrefixListResolverIds=["ipam-plr-0000000000000000"]) and ec2.describe_ipam_prefix_list_resolver_targets(IpamPrefixListResolverId="ipam-plr-0000000000000000") -> expect code InvalidIpamPrefixListResolverId.NotFound with message "The specified IPAM Prefix List Resolver does not exist."; observe the reworded, id-interpolated message.
4. ec2.describe_ipam_prefix_list_resolver_targets(IpamPrefixListResolverTargetIds=["ipam-plrt-0000000000000000"]) -> expect code InvalidIpamPrefixListResolverTargetId.NotFound with message "The specified IPAM Prefix List Resolver Target does not exist."; observe the reworded, id-interpolated message.
