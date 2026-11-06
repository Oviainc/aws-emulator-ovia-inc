Issue-Source: trace-sweep-divergence
Issue-Trace: scheduler-catalog-list-schedule-groups
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: scheduler-catalog-list-schedule-groups
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler|ListScheduleGroups|2xx->2xx|$.ScheduleGroups[Arn="arn:aws:scheduler:us-west-2:000000000000:schedule-group/default"].CreationDate
Issue-Key: scheduler|ListScheduleGroups|2xx->2xx|$.ScheduleGroups[Arn="arn:aws:scheduler:us-west-2:000000000000:schedule-group/default"].LastModificationDate
Issue-Commit: b42c9a6d9f6e60a02ade0b16252d8ca018526673

# scheduler ListScheduleGroups emits CreationDate and LastModificationDate on the built-in default schedule group

> _Note: this issue was identified against the code base at commit `b42c9a6d9f6e60a02ade0b16252d8ca018526673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListScheduleGroups

## Description
One root cause in the EventBridge Scheduler wire surface, found by replaying simparity-wire-aws-v1 against the emulator.

Cause 1 of 1: ListScheduleGroups projects creation/modification timestamps onto the service-created `default` schedule group, where AWS sends neither member.

AWS: recorded behavior only. The Smithy model (com.amazonaws.scheduler#ScheduleGroupSummary) declares CreationDate and LastModificationDate as OPTIONAL timestamps (neither is @required; only ListScheduleGroupsOutput.ScheduleGroups itself is required), and it says nothing about the account's built-in group. The recording is what pins the behavior: on a fresh account ListScheduleGroups answers HTTP 200 and the summary whose Arn ends in `schedule-group/default` carries NEITHER key — both are absent from that object, not null — while user-created groups do carry both. Hypothesis: the default group is not a created resource on the service side, so it has no create/modify instants to report.

Emulator: HTTP 200 with the right group set, but the `default` entry carries both members, serialized as epoch-seconds numbers (e.g. CreationDate and LastModificationDate on the `arn:aws:...:schedule-group/default` summary). The replay reports them as type_mismatch: expected absent, got a number. Hypothesis: the built-in group is seeded through the same constructor/projection as a user-created group, which stamps both dates.

Why: a client that uses the presence of CreationDate/LastModificationDate to tell the account's built-in default group apart from the groups it owns — a common guard before DeleteScheduleGroup, which cannot delete `default` — classifies the default group as deletable user state and tries to delete or reconcile it. Code that round-trips a listing back into a desired-state comparison also sees a permanent spurious diff for the default group.

Fix: the summary projection in the ListScheduleGroups handler (services/service-com-amazonaws-scheduler-awschronosservice/src/ops/list_schedule_groups.rs), or the seeding of the built-in group in that crate's src/state.rs — the default group must be stored/projected with those two members unset so the serializer omits the keys entirely rather than writing nulls or stamped values.

Relation to already-open issues: the open issue 20261030T010351Z-scheduler-wire-responses-drop-validationexception-messages-modeled-getschedule.md DESCRIBES this same behavior as its "Cause 2 of 8", but the replay reports this cluster with an empty `owned_by`, i.e. that issue does not own this cluster's replay key: its Issue-Key line for this operation is truncated mid-ARN (`scheduler|ListScheduleGroups|2xx->2xx|$.ScheduleGroups[Arn="arn`) and its Issue-Trace-Ids list names fixtures without their step keys, so the ownership join does not bind scheduler-catalog-list-schedule-groups#task#1. This issue exists to carry that binding so the step is actually re-replayed; a fixer closing either one should verify this fixture step.

## Evidence
Replay of simparity-wire-aws-v1: 103 steps, 86 passed, 17 mismatched, score 83.5%, emulator up (/__admin/health 200, data listener accepting). 9 divergence clusters; 8 are reported with a non-empty owned_by (7 scheduler clusters owned by 20261030T010351Z-scheduler-wire-responses-drop-validationexception-messages-modeled-getschedule.md, 1 iam CreateRole cluster owned by 20261029T184359Z-iam-policy-documents-are-returned-as-raw-json.md) and are excluded here. This is the one unowned cluster.

Cause 1 — ListScheduleGroups, expected HTTP 200 / actual HTTP 200, no error code involved:
  $.ScheduleGroups[Arn="arn:aws:scheduler:<region>:<account>:schedule-group/default"].CreationDate — type_mismatch (expected: member absent, got: epoch-seconds number)
  $.ScheduleGroups[Arn="arn:aws:scheduler:<region>:<account>:schedule-group/default"].LastModificationDate — type_mismatch (expected: member absent, got: epoch-seconds number)
The region, account id and the epoch values are per-environment/per-run values and are NOT the requirement; the requirement is that for the summary whose Name/Arn is the built-in `default` group both keys are omitted, and that for user-created groups they are present. Every other member of every summary (Arn, Name, State) matched.

Cluster affected steps: scheduler-catalog-list-schedule-groups#task#1 (affected_count 1). Divergence key: service=scheduler|ListScheduleGroups|status=200->200|keys=$.ScheduleGroups[...default].CreationDate,$.ScheduleGroups[...default].LastModificationDate. Smithy evidence cited: ListScheduleGroups http GET /schedule-groups, output ScheduleGroups (required) of ScheduleGroupList; ScheduleGroupSummary members Arn, Name, State, CreationDate, LastModificationDate, all optional.

## Steps to Reproduce
1. Boot the emulator from this checkout; do not create any schedule group (the built-in `default` group is all that must exist).
2. Call scheduler.list_schedule_groups() (HTTP GET /schedule-groups).
3. Find the entry whose Arn ends in `schedule-group/default` and inspect its JSON keys.
   Observed: the object contains CreationDate and LastModificationDate as epoch-seconds numbers.
   Expected (as recorded against AWS): the object contains Arn, Name and State only — both date keys are absent.
4. For contrast, create_schedule_group(Name=<any new name>) and list again: the user-created entry SHOULD carry both dates, so the fix must suppress them for the built-in group only, not drop them from the shape.
Replay fixture: scheduler-catalog-list-schedule-groups, step task#1.
