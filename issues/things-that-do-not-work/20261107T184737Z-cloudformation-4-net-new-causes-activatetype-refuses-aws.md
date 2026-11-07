Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot cloudformation.type-configuration-set-and-batch-describe cloudformation.continue-update-rollback-on-stable-stack-fails cloudformation.stack-continue-update-rollback-lifecycle cloudformation.stack-update-cancel-then-continue-rollback cloudformation.type-registration-lifecycle cloudformation.changeset-execute-invalid-status-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot|_activate_guard_hook#17|AssertionError|raise AssertionError(f"ActivateType for AWS::Hooks::GuardHook never succeeded: {last_exc}")
Issue-Key: cloudformation.type-configuration-set-and-batch-describe|_activate_guard_hook#9|AssertionError|raise AssertionError(f"ActivateType for AWS::Hooks::GuardHook never succeeded: {last_exc}")
Issue-Key: cloudformation.continue-update-rollback-on-stable-stack-fails|_next_request#32|AssertionError|raise AssertionError(f"no custom-resource request arrived within {timeout_s}s")
Issue-Key: cloudformation.stack-continue-update-rollback-lifecycle|_next_request#25|AssertionError|raise AssertionError(f"no custom-resource request arrived within {timeout_s}s")
Issue-Key: cloudformation.stack-update-cancel-then-continue-rollback|_next_request#30|AssertionError|raise AssertionError(f"no custom-resource request arrived within {timeout_s}s")
Issue-Key: cloudformation.type-registration-lifecycle|_wait_type_not_found#30|AssertionError|raise AssertionError(f"type {type_name} still resolves via DescribeType after {timeout_s}s")
Issue-Key: cloudformation.changeset-execute-invalid-status-rejected|_wait_change_set_status#12|AssertionError|raise AssertionError(
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# cloudformation: 4 net-new causes — ActivateType refuses AWS's own hook publisher id, SNS-backed custom resources are never invoked, DeregisterType leaves the type resolvable, and a macro change set settles FAILED

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateType, CreateStack (Custom::Resource with an SNS ServiceToken), DeregisterType, CreateChangeSet

## Description
This issue carries 4 net-new root causes.

Cause 1 of 4: ActivateType refuses the publisher id of AWS's own public hook extensions, so no AWS-managed hook can be activated and nothing downstream of an activated type (SetTypeConfiguration, BatchDescribeTypeConfigurations) can be reached.
Failing check ids for this cause: cloudformation.concurrent-batch-describe-type-configurations-consistent-snapshot, cloudformation.type-configuration-set-and-batch-describe.
AWS: Smithy: ActivateType input PublisherId -> PublisherId, @length min 1 max 40, @pattern ^[0-9a-zA-Z]{12,40}$; modeled errors CFNRegistryException (400) and TypeNotFoundException (404) — note ValidationError/ValidationException is not among them. The recording shows ActivateType(Type="HOOK", TypeName="AWS::Hooks::GuardHook", PublisherId="aws-hooks", TypeNameAlias=<alias>, ExecutionRoleArn=<role>) answered 200 with the activated private type's Arn: AWS does not apply the third-party publisher-id pattern to its own first-party public extensions, whose publisher id is the literal "aws-hooks".
Emulator: the call is refused with error code ValidationError and the message "1 validation error detected: Value 'aws-hooks' at 'publisherId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{12,40}$". The fixed text is "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; the value, member name and pattern are filled from the request and the model. The refusal is retried for 60 s and never succeeds.
Why: the whole AWS-managed hook surface is unreachable. A client cannot activate AWS::Hooks::GuardHook (or any AWS-published extension), so hook configuration, hook invocation on stack operations and the extension configuration APIs that depend on an activated type are all inaccessible — and the refusal is an error class ActivateType does not even model.
Fix: ActivateType's input validation — exempt AWS-owned public extensions from the third-party PublisherId pattern (resolve the type by Type+TypeName against the public registry when the publisher is AWS) and keep the pattern check for genuinely third-party publisher ids.

Cause 2 of 4: a Custom::Resource whose ServiceToken is an SNS topic ARN never produces a provider request — the topic's subscriber receives nothing, so no custom-resource lifecycle can be driven.
Failing check ids for this cause: cloudformation.continue-update-rollback-on-stable-stack-fails, cloudformation.stack-continue-update-rollback-lifecycle, cloudformation.stack-update-cancel-then-continue-rollback.
AWS: recorded behavior only — the custom-resource provider protocol is not in the Smithy model. The recording shows that for a resource of a Custom:: type whose ServiceToken is an SNS topic ARN, CloudFormation publishes one message per lifecycle event (Create/Update/Delete) to that topic, whose body is the provider request carrying RequestType, StackId, RequestId, LogicalResourceId, ResourceProperties and ResponseURL; a subscriber (here an SQS queue on that topic) receives it within seconds and the stack waits on the response put to ResponseURL.
Emulator: no message ever arrives — all three checks fail with "no custom-resource request arrived within 90.0s" while the queue is empty. Because the request is never published, the stack never reaches UPDATE_ROLLBACK_FAILED, so ContinueUpdateRollback's success path and its ValidationError refusal on a stable/deleted stack are both unobservable.
Why: SNS-backed custom resources — the documented alternative to Lambda-backed ones — do not work at all. Any stack that models an external provisioning step this way hangs or completes without the step having happened, and the rollback state machine that a failing custom resource is used to reach cannot be driven.
Fix: the CloudFormation custom-resource provisioner — when a Custom:: resource's ServiceToken names an SNS topic, publish the provider request envelope to that topic for each lifecycle event and await the ResponseURL callback, as it already must for a Lambda ServiceToken.

