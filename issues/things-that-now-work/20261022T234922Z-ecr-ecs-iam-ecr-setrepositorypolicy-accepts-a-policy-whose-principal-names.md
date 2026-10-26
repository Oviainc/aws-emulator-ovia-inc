Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__iam.ecracc-repository-policy-naming-a-missing-execution-role-rejected
Issue-Key: ecr__ecs__iam.ecracc-repository-policy-naming-a-missing-execution-role-rejected|_error#21|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# ECR SetRepositoryPolicy accepts a policy whose Principal names an IAM role that does not exist

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetRepositoryPolicy

## Description
Cause 1 of 1: SetRepositoryPolicy does not resolve the policy's Principal, so a grant to a nonexistent role is stored instead of refused.
AWS: ECS and ECR validate the same role reference differently — RegisterTaskDefinition stores an executionRoleArn verbatim without checking IAM, while SetRepositoryPolicy naming that same ARN as Principal is rejected with InvalidParameterException (client error, HTTP 400; Smithy ecr#SetRepositoryPolicy models InvalidParameterException), and the repository's existing policy is left untouched. The recorded message is "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Principal not found'".
Emulator: the call SUCCEEDS (HTTP 200) and the policy granting the ghost role is stored. The check's positive control — the same grant to a role that does exist — is accepted as expected, so the emulator accepts both and never distinguishes them.
Why: this inverts which half of an access question fails. An operator debugging "the task cannot pull the image" is told the repository policy is fine when the principal in it does not exist; the configuration is accepted locally and rejected in production.
Fix: resolve each statement's Principal against IAM in the SetRepositoryPolicy handler's validation and raise InvalidParameterException naming the PolicyText parameter when the principal cannot be found. Related: the same handler also stores statements carrying a Resource element, filed under service ecr.
Traces: ecr__ecs__iam.ecracc-repository-policy-naming-a-missing-execution-role-rejected.

## Evidence
Setup confirmed the role is absent: iam GetRole(RoleName=<ghost>) -> ('NoSuchEntity', 'The role with name <ghost> cannot be found.'), and RegisterTaskDefinition stored executionRoleArn=<ghost arn> verbatim.
expected: ecr SetRepositoryPolicy(repositoryName=R, policyText=<Allow pull to Principal {AWS: <ghost arn>}>) -> ('InvalidParameterException', "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Principal not found'") with the previously stored policy unchanged.
actual: the call SUCCEEDED (AssertionError: "the call SUCCEEDED; real AWS rejects it, so there is no error").

## Steps to Reproduce
1. Create an IAM role E (ECS task execution) and compute a second ARN <ghost> in the same path for a role never created; confirm with iam GetRole that it does not exist. 2. ecr CreateRepository R. 3. SetRepositoryPolicy on R granting ecr pull actions to Principal {AWS: <E arn>} -> accepted (control). 4. SetRepositoryPolicy on R granting the same actions to Principal {AWS: <ghost>} -> observe HTTP 200 instead of InvalidParameterException, and that the stored policy was overwritten.
