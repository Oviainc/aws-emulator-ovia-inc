Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.continue-update-rollback-on-stable-stack-fails cloudformation.stack-continue-update-rollback-lifecycle cloudformation.stack-update-cancel-then-continue-rollback cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old cloudformation.type-configuration-set-and-batch-describe cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot
Issue-Key: cloudformation.continue-update-rollback-on-stable-stack-fails|_next_request#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=15)
Issue-Key: cloudformation.stack-continue-update-rollback-lifecycle|_next_request#13|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=15)
Issue-Key: cloudformation.stack-update-cancel-then-continue-rollback|_next_request#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=15)
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old|pull#12|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudformation.type-configuration-set-and-batch-describe|_activate_guard_hook#9|Timeout|time.sleep(3)
Issue-Key: cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot|_activate_guard_hook#17|Timeout|time.sleep(3)
Issue-Commit: 8fdd90c15790d38aa3c5d0088f99b03c79823121

# cloudformation: a custom resource's LATER provider requests never arrive (nothing drives them between calls), and the activate-a-Guard-Hook loop still runs out (6 of the 7 keys)

> _Split out of `issues/things-that-now-work/20261025T180059Z-cloudformation-custom-resource-providers-and-hook-configuration.md`,
> whose one passing key is the custom-resource request round trip. This report carries the
> replay keys that do NOT pass, re-filed here so they are not recorded as working. Everything
> below is what that fix established about them; a later fixer starts from here rather than
> from scratch._

## Operation
UpdateStack, ContinueUpdateRollback, ActivateType

## Description

=== Cause 1 of 2: a custom resource's provider receives the FIRST request of a stack operation and nothing delivers the ones a LATER phase owes ===

The parent fix made every stack operation that acts on a `Custom::*` resource send its
provider a request and hold the resource until the answer lands at the `ResponseURL`
(`src/state/provisioning/custom_resource.rs`, `src/service/custom_resource_plane.rs`), and the
four keys here moved: each check now receives its first provider request and fails later,
waiting for the NEXT one.

