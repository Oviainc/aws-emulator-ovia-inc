Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Step: task#25
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateIpamPrefixListResolver|2xx->2xx|$.CreateIpamPrefixListResolverResponse.ipamPrefixListResolver.lastVersionCreationStatus
Issue-Key: ec2|DescribeIpamPrefixListResolvers|2xx->2xx|$
Issue-Key: ec2|DescribeIpamPrefixListResolvers|2xx->2xx|$.DescribeIpamPrefixListResolversResponse.ipamPrefixListResolverSet.item.lastVersionCreationStatus
Issue-Key: ec2|DescribeIpamPrefixListResolvers|2xx->2xx|$.DescribeIpamPrefixListResolversResponse.ipamPrefixListResolverSet.item.lastVersionCreationStatusMessage
Issue-Key: ec2|CreateIpamPrefixListResolverTarget|2xx->2xx|$.CreateIpamPrefixListResolverTargetResponse.ipamPrefixListResolverTarget.desiredVersion
Issue-Key: ec2|DescribeIpamPrefixListResolverTargets|2xx->2xx|$.DescribeIpamPrefixListResolverTargetsResponse.ipamPrefixListResolverTargetSet.item.desiredVersion
Issue-Key: ec2|DescribeIpamPrefixListResolverTargets|4xx->2xx|$
Issue-Key: ec2|DescribeIpamPrefixListResolverTargets|4xx->2xx|$.Response
Issue-Key: ec2|DescribeIpamPrefixListResolverTargets|4xx->2xx|status
Issue-Key: ec2|DescribeIpamPrefixListResolvers|4xx->2xx|$
Issue-Key: ec2|DescribeIpamPrefixListResolvers|4xx->2xx|$.Response
Issue-Key: ec2|DescribeIpamPrefixListResolvers|4xx->2xx|status
Issue-Commit: 57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf

# EC2 IPAM prefix-list-resolver reads omit version members and deleted resolvers stay readable

