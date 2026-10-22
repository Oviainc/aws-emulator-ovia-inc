Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.create-default-group-forbidden scheduler.default-schedule-group-deletion-rejected scheduler.delete-default-group-forbidden scheduler.create-schedule-group-name-invalid-characters-rejected scheduler.schedule-nonexistent-target-role-rejected scheduler.deploy-cfn-schedule-invokes-sqs-target scheduler.deploy-cfn-schedule-update-changes-target-behavior scheduler.deploy-cloudformation-schedule-invokes-target
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.create-default-group-forbidden|run#3|AssertionError|assert code == "ValidationException", f"expected reserved-name rejection, got {code}"
Issue-Key: scheduler.default-schedule-group-deletion-rejected|run#3|AssertionError|assert code == "ValidationException", f"unexpected code: {code!r}"
Issue-Key: scheduler.delete-default-group-forbidden|run#3|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: scheduler.create-schedule-group-name-invalid-characters-rejected|run#10|AssertionError|assert message == pattern_message, f"expected message {pattern_message!r}, got {message!r}"
Issue-Key: scheduler.schedule-nonexistent-target-role-rejected|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: scheduler|GetSchedule|ResourceNotFoundException
Issue-Key: sqs|GetQueueUrl|AWS.SimpleQueueService.NonExistentQueue
Issue-Key: scheduler.deploy-cloudformation-schedule-invokes-target|run#11|KeyError|queue_url = outputs["QueueUrl"]
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# EventBridge Scheduler diverges in 5 ways: reserved 'default' group rejection class, unimplemented DeleteScheduleGroup, anchored pattern-validation message, unvalidated Target.RoleArn, and CloudFormation that provisions nothing (5 causes)

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScheduleGroup

## Description
Five independent root causes in the EventBridge Scheduler emulator (services/service-com-amazonaws-scheduler-awschronosservice). All five must be closed before this issue is submitted.

=== Cause 1 of 5: CreateScheduleGroup treats the reserved name 'default' as a name collision instead of a validation refusal ===
AWS: Smithy (com-amazonaws-scheduler-awschronosservice): CreateScheduleGroup POST /schedule-groups/{Name}; input Name -> ScheduleGroupName @length 1..64 @pattern ^[0-9a-zA-Z-_.]+$; modeled errors ValidationException (400), ConflictException (409), ServiceQuotaExceededException (402), ThrottlingException (429), InternalServerException (500). Recorded behavior: creating the reserved built-in group named literally 'default' is refused with HTTP 400 / error code ValidationException and the message "This operation cannot be performed on schedule group default." (example text) — the reserved name is a request-validation refusal, evaluated before any existence check, not a collision with the existing resource.
Emulator: returns error code ConflictException for CreateScheduleGroup(Name="default").
Why: a client cannot tell a permanent, never-retriable refusal from an ordinary name collision. SDK/IaC logic that treats ConflictException as "already exists, adopt it" will adopt and then mutate the account's built-in default group; logic that branches on ValidationException for reserved names never runs.
Fix: the CreateScheduleGroup handler's input validation — order the reserved-name check ahead of the conflict/existence check and raise ValidationException (400).
Traces: scheduler.create-default-group-forbidden

=== Cause 2 of 5: DeleteScheduleGroup answers NotImplemented ===
AWS: Smithy: DeleteScheduleGroup DELETE /schedule-groups/{Name}, idempotent, input Name (@pattern ^[0-9a-zA-Z-_.]+$) + ClientToken (httpQuery clientToken), modeled errors ValidationException (400), ResourceNotFoundException (404), ConflictException (409), ThrottlingException (429), InternalServerException (500). Recorded behavior: DeleteScheduleGroup(Name="default") is refused with 400 ValidationException (message example "This operation cannot be performed on schedule group default.") and leaves the default group ACTIVE; an ordinary empty group IS deleted (it is observable in state DELETING and then GetScheduleGroup answers 404 ResourceNotFoundException, message example "Schedule group <name> does not exist."); deleting an already-deleted group answers 404 ResourceNotFoundException.
Emulator: every DeleteScheduleGroup call answers error code NotImplemented ("operation ... is scaffolded but not implemented in this emulator"), so neither the reserved-group refusal nor ordinary deletion exists.
Why: no schedule group can ever be torn down, so every suite that creates groups leaks them, and the protection on the built-in 'default' group cannot be observed at all.
Fix: implement services/service-com-amazonaws-scheduler-awschronosservice/src/ops/delete_schedule_group.rs plus the group state transition (DELETING -> removed) it needs.
Traces: scheduler.default-schedule-group-deletion-rejected, scheduler.delete-default-group-forbidden

