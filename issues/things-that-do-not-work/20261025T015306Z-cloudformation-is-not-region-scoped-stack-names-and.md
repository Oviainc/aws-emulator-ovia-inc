Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.ec2mr-cfnres-same-stack-name-two-regions-resolve-independently cloudformation.gjwr-same-stack-name-deploys-per-region-with-region-scoped-exports cloudformation.deep-cloudformation-3-cfn-flight-stackset-tolerance-one-absorbs-region1-collision-and-deploys-region2 cloudformation.deep-cloudformation-3-cfn-flight-stackset-zero-tolerance-collision-fails-operation-and-cancels-region2 cloudformation.rollback-cancel-continue-flow cloudformation.organizations-access-toggle-lifecycle cloudformation.type-publishing-lifecycle cloudformation.stack-tag-value-unicode-round-trip
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|CreateStack|AlreadyExistsException
Issue-Key: cloudformation.deep-cloudformation-3-cfn-flight-stackset-tolerance-one-absorbs-region1-collision-and-deploys-region2|run#11|AssertionError|assert _get_parameter_error(ssm2, param_name) == ("ParameterNotFound", ""), (
Issue-Key: cloudformation.deep-cloudformation-3-cfn-flight-stackset-zero-tolerance-collision-fails-operation-and-cancels-region2|run#11|AssertionError|assert _get_parameter_error(ssm2, param_name) == ("ParameterNotFound", ""), (
Issue-Key: cloudformation.rollback-cancel-continue-flow|run#7|AssertionError|assert code_noop == "ValidationError", code_noop
Issue-Key: cloudformation.organizations-access-toggle-lifecycle|run#3|AssertionError|assert initial_err == "ValidationError", (
Issue-Key: cloudformation.type-publishing-lifecycle|run#4|AssertionError|assert register_code == "CFNRegistryException", (
Issue-Key: cloudformation.stack-tag-value-unicode-round-trip|run#13|AssertionError|assert tags["Owner"] == expected_tag_value, (
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudFormation is not region-scoped (stack names and stack-set deployments leak across regions), CancelUpdateStack/DescribeOrganizationsAccess/RegisterPublisher answer NotImplemented, and stack tag values are ASCII-coerced (6 causes)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelUpdateStack

## Description
This issue bundles 6 net-new root causes. The open issue 20261025T000319Z-cloudformation-glue-iam-s3-2-net-new-root.md records the global-stack-namespace defect for a different pair of check ids; the replay keys below are not carried by any open issue.

Cause 1 of 6: stack identity is a single global namespace, not per-region.
AWS: stacks are regional resources — the same stack name can exist independently in two regions, and each region's stack resolves to its own resources and exports.
Emulator: creating the same stack name in a second region is refused with 400 AlreadyExistsException 'Stack [<name>] already exists'.
Why: any multi-region deployment (identical stack per region, region-scoped exports) is impossible.
Fix: key stack state by (region, name/id) in the cloudformation service state and in the create/describe handlers.
Traces: cloudformation.ec2mr-cfnres-same-stack-name-two-regions-resolve-independently, cloudformation.gjwr-same-stack-name-deploys-per-region-with-region-scoped-exports.

Cause 2 of 6: resources provisioned by a region-1 deployment are visible in region 2.
AWS: a stack/stack-set instance deployed into one region creates its resources only in that region; a name collision in region 1 must not exist in region 2 (recorded case: an SSM parameter created by the region-1 instance).
Emulator: the parameter the region-1 deployment collided on is already present in us-east-1, so the collision is not region-1-only and the tolerance/zero-tolerance stack-set operations take the wrong branch.
Why: region-scoped failure tolerance in stack sets cannot be modelled; a per-region deploy appears to have already happened. Hypothesis: the provisioning engine writes resources into a region-agnostic store.
Fix: the provisioning engine must create each resource in the target region's service state.
Traces: cloudformation.deep-cloudformation-3-cfn-flight-stackset-tolerance-one-absorbs-region1-collision-and-deploys-region2, cloudformation.deep-cloudformation-3-cfn-flight-stackset-zero-tolerance-collision-fails-operation-and-cancels-region2.