Cause 3 of 4: DeregisterType does not retire the type — DescribeType keeps resolving it afterwards.
Failing check ids for this cause: cloudformation.type-registration-lifecycle.
AWS: Smithy: DeregisterType(Arn | Type+TypeName[+VersionId]) returns an empty output and models CFNRegistryException (400) and TypeNotFoundException (404); DescribeType models TypeNotFoundException (404). The recording shows that after the last version of a private type is deregistered, DescribeType for that type name fails with TypeNotFoundException and keeps failing.
Emulator: after DeregisterType the check polls DescribeType for 60 s and the type still resolves: "type <Namespace>::<Namespace>::Test::MODULE still resolves via DescribeType after 60s" (the namespace and the MODULE suffix come from the registered type name; the random infix is per-run and not part of the requirement). No TypeNotFoundException is ever raised.
Why: deregistration is not durable, so the registry's lifecycle is inconsistent across its own APIs: a client that deregisters a type and then relies on DescribeType to confirm removal (or on the type name becoming free) is told the type is still there, and a deregistered type can still be referenced.
Fix: DeregisterType's state mutation in the extension registry — mark the version/type deregistered so DescribeType, ListTypes and ListTypeVersions stop resolving it and answer TypeNotFoundException.

Cause 4 of 4: a change set whose template invokes a macro settles in Status FAILED instead of staying CREATE_IN_PROGRESS while the macro runs, so the "ExecuteChangeSet on a change set that is not CREATE_COMPLETE is refused" behavior cannot be observed.
Failing check ids for this cause: cloudformation.changeset-execute-invalid-status-rejected.
AWS: recorded behavior only for the timing; the Smithy model gives the surface (CreateChangeSet -> DescribeChangeSet Status, and ExecuteChangeSet's InvalidChangeSetStatusException). The recording shows that a change set over a template carrying a Transform whose macro Lambda is slow stays in Status CREATE_IN_PROGRESS for the duration of the macro invocation and then moves to CREATE_COMPLETE; while it is CREATE_IN_PROGRESS, ExecuteChangeSet is refused with InvalidChangeSetStatus.
Emulator: the change set reaches Status FAILED instead — the check waits 60 s for CREATE_IN_PROGRESS and reports "change set <name> did not reach CREATE_IN_PROGRESS within 60.0s; last='FAILED'". Because the change set is already terminal-failed, the refusal window never exists. Hypothesis: the macro transform is not performed (or fails immediately), and the change set is marked FAILED rather than being created and held while the macro is invoked.
Why: templates that use macros cannot be previewed through a change set at all, and a client cannot observe or depend on the documented in-progress refusal of ExecuteChangeSet — a retry loop keyed on InvalidChangeSetStatus instead sees a permanently FAILED change set.
Fix: the CreateChangeSet transform path — invoke the macro asynchronously, keep the change set in CREATE_IN_PROGRESS until the macro returns, and only mark it FAILED when the macro itself fails (with the macro's reason).

Traces: the seven check ids in trace_ids, grouped per cause above.

## Evidence
Cause 1 — Expected: ActivateType(Type=HOOK, TypeName=AWS::Hooks::GuardHook, PublisherId=aws-hooks, TypeNameAlias=..., ExecutionRoleArn=...) -> 200 with Arn. Actual: ClientError (ValidationError) "1 validation error detected: Value 'aws-hooks' at 'publisherId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{12,40}$", retried for 60 s, then AssertionError: ActivateType for AWS::Hooks::GuardHook never succeeded.

Cause 2 — Expected: one provider request per lifecycle event published to the Custom:: resource's SNS ServiceToken topic and visible on its SQS subscriber within seconds. Actual: AssertionError: no custom-resource request arrived within 90.0s (all three checks; queue empty).

Cause 3 — Expected: after DeregisterType, DescribeType answers TypeNotFoundException. Actual: AssertionError: type <ns>::<ns>::Test::MODULE still resolves via DescribeType after 60s.

Cause 4 — Expected: DescribeChangeSet Status == CREATE_IN_PROGRESS while the macro Lambda runs, and ExecuteChangeSet refused with InvalidChangeSetStatus in that window. Actual: AssertionError: change set <name> did not reach CREATE_IN_PROGRESS within 60.0s; last='FAILED'.

## Steps to Reproduce
Cause 1: create an IAM role trusted by hooks.cloudformation.amazonaws.com, then call ActivateType with Type=HOOK, TypeName=AWS::Hooks::GuardHook, PublisherId=aws-hooks, a TypeNameAlias and that ExecutionRoleArn.
Cause 2: create an SNS topic with an SQS subscription, then CreateStack with a resource of type Custom::Resource whose Properties.ServiceToken is the topic ARN, and poll the queue for the Create provider request.
Cause 3: RegisterType a MODULE from an S3 package, wait for the registration to complete, DeregisterType it, then poll DescribeType for the type name.
Cause 4: CreateStack a macro whose Lambda sleeps ~90 s, then CreateChangeSet for a template that declares that macro in Transform and read DescribeChangeSet Status.
Or replay the check ids in trace_ids.
