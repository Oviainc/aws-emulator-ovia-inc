Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.create-stack-s3-bucket-name-conflict-rolls-back cloudformation.cfn-topstacks-e2e-top-stacks-resource-breakdown-lifecycle cloudformation.cfn-topstacks-rolled-back-create-leaves-the-active-set cloudformation__s3.cfn-topstacks-nested-stack-with-a-missing-template-rejected cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources cloudformation.resource-schema-rejects-unknown-and-mistyped-properties cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists cloudformation.update-rollback-restores-pre-update-state cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback cloudformation__lambda.cfnuf-change-set-on-update-failed-stack-diffs-against-the-rolled-back-state cloudformation__lambda.cfnuf-continue-update-rollback-and-cancel-update-on-update-failed-rejected cloudformation__lambda.cfnuf-drift-detection-on-update-failed-stack-rejected cloudformation__lambda.cfnuf-execute-change-set-on-update-failed-without-disable-rollback-rejected cloudformation__lambda.cfnuf-rollback-stack-from-update-failed-restores-the-last-stable-template cloudformation__sqs.cfn-update-out-of-range-visibility-timeout-rejected-rolls-back cloudformation.events-chronological-ordering cloudformation.op-contract-setStackPolicy cloudformation.changeset-not-found-and-invalid-status-errors cloudformation.rollback-stack-physically-deletes-the-preserved-resource cloudformation.deep-cloudformation-2-express-disable-rollback-conflict-rejected cloudformation__cloudwatch__s3__sns__sqs.cfn-rollback-trigger-alarm-reverts-update-and-restores-property cloudformation.deletion-and-update-replace-policy-retain cloudformation__dynamodb__s3.cfn-cli-delete-stack-retains-s3-bucket-with-retain-policy-positive cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact cloudformation.deep-cloudformation-1-cfn-enforce-force-delete-stack-orphans-functional-bucket
Issue-Key: cloudformation.create-stack-s3-bucket-name-conflict-rolls-back|run#10|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.cfn-topstacks-e2e-top-stacks-resource-breakdown-lifecycle|phase#9|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudformation.cfn-topstacks-rolled-back-create-leaves-the-active-set|_create#5|AssertionError|assert status == expect, (
Issue-Key: cloudformation__s3.cfn-topstacks-nested-stack-with-a-missing-template-rejected|_create#6|AssertionError|assert status == expect, (
Issue-Key: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack|run#13|AssertionError|assert broken_status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources|run#5|AssertionError|assert final == "CREATE_FAILED", f"a create with a missing alias version reached {final!r}"
Issue-Key: cloudformation.resource-schema-rejects-unknown-and-mistyped-properties|_assert_schema_failure#11|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists|run#13|AssertionError|assert status_b == "CREATE_COMPLETE", f"the importing stack ended in {status_b!r}"
Issue-Key: cloudformation.update-rollback-restores-pre-update-state|run#12|AssertionError|assert rolled["StackStatus"] == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback|run#16|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__lambda.cfnuf-change-set-on-update-failed-stack-diffs-against-the-rolled-back-state|run#3|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-continue-update-rollback-and-cancel-update-on-update-failed-rejected|run#3|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-drift-detection-on-update-failed-stack-rejected|run#3|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-execute-change-set-on-update-failed-without-disable-rollback-rejected|run#4|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__lambda.cfnuf-rollback-stack-from-update-failed-restores-the-last-stable-template|run#4|AssertionError|assert _break_aliases(stack, ["593"]) == "UPDATE_FAILED", f"status {stack.status()!r}"
Issue-Key: cloudformation__sqs.cfn-update-out-of-range-visibility-timeout-rejected-rolls-back|run#16|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.events-chronological-ordering|run#13|AssertionError|assert updated["StackStatus"] == "UPDATE_COMPLETE", updated.get("StackStatusReason")
Issue-Key: cloudformation.op-contract-setStackPolicy|run#8|AssertionError|assert status == "UPDATE_COMPLETE", f"positive-control update must succeed with no stack policy, got {status}"
Issue-Key: cloudformation.changeset-not-found-and-invalid-status-errors|run#25|AssertionError|assert exec_status == "UPDATE_COMPLETE", (
Issue-Key: cloudformation.rollback-stack-physically-deletes-the-preserved-resource|run#11|AssertionError|assert parked == "CREATE_FAILED", (
Issue-Key: cloudformation.deep-cloudformation-2-express-disable-rollback-conflict-rejected|run#6|AssertionError|raise AssertionError(
Issue-Key: cloudformation__cloudwatch__s3__sns__sqs.cfn-rollback-trigger-alarm-reverts-update-and-restores-property|run#18|AssertionError|assert triggered == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.deletion-and-update-replace-policy-retain|run#21|AssertionError|assert _param_exists(ssm, rolling_v1), (
Issue-Key: cloudformation__dynamodb__s3.cfn-cli-delete-stack-retains-s3-bucket-with-retain-policy-positive|run#15|AssertionError|assert bucket_name in all_buckets, f"retained bucket {bucket_name} vanished after stack delete"
Issue-Key: cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact|_resource_failure_reason#21|AssertionError|raise AssertionError(
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-force-delete-stack-orphans-functional-bucket|run#16|AssertionError|assert _resource_status(cfn, stack_id, "B") == "DELETE_FAILED", (
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudFormation never rolls a failed stack back: failed creates/updates end COMPLETE or park in *_FAILED, DisableRollback/RollbackTriggers/Retain policies are ignored, and delete failures are invisible (9 causes)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
This issue bundles 9 net-new root causes in the CloudFormation stack state machine. Related open issues describe the same family for individual resource types (20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md, 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md); none of them carry the replay keys below.