=== Cause 3 of 5: pattern-constraint ValidationException message quotes the regex WITH its anchors ===
AWS: Smithy: ScheduleGroupName (and Name on CreateSchedule, GroupName, ClientToken) carry @pattern ^[0-9a-zA-Z-_.]+$. The message rule is known: AWS builds it from the request value, the member path and the modeled constraint, rendering the pattern with the leading ^ and trailing $ anchors stripped — recorded form: "1 validation error detected: Value '<value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z-_.]+".
Emulator: renders the raw modeled pattern including anchors: "... Member must satisfy regular expression pattern: ^[0-9a-zA-Z-_.]+$". Code (ValidationException) and status are correct; only the derived pattern text diverges, and it diverges by a known rule, not by wording.
Why: any client, test or log scraper that reproduces AWS's documented validation-message rule mismatches, and the divergence is systematic across every pattern-constrained member of the service (CreateScheduleGroup.Name, GetScheduleGroup.Name, CreateSchedule.Name/GroupName/ClientToken, ...).
Fix: the shared constraint-validation message renderer used by the scheduler ops' validation layer — strip anchors when interpolating @pattern. Same class as open issues 20261022T031023Z-workspaces-instances-validationexception-message-construction-diverges-from-aws-for-workspaces and 20261022T031238Z-account-validationexception-message-ignores-the-smithy-pattern-constraint, neither of which covers scheduler.
Traces: scheduler.create-schedule-group-name-invalid-characters-rejected

=== Cause 4 of 5: CreateSchedule accepts a Target.RoleArn naming a role that does not exist ===
AWS: Smithy: CreateSchedule POST /schedules/{Name}, Target required, modeled errors ValidationException (400), ResourceNotFoundException (404), ConflictException (409). Recorded behavior: a Target.RoleArn whose IAM role was never created is refused at create time with HTTP 400 / ValidationException (message example "The execution role you provide must allow AWS EventBridge Scheduler to assume the role."), because Scheduler verifies at create time that it can assume the supplied execution role; the schedule is NOT persisted (a following GetSchedule answers ResourceNotFoundException). An otherwise identical request naming a role that exists and trusts scheduler.amazonaws.com succeeds.
Emulator: answers HTTP 200 and mints a ScheduleArn (e.g. arn:aws:scheduler:<region>:<account>:schedule/default/<name>) for a RoleArn pointing at a nonexistent role.
Why: over-permissive — a misconfigured execution role is accepted, so a schedule that real AWS would never create is created, and the client only discovers the breakage at fire time (or never, in the emulator).
Fix: the CreateSchedule handler's validation — resolve Target.RoleArn against IAM state and check the trust policy admits scheduler.amazonaws.com before persisting; refuse with ValidationException and persist nothing. UpdateSchedule needs the same gate.
Traces: scheduler.schedule-nonexistent-target-role-rejected

=== Cause 5 of 5: CloudFormation stacks containing AWS::Scheduler::* reach CREATE_COMPLETE but provision nothing and expose no Outputs ===
AWS: recorded behavior only (no Smithy statement about CFN resource types). A stack whose template declares AWS::Scheduler::ScheduleGroup / AWS::Scheduler::Schedule (and the AWS::SQS::Queue target it references) and which reports CREATE_COMPLETE has really created those resources, and its declared Outputs are resolvable: GetSchedule returns the schedule in the named group, and GetQueueUrl resolves the queue.
Emulator: the stack reports CREATE_COMPLETE, but nothing was created and no Outputs were emitted — GetSchedule answers 404 ResourceNotFoundException (messages observed: "Schedule <name> does not exist in schedule group default." and "Schedule <name> does not exist in schedule group <group>."), the stack's Outputs map has no QueueUrl key at all (KeyError on lookup), and GetQueueUrl answers AWS.SimpleQueueService.NonExistentQueue for the template's queue. There is no CREATE_FAILED and no rollback, so the deploy looks successful.
Why: silent success is the worst outcome — every deploy-then-exercise workflow (deploy a schedule by IaC, then assert it fires) passes its deploy step and then fails mysteriously downstream, and a client can never distinguish "provisioned" from "ignored".
Fix: the CloudFormation resource-provisioning layer — register handlers for the AWS::Scheduler::ScheduleGroup / AWS::Scheduler::Schedule resource types (and resolve Outputs / Fn::GetAtt / Ref for them), or fail the stack with CREATE_FAILED for an unknown type instead of reporting CREATE_COMPLETE. Open issue 20261022T031509Z-xray-cloudformation-never-provisions-aws-xray-group-aws-xray reports the identical shape for AWS::XRay::*; it does not cover the Scheduler types, and the "report CREATE_COMPLETE for unprovisioned types" behaviour is the shared root.
Traces: scheduler.deploy-cfn-schedule-invokes-sqs-target, scheduler.deploy-cfn-schedule-update-changes-target-behavior, scheduler.deploy-cloudformation-schedule-invokes-target

