Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workmailmessageflow.get-raw-message-content-permissions-boundary-restriction-rejected
Issue-Shape-Id: com.amazonaws.workmailmessageflow#GiraffeMessageInTransitService
Issue-Key: workmailmessageflow|CreatePolicy|NotImplemented
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# workmailmessageflow permissions-boundary authorization cannot be exercised: IAM CreatePolicy answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicy

## Description
Cause 1 of 1: the IAM `CreatePolicy` operation is still a scaffold stub, so the very first call of the workmailmessageflow permissions-boundary check is refused and the whole authorization behavior behind it is unreachable. (This check is grounded in the workmailmessageflow suite — hence this issue's service token — but the stub itself lives in the IAM crate. It is distinct from the already-filed `Implement iam PutRolePolicy: unimplemented scaffold stub` (20261022T043627Z-iam-putrolepolicy-stub.md) and from `Implement iam AttachRolePolicy` / `Implement iam CreateServiceSpecificCredential`: no open issue names CreatePolicy, and the suite did not mark this failure as already filed or owned.)

AWS: Smithy — the IAM model binds `CreatePolicy` (CreatePolicyRequest -> CreatePolicyResponse, awsQuery), which creates a managed policy and answers HTTP 200 with Policy.Arn; its modeled client errors are the IAM ones (InvalidInput, LimitExceeded, EntityAlreadyExists, MalformedPolicyDocument, ...). `NotImplemented` is not a modeled outcome of CreatePolicy on any status, and 501 is not a status AWS IAM returns for it. The policy it creates is then usable as a role's PermissionsBoundary; a role whose identity policy allows workmailmessageflow:GetRawMessageContent but whose boundary grants only an unrelated action must be denied with AccessDeniedException, while the same role without the boundary reads through to ResourceNotFoundException.

Emulator: the request is answered `(NotImplemented) when calling the CreatePolicy operation: operation CreatePolicy is scaffolded but not implemented in this emulator`. services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_policy.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its handle() returns that error unconditionally. Because the boundary policy can never be created, nothing downstream in the check runs: the bounded/unbounded role pair is never built, so whether workmailmessageflow authorization honors a permissions boundary — and whether DeleteRolePermissionsBoundary restores the identity policy's grant — is completely unverified by this emulator.

Why: any client that provisions a managed policy before using it (permissions boundaries, boundary-capped roles, policy attachment flows, and every least-privilege test harness built on them) fails at the first call with a 501 that no AWS IAM endpoint produces, instead of receiving a policy ARN. Authorization-sensitive emulation silently has no coverage behind it.

Fix: implement the operation in its own handler, services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_policy.rs (create the managed policy in the IAM crate's state, return the modeled CreatePolicyResponse, and remove the SCAFFOLDED marker line), so a created policy ARN can be used as a role PermissionsBoundary and the permissions-boundary denial path becomes reachable.

Traces: workmailmessageflow.get-raw-message-content-permissions-boundary-restriction-rejected (errored on the first iam.create_policy call). Emulator source: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_policy.rs lines 3-12.

## Evidence
Expected (AWS): iam.create_policy(PolicyName=..., PolicyDocument=..., Tags=[...]) -> HTTP 200 with Policy.Arn, which is then usable as a role's PermissionsBoundary; the bounded role's GetRawMessageContent is denied with AccessDeniedException while the unbounded twin reaches ResourceNotFoundException.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePolicy operation: operation CreatePolicy is scaffolded but not implemented in this emulator — raised at the check's first call, so no role, no boundary and no authorization assertion is ever exercised.

## Steps to Reproduce
1. Build and boot the emulator; create an IAM client against it.
2. Call iam.create_policy(PolicyName="sim2real-wmf-boundary-1", PolicyDocument=json.dumps({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:GetCallerIdentity","Resource":"*"}]})).
3. Observe HTTP 501 ClientError(NotImplemented) instead of a 200 carrying Policy.Arn.
4. Replay: workmailmessageflow.get-raw-message-content-permissions-boundary-restriction-rejected.
