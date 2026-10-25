Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot cloudformation.continue-update-rollback-on-stable-stack-fails cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip cloudformation.deep-cloudformation-3-cfn-flight-update-while-wait-condition-barriered-rejected-then-succeeds cloudformation.deploy-iam-role-assumable-by-trusted-service cloudformation.detect-stackset-drift-lifecycle cloudformation.ec2mr-cfnres-in-progress-stack-omits-unstarted-resource cloudformation.execute-changeset-rejects-non-executable-status cloudformation.execute-failed-changeset-rejected cloudformation.export-deletion-blocked-while-imported-rejected cloudformation.list-stack-instance-resource-drifts cloudformation.signal-resource-invalid-status-rejected cloudformation.stack-continue-update-rollback-lifecycle cloudformation.stack-set-drift-detection-lifecycle cloudformation.stack-update-cancel-then-continue-rollback cloudformation.type-configuration-set-and-batch-describe cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed cloudformation__dynamodb__s3.cfn-deploy-iam-role-trust-mismatched-principal-negative cloudformation__dynamodb__s3.s3-replication-role-trust-s3-positive cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data
Issue-Key: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists|_http_until#14|Timeout|time.sleep(3.0)
Issue-Key: cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot|_activate_guard_hook#17|Timeout|time.sleep(3)
Issue-Key: cloudformation.continue-update-rollback-on-stable-stack-fails|_next_request#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=15)
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip|_wait_queue_updated#18|Timeout|time.sleep(3)
Issue-Key: cloudformation.deep-cloudformation-3-cfn-flight-update-while-wait-condition-barriered-rejected-then-succeeds|_wait_resource_status#8|Timeout|time.sleep(2)
Issue-Key: cloudformation.deploy-iam-role-assumable-by-trusted-service|_wait_assume_role_denied_with_retry#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation.detect-stackset-drift-lifecycle|_bootstrap_self_managed_stackset#3|Timeout|time.sleep(5)
Issue-Key: cloudformation.ec2mr-cfnres-in-progress-stack-omits-unstarted-resource|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation.execute-changeset-rejects-non-executable-status|_wait_stack#11|Timeout|time.sleep(3)
Issue-Key: cloudformation.execute-failed-changeset-rejected|_wait_stack_status#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation.export-deletion-blocked-while-imported-rejected|_wait_for_import_registered#15|Timeout|time.sleep(4)
Issue-Key: cloudformation.list-stack-instance-resource-drifts|_bootstrap_self_managed_stackset#3|Timeout|time.sleep(5)
Issue-Key: cloudformation.signal-resource-invalid-status-rejected|_wait_resource_blocked#5|Timeout|time.sleep(2)
Issue-Key: cloudformation.stack-continue-update-rollback-lifecycle|_next_request#13|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=15)
Issue-Key: cloudformation.stack-set-drift-detection-lifecycle|_create_stack_set_with_retry#5|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation.stack-update-cancel-then-continue-rollback|_next_request#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=15)
Issue-Key: cloudformation.type-configuration-set-and-batch-describe|_activate_guard_hook#9|Timeout|time.sleep(3)
Issue-Key: cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__dynamodb__s3.cfn-deploy-iam-role-trust-mismatched-principal-negative|run#17|Timeout|time.sleep(10)
Issue-Key: cloudformation__dynamodb__s3.s3-replication-role-trust-s3-positive|_wait_for_replica#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old|pull#12|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data|pull#14|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudFormation long-running operations never converge: 23 behavioral checks exhaust the harness's 60s wait because stacks, stack sets, wait conditions and custom resources never reach the state they poll for (1 cause)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: CloudFormation operations that a client must poll to completion never reach a terminal (or expected intermediate) state, so 23 checks in this suite end with 'Check timed out after 60.0s' rather than a wrong value.

AWS: every one of these flows converges in seconds to a state the documented poll loop can observe — a stack reaching CREATE_COMPLETE/ROLLBACK_COMPLETE or being observable in CREATE_IN_PROGRESS with its unstarted resources omitted; a change set reaching CREATE_COMPLETE or FAILED so the non-executable-status refusal can be read; UPDATE_ROLLBACK_FAILED so ContinueUpdateRollback is reachable; a WaitCondition handle being signalled; a StackSet/StackInstance drift-detection operation reaching SUCCEEDED; a custom resource's provider responding so GetAtt returns its response data; an export's deletion being refused while an import exists.
Emulator: none of these states is ever reached within the harness's 60s cap — the poll loop is still seeing a non-terminal (or absent) status when the check is cut off, so the operation produced no observable outcome at all.
Why: a client cannot wait for any of these operations. Waiters hang, CI deploys never finish, and the status-gated refusals (ExecuteChangeSet on a non-executable change set, ContinueUpdateRollback, SignalResource with an invalid status, export deletion while imported) can never be exercised, so the whole async half of the control plane is unusable. These are distinct from the rollback defects filed separately: there the stack reaches a WRONG terminal state, here it reaches NO state.
Fix: the async operation driver in the cloudformation service crate — the task that advances stack, change-set, stack-set, drift-detection, wait-condition and custom-resource operations through their status machines, plus the status transitions each of the corresponding handlers publishes.
Traces: the 23 check ids in trace_ids, each reporting 'Error: Check timed out after 60.0s'. Per-group examples: change sets (execute-changeset-rejects-non-executable-status, execute-failed-changeset-rejected), continue/cancel rollback (continue-update-rollback-on-stable-stack-fails, stack-continue-update-rollback-lifecycle, stack-update-cancel-then-continue-rollback, deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip), stack sets and drift (detect-stackset-drift-lifecycle, stack-set-drift-detection-lifecycle, list-stack-instance-resource-drifts), wait conditions and signalling (deep-cloudformation-3-cfn-flight-update-while-wait-condition-barriered-rejected-then-succeeds, signal-resource-invalid-status-rejected), custom resources (the two cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-* checks), exports (export-deletion-blocked-while-imported-rejected), in-progress reads (ec2mr-cfnres-in-progress-stack-omits-unstarted-resource), type configuration (concurrent-batch-describe-type-configurations-consistent-snapshot, type-configuration-set-and-batch-describe), and deploys whose stack never settles (cdk-deployed-l2-trio-serves-http-and-persists, deploy-iam-role-assumable-by-trusted-service, cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse, cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed, cloudformation__dynamodb__s3.cfn-deploy-iam-role-trust-mismatched-principal-negative, cloudformation__dynamodb__s3.s3-replication-role-trust-s3-positive).

