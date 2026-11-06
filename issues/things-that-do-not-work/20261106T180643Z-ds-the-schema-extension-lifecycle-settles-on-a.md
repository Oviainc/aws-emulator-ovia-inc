Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.schema-extension-lifecycle-start-cancel
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.schema-extension-lifecycle-start-cancel|_wait_for_extension_status#14|Timeout|time.sleep(interval_s)
Issue-Commit: c1e06d62d4b8595e375ce443a3d201b914ca1cf5

# ds: the schema-extension lifecycle settles on a single 500 ms clock and skips every intermediate SchemaExtensionStatus, so a start/cancel lifecycle is never observable and the cancel window is a race (1 cause)

> _Note: this issue was identified against the code base at commit `c1e06d62d4b8595e375ce443a3d201b914ca1cf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSchemaExtension

## Description
Cause 1 of 1: StartSchemaExtension / CancelSchemaExtension drive a schema extension through its whole lifecycle on one 500 ms internal span and never report the intermediate statuses AWS reports, so the documented start -> in-progress -> CancelInProgress -> Cancelled lifecycle cannot be observed by a client polling on AWS-realistic intervals, and whether a cancel is accepted at all depends on request latency.

AWS: Smithy: SchemaExtensionStatus is an enum with values Initializing, CreatingSnapshot, UpdatingSchema, Replicating, CancelInProgress, RollbackInProgress, Cancelled, Failed, Completed; ListSchemaExtensions reports SchemaExtensionInfo.SchemaExtensionStatus (plus StartDateTime and, once there is an end, EndDateTime). CancelSchemaExtension's own modelled documentation: "Once a schema extension has started replicating to all domain controllers, the task can no longer be canceled. A schema extension can be canceled during any of the following states; Initializing, CreatingSnapshot, and UpdatingSchema", and it declares ClientException / EntityDoesNotExistException (HTTP 400). Recorded behavior only for the timing: applying a schema extension to a Managed Microsoft AD takes minutes, during which the extension is reported in those in-progress statuses, so a cancel issued immediately after the start is accepted and the entry then reads CancelInProgress and settles on Cancelled. The directory the extension is applied to likewise passes Requested -> Creating -> Created -> Active (DirectoryStage enum carries Creating and Created) over the tens of minutes the controllers take.

Emulator: the behavioral check ds.schema-extension-lifecycle-start-cancel timed out after 60.0s. The check creates a Microsoft AD, polls DescribeDirectories for Active at 30 s intervals, calls StartSchemaExtension (CreateSnapshotBeforeSchemaExtension=false), calls CancelSchemaExtension, then polls ListSchemaExtensions at 20 s intervals for status Cancelled. The emulator settles every transition involved on one 500 ms span: state.rs TRANSITION_SPAN = 500 ms, advance_clock moves a directory Requested -> Active (there is no Creating/Created branch at all), an extension Initializing -> Completed, and a cancelled extension CancelInProgress -> Cancelled; CreatingSnapshot, UpdatingSchema, Replicating and RollbackInProgress are never reported by any path. Hypothesis: because each transient status lives only 500 ms, both of the check's poll loops see the transient value on their first poll and then sleep a full AWS-realistic interval (30 s for the directory stage, 20 s for the extension status), so the lifecycle is never observed to completion inside the check's budget. A second consequence of the same 500 ms span, visible in the handlers, is that the extension reaches Completed 500 ms after the start, so CancelSchemaExtension issued any later than that is refused with ClientException "Schema extension <e-id> is in status <status> and can no longer be canceled; a schema extension can be canceled only while it is in Initializing, CreatingSnapshot, UpdatingSchema" (fixed text emulator-authored; the id and status come from the request and from state) although AWS would accept the cancel for minutes.

Why: a client that drives a schema extension the way AWS documents it — start, observe it in progress, cancel it, wait for Cancelled — cannot do so here: it either never sees an in-progress status, or its cancel is refused as too late half a second after the start. Status-driven automation (wait-for-completion loops, cancel-on-timeout logic, dashboards branching on CreatingSnapshot/UpdatingSchema/Replicating) is untestable, and a cancel AWS honours comes back as a 400 ClientException.

Fix: the schema-extension status progression — the record StartSchemaExtension writes, the transient the CancelSchemaExtension handler writes, and the status transitions state.rs advance_clock materialises for SchemaExtensionRecord — must walk the modelled statuses and hold a cancellable status long enough for a caller to cancel. It depends on the directory stage progression, whose instant Requested -> Active jump is already reported for CreateMicrosoftAD as cause 1 of the open issue 20261024T015148Z-ds-directory-service-a-new-microsoft-ad-is-instantly.md; that issue does not carry this check or the schema-extension statuses, so this one is filed separately and must be closed with it.

Traces: ds.schema-extension-lifecycle-start-cancel.

## Evidence
Failing check (behavioral suite, service heading ### ds):
  [mismatch] ds.schema-extension-lifecycle-start-cancel — Error: Check timed out after 60.0s

Expected (AWS): after StartSchemaExtension on an Active Managed Microsoft AD, ListSchemaExtensions reports the entry with SchemaExtensionStatus in Initializing / CreatingSnapshot / UpdatingSchema for the minutes it is being applied; CancelSchemaExtension issued during those statuses is accepted (200, empty body); the entry then reports CancelInProgress and finally Cancelled with an EndDateTime.

Actual (emulator): the lifecycle is driven by state.rs TRANSITION_SPAN = 500 ms — advance_clock takes the directory Requested -> Active (never Creating/Created) and the extension Initializing -> Completed, or CancelInProgress -> Cancelled, 500 ms after the call that owed it; CreatingSnapshot, UpdatingSchema, Replicating and RollbackInProgress are never reported. The check never observes the target status within its 60 s budget and is killed. In the same handlers, a CancelSchemaExtension arriving more than 500 ms after the start is refused 400 ClientException "Schema extension <e-...> is in status Completed and can no longer be canceled; a schema extension can be canceled only while it is in Initializing, CreatingSnapshot, UpdatingSchema" — emulator-authored wording for a call AWS accepts.

## Steps to Reproduce
1. ds:CreateMicrosoftAD with two subnets in two AZs; poll ds:DescribeDirectories until DirectoryDescriptions[0].Stage == 'Active' (note it jumps from 'Requested' straight to 'Active' ~500 ms after the create, never reporting 'Creating'/'Created').
2. ds:StartSchemaExtension(DirectoryId=<id>, CreateSnapshotBeforeSchemaExtension=false, LdifContent=<a valid attributeSchema add>, Description='...'), keep the SchemaExtensionId.
3. Immediately call ds:ListSchemaExtensions(DirectoryId=<id>) repeatedly and record SchemaExtensionStatus over the next few seconds: it is 'Initializing' only for ~500 ms and then 'Completed'; 'CreatingSnapshot', 'UpdatingSchema' and 'Replicating' never appear.
4. ds:CancelSchemaExtension(DirectoryId=<id>, SchemaExtensionId=<e-id>) more than 500 ms after step 2: it is refused 400 ClientException as no longer cancellable, although AWS accepts a cancel for minutes.
5. Replay the check id in trace_ids: it polls the directory stage at 30 s and the extension status at 20 s, as a client written against AWS would, and times out at 60 s without ever seeing SchemaExtensionStatus 'Cancelled'.
