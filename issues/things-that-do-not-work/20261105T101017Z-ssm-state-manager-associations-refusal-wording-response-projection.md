Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-association-and-opsitem-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-association-and-opsitem-error-paths ssm-association-create-and-list ssm-association-create-list-delete ssm-association-lifecycle-listing ssm-state-manager-association-workflow
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|CreateAssociation|4xx->4xx|$.Message
Issue-Key: ssm|CreateAssociation|2xx->2xx|$.AssociationDescription.DocumentVersion
Issue-Key: ssm|DescribeAssociation|2xx->2xx|$.AssociationDescription.DocumentVersion
Issue-Key: ssm|DescribeAssociation|2xx->2xx|$.AssociationDescription.LastExecutionDate
Issue-Key: ssm|DescribeAssociation|2xx->2xx|$.AssociationDescription.LastSuccessfulExecutionDate
Issue-Key: ssm|DescribeAssociation|2xx->2xx|$.AssociationDescription.Overview.AssociationStatusAggregatedCount
Issue-Key: ssm|DescribeAssociation|2xx->2xx|$.AssociationDescription.Overview.DetailedStatus
Issue-Key: ssm|DescribeAssociation|2xx->2xx|$.AssociationDescription.Overview.Status
Issue-Key: ssm|DescribeAssociation|4xx->2xx|$
Issue-Key: ssm|DescribeAssociation|4xx->2xx|$.Message
Issue-Key: ssm|DescribeAssociation|4xx->2xx|$.__type
Issue-Key: ssm|DescribeAssociation|4xx->2xx|status
Issue-Commit: 272a9e194c556b77cae169106dcdf0edb27f0f40

# ssm State Manager associations: refusal wording, response projection, execution status and deleted-association reads all diverge

