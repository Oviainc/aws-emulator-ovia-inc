Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied
Issue-Key: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied|run#65|AssertionError|assert status == "ROLLBACK_COMPLETE", f"stack 1 ended {status!r}, expected ROLLBACK_COMPLETE"
Issue-Commit: d958b1b01392fe431b86227462c36506cac78d51

# cloudformation__iam__sts: a stack whose AWS::IAM::ManagedPolicy has an invalid PolicyDocument (statement with no Resource) ends CREATE_COMPLETE instead of CREATE_FAILED -> ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `d958b1b01392fe431b86227462c36506cac78d51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation create whose IAM managed-policy resource carries a policy document real IAM refuses reaches a SUCCESS terminal state, so no resource failure and no rollback is ever observed. Failing check: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied.

AWS: a stack template declaring AWS::IAM::Role (pinned RoleName) plus an AWS::IAM::ManagedPolicy that DependsOn the role and whose PolicyDocument has an Allow statement with an Action but NO Resource element is created with CAPABILITY_NAMED_IAM. The policy resource provider calls IAM, and IAM refuses that document: Smithy (com.amazonaws.iam) CreatePolicy declares MalformedPolicyDocumentException with awsQueryError code MalformedPolicyDocument, httpError 400; the recorded IAM message for a resource-less statement is the fixed text "Policy statement must contain resources." (no variable parts). CloudFormation therefore emits ManagedPolicy CREATE_FAILED with a ResourceStatusReason that wraps the provider refusal (fixed fragments "Policy statement must contain resources.", "Service: Iam", "Status Code: 400", "HandlerErrorCode: InvalidRequest"; the embedded IAM Request ID and RequestToken are per-run values, not part of the template), moves the stack to ROLLBACK_IN_PROGRESS and then to the terminal ROLLBACK_COMPLETE (Smithy StackStatus enum includes CREATE_FAILED, ROLLBACK_IN_PROGRESS, ROLLBACK_COMPLETE), and deletes the role it had already created, so iam:GetRole for the pinned role name answers 404 NoSuchEntity with "The role with name <RoleName> cannot be found." and no managed policy remains under the template's Path.

Emulator: the same create reaches the terminal status CREATE_COMPLETE. The stack is reported as a successful deployment, so there is no CREATE_FAILED resource transition, no rollback, and the resource the failing one depended on is left in place. Hypothesis (neither the model nor the recording states which half is at fault): either the managed-policy resource is provisioned without validating PolicyDocument against the rule IAM enforces, so nothing ever fails, or the provider failure is swallowed and the stack is still marked COMPLETE. The emulator's own IAM CreatePolicy path with the identical document is asserted separately by this check and was not reached.

Why: a client cannot distinguish a deployment that worked from one whose IAM policy was rejected. CI that gates on DescribeStacks StackStatus == CREATE_COMPLETE passes for a stack that on AWS rolls back, and the roles the failed stack created are left behind instead of being deleted, so cleanup and name-reuse logic (the pinned RoleName is free again after rollback on AWS) diverges. Infrastructure authors also lose the only signal that their policy document is malformed.

Fix: the CloudFormation create/rollback state machine (resource provider error propagation -> stack status transition and resource deletion) plus the AWS::IAM::ManagedPolicy resource provider's PolicyDocument validation, which must surface IAM's MalformedPolicyDocument 400 as the resource's CREATE_FAILED reason rather than succeeding.

Related already-open issue, named here because it looks similar but does not carry this check's replay key: 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md (failed creates/updates ending COMPLETE). This failure is reported separately because the suite did not mark the check as owned by it; if the two share a root cause, close them together.

Traces: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied (failing assertion at the "Stack 1 outcome" step; all earlier assertions in the check — the CAPABILITY_IAM refusal, the GetAtt RoleId output, the in-place trust flip keeping RoleId, the replacing rename — passed).

## Evidence
Replay output for cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied:

  AssertionError: stack 1 ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE
  (assert status == "ROLLBACK_COMPLETE", f"stack 1 ended {status!r}, expected ROLLBACK_COMPLETE")

Expected (real AWS): DescribeStacks StackStatus == ROLLBACK_COMPLETE for the stack, DescribeStackEvents containing, in order, (RoleA, CREATE_COMPLETE) -> (BadPolicy, CREATE_FAILED) -> (RoleA, DELETE_COMPLETE), the CREATE_FAILED ResourceStatusReason containing the fixed fragments "Policy statement must contain resources.", "Service: Iam", "Status Code: 400", "HandlerErrorCode: InvalidRequest", iam:GetRole on the pinned role name answering NoSuchEntity / "The role with name <RoleName> cannot be found.", and no managed policy listed under the template's Path.

Actual (emulator): DescribeStacks StackStatus == CREATE_COMPLETE (a success terminal state); the stack never fails and never rolls back, so the dependent role created earlier in the stack is not deleted.

Model grounding: Smithy com.amazonaws.iam CreatePolicy errors include MalformedPolicyDocumentException (awsQueryError MalformedPolicyDocument, HTTP 400); Smithy com.amazonaws.cloudformation StackStatus enum includes CREATE_FAILED, ROLLBACK_IN_PROGRESS, ROLLBACK_COMPLETE and ResourceStatus includes CREATE_FAILED / DELETE_COMPLETE.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral check cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied.

Or by hand, against the emulator endpoint:
1. sts:GetCallerIdentity to obtain <account>.
2. cloudformation:CreateStack with Capabilities=[CAPABILITY_NAMED_IAM] and a template whose Resources are:
   - RoleA: AWS::IAM::Role with RoleName=<pinned-name> and AssumeRolePolicyDocument allowing sts:AssumeRole for Principal {"AWS": "arn:aws:iam::<account>:root"};
   - BadPolicy: AWS::IAM::ManagedPolicy, DependsOn RoleA, Roles=[{"Ref":"RoleA"}], ManagedPolicyName=<name>, Path=/<name>/, PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject"}]}  (no Resource element).
3. Poll cloudformation:DescribeStacks until the stack reaches a terminal status.
   Observed: CREATE_COMPLETE. Expected on AWS: ROLLBACK_COMPLETE.
4. cloudformation:DescribeStackEvents for the stack: expect RoleA CREATE_COMPLETE, then BadPolicy CREATE_FAILED whose reason contains "Policy statement must contain resources." / "Service: Iam" / "Status Code: 400" / "HandlerErrorCode: InvalidRequest", then RoleA DELETE_COMPLETE.
5. iam:GetRole with RoleName=<pinned-name>: expect NoSuchEntity 404 with "The role with name <pinned-name> cannot be found." after the rollback; and iam:ListPolicies with PathPrefix=/<name>/ must list nothing.
