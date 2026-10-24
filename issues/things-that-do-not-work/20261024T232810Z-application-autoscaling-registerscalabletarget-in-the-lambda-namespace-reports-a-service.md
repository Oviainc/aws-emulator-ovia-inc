Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up
Issue-Key: application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up|run#35|AssertionError|assert _SLR_NAME in _slr_names(iam, _SLR_PRINCIPAL), (
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# RegisterScalableTarget in the lambda namespace reports a service-linked RoleARN that is never minted in IAM: no role exists under /aws-service-role/lambda.application-autoscaling.amazonaws.com/

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterScalableTarget

## Description
One net-new root cause, grounded in the single failing behavioral check of this sweep that is neither ALREADY FILED nor OWNED by an open issue.

Adjacency note for the fixer: open issue 20261024T115439Z-application-autoscaling-application-auto-scaling-scheduled-actions-never-fire-target.md carries, as its cause 4, the DynamoDB twin of this defect (AWSServiceRoleForApplicationAutoScaling_DynamoDBTable never minted), grounded in check application-autoscaling.service-linked-role-is-the-role-the-scalable-target-uses. That issue does NOT carry the check id below and does not describe the lambda namespace, so this is filed separately; if a single per-namespace minting fix closes both, say so there. Resolved issue 20261022T032542Z-application-auto-scaling-control-plane-diverges-in-9.md fixed cause 4 'the suffixed, per-namespace service-linked role' — that fix made the REPORTED ARN per-namespace (the ARN assertion in this check passes), but it did not create the role in IAM state, which is the remaining gap described here. This is not a refusal-left-refused reopen: the RegisterScalableTarget call itself succeeds with HTTP 200.

Cause 1 of 1: registering a Lambda alias as a scalable target reports a Lambda-specific service-linked RoleARN, but no such role is ever created in IAM state.

AWS: Smithy RegisterScalableTarget (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json) takes ServiceNamespace, ResourceId and ScalableDimension as required, with RoleARN optional; ScalableTarget.RoleARN is modeled as "The ARN of an IAM role that allows Application Auto Scaling to modify the scalable target on your behalf." The model says nothing further about minting, so: documented behavior — the first registration in a service-linked namespace creates that namespace's service-linked role in the account. For the lambda namespace (ScalableDimension lambda:function:ProvisionedConcurrency) the role name is AWSServiceRoleForApplicationAutoScaling_LambdaConcurrency and the trust principal is lambda.application-autoscaling.amazonaws.com, so the role is addressable at arn:<partition>:iam::<account>:role/aws-service-role/lambda.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling_LambdaConcurrency and is therefore returned by IAM ListRoles with PathPrefix /aws-service-role/lambda.application-autoscaling.amazonaws.com/. The role name and trust principal are fixed API constants, not account-varying values. Both are per-namespace: a lambda target must not be backed by the DynamoDB role.

Emulator: RegisterScalableTarget succeeds (HTTP 200) and DescribeScalableTargets reports the correct per-namespace RoleARN — that assertion passes — but the role behind it is never created. IAM ListRoles with PathPrefix /aws-service-role/lambda.application-autoscaling.amazonaws.com/ returns an EMPTY role list, so the reported RoleARN resolves to nothing. Replay detail: AssertionError: "IAM has no service-linked role named 'AWSServiceRoleForApplicationAutoScaling_LambdaConcurrency' under the /aws-service-role/lambda.application-autoscaling.amazonaws.com/ path even though a Lambda scalable target claims to use it; the roles found there were []".

Why: a client relying on AWS's behavior reads the RoleARN off the scalable target and then inspects it — GetRole, ListRoles under the service-linked path, ListAttachedRolePolicies, or IAM policy/permission generation and drift checks. Against the emulator every one of those lookups fails on a role the service itself just claimed to be using, so permission-validation and bootstrap flows that AWS satisfies cannot run at all. It also makes the per-namespace-ness of the role unverifiable through IAM: the only observable copy of the role identity is the string in the AAS record.

Fix: the RegisterScalableTarget handler in services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/register_scalable_target.rs — on first registration for a service-linked namespace, materialise the per-namespace service-linked role in the IAM emulator's state (correct role name, /aws-service-role/<principal>/ path and trust policy for that principal) in addition to projecting its ARN as the target's RoleARN, so the ARN the service reports is resolvable through IAM. The namespace-to-role-name/principal mapping belongs beside the existing RoleARN defaulting, which already derives the right string.

