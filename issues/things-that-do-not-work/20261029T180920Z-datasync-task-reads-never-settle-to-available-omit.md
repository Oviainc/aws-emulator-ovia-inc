Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-task-and-filter-validation-errors
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-task-and-filter-validation-errors datasync-task-options-defaults-and-echo datasync-task-lifecycle-and-tags
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|DescribeTask|2xx->2xx|$.DestinationNetworkInterfaceArns
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Excludes
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Includes
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.Gid
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.PosixPermissions
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.PreserveNfsAcls
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.SecurityDescriptorCopyFlags
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.Uid
Issue-Key: datasync|DescribeTask|2xx->2xx|$.SourceNetworkInterfaceArns
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Status
Issue-Key: datasync|ListTasks|2xx->2xx|$.Tasks[*].Status
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Name
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.Atime
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.BytesPerSecond
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.Mtime
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.ObjectTags
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.OverwriteMode
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.TaskQueueing
Issue-Key: datasync|DescribeTask|2xx->2xx|$.Options.VerifyMode
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# DataSync task reads never settle to AVAILABLE, omit always-present lists and drop UpdateTask edits (4 causes)

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTask

## Description
Four root causes in the successful (HTTP 200) task-read surface: the task's lifecycle state never advances, two always-present list members are omitted, the Options the caller supplied are replaced by modeled defaults with one member never serialized at all, and UpdateTask's edits are not persisted. Status is 200 on both sides in all four — the divergence is in the response body. Smithy model consulted: services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json — DescribeTask output (Status: TaskStatus, Name, SourceNetworkInterfaceArns, DestinationNetworkInterfaceArns, Options, Excludes/Includes: FilterList @length(max 1)), TaskListEntry {TaskArn, Status, Name, TaskMode}, enum TaskStatus [AVAILABLE, CREATING, QUEUED, RUNNING, UNAVAILABLE], and shape Options.

=== Cause 1 of 4: a created task stays Status=CREATING forever ===
AWS: TaskStatus is the enum [AVAILABLE, CREATING, QUEUED, RUNNING, UNAVAILABLE]. CREATING is a transient state: after CreateTask, DataSync validates the task and the task becomes AVAILABLE, which is the state every subsequent read reports (and the state a task must be in to be started). Both DescribeTask.Status and the ListTasks summary TaskListEntry.Status report AVAILABLE for an idle, validated task.
Emulator: every read of a freshly created task reports Status='CREATING' and it never changes — DescribeTask ("$.Status: value_mismatch (expected 'AVAILABLE', got 'CREATING')") and ListTasks ("$.Tasks[0].Status: value_mismatch (expected 'AVAILABLE', got 'CREATING')") alike. The state is assigned at creation and never advanced.
Why: AVAILABLE is the readiness signal for the whole service. Any client that polls DescribeTask/ListTasks until the task is AVAILABLE before starting it — the documented pattern, and what CloudFormation-style provisioners and SDK waiters do — hangs forever against the emulator, and clients that gate on Status reject the task as not ready.
Fix: the task state machine in the service's state layer — a task must leave CREATING and settle on AVAILABLE once created, and both the DescribeTask serializer and the ListTasks TaskListEntry projection must read that settled value.
Traces: datasync-task-and-filter-validation-errors#task#15, datasync-task-lifecycle-and-tags#task#7 (ListTasks), datasync-task-lifecycle-and-tags#task#9, datasync-task-options-defaults-and-echo#task#9.

=== Cause 2 of 4: always-present list members are omitted instead of serialized empty ===
AWS: DescribeTask output declares SourceNetworkInterfaceArns and DestinationNetworkInterfaceArns (lists), plus Excludes and Includes (FilterList). DataSync always serializes these members: for an agentless S3-to-S3 task that has no network interfaces and no filter rules they are present as EMPTY arrays, and when filters were supplied they hold them. Presence does not depend on the value being non-empty.
Emulator: the members are absent from the body entirely — "$.SourceNetworkInterfaceArns: missing (expected [], got None)", "$.DestinationNetworkInterfaceArns: missing (expected [], got None)", "$.Excludes: missing (expected [], got None)", "$.Includes: missing (expected [], got None)". The serializer skips empty/unset collections.
Why: SDKs surface an omitted list as null/None rather than an empty list, so client code that iterates response.Excludes or len(response.SourceNetworkInterfaceArns) — safe against AWS, where the member is always there — raises against the emulator. It also makes a filter-less task indistinguishable from one whose filters failed to load.
Fix: the DescribeTask serializer — emit these list members unconditionally, as empty arrays when the task holds no entries.
Traces: datasync-task-and-filter-validation-errors#task#15, datasync-task-options-defaults-and-echo#task#9, datasync-task-lifecycle-and-tags#task#9.