Cause 3 of 6: CancelUpdateStack is an unimplemented scaffold stub.
AWS: Smithy: CancelUpdateStack(StackName required, ClientRequestToken) returns Unit and models TokenAlreadyExistsException; called with no update in flight it is refused with 400 ValidationError (recorded message 'CancelUpdateStack cannot be called from current stack status'), and called during an update it cancels it and rolls the stack back.
Emulator: every call answers NotImplemented.
Why: there is no way to abort a running update, and the status-gated refusal taxonomy for the rollback trio is unreachable.
Fix: implement the CancelUpdateStack handler in the cloudformation service crate (ops/cancel_update_stack.rs) over the update engine.
Traces: cloudformation.rollback-cancel-continue-flow.

Cause 4 of 6: DescribeOrganizationsAccess is an unimplemented scaffold stub.
AWS: from a non-management, non-delegated-admin account the call is refused with ValidationError; the organizations-access toggle (ActivateOrganizationsAccess/DeactivateOrganizationsAccess) reports its status.
Emulator: NotImplemented.
Fix: implement the DescribeOrganizationsAccess handler and its caller-identity gate.
Traces: cloudformation.organizations-access-toggle-lifecycle.

Cause 5 of 6: RegisterPublisher is an unimplemented scaffold stub.
AWS: RegisterPublisher succeeds for an eligible account and returns a PublisherId, which the type-publishing lifecycle then uses.
Emulator: NotImplemented, so the whole publish lifecycle is blocked.
Fix: implement the RegisterPublisher handler and publisher state in the registry.
Traces: cloudformation.type-publishing-lifecycle.

Cause 6 of 6: stack tag values are ASCII-coerced.
AWS: Smithy: Tag.Value is a string of length 1-256 with no charset restriction; a non-ASCII value round-trips byte-for-byte through CreateStack/DescribeStacks.
Emulator: every non-ASCII character is replaced with '?'.
Why: tags are corrupted in storage, which is silent data loss rather than a refusal.
Fix: stop transcoding tag values in the tag storage/serialization path.
Traces: cloudformation.stack-tag-value-unicode-round-trip.

## Evidence
Replayed suite:
- ec2mr-cfnres-same-stack-name-two-regions-resolve-independently and gjwr-same-stack-name-deploys-per-region-with-region-scoped-exports: "botocore.errorfactory.AlreadyExistsException: An error occurred (AlreadyExistsException) when calling the CreateStack operation: Stack [<name>] already exists".
- deep-cloudformation-3-cfn-flight-stackset-*: "parameter '<name>' unexpectedly exists in us-east-1; the collision must be region-1-only".
- rollback-cancel-continue-flow: cancel_update_stack with no update in flight -> code 'NotImplemented', expected 'ValidationError'.
- organizations-access-toggle-lifecycle: "expected a non-management/non-delegated-admin account to be refused with ValidationError on DescribeOrganizationsAccess, got 'NotImplemented'".
- type-publishing-lifecycle: "unexpected RegisterPublisher rejection: NotImplemented".
- stack-tag-value-unicode-round-trip: '\u65e5\u672c\u8a9e\u30bf\u30b0 Bj\u00f6rk caf\u00e9_100:...' came back as '????? Bj?rk caf?_100:...'.

## Steps to Reproduce
1. Boot the emulator, replay the suite, re-run the trace_ids.
2. Cause 1 repro: CreateStack(StackName='dup') in us-west-2, then CreateStack(StackName='dup') in us-east-1 — the second answers AlreadyExistsException.
3. Cause 3 repro: CreateStack, wait CREATE_COMPLETE, then CancelUpdateStack(StackName=...) — expect 400 ValidationError, got NotImplemented.
4. Cause 6 repro: CreateStack with Tags=[{Key:'k',Value:'日本語 Björk'}], then DescribeStacks and compare the value byte-for-byte.