Caveat for the fixer: the grounding check is self-labelled DRAFT/unblessed (no real-AWS replay oracle exists for that file). The role name, trust principal and ARN path above are the documented constants it asserts and are safe to pin; the attachment shape (which managed policy, if any, is attached) is NOT asserted by this check and should be confirmed before pinning it. The check's later legs — that MinCapacity above the alias's live provisioned concurrency drives Lambda PutProvisionedConcurrencyConfig, and that lowering MaxCapacity pushes it back down — were never reached in this replay, so nothing is claimed about them here; they will be exercised once the role lookup passes.

Traces: application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up (suite section application-autoscaling__lambda, 0/1 passed).

## Evidence
Replay report (simparity-behavioral-ga-prod-v1): 48/134 passed, 79 mismatched, 7 errored. Per-service: application-autoscaling__lambda 0/1 (0%), 1 failed. Of the 86 failing checks, 85 are marked OWNED by an open issue or ALREADY FILED against an unimplemented scaffold stub and are skipped; this is the one surviving failure.

[mismatch] application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up
  File "check_application_autoscaling__lambda_min_capacity_pulls_lambda_provisioned_concurrency_up.py", line 340, in run
    assert _SLR_NAME in _slr_names(iam, _SLR_PRINCIPAL), (
  AssertionError: IAM has no service-linked role named 'AWSServiceRoleForApplicationAutoScaling_LambdaConcurrency' under the /aws-service-role/lambda.application-autoscaling.amazonaws.com/ path even though a Lambda scalable target claims to use it; the roles found there were []

Expected (AWS): iam.list_roles(PathPrefix='/aws-service-role/lambda.application-autoscaling.amazonaws.com/') includes a role named 'AWSServiceRoleForApplicationAutoScaling_LambdaConcurrency' after the first lambda-namespace RegisterScalableTarget.
Actual (emulator): that ListRoles call returns an empty Roles list.

What DID match, and bounds the cause:
- aas.register_scalable_target(ServiceNamespace='lambda', ResourceId='function:<fn>:BLUE', ScalableDimension='lambda:function:ProvisionedConcurrency', MinCapacity=2, MaxCapacity=10) returned HTTP 200.
- describe_scalable_targets reported (MinCapacity, MaxCapacity) == (2, 10) as registered.
- describe_scalable_targets reported RoleARN == arn:<partition>:iam::<account>:role/aws-service-role/lambda.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling_LambdaConcurrency, i.e. the per-namespace string is already correct; the assertion that failed is the next one, the IAM existence of that role.

The divergence is therefore purely the missing IAM-state side effect of registration, not the AAS wire response and not a message difference. Verdict is 'mismatch', not an error on an unimplemented stub.

Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/register_scalable_target.rs (RoleARN defaulting) and the IAM emulator's role state.

## Steps to Reproduce
Boot the emulator and drive these with botocore against the emulator endpoint.

1. Create an IAM execution role trusting lambda.amazonaws.com, attach arn:<partition>:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole, and create a python3.12 Lambda function with it; wait for State == 'Active'.
2. publish_version, then create_alias(Name='BLUE', FunctionVersion=<that version>).
3. Confirm the alias has no provisioned concurrency yet: lambda.get_provisioned_concurrency_config(FunctionName=<fn>, Qualifier='BLUE') must raise ProvisionedConcurrencyConfigNotFoundException.
4. application-autoscaling.register_scalable_target(ServiceNamespace='lambda', ResourceId='function:<fn>:BLUE', ScalableDimension='lambda:function:ProvisionedConcurrency', MinCapacity=2, MaxCapacity=10) — succeeds with HTTP 200.
5. application-autoscaling.describe_scalable_targets(ServiceNamespace='lambda', ResourceIds=['function:<fn>:BLUE']) — note the reported RoleARN; it already names AWSServiceRoleForApplicationAutoScaling_LambdaConcurrency under the lambda.application-autoscaling.amazonaws.com path.
6. iam.list_roles(PathPrefix='/aws-service-role/lambda.application-autoscaling.amazonaws.com/') — AWS lists AWSServiceRoleForApplicationAutoScaling_LambdaConcurrency here; the emulator returns Roles == []. Equivalently iam.get_role(RoleName='AWSServiceRoleForApplicationAutoScaling_LambdaConcurrency') answers NoSuchEntity for a role the scalable target claims to use.
7. Contrast with the DynamoDB namespace (open issue 20261024T115439Z cause 4): register a dynamodb:table:ReadCapacityUnits target and run iam.list_roles(PathPrefix='/aws-service-role/dynamodb.application-autoscaling.amazonaws.com/') — same empty result, which is why a single per-namespace minting fix should be validated against both namespaces.

To confirm a fix, re-replay application-autoscaling__lambda.min-capacity-pulls-lambda-provisioned-concurrency-up. Note that passing step 6 only unblocks the rest of that check (the provisioned-concurrency floor/ceiling enforcement legs), which this sweep never reached.