> _Note: this issue was identified against the code base at commit `272a9e194c556b77cae169106dcdf0edb27f0f40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssociation

## Description
The emulator's SSM State Manager association surface (CreateAssociation / DescribeAssociation and the association store behind them) diverges from AWS on four independent counts: the not-found refusal message is emulator-authored, the AssociationDescription projection drops modelled members, the association execution overview never progresses, and a deleted association keeps answering 200. This bundle carries 4 causes; all four must be closed.

Cause 1 of 4 — CreateAssociation InvalidDocument message is emulator-authored (fixture ssm-association-and-opsitem-error-paths, step task#3).
AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm) CreateAssociation declares error InvalidDocument (errorKind client, httpError 400, awsQueryError code InvalidDocument), whose shape doc is "The specified SSM document doesn't exist." and whose only member is Message. Recorded behavior: HTTP 400 with Message = "Document does not exist: <Name>". Fixed text is "Document does not exist: "; the single variable part is the document name taken verbatim from the request's Name member. Status and error code match today; only the message text diverges.
Emulator: returns HTTP 400 but with Message = "Document <Name> doesn't exist" — a different sentence shape (name interpolated mid-string, apostrophised negation) rather than AWS's "Document does not exist: <name>" prefix form.
Why: clients and test suites that assert or log the InvalidDocument message (and any human reading it) see wording no AWS region emits; message-matching retry/branch logic and golden-output comparisons break even though the status and code are right.
Fix: the CreateAssociation handler's document-existence validation, where the InvalidDocument Message string is composed.
Traces: ssm-association-and-opsitem-error-paths#task#3.

Cause 2 of 4 — CreateAssociation response omits AssociationDescription.DocumentVersion (fixture ssm-association-create-and-list, step setup#3).
AWS: Smithy AssociationDescription.DocumentVersion targets DocumentVersion with pattern ^([$]LATEST|[$]DEFAULT|^[1-9][0-9]*$)$ and doc "The document version." When a CreateAssociation request names a document without an explicit DocumentVersion, the recording shows AWS resolving and echoing the default version, i.e. DocumentVersion = "$DEFAULT", in the CreateAssociation result. The value is derived from the association's resolved document version, not a literal constant of one run.
Emulator: the member is absent from AssociationDescription in the CreateAssociation 200 response (status and the rest of the body otherwise agree).
Why: a client that reads back which document version an association was pinned to gets nothing, so it cannot tell a $DEFAULT-tracking association from a version-pinned one, and SDK code dereferencing the field after create fails.
Fix: the CreateAssociation serializer / the association record in state — resolve and persist the document version at create time and project it into AssociationDescription.
Traces: ssm-association-create-and-list#setup#3 (cluster also covers ssm-association-create-list-delete#setup#1, ssm-association-lifecycle-listing#setup#3, ssm-state-manager-association-workflow#setup#1, ssm-state-manager-association-workflow#task#12).

Cause 3 of 4 — DescribeAssociation drops DocumentVersion, the execution dates and the Overview detail, and the association never leaves Pending (fixture ssm-association-create-and-list, step setup#4).
AWS: Smithy AssociationDescription declares DocumentVersion, LastExecutionDate ("The date on which the association was last run."), LastSuccessfulExecutionDate ("The last date on which the association was successfully run.") and Overview -> AssociationOverview{Status, DetailedStatus, AssociationStatusAggregatedCount}; AssociationOverview.Status doc states "Status can be: Pending, Success, or Failed." The recording shows a freshly created association converging to Overview.Status = "Success" with Overview.DetailedStatus = "Success", an AssociationStatusAggregatedCount map present, and the two execution timestamps populated once it has run. The timestamps and counts are state-derived values, not fixed literals.
Emulator: DescribeAssociation returns 200 but with DocumentVersion, LastExecutionDate, LastSuccessfulExecutionDate, Overview.DetailedStatus and Overview.AssociationStatusAggregatedCount all missing, and Overview.Status pinned at "Pending" — it stayed Pending across 30 polling attempts, so it never progresses at all.
Why: every State Manager workflow that waits for an association to become Success hangs or times out against the emulator, and compliance/reporting clients that read DetailedStatus, the aggregated counts or the last-run timestamps see an association that appears to have never executed.
Fix: the association state machine in the association store (advance an association past Pending to Success and stamp the execution dates) plus the DescribeAssociation serializer, which must project DocumentVersion and the full AssociationOverview.
Traces: ssm-association-create-and-list#setup#4 (cluster also covers ssm-association-create-list-delete#setup#2, ssm-association-lifecycle-listing#setup#4, ssm-state-manager-association-workflow#task#2).

Cause 4 of 4 — DescribeAssociation on a deleted association answers 200 with the stale record instead of AssociationDoesNotExist (fixture ssm-association-create-list-delete, step teardown#5).
AWS: Smithy DescribeAssociation declares error AssociationDoesNotExist (errorKind client, httpError 400, awsQueryError code AssociationDoesNotExist), shape doc "The specified association doesn't exist.", member Message. Recorded behavior after DeleteAssociation: HTTP 400, __type "AssociationDoesNotExist", Message "Association <AssociationId> does not exist." — fixed text "Association " ... " does not exist." with the association id taken from the request.
Emulator: returns HTTP 200 and the full AssociationDescription of the association that was just deleted (same AssociationId, Name, Targets, AssociationName, Overview.Status Pending), and keeps doing so for 30 polling attempts — the delete left the record readable, so no existence check runs on the read path.
Why: a deleted association remains visible forever, so teardown/idempotency logic that polls until the read refuses never terminates, and any client using DescribeAssociation to confirm deletion concludes the resource still exists.
Fix: DeleteAssociation must remove (or tombstone) the record in the association store, and the DescribeAssociation handler must raise AssociationDoesNotExist for an unknown or deleted AssociationId.
Traces: ssm-association-create-list-delete#teardown#5 (cluster also covers ssm-association-lifecycle-listing#teardown#9, ssm-state-manager-association-workflow#teardown#14).

No open issue covers these: the open ssm association issues are unimplemented-operation stubs (ListAssociations, UpdateAssociation, CreateAssociationBatch, StartAssociationsOnce, DescribeAssociationExecutions, DescribeInstanceAssociationsStatus) and the ssm document/parameter/OpsCenter fidelity issues, none of which touch CreateAssociation/DescribeAssociation projection, status progression or the deleted-association read path.

## Evidence
Cause 1 (ssm-association-and-opsitem-error-paths#task#3, CreateAssociation, 400 -> 400): $.Message expected 'Document does not exist: sim2real-probe-ssm-probe-missing-doc-73edb328', got "Document sim2real-probe-ssm-probe-missing-doc-73edb328 doesn't exist".

Cause 2 (ssm-association-create-and-list#setup#3, CreateAssociation, 200 -> 200): $.AssociationDescription.DocumentVersion missing (expected '$DEFAULT', got None). 5 affected steps.

Cause 3 (ssm-association-create-and-list#setup#4, DescribeAssociation, 200 -> 200): $.AssociationDescription.DocumentVersion missing (expected '$DEFAULT'); $.AssociationDescription.LastExecutionDate missing; $.AssociationDescription.LastSuccessfulExecutionDate missing; $.AssociationDescription.Overview.AssociationStatusAggregatedCount missing (expected {}); $.AssociationDescription.Overview.DetailedStatus missing (expected 'Success'); $.AssociationDescription.Overview.Status expected 'Success', got 'Pending' — poll: not in ['Success'] after 30 attempt(s). 4 affected steps.

Cause 4 (ssm-association-create-list-delete#teardown#5, DescribeAssociation): status expected 400, got 200; $.__type missing (expected 'AssociationDoesNotExist'); $.Message missing (expected 'Association <AssociationId> does not exist.'); poll: error code None not in ['AssociationDoesNotExist'] after 30 attempt(s) — body returned the deleted AssociationDescription (Name AWS-RunShellScript, Overview.Status 'Pending'). 3 affected steps.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay ssm-association-and-opsitem-error-paths step task#3 — CreateAssociation naming a document that was never created; compare the 400 body's Message against 'Document does not exist: <Name>'.
3. Cause 2: replay ssm-association-create-and-list step setup#3 — CreateDocument, then CreateAssociation on it with no DocumentVersion; inspect AssociationDescription.DocumentVersion in the 200 body.
4. Cause 3: replay ssm-association-create-and-list step setup#4 — DescribeAssociation on the association created in setup#3 and poll; inspect DocumentVersion, LastExecutionDate, LastSuccessfulExecutionDate and Overview.{Status,DetailedStatus,AssociationStatusAggregatedCount}.
5. Cause 4: replay ssm-association-create-list-delete through teardown#5 — CreateAssociation, DeleteAssociation, then DescribeAssociation with the same AssociationId; expect 400 AssociationDoesNotExist, observe 200 with the deleted record.