Cause 1 of 9: a resource that fails to create does not roll the stack back.
AWS: CreateStack defaults to OnFailure=ROLLBACK (Smithy: CreateStack input OnFailure enum, DisableRollback boolean). When any resource fails CREATE, CloudFormation deletes the already-created resources and the stack terminates in ROLLBACK_COMPLETE.
Emulator: the stack reaches CREATE_COMPLETE even though the failing resource could not be created (duplicate S3 bucket name, nested stack whose TemplateURL does not exist, two ELBv2 listener rules on the same priority, a Lambda alias naming a missing version).
Why: a client that polls DescribeStacks for a terminal status reads success for a deploy that did not happen, and then uses resources that do not exist. Rollback-driven CI gates never fire.
Fix: the CreateStack provisioning engine's failure path (the stack-engine/resource-provisioner in the cloudformation service crate) must transition the stack through ROLLBACK_IN_PROGRESS to ROLLBACK_COMPLETE instead of completing.
Traces: cloudformation.create-stack-s3-bucket-name-conflict-rolls-back ('expected ROLLBACK_COMPLETE, got CREATE_COMPLETE'), cloudformation.cfn-topstacks-e2e-top-stacks-resource-breakdown-lifecycle, cloudformation.cfn-topstacks-rolled-back-create-leaves-the-active-set, cloudformation__s3.cfn-topstacks-nested-stack-with-a-missing-template-rejected, cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack, cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources.

Cause 2 of 9: when a create failure IS detected, the stack parks in CREATE_FAILED and never rolls back.
AWS: CREATE_FAILED is not a terminal status for a rollback-enabled create; the stack continues to ROLLBACK_IN_PROGRESS/ROLLBACK_COMPLETE.
Emulator: the stack stays in CREATE_FAILED forever, so callers waiting for ROLLBACK_COMPLETE never converge.
Why: the documented recovery path (delete the ROLLBACK_COMPLETE stack, or RollbackStack) cannot be taken, and the stack's resources are left half-created.
Fix: same failure path; CREATE_FAILED must be a transient state that drives the rollback phase.
Traces: cloudformation.resource-schema-rejects-unknown-and-mistyped-properties ('must fail its resource-schema validation and roll back, got CREATE_FAILED'), cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists ('the importing stack ended in CREATE_FAILED').