=== Cause 3 of 4: the Options the caller supplied are replaced by modeled defaults, and Options.PreserveNfsAcls is never serialized ===
AWS: Options is a structure whose members (Uid, Gid, PosixPermissions, SecurityDescriptorCopyFlags, Atime, Mtime, ObjectTags, OverwriteMode, VerifyMode, TaskQueueing, BytesPerSecond, ...) each have a documented default (Uid/Gid INT_VALUE, PosixPermissions PRESERVE, SecurityDescriptorCopyFlags OWNER_DACL, Atime BEST_EFFORT, Mtime PRESERVE, ObjectTags PRESERVE, OverwriteMode ALWAYS, TaskQueueing ENABLED, BytesPerSecond -1) — but a value the caller SUPPLIED at CreateTask replaces that default and is what every later DescribeTask echoes. DataSync additionally serializes Options.PreserveNfsAcls (value 'NONE' for an S3-to-S3 task); recorded behavior only — the on-disk Smithy Options structure does not declare PreserveNfsAcls (query for the shape returns "not found"), so the recording is the only source for that member.
Emulator: the response carries the modeled DEFAULT for members the request set explicitly — Gid 'INT_VALUE' and Uid 'INT_VALUE' where NONE was supplied, PosixPermissions 'PRESERVE' where NONE was supplied, SecurityDescriptorCopyFlags 'OWNER_DACL' where NONE was supplied — and Options.PreserveNfsAcls is missing from the body in every task read ("expected 'NONE', got None"), including the fixture whose other defaults do match.
Why: a client reads back Options to confirm the transfer configuration it asked for (and UpdateTask round-trips it). Against the emulator a task configured to ignore POSIX ownership and permissions reports that it preserves them, so the configuration a client believes is in effect is the opposite of what it set — and the missing PreserveNfsAcls member means code reading it gets null for a member AWS always populates.
Fix: the CreateTask handler/state must persist the caller-supplied Options members instead of discarding them in favour of defaults, and the DescribeTask serializer must emit the full Options structure DataSync returns, PreserveNfsAcls included.
Traces: datasync-task-and-filter-validation-errors#task#15, datasync-task-options-defaults-and-echo#task#9, datasync-task-lifecycle-and-tags#task#9.

=== Cause 4 of 4: UpdateTask's edits are not persisted — the task reads back with its pre-update values ===
AWS: UpdateTask mutates the stored task; a following DescribeTask reports the NEW Name, the NEW Excludes/Includes filter rules and the NEW Options values. In the recording the task is updated (a renamed task, Includes=[{FilterType: SIMPLE_PATTERN, Value: '/data'}], Excludes=[{FilterType: SIMPLE_PATTERN, Value: '*.tmp|/scratch'}], and Options with Atime/Mtime/Uid/Gid/PosixPermissions/ObjectTags/VerifyMode set to NONE, OverwriteMode NEVER, TaskQueueing DISABLED, BytesPerSecond limited) and DescribeTask echoes exactly that.
Emulator: DescribeTask after the update returns the task's ORIGINAL Name (the update's renamed value is not there), no Includes/Excludes at all, and every Options member back at its modeled default (Atime BEST_EFFORT, Mtime PRESERVE, Uid/Gid INT_VALUE, PosixPermissions PRESERVE, ObjectTags PRESERVE, VerifyMode POINT_IN_TIME_CONSISTENT, OverwriteMode ALWAYS, TaskQueueing ENABLED, BytesPerSecond -1) — i.e. the whole update was accepted and dropped.
Why: UpdateTask silently no-ops. A client that reconfigures a task (changing filters, disabling queueing, applying a bandwidth cap, renaming) gets a success response and no change, so the task keeps transferring under the old configuration and the discrepancy is invisible until a transfer behaves wrongly.
Fix: the UpdateTask handler and the task state it writes — apply Name, Includes, Excludes, Schedule and every supplied Options member to the stored task so subsequent reads reflect them.
Traces: datasync-task-lifecycle-and-tags#task#9.

## Evidence
Replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true). Four verdict=mismatch clusters, all expected_status 200 / actual_status 200 — body divergences on the success path. Named values below (task names, ARNs, the 2097152 byte cap) are the recordings' request/state values filling the templates, not requirements.

1) datasync-task-and-filter-validation-errors#task#15, DescribeTask (cluster also covers datasync-task-options-defaults-and-echo#task#7) — Causes 1, 2, 3:
   $.Status: expected 'AVAILABLE', got 'CREATING'
   $.Excludes / $.Includes: missing (expected [])
   $.SourceNetworkInterfaceArns / $.DestinationNetworkInterfaceArns: missing (expected [])
   $.Options.Gid: expected 'NONE', got 'INT_VALUE'; $.Options.Uid: expected 'NONE', got 'INT_VALUE'
   $.Options.PosixPermissions: expected 'NONE', got 'PRESERVE'
   $.Options.SecurityDescriptorCopyFlags: expected 'NONE', got 'OWNER_DACL'
   $.Options.PreserveNfsAcls: missing (expected 'NONE')

