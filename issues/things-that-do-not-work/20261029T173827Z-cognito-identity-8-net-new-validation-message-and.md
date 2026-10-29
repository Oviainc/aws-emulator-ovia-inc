Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.deep-cognito-identity-2-disabled-identity-merge-destination-lookup-rejected-old-token-still-vends cognito-identity.delete-identities-oversized-batch-rejected cognito-identity.delete-identities-too-many-requests-limit cognito-identity.tag-resource-invalid-arn-rejected cognito-identity.tag-resource-reserved-prefix-rejected cognito-identity.tag-resource-rejects-oversized-tag-value cognito-identity.set-identity-pool-roles-missing-rules-configuration-rejected cognito-identity.set-principal-tag-attribute-map-invalid-keys-rejected cognito-identity.deploy-identity-pool-cfn-role-mappings-enforced cognito-identity.deploy-identity-pool-cfn-update-role-mapping-takes-effect
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.deep-cognito-identity-2-disabled-identity-merge-destination-lookup-rejected-old-token-still-vends|run#34|AssertionError|assert after[disabled_id] == [DISABLED, ctx.region], (
Issue-Key: cognito-identity.delete-identities-oversized-batch-rejected|run#7|AssertionError|assert message == expected, f"expected refusal message {expected!r}, got {message!r}"
Issue-Key: cognito-identity.delete-identities-too-many-requests-limit|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-identity.tag-resource-invalid-arn-rejected|run#13|AssertionError|assert message == _MALFORMED_ARN_MESSAGE, message
Issue-Key: cognito-identity.tag-resource-reserved-prefix-rejected|run#10|AssertionError|assert message == "Caller is an end user and not allowed to mutate system tags", message
Issue-Key: cognito-identity.tag-resource-rejects-oversized-tag-value|run#11|AssertionError|assert message == (
Issue-Key: cognito-identity.set-identity-pool-roles-missing-rules-configuration-rejected|run#16|AssertionError|assert message == "RuleConfiguration is required for Rules", (
Issue-Key: cognito-identity.set-principal-tag-attribute-map-invalid-keys-rejected|run#13|AssertionError|assert message == (
Issue-Key: cognito-identity|GetIdentityPoolRoles|ValidationException
Issue-Commit: c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40

# cognito-identity: 8 net-new validation-message and CloudFormation root causes — tag/role-mapping/DeleteIdentities refusals are re-worded, and a CFN-deployed identity pool is still addressed by NAME

