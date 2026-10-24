Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.lambda-secretsmanager-cross-account-secret-denied
Issue-Key: lambda__rds__secretsmanager|PutResourcePolicy|NotImplemented
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# Secrets Manager PutResourcePolicy is an unimplemented scaffold stub, so no secret resource policy can be attached or enforced

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
One cause. AWS: Secrets Manager PutResourcePolicy attaches a resource-based policy to a secret and returns {ARN, Name}; the attached policy is then enforced on every access to that secret, and the matching GetResourcePolicy returns the stored document. A policy that does not grant the calling principal (for example a secret in another account, or a policy whose statements deny the caller) makes GetSecretValue fail with AccessDeniedException — which is the entire point of the cross-account denial the check exercises: a Lambda execution role in one account reading a secret in another is refused unless the secret's resource policy allows it.

Emulator: PutResourcePolicy answers error code 'NotImplemented' with the body 'operation PutResourcePolicy is scaffolded but not implemented in this emulator'. No resource policy can be attached at all, so no cross-account or deny decision about a secret can ever be made, and the check cannot even set up its negative case. (This is a different operation from the one covered by the already-open issue 'lambda Invoke refuses every real invocation with NotImplemented', which the sibling check lambda-secretsmanager-resource-policy-denies-principal-rejected is owned by.)

Why: resource-policy-gated secret access is unavailable. A client cannot share a secret across accounts, cannot restrict a secret to one role, and — worse for a test target — gets no denial where AWS denies, so a workload that would be refused in production appears to work.

Fix: the PutResourcePolicy handler in the Secrets Manager service (services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/put_resource_policy.rs), its stored policy document on the secret record, the matching GetResourcePolicy/DeleteResourcePolicy projections, and the authorization check on GetSecretValue that must consult it and raise AccessDeniedException.

Traces: lambda__rds__secretsmanager.lambda-secretsmanager-cross-account-secret-denied.

## Evidence
lambda__rds__secretsmanager.lambda-secretsmanager-cross-account-secret-denied — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutResourcePolicy operation: operation PutResourcePolicy is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. secretsmanager: CreateSecret, then PutResourcePolicy on it with any valid policy document (for example one granting only a single role ARN secretsmanager:GetSecretValue).
3. The call returns error code NotImplemented instead of {ARN, Name}; GetResourcePolicy therefore has nothing to return and GetSecretValue is never gated by the policy.
