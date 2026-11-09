Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-idp.cfn-log-delivery-configuration-round-trip-and-removed-on-update cloudformation__cognito-idp.cfn-user-pool-risk-configuration-attachment-round-trip-and-removed-on-update cloudformation__cognito-idp.cfn-user-pool-ui-customization-attachment-round-trip-and-removed-on-update cloudformation__cognito-idp.cfn-managed-login-branding-and-terms-round-trip-and-removed-on-update cloudformation__cognito-idp.cfn-user-pool-client-properties-round-trip-and-removed-on-stack-delete cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-proves-and-removes-membership cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-nonexistent-username-rejected-rollback
Issue-Key: cloudformation__cognito-idp.cfn-log-delivery-configuration-round-trip-and-removed-on-update|run#11|AssertionError|assert pools == [out["PoolRef"]], f"user pools named {pool_name!r}: {pools!r}"
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-risk-configuration-attachment-round-trip-and-removed-on-update|run#9|AssertionError|assert pools == [out["PoolRef"]], f"user pools named {pool_name!r}: {pools!r}"
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-ui-customization-attachment-round-trip-and-removed-on-update|run#12|AssertionError|assert pools == [out["PoolRef"]], f"user pools named {pool_name!r}: {pools!r}"
Issue-Key: cloudformation__cognito-idp|CreateStack|ValidationError
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-client-properties-round-trip-and-removed-on-stack-delete|_outputs#10|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stacks[0].get("Outputs", [])}
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-proves-and-removes-membership|run#6|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Key: cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-nonexistent-username-rejected-rollback|failed_reason#7|AssertionError|assert len(rows) == 1, f"expected one {status} row for {logical_id}, got {rows!r}"
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# cloudformation__cognito-idp: 4 net-new causes — CFN-declared user pools never exist in Cognito, AWS::Cognito::Terms is an unknown resource type, stack Outputs carry no OutputValue, and UserPoolGroup/UserPoolUserToGroupAttachment never deploy

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four net-new root causes in CloudFormation provisioning of Cognito resources.

Cause 1 of 4: a user pool declared by a CREATE_COMPLETE stack does not exist in Cognito.
AWS: Smithy: ListUserPools returns UserPools[].{Id,Name} for every pool in the region. A stack that reaches CREATE_COMPLETE having declared AWS::Cognito::UserPool has created the pool, so exactly one listed pool carries the template's PoolName and its Id equals the Ref the stack output exports.
Emulator: after CREATE_COMPLETE, no pool with the template's name is listed ([]), although the stack output still carries a pool reference. The pool name is a per-run generated value; the requirement is that the pool the stack Refs is listable by that name.
Why: every follow-on read or write against the provisioned pool (SetLogDeliveryConfiguration, SetRiskConfiguration, SetUICustomization, describe, delete) is aimed at a pool the control plane does not have, so a CFN-provisioned Cognito estate cannot be driven through the Cognito API at all.
Fix: the cloudformation resource provider for AWS::Cognito::UserPool and its attachment resources — create the pool in cognito-idp state and make Ref/GetAtt resolve to that pool.
Traces: cloudformation__cognito-idp.cfn-log-delivery-configuration-round-trip-and-removed-on-update, cloudformation__cognito-idp.cfn-user-pool-risk-configuration-attachment-round-trip-and-removed-on-update, cloudformation__cognito-idp.cfn-user-pool-ui-customization-attachment-round-trip-and-removed-on-update.

Cause 2 of 4: AWS::Cognito::Terms is not a registered resource type.
AWS: recorded behavior only (the CFN resource-type registry is not Smithy-modelled). CreateStack accepts a template declaring AWS::Cognito::Terms (the managed-login terms-of-use resource) and provisions it; only a type AWS does not know is refused with ValidationError 'Template format error: Unrecognized resource types: [<type>]'.
Emulator: CreateStack refuses the whole template with ValidationError, message 'Template format error: Unrecognized resource types: [AWS::Cognito::Terms]' — the fixed text is 'Template format error: Unrecognized resource types: [...]' and the bracketed list is taken from the template, so here the emulator is applying AWS's not-a-type refusal to a type that IS one.
Why: no managed-login branding/terms template can be deployed, and the refusal misreports a supported type as unknown.
Fix: the cloudformation resource-type registry plus a provider for AWS::Cognito::Terms (alongside AWS::Cognito::ManagedLoginBranding).
Traces: cloudformation__cognito-idp.cfn-managed-login-branding-and-terms-round-trip-and-removed-on-update.