> _Note: this issue was identified against the code base at commit `c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Eight distinct root causes. Each is a separate work item; all eight must be closed before submitting. Open issues that are related but do NOT carry these replay keys: 20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md, 20261024T145006Z-cognito-identity-listtagsforresource-ignores-iam-authorization-a-foreign.md, 20261025T061337Z-cognito-identity-6-net-new-root-causes-cfn.md, 20261028T195228Z-cognito-identity-2-net-new-cloudformation-root-causes.md.

Cause 1 of 8 — MergeDeveloperIdentities leaves the disabled destination identity with an incomplete Logins list.
AWS: recorded behavior only (measured us-west-2, four independent pools): the merge disables the DESTINATION identity, which stays describable, and DescribeIdentity returns its Logins as exactly the two-element sentinel ["DISABLED", "<region>"] (the second element is the pool's region, taken from state). The survivor carries the developer provider once per merged user.
Emulator: DescribeIdentity returns ["DISABLED"] — the region element is missing.
Why: client code that reads the sentinel (and the merge-direction detection built on it) sees a shape AWS never returns, so a merged-away identity cannot be identified reliably.
Fix: the MergeDeveloperIdentities state transition that rewrites the destination identity's logins.
Traces: cognito-identity.deep-cognito-identity-2-disabled-identity-merge-destination-lookup-rejected-old-token-still-vends.

Cause 2 of 8 — DeleteIdentities constraint-violation message is rendered per-member instead of per-parameter.
AWS: ValidationException whose message renders the WHOLE rejected list once at the parameter path and lists the member constraints together: "1 validation error detected: Value '[<id>, <id>, ...]' at 'identityIdsToDelete' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 55, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\\w-]+:[0-9a-f-]+]" — fixed text with the submitted list and the modeled constraints (Smithy: DeleteIdentities.IdentityIdsToDelete @length min 1 max 60, member IdentityId @length 1..55 @pattern ^[\\w-]+:[0-9a-f-]+$).
Emulator: reports one indexed member instead — "1 validation error detected: Value 'not-a-real-identity-id' at 'identityIdsToDelete.2.member' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]+:[0-9a-f-]+".
Why: the message is the only machine-readable description of which input was rejected; a client parsing the parameter path and the constraint list gets a path AWS never emits and a truncated constraint set.
Fix: the request-validation message renderer for list members in the cognito-identity service.
Traces: cognito-identity.delete-identities-oversized-batch-rejected, cognito-identity.delete-identities-too-many-requests-limit.

Cause 3 of 8 — TagResource / ListTagsForResource malformed-ARN message.
AWS: InvalidParameterException (Smithy: modeled on TagResource, @httpError 400) with the fixed text "ARN in SecureServiceAuthStrategy failed validation:" — AWS does not echo the submitted ARN. The same message is returned by ListTagsForResource for the same ARN.
Emulator: "ResourceArn '<arn>' is not the ARN of an Amazon Cognito identity pool." (the code is right).
Why: clients matching the AWS wording cannot classify the refusal.
Fix: the ARN validation used by the TagResource/UntagResource/ListTagsForResource handlers.
Traces: cognito-identity.tag-resource-invalid-arn-rejected.

Cause 4 of 8 — TagResource reserved 'aws:' tag-key message.
AWS: InvalidParameterException with the fixed text "Caller is an end user and not allowed to mutate system tags" — no variable parts, and the tag is not stored.
Emulator: "Tags key '<key>' is not valid: the 'aws:' prefix is reserved for Amazon Web Services use." (the code is right).
Why: same as Cause 3 — the only distinguishing part of the refusal is wrong.
Fix: the reserved-prefix check in the TagResource handler's validation.
Traces: cognito-identity.tag-resource-reserved-prefix-rejected.

Cause 5 of 8 — TagResource oversized tag VALUE message shape.
AWS: ValidationException rendering the whole map at the 'tags' path with the full value and both bounds: "1 validation error detected: Value '{<key>=<value>}' at 'tags' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0]".
Emulator: paths the individual entry ('tags.<key>'), truncates the value to "... [300 bytes]" and lists only the upper bound: "... at 'tags.oversize-key' failed to satisfy constraint: Member must have length less than or equal to 256".
Why: a client parsing the path/constraint list sees a path AWS does not emit and a value AWS does not truncate.
Fix: the same validation message renderer as Cause 2, map-value branch.
Traces: cognito-identity.tag-resource-rejects-oversized-tag-value.

Cause 6 of 8 — SetIdentityPoolRoles Rules-type role mapping with no RulesConfiguration.
AWS: InvalidParameterException with the fixed text "RuleConfiguration is required for Rules" — no variable parts (no provider name, no member path).
Emulator: "RoleMappings['<provider>'].RulesConfiguration is required when Type is 'Rules'.".
Why: the code matches but the message does not, so AWS-worded handling of a misconfigured role mapping does not fire.
Fix: the RoleMappings validation in the SetIdentityPoolRoles handler.
Traces: cognito-identity.set-identity-pool-roles-missing-rules-configuration-rejected.

Cause 7 of 8 — SetPrincipalTagAttributeMap renders the rejected PrincipalTags map in request order instead of AWS's order.
AWS: ValidationException "1 validation error detected: Value '{<key>=<value>, <key>=<value>}' at 'principalTags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]", where AWS renders the map in its OWN stable key order — for two oversized keys a… and b…, b is rendered before a (Smithy: SetPrincipalTagAttributeMap.PrincipalTags @length 0..50).
Emulator: identical text but the entries are rendered in the order supplied by the request (a… then b…).
Why: the message is asserted byte-for-byte by clients and by this suite; the ordering is part of AWS's rendering, not of the request.
Fix: the map rendering inside the validation message renderer.
Traces: cognito-identity.set-principal-tag-attribute-map-invalid-keys-rejected.

Cause 8 of 8 — a CloudFormation-deployed AWS::Cognito::IdentityPool is still addressed by NAME, so every follow-up identity-pool call is rejected on the id pattern.
AWS: Ref of an AWS::Cognito::IdentityPool resolves to the identity pool ID ("<region>:<uuid>"), which satisfies Smithy's IdentityPoolId @pattern ^[\\w-]+:[0-9a-f-]+$, so GetIdentityPoolRoles/GetId/DescribeIdentityPool on the stack's output succeed.
Emulator: Ref yields the pool NAME, and the subsequent call is refused ValidationException "1 validation error detected: Value '<pool-name>' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]+:[0-9a-f-]+".
Why: no stack that creates an identity pool can be used at all — the whole CFN path to cognito-identity is dead. Named in open issues 20261025T061337Z and 20261028T195228Z, neither of which carries these two checks' keys.
Fix: the AWS::Cognito::IdentityPool resource's Ref/physical-id resolution in the CloudFormation resource provider.
Traces: cognito-identity.deploy-identity-pool-cfn-role-mappings-enforced, cognito-identity.deploy-identity-pool-cfn-update-role-mapping-takes-effect.

## Evidence
Cause 1: "the disabled identity's Logins must be exactly ['DISABLED', 'us-west-2'], got ['DISABLED']".
Cause 2: expected "... Value '[us-west-2:cbb07df8-..., not-a-real-identity-id]' at 'identityIdsToDelete' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 55, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\\w-]+:[0-9a-f-]+]", got "... Value 'not-a-real-identity-id' at 'identityIdsToDelete.2.member' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]+:[0-9a-f-]+".
Cause 3: expected message 'ARN in SecureServiceAuthStrategy failed validation:'; got "ResourceArn 'this-is-definitely-not-an-arn-1234567890' is not the ARN of an Amazon Cognito identity pool.".
Cause 4: expected 'Caller is an end user and not allowed to mutate system tags'; got "Tags key 'aws:reserved-key' is not valid: the 'aws:' prefix is reserved for Amazon Web Services use.".
Cause 5: expected "... Value '{oversize-key=<300-byte value>}' at 'tags' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0]"; got "... Value 'vvvv... [300 bytes]' at 'tags.oversize-key' failed to satisfy constraint: Member must have length less than or equal to 256".
Cause 6: expected 'RuleConfiguration is required for Rules'; got "RoleMappings['graph.facebook.com'].RulesConfiguration is required when Type is 'Rules'.".
Cause 7: expected "...Value '{bbbb…=custom:b, aaaa…=custom:a}' at 'principalTags'…"; got the same text with the entries rendered a-then-b.
Cause 8: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the GetIdentityPoolRoles operation: 1 validation error detected: Value 'sim2real-behav-pool-69076738a988' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]+:[0-9a-f-]+ (same on the update check, and on GetId/DescribeIdentityPool in sibling checks).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay exactly the checks in trace_ids; each cause's own checks are listed under its 'Traces:' line.
3. By hand for Cause 8: deploy a stack with an AWS::Cognito::IdentityPool, read the stack's Ref/Outputs value, and call GetIdentityPoolRoles with it — it must be a '<region>:<uuid>' pool id and the call must succeed.
4. By hand for Cause 3: TagResource(ResourceArn='this-is-definitely-not-an-arn-1234567890', Tags={'k':'v'}) must answer InvalidParameterException 'ARN in SecureServiceAuthStrategy failed validation:', and ListTagsForResource on the same ARN must answer identically.
