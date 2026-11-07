Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfnuf-alias-update-to-missing-versions-rejected-with-rollback-disabled cloudformation__lambda.cfnuf-default-update-to-missing-alias-version-rolls-back-automatically cloudformation__lambda.cfnuf-delete-stack-straight-from-update-failed-releases-everything cloudformation__lambda.cfnuf-fix-forward-from-update-failed-completes-without-touching-the-aliases cloudformation__lambda.cfnuf-get-template-of-update-failed-stack-returns-the-failed-template cloudformation__lambda.cfnuf-resubmitting-the-failing-template-from-update-failed-retries-the-update cloudformation__lambda.cfnuf-rollback-stack-outside-a-failed-status-rejected cloudformation__lambda.cfnuf-update-from-update-failed-without-disable-rollback-rejected cloudformation__lambda.cfnuf-update-failed-reason-wraps-lambdas-updatealias-rejection-verbatim cloudformation__lambda.cdk-hotswap-lambda-code-update-skips-stack-update-timeout-change-falls-back cloudformation__lambda.cfnuf-update-failed-events-name-the-version-qualified-missing-function cloudformation__lambda.cfnuf-version-and-alias-outputs-round-trip-through-lambda cloudformation__lambda.sam-autopublish-codesha256-governs-version-publication cloudformation__lambda.sam-function-globals-autopublishalias-publishes-new-version
Issue-Key: cloudformation__lambda.cfnuf-alias-update-to-missing-versions-rejected-with-rollback-disabled|run#4|AssertionError|assert final == "UPDATE_FAILED", (
Issue-Key: cloudformation__lambda.cfnuf-default-update-to-missing-alias-version-rolls-back-automatically|run#4|AssertionError|assert final == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__lambda.cfnuf-delete-stack-straight-from-update-failed-releases-everything|run#4|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-fix-forward-from-update-failed-completes-without-touching-the-aliases|run#4|AssertionError|assert _break_aliases(stack, ["593", "268"]) == "UPDATE_FAILED", f"{stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-get-template-of-update-failed-stack-returns-the-failed-template|run#5|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-resubmitting-the-failing-template-from-update-failed-retries-the-update|run#7|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-rollback-stack-outside-a-failed-status-rejected|run#8|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-update-from-update-failed-without-disable-rollback-rejected|run#3|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-update-failed-reason-wraps-lambdas-updatealias-rejection-verbatim|run#5|AssertionError|assert _break_aliases(stack, [want["Fn0Alias"], want["Fn1Alias"]]) == "UPDATE_FAILED", (
Issue-Key: cloudformation__lambda|GetFunction|ResourceNotFoundException
Issue-Key: cloudformation__lambda.cfnuf-update-failed-events-name-the-version-qualified-missing-function|outputs#3|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in self.describe().get("Outputs") or []}
Issue-Key: cloudformation__lambda.cfnuf-version-and-alias-outputs-round-trip-through-lambda|outputs#4|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in self.describe().get("Outputs") or []}
Issue-Key: cloudformation__lambda.sam-autopublish-codesha256-governs-version-publication|run#13|AssertionError|assert v1 == [f"FnVersion{sha_a[:10]}"], f"generated versions {v1!r}"
Issue-Key: cloudformation__lambda.sam-function-globals-autopublishalias-publishes-new-version|run#13|AssertionError|assert len(v1) == 1 and _VERSION_ID.match(v1[0]), (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/lambda: 4 net-new root causes — Lambda alias/version resource failures are never detected, functions are invisible to the Lambda API, Outputs carry no OutputValue, SAM AutoPublishAlias publishes nothing

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Four distinct net-new root causes. Open issue 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md owns other checks in this family; none of the replay keys below are carried by an open issue.