Cause 3 of 9: a failed UpdateStack reports UPDATE_COMPLETE.
AWS: a resource that fails during an update drives UPDATE_ROLLBACK_IN_PROGRESS -> UPDATE_ROLLBACK_COMPLETE (or UPDATE_ROLLBACK_FAILED); the pre-update property values are restored.
Emulator: the stack reports UPDATE_COMPLETE with the failed change neither applied nor rolled back.
Why: every update-failure workflow (ContinueUpdateRollback, RollbackStack, change sets against an UPDATE_FAILED stack, drift on a rolled-back stack) is unreachable, and a client believes a broken update succeeded.
Fix: the UpdateStack provisioning engine's failure path in the cloudformation service crate.
Traces: cloudformation.update-rollback-restores-pre-update-state, cloudformation__cognito-idp.cognito-user-pool-group-explicit-generated-name-rejected-rollback, and the five cloudformation__lambda.cfnuf-* checks that all assert on "status 'UPDATE_COMPLETE'".

Cause 4 of 9: an update that fails on a provider-side constraint parks in UPDATE_FAILED and never rolls back.
AWS: an invalid property value rejected by the owning service fails the resource and the stack rolls back to UPDATE_ROLLBACK_COMPLETE (recorded example: SQS caps VisibilityTimeout at 43200).
Emulator: terminal UPDATE_FAILED.
Why: the stack is left in a state no documented API call can leave.
Fix: same update failure path.
Traces: cloudformation__sqs.cfn-update-out-of-range-visibility-timeout-rejected-rolls-back.

Cause 5 of 9: in-place property updates of existing resources are not implemented, so legitimate updates fail.
AWS: UpdateStack applies a changed property to the existing resource with no interruption or with replacement, per the resource type's update behaviour, and the stack reaches UPDATE_COMPLETE.
Emulator: the update fails with the resource-level reason 'NotImplemented: this emulator does not change the properties of an existing AWS::SNS::Topic in place; the resource was left as it is', so a positive-control UpdateStack ends UPDATE_FAILED.
Why: no deploy-then-modify workflow works, and the UPDATE_FAILED is not even a real AWS outcome for the template under test.
Fix: the per-resource-type update handlers in the cloudformation provisioning engine.
Traces: cloudformation.events-chronological-ordering, cloudformation.op-contract-setStackPolicy ('positive-control update must succeed with no stack policy, got UPDATE_FAILED'), cloudformation.changeset-not-found-and-invalid-status-errors ('expected UPDATE_COMPLETE, got UPDATE_FAILED').

Cause 6 of 9: DisableRollback is ignored.
AWS: Smithy: CreateStack.DisableRollback (boolean) and the mutually exclusive OnFailure enum; with DisableRollback=true a failed create must stay in CREATE_FAILED with its resources preserved, and specifying DisableRollback together with a conflicting OnFailure is rejected with ValidationError.
Emulator: the create completes (CREATE_COMPLETE) regardless, and the conflicting DisableRollback/OnFailure combination is accepted and a stack is created.
Why: the preserve-for-debugging contract is lost, and the resources RollbackStack is supposed to clean up later are never parked.
Fix: CreateStack/UpdateStack input validation plus the rollback decision in the provisioning engine.
Traces: cloudformation.rollback-stack-physically-deletes-the-preserved-resource ('CreateStack(DisableRollback=True) must park the failed create in CREATE_FAILED, got CREATE_COMPLETE'), cloudformation.deep-cloudformation-2-express-disable-rollback-conflict-rejected.

Cause 7 of 9: RollbackConfiguration RollbackTriggers are never evaluated.
AWS: Smithy: CreateStack/UpdateStack.RollbackConfiguration; an alarm listed in RollbackTriggers that goes into ALARM during the monitoring window forces the update to roll back and the pre-update property is restored.
Emulator: the update reaches UPDATE_COMPLETE with the alarm in ALARM.
Why: alarm-gated deploys are silently unguarded.
Fix: RollbackConfiguration handling in the update engine (monitoring-time-in-minutes window + trigger evaluation).
Traces: cloudformation__cloudwatch__s3__sns__sqs.cfn-rollback-trigger-alarm-reverts-update-and-restores-property.

