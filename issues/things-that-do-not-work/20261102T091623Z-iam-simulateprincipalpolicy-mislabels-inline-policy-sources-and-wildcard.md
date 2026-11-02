Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__kms__secretsmanager.ecsm-direct-kms-decrypt-honours-the-secret-arn-encryption-context-condition ecr__ecs__iam.ecracc-authorization-token-allowed-only-against-the-wildcard-resource cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role
Issue-Key: ec2__iam__kms__secretsmanager.ecsm-direct-kms-decrypt-honours-the-secret-arn-encryption-context-condition|run#25|AssertionError|assert scoped == ("allowed", [f"role_{role}_kms-decrypt"], []), f"with the secret's ARN: {scoped!r}"
Issue-Key: ecr__ecs__iam.ecracc-authorization-token-allowed-only-against-the-wildcard-resource|run#10|AssertionError|assert scoped == {"ecr:GetAuthorizationToken": "implicitDeny",
Issue-Key: cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role|run#21|AssertionError|assert trust["Statement"][0]["Principal"] == {"AWS": f"arn:aws:iam::{acct}:root"}, (
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# iam: SimulatePrincipalPolicy mislabels inline-policy sources and wildcard-only actions, and policy Principals are not canonicalized to the account root ARN

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulatePrincipalPolicy

## Description
This issue carries 3 root causes in IAM, reached through cross-service checks. They are not covered by 20261025T102129Z-iam-10-net-new-behavioral-root-causes-simulateprincipalpolicy.md or 20261029T192356Z-iam-9-net-new-authorization-and-policy-evaluation.md, which carry other checks' replay keys.

Cause 1 of 3: SimulatePrincipalPolicy's MatchedStatements do not identify an inline policy by its entity-qualified source id.
AWS: recorded behavior only. For a statement that matched an INLINE policy on the simulated principal, MatchedStatements[].SourcePolicyId is the entity-qualified form '<entity-type>_<entity-name>_<policy-name>' — e.g. 'role_<roleName>_<inlinePolicyName>' for an inline policy on a role. Fixed text is the two underscores; the entity type, entity name and policy name come from state.
Emulator: returns the bare inline policy name ('kms-decrypt' instead of 'role_<roleName>_kms-decrypt'), so the statement's owning entity is lost.
Why: a caller that attributes a simulated allow/deny to the exact policy that produced it (the normal use of the simulator in a least-privilege review) cannot tell which entity's inline policy matched.
Fix: the SimulatePrincipalPolicy evaluator's SourcePolicyId construction for inline identity policies.
Traces: ec2__iam__kms__secretsmanager.ecsm-direct-kms-decrypt-honours-the-secret-arn-encryption-context-condition.

Cause 2 of 3: an action that AWS only supports on the wildcard resource is simulated as allowed against a concrete resource ARN.
AWS: ecr:GetAuthorizationToken takes no resource, so simulating it against a repository ARN yields EvalDecision 'implicitDeny', while ecr:BatchGetImage against the same repository ARN yields 'allowed'. The two must differ.
Emulator: answers 'allowed' for BOTH ecr:GetAuthorizationToken and ecr:BatchGetImage against the repository ARN, i.e. the action's resource-type requirement is not modelled.
Why: a reviewer using the simulator to prove a token grant must be scoped to '*' is told a repository-scoped grant suffices, which it does not on real AWS.
Fix: the simulator's resource matching must consult the action's supported resource types and return implicitDeny for an action that only applies to '*'.
Traces: ecr__ecs__iam.ecracc-authorization-token-allowed-only-against-the-wildcard-resource.

Cause 3 of 3: a bare account id in a policy Principal is not canonicalized to the account root ARN when the document is read back.
AWS: a policy document naming Principal {"AWS": "<account-id>"} is stored canonicalized, so reading it back (here the trust policy of a role through GetRole's AssumeRolePolicyDocument) yields Principal {"AWS": "arn:aws:iam::<account-id>:root"}. Fixed text is the ARN template; the account id comes from the request.
Emulator: returns the Principal exactly as submitted, {"AWS": "000000000000"}.
Why: tooling that compares a deployed trust policy against the canonical form AWS stores (CDK bootstrap and drift checks do exactly this) reports a spurious difference, and principal matching that expects the root ARN fails.
Fix: the policy-document normalization applied on write by CreateRole/UpdateAssumeRolePolicy and surfaced on read by GetRole.
Traces: cloudformation__iam__lambda__s3.cdk-deploy-role-passrole-rejects-non-cfn-exec-role.

## Evidence
Cause 1: "AssertionError: with the secret's ARN: ('allowed', ['kms-decrypt'], [])" where ('allowed', ['role_<roleName>_kms-decrypt'], []) was required.
Cause 2: "against 'arn:aws:ecr:us-west-2:000000000000:repository/sim2real-ecracc-repo-...' the token action must not match while BatchGetImage does; got {'ecr:GetAuthorizationToken': 'allowed', 'ecr:BatchGetImage': 'allowed'}".
Cause 3: "deploy role's first trust principal was {'AWS': '000000000000'}, expected the account root" (arn:aws:iam::000000000000:root).

## Steps to Reproduce
1. Cause 1: put an inline policy named kms-decrypt on a role, SimulatePrincipalPolicy for kms:Decrypt with the key ARN and read MatchedStatements[].SourcePolicyId.
2. Cause 2: grant ecr:GetAuthorizationToken and ecr:BatchGetImage on '*' and simulate both against a repository ARN.
3. Cause 3: CreateRole with a trust policy naming Principal {"AWS": "<account-id>"} and read AssumeRolePolicyDocument back with GetRole.
4. Or replay the check ids in trace_ids.