What that fix established: every provider request is published from the CloudFormation call
that owes it — `reconcile` for a create or an update, `delete_pass` for a removal, and
`advance_stabilisations` / `settle_due_rollbacks` for what a landed answer or a clock-owed
rollback owes — because nothing in this emulator runs between calls (documented in
`state::provisioning::advance_stabilisations`: "`settle` cannot make that call … so the
operations that REPORT a stack's progress ask here"). These four checks answer one request and
then poll SQS for the next WITHOUT calling CloudFormation again, so the request that would
follow — the replacement's cleanup `Delete`, or the `Update` a cancelled update's rollback
owes — is never published and the check runs out its window:

* `cloudformation__sns__sqs.xsvc-…-new-physical-id-on-update-triggers-cleanup-delete-of-old`
  (`pull#17`): the `Update` is answered with a NEW `PhysicalResourceId`, which is a
  replacement; the cleanup `Delete` of the old id with the OLD properties is built and sent by
  `advance_stabilisations` (pinned end to end in
  `tests/all/custom_resource_providers.rs::an_update_answered_with_a_new_physical_id_replaces_the_resource`),
  but only when a CloudFormation read runs after the answer.
* `cloudformation.continue-update-rollback-on-stable-stack-fails` (`_next_request#32`),
  `cloudformation.stack-continue-update-rollback-lifecycle` (`_next_request#25`) and
  `cloudformation.stack-update-cancel-then-continue-rollback` (`_next_request#30`): same shape,
  for the request a cancelled or failed update's rollback owes. `settle_due_rollbacks` now
  records the waits that rollback's own `reconcile` opened, and `failed_status_of` answers
  `UPDATE_ROLLBACK_FAILED` for a revert that cannot be carried through, so the statuses those
  checks assert are reachable once the request is delivered.

Fix: give the provider requests a driver that does not depend on a client's next
CloudFormation call. The request is an in-process call (SNS `Publish` / Lambda `Invoke`) made
with an `OperationContext`, which `Service::override_routing_error` — the seam the
`ResponseURL` plane is served from — does not have, so the plane cannot publish the follow-up
itself as written. Either the plane gains a path that can make that call, or the work a landed
answer owes is performed by whatever the emulator can run between requests.

=== Cause 2 of 2: the activate-a-Guard-Hook-then-configure-it flow still runs its retry loop out ===

Both checks still spend their whole 60 s window inside their OWN retry loop
(`_activate_guard_hook`, `time.sleep(3)`), and `replay_issue_units` names no call the emulator
diverged on — only the sleep the loop makes — so WHICH call the loop is waiting for is not
visible from the replay.

What the parent fix established, so a later fixer does not repeat it:

* every registry stub the parent report named is implemented: `RegisterType`,
  `DescribeTypeRegistration`, `SetTypeDefaultVersion` and `ListTypeRegistrations` were already,
  and `ListTypeVersions` is now (`src/ops/list_type_versions.rs`); the parent report's
  "answers HTTP 501" paragraph is therefore stale, and no 501 remains in this service's
  registry surface (the stubs left in this crate are the three organizations-access operations
  and the three stack-refactor ones).
* `ActivateType` resolves an AWS-published Hook (`AWS::Hooks::GuardHook`,
  `AWS::Hooks::LambdaHook` — `registry::PUBLIC_EXTENSION_CATALOG`) by kind and name with no
  publisher id, and now ALSO by the public-extension ARN this registry publishes for one,
  which carries no publisher segment (`registry::split_public_type_arn` reads both forms;
  pinned by `registry.rs`'s own table and by `tests/all/public_extension_arns.rs`). Before,
  that ARN was refused `CFNRegistryException` "TypeName is invalid", which a retry loop would
  spin on.
* `AWS::CloudFormation::GuardHook` and `AWS::CloudFormation::LambdaHook` are in the published
  resource-type catalogue, so a template declaring either is no longer refused as an
  unrecognised type.
* `SetTypeConfiguration` and `BatchDescribeTypeConfigurations` answer for an activated Hook
  (`tests/all/type_configurations.rs`), as the parent report already said.

Fix: recover what `_activate_guard_hook` actually calls — the recorded request sequence for
either check between its `ActivateType` and its `SetTypeConfiguration` — and close whichever
call the loop retries. Guessing is not available: the replay reports only the sleep, so any
change made without that sequence cannot be shown to address it.

## Evidence
Replay of the parent report's seven units after that fix: one passes
(`…-request-roundtrip-and-getatt-returns-response-data`), four keys are `moved` and two are
`still_failing`.

* `…-new-physical-id-on-update-triggers-cleanup-delete-of-old` — moved, now
  `pull#17|Timeout|for msg in resp.get("Messages", [])`.
* `continue-update-rollback-on-stable-stack-fails` — moved, now `_next_request#32|Timeout|raise
  AssertionError(f"no custom-resource request arrived within {timeout_s}s")`.
* `stack-continue-update-rollback-lifecycle` — moved, now `_next_request#25`, same assertion.
* `stack-update-cancel-then-continue-rollback` — moved, now `_next_request#30`, same assertion
  (an earlier pass of the same fix reached `run#28`, "a revert that fails should stick the
  stack in UPDATE_ROLLBACK_FAILED", which is the status now reachable).
* `type-configuration-set-and-batch-describe` and
  `concurrent-batch-describe-type-configurations-consistent-snapshot` — still_failing at
  `_activate_guard_hook#9` / `#17`, `time.sleep(3)`, with no divergent call named.

## Steps to Reproduce
1. Boot the emulator from this checkout and `replay_issue_units` for this report.
2. Cause 1: create an SNS topic, subscribe an SQS queue, `CreateStack` with a
   `Custom::Probe` whose `ServiceToken` is that topic, answer the `Create` request at its
   `ResponseURL`, poll `DescribeStacks` until `CREATE_COMPLETE`, then `UpdateStack` with a
   changed property and answer the `Update` with a DIFFERENT `PhysicalResourceId`. The cleanup
   `Delete` arrives on the queue only after a further CloudFormation call; without one, nothing
   publishes it.
3. Cause 2: replay either check and capture the request sequence its `_activate_guard_hook`
   helper makes — the call it retries is the one to fix.
