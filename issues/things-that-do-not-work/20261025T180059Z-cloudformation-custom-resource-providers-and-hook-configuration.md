Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.continue-update-rollback-on-stable-stack-fails cloudformation.stack-continue-update-rollback-lifecycle cloudformation.stack-update-cancel-then-continue-rollback cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data cloudformation.type-configuration-set-and-batch-describe cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot
Issue-Key: cloudformation.continue-update-rollback-on-stable-stack-fails|_next_request#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=15)
Issue-Key: cloudformation.stack-continue-update-rollback-lifecycle|_next_request#13|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=15)
Issue-Key: cloudformation.stack-update-cancel-then-continue-rollback|_next_request#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=15)
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old|pull#12|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data|pull#14|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudformation.type-configuration-set-and-batch-describe|_activate_guard_hook#9|Timeout|time.sleep(3)
Issue-Key: cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot|_activate_guard_hook#17|Timeout|time.sleep(3)
Issue-Commit: 8fdd90c15790d38aa3c5d0088f99b03c79823121

# cloudformation: custom-resource providers are never invoked, and the activate-a-Hook-then-configure-it flow still cannot complete

> _Split out of `20261025T180059Z-cloudformation-4-net-new-behavioral-root-causes-custom.md`,
> whose other two causes (the change-set execute lifecycle and the WaitCondition-barriered
> update) are fixed. These two are untouched by that fix and keep the seven checks above._

## Operation
CreateStack, ActivateType

## Description

=== Cause 1 of 2: a template's custom resource is never invoked — no provider request is ever delivered to its ServiceToken ===
AWS: a resource of type `Custom::<Name>` / `AWS::CloudFormation::CustomResource` names a provider in its `ServiceToken` property (an SNS topic ARN or a Lambda ARN). On every stack operation touching the resource, CloudFormation PUBLISHES a provider request to that token and holds the stack in `CREATE_IN_PROGRESS` / `UPDATE_IN_PROGRESS` until the provider PUTs a response to the request's pre-signed `ResponseURL`. The request carries `RequestType` (`Create`|`Update`|`Delete`), `RequestId`, `StackId`, `LogicalResourceId`, `ResponseURL`, `ResourceProperties`, plus `PhysicalResourceId` and `OldResourceProperties` on an Update/Delete. A provider that answers an Update with a NEW `PhysicalResourceId` makes the resource a replacement: the stack goes to `UPDATE_COMPLETE_CLEANUP_IN_PROGRESS` and a cleanup `Delete` request for the OLD id with the OLD properties is sent; the same id means an in-place update and NO Delete. `Fn::GetAtt` on the resource resolves from the response's `Data`. The custom-resource protocol is not in the Smithy model (recorded behaviour only).
Emulator: the provider mailbox is never written. Each of these checks subscribes an SQS queue to the SNS topic it passes as `ServiceToken`, creates the stack, and then blocks reading the queue for the `Create` request; no request ever arrives, so the check runs out its window on the first `_next_request` / mailbox `get`.
Why: a client cannot use custom resources at all — every CDK/SAM construct backed by one hangs instead of completing.
Fix: the CloudFormation provisioning path needs (1) a provisioner arm for `Custom::*` / `AWS::CloudFormation::CustomResource` in `services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs` that publishes the provider request to the `ServiceToken`, (2) a `ResponseURL` the provider can really PUT to, served in process (a pre-signed S3 PUT the emulator's own endpoint answers), with the stack held at a `state::stabilising` barrier until the response lands — `state::provisioning::Readiness` has to carry the provider's `PhysicalResourceId` and `Data` back for that — and `Fn::GetAtt` resolving from that `Data`, and (3) the replacement rule on a changed `PhysicalResourceId` (`UPDATE_COMPLETE_CLEANUP_IN_PROGRESS` plus the cleanup `Delete` of the old id with the old properties).
Traces: cloudformation.continue-update-rollback-on-stable-stack-fails, cloudformation.stack-continue-update-rollback-lifecycle, cloudformation.stack-update-cancel-then-continue-rollback, cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old, cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data

=== Cause 2 of 2: the activate-a-Guard-Hook-then-configure-it flow still cannot complete ===
AWS: `ActivateType` records the activation of a public extension and answers its ARN; `SetTypeConfiguration` persists the account-level configuration of that extension and answers a `ConfigurationArn`; `BatchDescribeTypeConfigurations` reads that `Configuration`, `Alias` and `Arn` back, and a concurrent write is observed as a whole pre- or post-image.
Emulator: those three operations ARE implemented now (`src/ops/activate_type.rs`, `src/ops/set_type_configuration.rs`, `src/ops/batch_describe_type_configurations.rs`, against `state::registry`'s `StoredTypeConfiguration`), and the rule is covered by `tests/all/type_configurations.rs` — but both checks still spend their whole window inside their own `_activate_guard_hook` retry loop (`time.sleep(3)`), so the loop makes at least one further call the emulator still answers HTTP 501 for. The registry operations that are still scaffolded stubs, and that such a loop plausibly makes, are `RegisterType`, `DescribeTypeRegistration` (the classic "register the hook, then poll the registration token until `ProgressStatus=COMPLETE`, sleeping 3s" pair), `SetTypeDefaultVersion`, `ListTypeVersions` and `ListTypeRegistrations`.
Why: a client still cannot turn a Hook on end to end.
Fix: implement the registration half of the registry — `RegisterType` (answering a `RegistrationToken` and adding the version, with the execution role and logging configuration the request names) and `DescribeTypeRegistration` (answering `IN_PROGRESS` and then `COMPLETE` with the `TypeArn`/`TypeVersionArn` on the registry's own clock, as `registry::PendingTypeTests` already does for contract tests), then `SetTypeDefaultVersion`. Each of those files carries its own stub header.
Traces: cloudformation.type-configuration-set-and-batch-describe, cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot

## Steps to Reproduce
Boot the emulator and replay the seven checks above from suite simparity-behavioral-ga-prod-v1.

Cause 1 — by hand: create an SNS topic, subscribe an SQS queue to it, `CreateStack` with `{"Resources":{"Thing":{"Type":"Custom::Probe","Properties":{"ServiceToken":"<topic ARN>","Marker":"v1"}}}}`, then `ReceiveMessage` on the queue — no provider request arrives and the custom resource never settles.

Cause 2 — by hand: run the activation flow a Guard Hook needs end to end (`RegisterType` + `DescribeTypeRegistration` poll, or `ActivateType`), then `SetTypeConfiguration(TypeArn=<the ARN just returned>, Configuration=<Hook config>, ConfigurationAlias="default")` and read it back with `BatchDescribeTypeConfigurations`. The last two calls work; the registration calls answer 501.