2) datasync-task-lifecycle-and-tags#task#7, ListTasks — Cause 1 in the list projection:
   $.Tasks[0].Status: expected 'AVAILABLE', got 'CREATING'

3) datasync-task-lifecycle-and-tags#task#9, DescribeTask after UpdateTask — Cause 4 (plus 1, 2, 3):
   $.Name: expected the updated name ('...-v2'), got the pre-update name
   $.Includes: missing (expected [{'FilterType': 'SIMPLE_PATTERN', 'Value': '/data'}])
   $.Excludes: missing (expected [{'FilterType': 'SIMPLE_PATTERN', 'Value': '*.tmp|/scratch'}])
   $.Options.Atime: expected 'NONE', got 'BEST_EFFORT'; $.Options.Mtime: expected 'NONE', got 'PRESERVE'
   $.Options.Uid/$.Options.Gid: expected 'NONE', got 'INT_VALUE'; $.Options.PosixPermissions: expected 'NONE', got 'PRESERVE'
   $.Options.ObjectTags: expected 'NONE', got 'PRESERVE'; $.Options.VerifyMode: expected 'NONE', got 'POINT_IN_TIME_CONSISTENT'
   $.Options.OverwriteMode: expected 'NEVER', got 'ALWAYS'; $.Options.TaskQueueing: expected 'DISABLED', got 'ENABLED'
   $.Options.BytesPerSecond: expected the requested cap (2097152), got -1 (unlimited)
   $.Options.SecurityDescriptorCopyFlags: expected 'NONE', got 'OWNER_DACL'; $.Options.PreserveNfsAcls: missing (expected 'NONE')
   $.SourceNetworkInterfaceArns / $.DestinationNetworkInterfaceArns: missing (expected [])
   $.Status: expected 'AVAILABLE', got 'CREATING'

4) datasync-task-options-defaults-and-echo#task#9, DescribeTask (cluster also covers task#12) — Causes 1, 2, 3:
   $.Status: expected 'AVAILABLE', got 'CREATING'
   $.SourceNetworkInterfaceArns / $.DestinationNetworkInterfaceArns: missing (expected [])
   $.Options.PreserveNfsAcls: missing (expected 'NONE')

Model references: TaskStatus enum [AVAILABLE, CREATING, QUEUED, RUNNING, UNAVAILABLE]; DescribeTask output members Status, Name, SourceNetworkInterfaceArns, DestinationNetworkInterfaceArns, Options, Excludes/Includes (FilterList @length max 1); TaskListEntry {TaskArn, Status, Name, TaskMode}; Options members VerifyMode/OverwriteMode/Atime/Mtime/Uid/Gid/PreserveDeletedFiles/PreserveDevices/PosixPermissions/BytesPerSecond(range min -1)/TaskQueueing/LogLevel/TransferMode/SecurityDescriptorCopyFlags/ObjectTags/ObjectMetadata — PreserveNfsAcls is NOT declared in the on-disk model (recorded behavior only).

Related open issue, which covers none of this: 20261023T110503Z-datasync-createtask-createlocations3-cross-member-and-access-validation.md concerns missing create-time validation and the task-EXECUTION stub, not task status settlement, task-read serialization or UpdateTask persistence.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 datasync fixtures.

Cause 1: create two S3 locations and CreateTask, then DescribeTask and ListTasks. Observe Status='CREATING' in both, and that it never becomes 'AVAILABLE' however long you poll. Replay: datasync-task-and-filter-validation-errors#task#15 and datasync-task-lifecycle-and-tags#task#7

Cause 2: DescribeTask on an agentless S3-to-S3 task with no filter rules. Observe Excludes, Includes, SourceNetworkInterfaceArns and DestinationNetworkInterfaceArns absent from the JSON body instead of present as []. Replay: datasync-task-options-defaults-and-echo#task#9

Cause 3: CreateTask with Options explicitly setting Uid=NONE, Gid=NONE, PosixPermissions=NONE, SecurityDescriptorCopyFlags=NONE, then DescribeTask. Observe the defaults INT_VALUE/INT_VALUE/PRESERVE/OWNER_DACL echoed back, and Options.PreserveNfsAcls missing in every task read. Replay: datasync-task-and-filter-validation-errors#task#15

Cause 4: create a task, UpdateTask it with a new Name, Includes=[{SIMPLE_PATTERN,'/data'}], Excludes=[{SIMPLE_PATTERN,'*.tmp|/scratch'}] and Options{Atime:NONE, Mtime:NONE, Uid:NONE, Gid:NONE, PosixPermissions:NONE, ObjectTags:NONE, VerifyMode:NONE, OverwriteMode:NEVER, TaskQueueing:DISABLED, BytesPerSecond:<cap>}, then DescribeTask. Observe the pre-update Name, no filter lists and every Option back at its default. Replay: datasync-task-lifecycle-and-tags#task#9
