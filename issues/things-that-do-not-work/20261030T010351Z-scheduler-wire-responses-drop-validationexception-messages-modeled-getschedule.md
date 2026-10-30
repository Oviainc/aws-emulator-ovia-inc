Issue-Source: trace-sweep-divergence
Issue-Trace: scheduler-create-get-delete-schedule-default-group
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: scheduler-create-get-delete-schedule-default-group scheduler-create-get-delete-schedule-in-group scheduler-create-schedule-dead-letter-config-round-trip scheduler-create-schedule-start-and-end-date-round-trip scheduler-flexible-time-window-max-minutes-out-of-range-rejected scheduler-schedule-create-get-delete-lifecycle scheduler-schedule-description-unicode-round-trip scheduler-schedule-start-end-date-timestamp-precision-preserved scheduler-catalog-list-schedule-groups scheduler-schedule-mutation-error-surface scheduler-schedule-tagging-roundtrip
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler|GetSchedule|2xx->2xx|$.ActionAfterCompletion
Issue-Key: scheduler|GetSchedule|2xx->2xx|$.ScheduleExpressionTimezone
Issue-Key: scheduler|GetSchedule|2xx->2xx|$.Target.RetryPolicy
Issue-Key: scheduler|ListScheduleGroups|2xx->2xx|$.ScheduleGroups[Arn="arn
Issue-Key: scheduler|CreateSchedule|4xx->4xx|$.Message
Issue-Key: scheduler|CreateSchedule|4xx->2xx|$.Message
Issue-Key: scheduler|CreateSchedule|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: scheduler|CreateSchedule|4xx->2xx|status
Issue-Key: scheduler|DeleteSchedule|4xx->4xx|$.Message
Issue-Key: scheduler|ListSchedules|4xx->4xx|$.Message
Issue-Key: scheduler|TagResource|4xx->4xx|$.Message
Issue-Key: scheduler|ListTagsForResource|4xx->4xx|$.Message
Issue-Commit: a00992dc094f0a072e21184aa939fd32e0af172d

# scheduler wire responses drop ValidationException messages, modeled GetSchedule defaults and the correct not-found resource

> _Note: this issue was identified against the code base at commit `a00992dc094f0a072e21184aa939fd32e0af172d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSchedule

## Description
Eight distinct root causes in the EventBridge Scheduler wire surface, all found by replaying simparity-wire-aws-v1 against the emulator. Each must be closed before this issue is submitted. None of them is owned by an existing issue: the open behavioral issue 20261023T191008Z-scheduler-control-plane-ten-divergences-in-createschedule-updateschedule.md covers the behavioral corpus (CreateSchedule/UpdateSchedule expression and timestamp validation, the schedule-not-found text, the enum-set ordering) and carries none of the wire fixture steps below; Causes 2-8 here are the wire-level faults it does not reach.

Cause 1 of 8: GetSchedule does not materialize the server-side defaults AWS fills in, so three modeled members are absent from the response.
AWS: GetSchedule always answers HTTP 200 with the schedule's effective configuration, including the values Scheduler supplies when the create request omitted them: ActionAfterCompletion defaults to NONE (Smithy: ActionAfterCompletion enum, values NONE | DELETE), ScheduleExpressionTimezone defaults to UTC (Smithy: ScheduleExpressionTimezone, @length 1..50), and Target.RetryPolicy is defaulted in full to MaximumEventAgeInSeconds 86400 and MaximumRetryAttempts 185 — the maxima of the modeled ranges (Smithy: RetryPolicy.MaximumEventAgeInSeconds @range 60..86400, RetryPolicy.MaximumRetryAttempts @range 0..185). The recorded responses carry all three on every GetSchedule, across eight independent fixtures.
Emulator: the GetSchedule body omits $.ActionAfterCompletion, $.ScheduleExpressionTimezone and $.Target.RetryPolicy entirely (JSON key absent, not null); every other member matches.
Why: a client reading back a schedule it just created sees no retry policy, no timezone and no completion action, so reconcilers compute a spurious diff against their desired state and re-write the schedule forever, and code that reads schedule.Target.RetryPolicy.MaximumRetryAttempts gets a null dereference where AWS always has a value.
Fix: the response assembly in the GetSchedule handler, services/service-com-amazonaws-scheduler-awschronosservice/src/ops/get_schedule.rs, or the defaulting of the stored schedule record on write in src/ops/create_schedule.rs / src/state.rs — the defaults must be persisted or projected, not left empty.
Traces: scheduler-create-get-delete-schedule-default-group#task#4 (representative; 8 fixtures in the cluster, incl. scheduler-create-get-delete-schedule-in-group#task#5, scheduler-create-schedule-dead-letter-config-round-trip#task#4, scheduler-schedule-create-get-delete-lifecycle#task#4).

Cause 2 of 8: ListScheduleGroups emits creation/modification timestamps for the service-created `default` group.
AWS: recorded behavior only — the Smithy model marks ScheduleGroupSummary.CreationDate and .LastModificationDate optional and says nothing about the default group. The recording shows the entry whose Arn ends in `schedule-group/default` carrying NEITHER member: both keys are absent from that summary, while user-created groups do carry them.
Emulator: emits CreationDate and LastModificationDate on the `default` entry (as epoch-seconds numbers), i.e. members where AWS sends none. Hypothesis: the default group is being manufactured through the same constructor as a user-created group, which stamps both dates.
Why: a client that uses the presence of CreationDate to distinguish the account's built-in default group from groups it owns (a common guard before DeleteScheduleGroup, which cannot delete `default`) treats the default group as deletable user state.
Fix: the summary projection in the ListScheduleGroups handler, src/ops/list_schedule_groups.rs, or the seeding of the built-in group in src/state.rs: the default group must be stored/projected without those timestamps.
Traces: scheduler-catalog-list-schedule-groups#task#1.

Cause 3 of 8: a constraint ValidationException from CreateSchedule has an empty body — the required Message member is missing.
AWS: an out-of-range FlexibleTimeWindow.MaximumWindowInMinutes (Smithy: @range 1..1440) is refused with HTTP 400 ValidationException whose Message is REQUIRED (Smithy: ValidationException.Message, required, httpError 400) and is rendered in the standard constraint form: fixed text `1 validation error detected: Value '` + the submitted value + `' at '` + the member's wire path + `' failed to satisfy constraint: Member must have value less than or equal to ` + the modeled maximum. For this member the fixed parts are the member path `flexibleTimeWindow.maximumWindowInMinutes` and the bound `1440`; the quoted value is whatever the request sent.
Emulator: answers the right status (400) but the body has no $.Message at all.
Why: every AWS SDK surfaces ValidationException with an empty message, so the caller is told only 'invalid request' and cannot see which member or which bound it violated — the whole diagnostic payload of the error is gone.
Fix: the ValidationException construction for range violations in the scheduler constraint validator (the shared boundary validation that builds `1 validation error detected: ...`, src/ops/types.rs) and/or the error serializer that writes the exception body.
Traces: scheduler-flexible-time-window-max-minutes-out-of-range-rejected#task#4.

Cause 4 of 8: the same empty-body ValidationException for ListSchedules MaxResults.
AWS: MaxResults above its modeled bound (Smithy: ListSchedules.MaxResults @range 1..100, httpQuery MaxResults) is refused with HTTP 400 ValidationException carrying the same constraint text, with member path `maxResults` and bound `100`, and the submitted value quoted from the request.
Emulator: HTTP 400 with no $.Message.
Why: as Cause 3 — a paginating client cannot learn the page-size ceiling from the error.
Fix: same component as Cause 3, reached from src/ops/list_schedules.rs.
Traces: scheduler-schedule-mutation-error-surface#task#7.

Cause 5 of 8: the same empty-body ValidationException for TagResource ResourceArn.
AWS: TagResource/ListTagsForResource accept only a SCHEDULE-GROUP ARN (Smithy: TagResourceArn @pattern ^arn:aws[a-z-]*:scheduler:[a-z0-9\-]+:\d{12}:schedule-group\/[0-9a-zA-Z-_.]+$, @length 1..1011). A schedule ARN is refused with HTTP 400 ValidationException whose Message is the pattern form of the constraint text: fixed `1 validation error detected: Value '` + the submitted ARN + `' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ` + that modeled pattern.
Emulator: HTTP 400 with no $.Message.
Why: as Cause 3; here the message is the only thing that tells the caller a schedule ARN is not taggable and a group ARN is wanted.
Fix: same component as Cause 3, for @pattern violations, reached from src/ops/tag_resource.rs.
Traces: scheduler-schedule-tagging-roundtrip#task#7.

Cause 6 of 8: CreateSchedule accepts a malformed Target.Arn instead of refusing it.
AWS: the target ARN is validated at the control plane and a non-ARN string is refused with HTTP 400 ValidationException, with the ARN-specific message whose fixed text is `Parameter ` + the offending value from the request + ` is not valid. Reason: Provided Arn is not in correct format.`, and with the modeled error type on the wire in the x-amzn-errortype header (Smithy: CreateSchedule errors include ValidationException, httpError 400).
Emulator: answers HTTP 200 and creates the schedule — no $.Message, no x-amzn-errortype header. The same step pair shows it for both mutations in the fixture.
Why: the caller is handed a ScheduleArn for a schedule that can never deliver, and the typo surfaces only as silent non-delivery at runtime instead of at deploy time; worse, a test asserting the refusal passes against AWS and fails against the emulator in the opposite direction (success where a 400 is required).
Fix: target-ARN validation in the CreateSchedule handler, src/ops/create_schedule.rs (its handle() goes from the flexible-window/role checks straight to the store insert), with the shared ARN check in src/state.rs so UpdateSchedule gets it too.
Traces: scheduler-schedule-mutation-error-surface#task#2 (cluster also covers #task#4).

Cause 7 of 8: DeleteSchedule against a NON-EXISTENT schedule group reports the missing schedule instead of the missing group.
AWS: when the request's GroupName does not exist, the ResourceNotFoundException (HTTP 404) names the GROUP: fixed text `Schedule group ` + the GroupName from the request + ` does not exist.` The group is resolved before the schedule, so the schedule name never appears.
Emulator: returns 404 ResourceNotFoundException with `Schedule ` + the schedule name + ` does not exist.` — correct status and error code, wrong resource named.
Why: the client cannot distinguish a deleted group from a deleted schedule, so reconciliation recreates the wrong resource — it re-creates a schedule into a group that is gone, which then fails for a third reason. (The behavioral issue cited above describes this for UpdateSchedule only; DeleteSchedule is unowned.)
Fix: order the lookups in src/ops/delete_schedule.rs so the group-existence branch runs first and uses the group-not-found message helper (`crate::state::group_does_not_exist`, src/state.rs).
Traces: scheduler-schedule-mutation-error-surface#task#6.

Cause 8 of 8: ListTagsForResource invents its own not-found text instead of the service's.
AWS: for a well-formed schedule-group ARN naming a group that does not exist, ListTagsForResource answers HTTP 404 ResourceNotFoundException with the group form of the message: fixed text `Schedule group ` + the group name parsed out of the request's ResourceArn + ` does not exist.`
Emulator: answers 404 ResourceNotFoundException with a generic `Resource not found: ` + the full ARN — a message shape this service never produces.
Why: callers and tooling that match the documented not-found text to tell 'group absent' from other 404s read the emulator's message as a different condition, and the message is not even in the service's own vocabulary.
Fix: the not-found branch of src/ops/list_tags_for_resource.rs must use the shared group-not-found message helper in src/state.rs rather than a local generic string.
Traces: scheduler-schedule-tagging-roundtrip#task#8.

## Evidence
Replay of simparity-wire-aws-v1: 103 steps, 86 passed, 17 mismatched, score 83.5%, emulator up (/__admin/health 200). Nine divergence clusters; one (iam CreateRole policy-document encoding) is owned by 20261029T184359Z-iam-policy-documents-are-returned-as-raw-json.md and is excluded. The eight scheduler clusters:
Cause 1 — GetSchedule, expected 200 / actual 200: $.ActionAfterCompletion missing (expected 'NONE'); $.ScheduleExpressionTimezone missing (expected 'UTC'); $.Target.RetryPolicy missing (expected {'MaximumEventAgeInSeconds': 86400, 'MaximumRetryAttempts': 185}). 8 affected fixtures.
Cause 2 — ListScheduleGroups, 200/200: $.ScheduleGroups[Arn="arn:aws:scheduler:<region>:<account>:schedule-group/default"].CreationDate and .LastModificationDate type_mismatch (expected absent, got an epoch-seconds number).
Cause 3 — CreateSchedule, 400/400: $.Message missing; expected "1 validation error detected: Value '<submitted>' at 'flexibleTimeWindow.maximumWindowInMinutes' failed to satisfy constraint: Member must have value less than or equal to 1440".
Cause 4 — ListSchedules, 400/400: $.Message missing; expected "1 validation error detected: Value '<submitted>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Cause 5 — TagResource, 400/400: $.Message missing; expected "1 validation error detected: Value '<submitted schedule ARN>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z-]*:scheduler:[a-z0-9\-]+:\d{12}:schedule-group\/[0-9a-zA-Z-_.]+".
Cause 6 — CreateSchedule, expected 400 / actual 200: $.Message missing (expected 'Parameter <value> is not valid. Reason: Provided Arn is not in correct format.'), $headers.x-amzn-errortype missing.
Cause 7 — DeleteSchedule, 404/404: $.Message value_mismatch — expected 'Schedule group <GroupName> does not exist.', got 'Schedule <Name> does not exist.'
Cause 8 — ListTagsForResource, 404/404: $.Message value_mismatch — expected 'Schedule group <GroupName> does not exist.', got 'Resource not found: <full ARN>'.
In every message above the bracketed parts are taken from the request or state; the surrounding text is the fixed template.

## Steps to Reproduce
Common setup (as the fixtures do): iam.create_role trusting scheduler.amazonaws.com with an sqs:SendMessage inline policy, and sqs.create_queue for the target ARN.
1 (Cause 1): create_schedule(Name=n, ScheduleExpression='rate(10 minutes)', FlexibleTimeWindow={'Mode':'OFF'}, Target={'Arn': queue_arn, 'RoleArn': role_arn}) with no ActionAfterCompletion, no ScheduleExpressionTimezone and no Target.RetryPolicy, then get_schedule(Name=n) -> all three members absent instead of 'NONE' / 'UTC' / {MaximumEventAgeInSeconds: 86400, MaximumRetryAttempts: 185}. Replay fixture scheduler-create-get-delete-schedule-default-group, step task#4.
2 (Cause 2): list_schedule_groups() on a fresh emulator -> the 'default' entry carries CreationDate/LastModificationDate. Fixture scheduler-catalog-list-schedule-groups, step task#1.
3 (Cause 3): create_schedule(..., FlexibleTimeWindow={'Mode':'FLEXIBLE','MaximumWindowInMinutes':1441}) -> 400 with an empty body. Fixture scheduler-flexible-time-window-max-minutes-out-of-range-rejected, step task#4.
4 (Cause 4): list_schedules(MaxResults=1000) -> 400 with no Message. Fixture scheduler-schedule-mutation-error-surface, step task#7.
5 (Cause 5): tag_resource(ResourceArn=<a schedule ARN, not a schedule-group ARN>, Tags=[...]) -> 400 with no Message. Fixture scheduler-schedule-tagging-roundtrip, step task#7.
6 (Cause 6): create_schedule(..., Target={'Arn':'not-an-arn','RoleArn':role_arn}) -> 200 + ScheduleArn instead of 400 ValidationException. Fixture scheduler-schedule-mutation-error-surface, step task#2.
7 (Cause 7): delete_schedule(Name=<any>, GroupName=<a group that was never created>) -> message names the schedule, not the group. Fixture scheduler-schedule-mutation-error-surface, step task#6.
8 (Cause 8): list_tags_for_resource(ResourceArn='arn:aws:scheduler:<region>:<account>:schedule-group/<missing group>') -> 'Resource not found: <arn>' instead of 'Schedule group <name> does not exist.' Fixture scheduler-schedule-tagging-roundtrip, step task#8.