Cause 1 of 4: an AWS::Lambda::Alias pointing at a version that does not exist is provisioned successfully, so the stack never enters UPDATE_FAILED/CREATE_FAILED.
AWS: recorded behavior only — Lambda's UpdateAlias/CreateAlias refuses a FunctionVersion that does not exist, so the resource fails; the stack ends UPDATE_FAILED (DisableRollback=True) or UPDATE_ROLLBACK_COMPLETE (default), and the resource's status reason wraps Lambda's rejection verbatim. All of the UPDATE_FAILED-recovery surface (GetTemplate of the failed template, fix-forward, resubmit, RollbackStack, update-from-UPDATE_FAILED refusals, DeleteStack straight from UPDATE_FAILED) is reachable only from that state.
Emulator: every one of these stacks settles UPDATE_COMPLETE (or CREATE_COMPLETE), so the failure state never exists and nine checks that exercise recovery from it cannot run.
Why: a client can point an alias at a nonexistent version and be told the deploy succeeded; no recovery API can be exercised.
Fix: the AWS::Lambda::Alias resource handler must propagate Lambda's version-not-found rejection, and the stack state machine must park in UPDATE_FAILED/roll back accordingly.
Traces: cloudformation__lambda.cfnuf-alias-update-to-missing-versions-rejected-with-rollback-disabled, cloudformation__lambda.cfnuf-default-update-to-missing-alias-version-rolls-back-automatically, cloudformation__lambda.cfnuf-delete-stack-straight-from-update-failed-releases-everything, cloudformation__lambda.cfnuf-fix-forward-from-update-failed-completes-without-touching-the-aliases, cloudformation__lambda.cfnuf-get-template-of-update-failed-stack-returns-the-failed-template, cloudformation__lambda.cfnuf-resubmitting-the-failing-template-from-update-failed-retries-the-update, cloudformation__lambda.cfnuf-rollback-stack-outside-a-failed-status-rejected, cloudformation__lambda.cfnuf-update-from-update-failed-without-disable-rollback-rejected, cloudformation__lambda.cfnuf-update-failed-reason-wraps-lambdas-updatealias-rejection-verbatim.

Cause 2 of 4: a function created by CloudFormation is not registered in Lambda.
AWS: recorded behavior only — after the stack reports the function CREATE_COMPLETE, GetFunction on its ARN returns the function (so `cdk deploy --hotswap` can update its code without a stack update).
Emulator: GetFunction answers ResourceNotFoundException 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>'.
Why: nothing deployed through CloudFormation can be invoked, inspected or hotswapped through the Lambda API.
Fix: the AWS::Lambda::Function resource handler must create the function in Lambda state.
Traces: cloudformation__lambda.cdk-hotswap-lambda-code-update-skips-stack-update-timeout-change-falls-back.

Cause 3 of 4: stack Outputs entries carry no OutputValue.
AWS: recorded behavior only — DescribeStacks returns {OutputKey, OutputValue, ...} per output once the stack is complete.
Emulator: the OutputValue key is absent (KeyError: 'OutputValue'), so version/alias outputs cannot be round-tripped through Lambda.
Why: any consumer reading a deployed identifier out of a stack output fails outright.
Fix: output resolution in the stack state / DescribeStacks serializer.
Traces: cloudformation__lambda.cfnuf-update-failed-events-name-the-version-qualified-missing-function, cloudformation__lambda.cfnuf-version-and-alias-outputs-round-trip-through-lambda.

Cause 4 of 4: SAM AutoPublishAlias publishes no version.
AWS: recorded behavior only — a serverless function with AutoPublishAlias (and a changed CodeSha256) is expanded into a template carrying one hashed AWS::Lambda::Version logical id of the form <Function>Version<10 hex chars>, and a new version is published on each code change.
Emulator: no versions are generated ('generated versions []') and the processed template contains only {Fn: AWS::Lambda::Function, FnRole: AWS::IAM::Role} — no Version resource.
Why: SAM's canary/alias deployment model cannot be used at all.
Fix: the AWS::Serverless transform's AutoPublishAlias expansion (and the version-publication it implies).
Traces: cloudformation__lambda.sam-autopublish-codesha256-governs-version-publication, cloudformation__lambda.sam-function-globals-autopublishalias-publishes-new-version.

## Evidence
C1: nine checks report the stack status as 'UPDATE_COMPLETE' (e.g. "an update to nonexistent alias versions with DisableRollback=True reached 'UPDATE_COMPLETE', not UPDATE_FAILED"; "a default update to a nonexistent alias version reached 'UPDATE_COMPLETE'"; plain "status 'UPDATE_COMPLETE'" for the recovery checks).
C2: botocore ResourceNotFoundException on GetFunction: 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:<generated name>'.
C3: KeyError: 'OutputValue'.
C4: 'generated versions []'; and 'the processed template must hold one hashed FnVersion<10 hex>; resources {Fn: AWS::Lambda::Function, FnRole: AWS::IAM::Role}'.
Account, region and generated names are per-run fillers.

## Steps to Reproduce
C1: deploy a stack with a function, a version and an alias, then update the alias's FunctionVersion to a version that does not exist, with and without DisableRollback; poll DescribeStacks and then exercise GetTemplate/RollbackStack/UpdateStack/DeleteStack from the resulting state. C2: deploy a function through CDK and call GetFunction on its ARN. C3: read DescribeStacks Outputs of those stacks. C4: deploy a SAM template whose function sets AutoPublishAlias, change CodeSha256, and list the function's versions and the processed template's resources.