## Evidence
Replayed suite: 23 cloudformation-family checks fail with the identical detail 'Error: Check timed out after 60.0s' and no HTTP-level divergence, i.e. the emulator answered the polls but never advanced the operation. The same suite shows 73/242 cloudformation checks passing, so the emulator is serving — this is convergence, not availability. Example poll loops that time out (from the replayed check sources): _wait_stack_status(cf, name, {'UPDATE_ROLLBACK_FAILED'}), _wait_changeset_status(cf, name, cs, {'CREATE_COMPLETE','FAILED'}), and stack-set operation-status polls.

## Steps to Reproduce
1. Boot the emulator and replay the suite; the 23 ids in trace_ids each report 'Check timed out after 60.0s'.
2. Minimal repro: CreateChangeSet(ChangeSetType='CREATE') then poll DescribeChangeSet for Status in {CREATE_COMPLETE, FAILED} — the status never reaches either.
3. Second repro: CreateStack with an AWS::CloudFormation::WaitCondition + WaitConditionHandle and poll DescribeStacks; the stack never leaves its non-terminal status.
4. Third repro: CreateStack with a custom resource backed by an SNS topic and poll for CREATE_COMPLETE, then GetAtt its response data.

## Resolution (partial: the wait-condition family)

What this report bundles is 23 poll loops under one heading, and they do not share one
implementation. The part that is now implemented is the WAIT-CONDITION half of the async
driver: a create or update whose template declares an `AWS::CloudFormation::WaitCondition`
is no longer completed by the clock — it waits until the wait condition has the distinct
success signals its `Count` asks for, `SignalResource` delivers them, a failure signal or
the declared `Timeout` fails the operation, and while it waits the stack reports the
resources whose work has started (the wait condition itself `*_IN_PROGRESS`, carrying the
reason that says its work was initiated) and not the ones that depend on it. Repro 3 above
now converges, `cloudformation.signal-resource-invalid-status-rejected` passes, and
`cloudformation.ec2mr-cfnres-in-progress-stack-omits-unstarted-resource` and
`cloudformation__sns__sqs.cfn-independent-branches-provision-concurrently-and-tear-down-in-reverse`
get past their polls and fail later.

Still open, and NOT resolved by that change — each of these checks blocks on an operation
of this service that is still a scaffold 501, and each of those operations has its own stub
issue:

* stack sets and stack-set drift (`detect-stackset-drift-lifecycle`,
  `list-stack-instance-resource-drifts`, `stack-set-drift-detection-lifecycle`):
  `CreateStackSet`, `CreateStackInstances`, `DescribeStackSet`,
  `DescribeStackSetOperation`, `DetectStackSetDrift`, `ListStackInstances`,
  `DescribeStackInstance`, `ListStackInstanceResourceDrifts`;
* the extension registry's writers (`type-configuration-set-and-batch-describe`,
  `concurrent-batch-describe-type-configurations-consistent-snapshot`): `ActivateType`,
  `SetTypeConfiguration`, `BatchDescribeTypeConfigurations`;
* continue/cancel rollback (`continue-update-rollback-on-stable-stack-fails`,
  `stack-continue-update-rollback-lifecycle`, `stack-update-cancel-then-continue-rollback`,
  `deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip`):
  `ContinueUpdateRollback`, `CancelUpdateStack`;
* cross-stack references (`export-deletion-blocked-while-imported-rejected`):
  `ListExports`, `ListImports` and `Fn::ImportValue`;
* `GetTemplateSummary`, which `ec2mr-cfnres-in-progress-stack-omits-unstarted-resource`
  now reaches;
* the remaining deploy flows, which wait on resources in the services the stack provisions
  (`cdk-deployed-l2-trio-serves-http-and-persists`,
  `deploy-iam-role-assumable-by-trusted-service`,
  `cfn-deploy-iam-role-trust-mismatched-principal-negative`,
  `s3-replication-role-trust-s3-positive`,
  `config-delivery-denied-when-bucket-policy-is-removed`) and on change-set and
  custom-resource behaviour (`execute-changeset-rejects-non-executable-status`,
  `execute-failed-changeset-rejected`,
  `deep-cloudformation-3-cfn-flight-update-while-wait-condition-barriered-rejected-then-succeeds`,
  the two `xsvc-cloudformation-3-sns-custom-resource-*` checks).