## Evidence
Cause 1 — scheduler.create-default-group-forbidden [mismatch]:
  AssertionError: expected reserved-name rejection, got ConflictException
  expected: code ValidationException, message "This operation cannot be performed on schedule group default."
  actual:   code ConflictException

Cause 2 — scheduler.default-schedule-group-deletion-rejected [mismatch]:
  AssertionError: unexpected code: 'NotImplemented'  (expected ValidationException from delete_schedule_group(Name="default"))
Cause 2 — scheduler.delete-default-group-forbidden [mismatch]:
  AssertionError: expected ValidationException, got NotImplemented

Cause 3 — scheduler.create-schedule-group-name-invalid-characters-rejected [mismatch]:
  expected message: "1 validation error detected: Value 'sim2real-sched-grp-bad-104fb4646092@invalid#name' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z-_.]+"
  actual message:   "1 validation error detected: Value 'sim2real-sched-grp-bad-104fb4646092@invalid#name' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z-_.]+$"
  (Smithy ScheduleGroupName @pattern is ^[0-9a-zA-Z-_.]+$; AWS strips the anchors when rendering the message.)

Cause 4 — scheduler.schedule-nonexistent-target-role-rejected [mismatch]:
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned
  {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'ScheduleArn': 'arn:aws:scheduler:us-west-2:000000000000:schedule/default/sim2real-behav-sched-badrole-8a66019c9e31'}
  expected: 400 ValidationException "The execution role you provide must allow AWS EventBridge Scheduler to assume the role."

Cause 5 — scheduler.deploy-cfn-schedule-invokes-sqs-target [error]:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetSchedule operation: Schedule sim2real-sched-faeffce51a59 does not exist in schedule group default.  (stack had reported CREATE_COMPLETE)
Cause 5 — scheduler.deploy-cfn-schedule-update-changes-target-behavior [error]:
  botocore.errorfactory.QueueDoesNotExist: An error occurred (AWS.SimpleQueueService.NonExistentQueue) when calling the GetQueueUrl operation: The queue named sim2real-sched-qa-e8695b370ee5 does not exist in this account and Region.
Cause 5 — scheduler.deploy-cloudformation-schedule-invokes-target [error]:
  KeyError: 'QueueUrl'  at  queue_url = outputs["QueueUrl"]  (stack Outputs empty after CREATE_COMPLETE)

## Steps to Reproduce
Cause 1: scheduler.create_schedule_group(Name="default") -> observe error code (ConflictException; expected ValidationException).
Cause 2: scheduler.delete_schedule_group(Name="default") -> observe NotImplemented; then create an ordinary group and scheduler.delete_schedule_group(Name=<group>) -> also NotImplemented.
Cause 3: scheduler.create_schedule_group(Name="<valid-prefix>@invalid#name") -> read Error.Message and compare the quoted pattern to AWS's unanchored rendering.
Cause 4: scheduler.create_schedule(Name=<n>, ScheduleExpression="rate(1 day)", FlexibleTimeWindow={"Mode":"OFF"}, State="DISABLED", Target={"Arn":"arn:aws:sqs:<region>:<acct>:placeholder","RoleArn":"arn:aws:iam::<acct>:role/<never-created>"}) -> returns 200 + ScheduleArn; expected 400 ValidationException and no persisted schedule.
Cause 5: cloudformation.create_stack with a template declaring AWS::SQS::Queue + AWS::Scheduler::ScheduleGroup + AWS::Scheduler::Schedule and an Outputs block; wait for CREATE_COMPLETE; then describe_stacks -> Outputs is empty, scheduler.get_schedule(Name=<template name>) -> ResourceNotFoundException, sqs.get_queue_url(QueueName=<template queue>) -> NonExistentQueue.