Cause 3 of 4: DescribeStacks returns Outputs entries with no OutputValue.
AWS: recorded behavior only. Each entry of Stacks[].Outputs for a CREATE_COMPLETE stack carries OutputKey AND OutputValue (the resolved value of the template's Outputs block).
Emulator: the Outputs entries have no OutputValue key at all, so reading the stack's outputs raises KeyError 'OutputValue'.
Why: every consumer that resolves a stack's exported ids (a client, a CDK/CFN pipeline, a nested stack) gets an output with a name and no value, and cannot address the resources the stack created.
Fix: the cloudformation DescribeStacks output serializer / the stack-output resolution that populates OutputValue. Related open issues that do NOT cover this check: 20261108T052651Z-cloudformation-3-net-new-causes-the-number-parameter.md, 20261109T113512Z-cloudformation-ec2-network-firewall-describestacks-returns-a-create.md.
Traces: cloudformation__cognito-idp.cfn-user-pool-client-properties-round-trip-and-removed-on-stack-delete.

Cause 4 of 4: a stack declaring AWS::Cognito::UserPoolGroup / AWS::Cognito::UserPoolUserToGroupAttachment never reaches the modelled outcome.
AWS: recorded behavior only. A pool + user + group + attachment template reaches CREATE_COMPLETE, the attachment proves the membership (AdminListGroupsForUser lists the group) and deleting the stack removes it. When the attachment names a Username that does not exist, the ATTACHMENT resource is the one that fails: the stack rolls back with exactly one CREATE_FAILED row for that logical id whose ResourceStatusReason is the handler-returned message 'Resource handler returned message: "<the Cognito refusal>" (RequestToken: <token>, HandlerErrorCode: <code>)'.
Emulator: the positive stack ends ROLLBACK_COMPLETE with the GROUP resource CREATE_FAILED (events: Pool CREATE_COMPLETE, User CREATE_COMPLETE, Group CREATE_FAILED, then deletes and ROLLBACK_COMPLETE), and in the negative case there is no CREATE_FAILED row for the attachment resource at all ([]) — the refusal is never attributed to the resource that caused it.
Why: group membership cannot be provisioned by a template, and a template with a bad username gives no diagnosable failed resource, so neither the success nor the refusal contract of these two resource types is usable.
Fix: the cloudformation providers for AWS::Cognito::UserPoolGroup and AWS::Cognito::UserPoolUserToGroupAttachment and the CREATE_FAILED/ResourceStatusReason they record.
Traces: cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-proves-and-removes-membership, cloudformation__cognito-idp.cfn-user-pool-user-to-group-attachment-nonexistent-username-rejected-rollback.

## Evidence
Cause 1 — AssertionError: user pools named 'sim2real-behav-coglogdelpool-7fec4a4a71ba': [] (also 'sim2real-behav-cogriskpool-…' and 'sim2real-behav-coguicpool-…').
Cause 2 — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Cognito::Terms].
Cause 3 — KeyError: 'OutputValue' while building {o['OutputKey']: o['OutputValue'] for o in stacks[0].get('Outputs', [])}.
Cause 4 — AssertionError: stack ended 'ROLLBACK_COMPLETE': [('Pool','CREATE_COMPLETE'),('User','CREATE_COMPLETE'),('Group','CREATE_FAILED'),('Group','DELETE_COMPLETE'),('User','DELETE_COMPLETE'),('Pool','DELETE_COMPLETE'),(<stack>,'ROLLBACK_COMPLETE')]; and AssertionError: expected one CREATE_FAILED row for Attach, got [].

## Steps to Reproduce
1) Cause 1: CreateStack with AWS::Cognito::UserPool (+ a LogDeliveryConfiguration / RiskConfigurationAttachment / UICustomizationAttachment), wait CREATE_COMPLETE, then ListUserPools filtered by the template's PoolName — empty.
2) Cause 2: CreateStack with an AWS::Cognito::Terms resource — refused ValidationError 'Unrecognized resource types: [AWS::Cognito::Terms]'.
3) Cause 3: CreateStack with an Outputs block exporting the pool/client ids, wait CREATE_COMPLETE, call DescribeStacks and read Stacks[0].Outputs — entries have OutputKey but no OutputValue.
4) Cause 4: CreateStack with Pool+User+Group+UserPoolUserToGroupAttachment — the Group resource goes CREATE_FAILED and the stack rolls back; repeat with the attachment naming a non-existent Username — no CREATE_FAILED row for the attachment is recorded.