Cause 8 of 9: DeletionPolicy: Retain and UpdateReplacePolicy: Retain are not honoured.
AWS: a resource carrying DeletionPolicy: Retain survives stack delete; one carrying UpdateReplacePolicy: Retain survives being replaced during an update.
Emulator: the resource is destroyed in both cases.
Why: data-retention intent is violated — the replayed checks show a retained S3 bucket vanishing after DeleteStack.
Fix: the delete/replace path of the provisioning engine must consult the resource's DeletionPolicy/UpdateReplacePolicy before destroying it.
Traces: cloudformation.deletion-and-update-replace-policy-retain, cloudformation__dynamodb__s3.cfn-cli-delete-stack-retains-s3-bucket-with-retain-policy-positive.

Cause 9 of 9: delete failures are never surfaced.
AWS: a resource that cannot be deleted (a non-empty S3 bucket) produces a DELETE_FAILED stack event naming that logical id and a ResourceStatusReason, and the stack ends DELETE_FAILED.
Emulator: no DELETE_FAILED event exists for the logical id and the resource is still reported CREATE_COMPLETE after the delete.
Why: the reason a delete failed is unauditable and force-delete/retain workflows cannot be driven.
Fix: the DeleteStack path must record per-resource DELETE_FAILED events and statuses.
Traces: cloudformation.delete-stack-retain-resources-leaves-the-retained-bucket-intact, cloudformation.deep-cloudformation-1-cfn-enforce-force-delete-stack-orphans-functional-bucket.

## Evidence
Replayed suite, service cloudformation (73/242 passed). Representative expected-vs-actual:
- create-stack-s3-bucket-name-conflict-rolls-back: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'.
- cfn-topstacks-rolled-back-create-leaves-the-active-set: "stack ...-bad reached 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE'".
- resource-schema-rejects-unknown-and-mistyped-properties: "must fail its resource-schema validation and roll back, got 'CREATE_FAILED'".
- cfnuf-* (5 checks): "AssertionError: status 'UPDATE_COMPLETE'" where UPDATE_FAILED/UPDATE_ROLLBACK_COMPLETE was owed.
- cfn-update-out-of-range-visibility-timeout-rejected-rolls-back: "ended in 'UPDATE_FAILED', expected 'UPDATE_ROLLBACK_COMPLETE' (SQS caps VisibilityTimeout at 43200)".
- events-chronological-ordering: "The following resource(s) failed to update: [Topic]. NotImplemented: this emulator does not change the properties of an existing AWS::SNS::Topic in place".
- rollback-stack-physically-deletes-the-preserved-resource: "CreateStack(DisableRollback=True) must park the failed create in CREATE_FAILED, got 'CREATE_COMPLETE'".
- cfn-rollback-trigger-alarm-reverts-update-and-restores-property: "expected the alarm to force a rollback, got 'UPDATE_COMPLETE'".
- deletion-and-update-replace-policy-retain: "UpdateReplacePolicy:Retain must leave the replaced resource ... in place; it was destroyed".
- cfn-cli-delete-stack-retains-s3-bucket-with-retain-policy-positive: "retained bucket ... vanished after stack delete".
- delete-stack-retain-resources-leaves-the-retained-bucket-intact: "has no DELETE_FAILED event for logical id 'Bucket'".
- deep-cloudformation-1-cfn-enforce-force-delete-stack-orphans-functional-bucket: "bucket resource status 'CREATE_COMPLETE', expected DELETE_FAILED".

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Re-run exactly the check ids in trace_ids.
3. Minimal manual repro for cause 1: CreateStack with a template declaring an S3 bucket whose name is already taken (or two ELBv2 listener rules with the same Priority), then poll DescribeStacks — the emulator reports CREATE_COMPLETE where AWS reports ROLLBACK_COMPLETE.
4. For cause 6: CreateStack(DisableRollback=true) with a template that cannot create, then DescribeStacks — expect CREATE_FAILED with resources preserved.
5. For cause 8: CreateStack with a bucket carrying DeletionPolicy: Retain, DeleteStack, then HeadBucket — the bucket must still exist.