> _Note: this issue was identified against the code base at commit `57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeIpamPrefixListResolvers

## Description
Six net-new wire divergences on the EC2 IPAM prefix-list-resolver surface, all in one lifecycle: the create/describe projections drop the modeled version-tracking members, and after the delete legs the resolver and its target remain readable instead of being refused. All six must be closed before submitting.

Neighbouring open issues do NOT cover these: 20261030T160355Z-ec2-ipam-prefix-list-resolver-delete-ops-answer.md is a behavioral issue about DeleteIpamPrefixListResolver*/DryRun answering NotImplemented and about the wording of the two NotFound messages, and it carries only behavioral check ids (ec2.op-contract-*) as its replay keys - none of the fixture steps named here. 20261030T000411Z-ec2-deleted-resources-stay-readable-post-delete-describes.md and 20261030T063325Z-ec2-deleted-resources-remain-readable-four-describe-paths.md name other describe paths, not the prefix-list-resolver family.

Cause 1 of 6: CreateIpamPrefixListResolver omits ipamPrefixListResolver.lastVersionCreationStatus.
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) models structure IpamPrefixListResolver with member LastVersionCreationStatus (xmlName lastVersionCreationStatus, ec2QueryName LastVersionCreationStatus, enum IpamPrefixListResolverVersionCreationStatus, doc "The status for the last time a version was created."). CreateIpamPrefixListResolver returns that structure, and the recording shows the member present immediately after create with the in-flight value (recorded: 'pending').
Emulator: the member is absent from the CreateIpamPrefixListResolverResponse body - replay: "$.CreateIpamPrefixListResolverResponse.ipamPrefixListResolver.lastVersionCreationStatus: missing (expected 'pending', got None)".
Why: a client that creates a resolver and then waits on the version-creation status has no field to read, so it cannot tell a pending version build from a finished or failed one.
Fix: the operation's serializer/state projection for IpamPrefixListResolver (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_ipam_prefix_list_resolver.rs) - populate and emit lastVersionCreationStatus.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#25.

Cause 2 of 6: DescribeIpamPrefixListResolvers omits lastVersionCreationStatus and lastVersionCreationStatusMessage, so a status poll can never converge.
AWS: the same Smithy structure also models LastVersionCreationStatusMessage (xmlName lastVersionCreationStatusMessage, String, doc "The status message for the last time a version was created."). Once the first version has been built, the describe projection carries the terminal status plus its message; the message's fixed text is "Successfully created version " followed by the version number and a period, the version number being the variable part taken from state (recorded: status 'success', message "Successfully created version 1.").
Emulator: both members are absent from every item of ipamPrefixListResolverSet - replay: "$.DescribeIpamPrefixListResolversResponse.ipamPrefixListResolverSet.item.lastVersionCreationStatus: missing (expected 'success', got None)", "...lastVersionCreationStatusMessage: missing (expected 'Successfully created version 1.', got None)", and consequently "poll: not in ['success'] after 61 attempt(s)" / "$: poll capped at 60s".
Why: every client or waiter that polls a resolver until its version build completes hangs until its own timeout, because the field it polls never appears.
Fix: the describe projection (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_ipam_prefix_list_resolvers.rs) plus the resolver state that records version-creation status and its message.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#27.

Cause 3 of 6: CreateIpamPrefixListResolverTarget omits ipamPrefixListResolverTarget.desiredVersion.
AWS: Smithy models IpamPrefixListResolverTarget with member DesiredVersion (xmlName desiredVersion, ec2QueryName DesiredVersion, BoxedLong, doc "The desired version of the prefix list that this target should synchronize with."). The create response carries the version the new target is pointed at (recorded: '1'; the number is state-derived, not part of the requirement).
Emulator: the member is absent - replay: "$.CreateIpamPrefixListResolverTargetResponse.ipamPrefixListResolverTarget.desiredVersion: missing (expected '1', got None)".
Why: a client cannot tell which resolver version a freshly created target is synchronizing toward, so it cannot compare desiredVersion with lastSyncedVersion to decide whether the target is converged.
Fix: the create handler/serializer for the target (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_ipam_prefix_list_resolver_target.rs).
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#28.

Cause 4 of 6: DescribeIpamPrefixListResolverTargets omits desiredVersion on every item.
AWS: same modeled member as Cause 3, present in the describe projection of each target.
Emulator: absent - replay: "$.DescribeIpamPrefixListResolverTargetsResponse.ipamPrefixListResolverTargetSet.item.desiredVersion: missing (expected '1', got None)".
Why: the read path a client uses to watch target convergence is missing the same field, so target sync state is unobservable.
Fix: the describe projection (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_ipam_prefix_list_resolver_targets.rs).
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#29.

Cause 5 of 6: a deleted IPAM prefix-list-resolver TARGET stays readable - the describe answers HTTP 200 with the target instead of the NotFound refusal.
AWS: recorded behavior only (EC2 is ec2Query; DescribeIpamPrefixListResolverTargets models no error shapes, so the refusal is a wire code). After the target is deleted, a describe scoped to that target id is refused with HTTP 400, error code InvalidIpamPrefixListResolverTargetId.NotFound and the message "The specified IPAM Prefix List Resolver Target does not exist." - that message is entirely fixed text with no variable part.
Emulator: answers HTTP 200 and keeps returning the target with state 'create-complete' for the full poll window - replay: "status: expected 400, got 200", "$.Response: missing (expected {'Errors': {'Error': {'Code': 'InvalidIpamPrefixListResolverTargetId.NotFound', 'Message': 'The specified IPAM Prefix List Resolver Target does not exist.'}}, ...}, got None)", "poll: error code None not in ['InvalidIpamPrefixListResolverTargetId.NotFound'] after 61 attempt(s)".
Why: deletion is not observable - teardown code that waits for the resource to disappear never finishes, and a client that treats a successful describe as proof of existence will operate on a resource that no longer exists.
Fix: the delete leg's state transition plus the id lookup in the describe handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_prefix_list_resolver_target.rs and describe_ipam_prefix_list_resolver_targets.rs) - remove the target from state on delete and refuse unknown ids with the NotFound code.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#31.

Cause 6 of 6: a deleted IPAM prefix-list-resolver stays readable - the describe answers HTTP 200 with the resolver instead of the NotFound refusal.
AWS: recorded behavior only. After the resolver is deleted, DescribeIpamPrefixListResolvers scoped to that id is refused with HTTP 400, error code InvalidIpamPrefixListResolverId.NotFound and the all-fixed message "The specified IPAM Prefix List Resolver does not exist." (no variable part, in particular the requested id is not interpolated).
Emulator: answers HTTP 200 and keeps returning the resolver with state 'create-complete' - replay: "status: expected 400, got 200", "$.Response: missing (expected {'Errors': {'Error': {'Code': 'InvalidIpamPrefixListResolverId.NotFound', ...}}}, got None)", "poll: error code None not in ['InvalidIpamPrefixListResolverId.NotFound'] after 61 attempt(s)".
Why: same as Cause 5 for the parent resource: the IPAM teardown sequence never converges and a client cannot distinguish a live resolver from a deleted one.
Fix: the delete leg's state transition plus the id lookup in the describe handler (delete_ipam_prefix_list_resolver.rs and describe_ipam_prefix_list_resolvers.rs).
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#33.

## Evidence
Replay of fixture ec2-ipam-consolidated-delete-and-deprovision-lifecycle (trace set simparity-wire-aws-v1), verbatim divergence details:

- task#25 CreateIpamPrefixListResolver, expected 200 / got 200: "$.CreateIpamPrefixListResolverResponse.ipamPrefixListResolver.lastVersionCreationStatus: missing (expected 'pending', got None)".
- task#27 DescribeIpamPrefixListResolvers, expected 200 / got 200: "$.DescribeIpamPrefixListResolversResponse.ipamPrefixListResolverSet.item.lastVersionCreationStatus: missing (expected 'success', got None)"; "...lastVersionCreationStatusMessage: missing (expected 'Successfully created version 1.', got None)"; "$: poll capped at 60s"; "poll: not in ['success'] after 61 attempt(s)". The emulator body it did serve contains only ownerId, ipamPrefixListResolverId, ipamPrefixListResolverArn, ipamArn, ipamRegion, addressFamily, state, tagSet.
- task#28 CreateIpamPrefixListResolverTarget, expected 200 / got 200: "$.CreateIpamPrefixListResolverTargetResponse.ipamPrefixListResolverTarget.desiredVersion: missing (expected '1', got None)".
- task#29 DescribeIpamPrefixListResolverTargets, expected 200 / got 200: "$.DescribeIpamPrefixListResolverTargetsResponse.ipamPrefixListResolverTargetSet.item.desiredVersion: missing (expected '1', got None)".
- task#31 DescribeIpamPrefixListResolverTargets, expected 400 / got 200: expected body {'Response': {'Errors': {'Error': {'Code': 'InvalidIpamPrefixListResolverTargetId.NotFound', 'Message': 'The specified IPAM Prefix List Resolver Target does not exist.'}}}}; actual body a 200 DescribeIpamPrefixListResolverTargetsResponse whose item still reports state 'create-complete'; "poll: error code None not in ['InvalidIpamPrefixListResolverTargetId.NotFound'] after 61 attempt(s)".
- task#33 DescribeIpamPrefixListResolvers, expected 400 / got 200: expected body {'Response': {'Errors': {'Error': {'Code': 'InvalidIpamPrefixListResolverId.NotFound', 'Message': 'The specified IPAM Prefix List Resolver does not exist.'}}}}; actual body a 200 DescribeIpamPrefixListResolversResponse whose item still reports state 'create-complete'; "poll: error code None not in ['InvalidIpamPrefixListResolverId.NotFound'] after 61 attempt(s)".

Model facts used (smithy_query on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): IpamPrefixListResolver members include LastVersionCreationStatus (lastVersionCreationStatus, enum IpamPrefixListResolverVersionCreationStatus) and LastVersionCreationStatusMessage (lastVersionCreationStatusMessage, String); IpamPrefixListResolverTarget members include DesiredVersion (desiredVersion, BoxedLong) and LastSyncedVersion (lastSyncedVersion, BoxedLong). Status/ids/request ids in the recording are per-run values; the requirements are the member presence, the HTTP status, the error codes and the fixed message texts quoted above.

## Steps to Reproduce
1. Replay fixture ec2-ipam-consolidated-delete-and-deprovision-lifecycle from trace set simparity-wire-aws-v1 against the emulator; steps task#25, task#27, task#28, task#29, task#31 and task#33 diverge.
2. Minimal repro: ec2.create_ipam(...) then ec2.create_ipam_prefix_list_resolver(IpamId=<ipam>, AddressFamily='ipv4') -> response must carry ipamPrefixListResolver.lastVersionCreationStatus; observe it missing.
3. ec2.describe_ipam_prefix_list_resolvers(IpamPrefixListResolverIds=[<resolver>]) -> item must carry lastVersionCreationStatus and lastVersionCreationStatusMessage ("Successfully created version <n>."); observe both missing, so any wait-for-'success' poll times out.
4. ec2.create_ipam_prefix_list_resolver_target(IpamPrefixListResolverId=<resolver>, PrefixListId=<pl>, TrackLatestVersion=True) and ec2.describe_ipam_prefix_list_resolver_targets(...) -> both must carry desiredVersion; observe it missing.
5. ec2.delete_ipam_prefix_list_resolver_target(IpamPrefixListResolverTargetId=<target>) then ec2.describe_ipam_prefix_list_resolver_targets(IpamPrefixListResolverTargetIds=[<target>]) -> expect HTTP 400 InvalidIpamPrefixListResolverTargetId.NotFound "The specified IPAM Prefix List Resolver Target does not exist."; observe HTTP 200 with the deleted target.
6. ec2.delete_ipam_prefix_list_resolver(IpamPrefixListResolverId=<resolver>) then ec2.describe_ipam_prefix_list_resolvers(IpamPrefixListResolverIds=[<resolver>]) -> expect HTTP 400 InvalidIpamPrefixListResolverId.NotFound "The specified IPAM Prefix List Resolver does not exist."; observe HTTP 200 with the deleted resolver.
