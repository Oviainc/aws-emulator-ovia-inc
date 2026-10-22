Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securityhub.batch-import-findings-round-trip securityhub.batch-update-findings-workflow-status-transition securityhub.update-findings-record-state-archive-has-no-effect securityhub.batch-import-findings-partial-failure-isolated securityhub.batch-import-findings-customer-fields-preserved-on-reimport
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub.batch-import-findings-round-trip|run#17|KeyError|assert found["RecordState"] == "ACTIVE", f"expected RecordState=ACTIVE for a new finding, got {found['RecordState']!r}"
Issue-Key: securityhub.batch-update-findings-workflow-status-transition|run#11|KeyError|assert landed["Workflow"]["Status"] == "NEW", f"expected freshly-imported finding Workflow.Status NEW, got {landed['Workflow']['Status']!r}"
Issue-Key: securityhub.update-findings-record-state-archive-has-no-effect|run#11|KeyError|assert landed["RecordState"] == "ACTIVE", f"expected freshly-imported finding ACTIVE, got {landed['RecordState']!r}"
Issue-Key: securityhub.batch-import-findings-partial-failure-isolated|run#13|AssertionError|assert resp["SuccessCount"] == 1, f"expected 1 success, got {resp}"
Issue-Key: securityhub.batch-import-findings-customer-fields-preserved-on-reimport|run#19|KeyError|assert updated["Workflow"]["Status"] == "NOTIFIED", updated.get("Workflow")
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# BatchImportFindings: missing service-set finding defaults, no AFF required-field validation, and re-import clobbers customer-managed fields

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchImportFindings

## Description
Three distinct root causes in the Security Hub CSPM finding-ingest path. All three are in `BatchImportFindings` (services/service-com-amazonaws-securityhub-securityhubapiservice/src/ops/batch_import_findings.rs) but each is an independent defect and must be closed independently.

=== Cause 1 of 3: an imported finding carries no service-populated RecordState or Workflow ===
Failing checks: securityhub.batch-import-findings-round-trip, securityhub.batch-update-findings-workflow-status-transition, securityhub.update-findings-record-state-archive-has-no-effect
AWS: a finding that Security Hub CSPM has ingested always has the service-populated lifecycle members set, independently of what the provider submitted. Smithy: AwsSecurityFinding$RecordState targets the RecordState enum with values [ACTIVE, ARCHIVED] ("The record state of a finding"); AwsSecurityFinding$Workflow targets Workflow, whose Status member documents "NEW - The initial state of a finding, before it is reviewed" and that Security Hub CSPM itself resets the status to NEW on certain transitions, i.e. the service, not the provider, owns this value. Neither member is modelled @required on input, so AWS supplies it: a freshly imported finding reads back with RecordState = ACTIVE and Workflow.Status = NEW. Recorded behavior confirms it: every check above asserts those two values on the first read after import.
Emulator: GetFindings returns the finding, but with the RecordState and Workflow keys entirely ABSENT from the serialized finding, so the client gets a KeyError on finding["RecordState"] / finding["Workflow"]. In batch_import_findings.rs the handler sets only `finding.region` and `finding.processed_at` before storing (and completes Severity); it never defaults `record_state` to ACTIVE nor `workflow` to Status=NEW, so whatever the provider omitted stays None and is omitted on the wire.
Why: RecordState and Workflow.Status are the two members every Security Hub consumer keys its triage logic on — filter findings by RecordState=ACTIVE, show the NEW queue, drive NEW->NOTIFIED->RESOLVED. A client reading a finding straight back from ingest sees neither field and either crashes on the missing key or silently treats the finding as having no lifecycle state. It also makes the whole workflow surface untestable: a NEW->NOTIFIED transition cannot be observed because the pre-state is absent.
Fix: the ingest path in the BatchImportFindings handler — default the service-owned lifecycle members when storing a finding (RecordState=ACTIVE, Workflow.Status=NEW when the provider supplied none), alongside the existing Region/ProcessedAt population; the same defaulting must apply to the create branch and the re-import branch of the store.
Traces: securityhub.batch-import-findings-round-trip (KeyError 'RecordState' asserting a new finding is ACTIVE); securityhub.batch-update-findings-workflow-status-transition (KeyError 'Workflow' asserting a freshly imported finding is Workflow.Status=NEW); securityhub.update-findings-record-state-archive-has-no-effect (KeyError 'RecordState' on the same pre-state assertion).

=== Cause 2 of 3: a finding with neither Severity nor FindingProviderFields is accepted instead of failing in isolation ===
Failing check: securityhub.batch-import-findings-partial-failure-isolated
AWS: BatchImportFindings validates each finding against the AWS Security Finding Format and reports a non-conforming one PER FINDING rather than failing the batch. A finding that populates neither Severity nor FindingProviderFields does not conform. Smithy: AwsSecurityFinding$Severity is NOT modelled @required and AwsSecurityFinding$FindingProviderFields is documented as the way "finding providers use FindingProviderFields to provide and update their own values for confidence, criticality, related findings, severity, and types" — so the requirement is the AFF schema's anyOf over the two, which the Smithy model does not express as a trait. Recorded behavior for such a finding: HTTP 200 with SuccessCount=1, FailedCount=1, and FailedFindings[0] = {Id: <the bad finding id>, ErrorCode: "InvalidInput", ErrorMessage: ...}. ErrorCode "InvalidInput" and the counts are the requirement; the recorded message is an example of the rule "name the missing required properties" — e.g. "Finding does not adhere to Amazon Finding Format. FindingId: <id> Error: data should have required property 'Severity', data should have required property 'FindingProviderFields', data should match some schema in anyOf." The valid sibling in the same batch still lands and is retrievable via GetFindings; the rejected one never becomes visible.
Emulator: both findings are accepted — the response is HTTP 200 with {'FailedCount': 0, 'SuccessCount': 2, 'FailedFindings': []}. `rejection_reason()` in batch_import_findings.rs checks SchemaVersion, Id/Title/Description/AwsAccountId lengths, Resources/Types counts, field-map sizes and timestamp parseability, but has no rule requiring Severity or FindingProviderFields, so a finding with Severity deleted passes every rule.
Why: the emulator is more permissive than AWS. A provider that ships a malformed finding gets a clean success locally and a per-finding InvalidInput rejection in production, so the error-handling path (read FailedFindings, re-emit the bad finding) is never exercised. Worse, the malformed finding becomes queryable in the emulator, so downstream code sees a finding with no severity at all — a shape AWS never stores.
Fix: the per-finding validation in the BatchImportFindings handler (`rejection_reason`) — add the AFF anyOf rule so a finding with neither Severity nor FindingProviderFields is pushed to FailedFindings with ErrorCode "InvalidInput" and a message naming the missing properties, not stored, and not counted in SuccessCount.
Traces: securityhub.batch-import-findings-partial-failure-isolated (asserts SuccessCount=1 / FailedCount=1 and FailedFindings[0].ErrorCode == "InvalidInput"; got SuccessCount=2, FailedCount=0).

=== Cause 3 of 3: re-importing an identifier overwrites customer-managed fields the provider may not update ===
Failing check: securityhub.batch-import-findings-customer-fields-preserved-on-reimport
AWS: once a finding exists, BatchImportFindings updates provider-owned fields only. The operation's own documentation in this service's Smithy model states: "After a finding is created, BatchImportFindings cannot be used to update the following finding fields and objects, which Security Hub CSPM customers use to manage their investigation workflow: Note, UserDefinedFields, VerificationState, Workflow" (and finding providers should route Confidence, Criticality, RelatedFindings, Severity and Types through FindingProviderFields). So re-importing the same ProductArn+Id with a changed Description updates Description while Workflow.Status and Note — set earlier by the customer through BatchUpdateFindings — survive unchanged. Recorded behavior: after a NOTIFIED/Note update and then a re-import, the finding reads back with the new Description AND Workflow.Status == NOTIFIED and the customer's Note.Text intact.
Emulator: the re-import branch replaces the stored record wholesale — in batch_import_findings.rs the update path is `existing.finding = finding;`, dropping every customer-managed member that the incoming provider payload does not carry. The check observes the customer's NOTIFIED workflow correctly applied by BatchUpdateFindings, then after the re-import the Workflow key is gone from the finding altogether (KeyError 'Workflow' on the post-reimport assertion), and Note is lost with it.
Why: this silently destroys investigation state. Any provider that periodically re-imports its findings (the normal integration pattern) wipes the customer's triage: suppressed findings pop back into the queue, resolution notes vanish, VerificationState and UserDefinedFields are lost. A client relying on AWS's guarantee that provider re-imports never touch the workflow surface is exactly inverted here.
Fix: the re-import (update-in-place) branch of the BatchImportFindings handler's store write — merge provider-owned fields onto the existing record while carrying forward the customer-managed members (Note, UserDefinedFields, VerificationState, Workflow) from the stored finding instead of assigning the incoming finding over it.
Traces: securityhub.batch-import-findings-customer-fields-preserved-on-reimport (after BatchUpdateFindings set Workflow.Status=NOTIFIED + Note and a re-import with a changed Description, asserts Workflow.Status is still NOTIFIED and Note.Text preserved; got KeyError 'Workflow').

## Evidence
Cause 1 — expected vs actual:
  expected (AWS): GetFindings on a just-imported finding returns RecordState = "ACTIVE" and Workflow = {"Status": "NEW"}.
  actual (emulator): the finding is returned but both keys are absent.
    securityhub.batch-import-findings-round-trip:
      assert found["RecordState"] == "ACTIVE"  ->  KeyError: 'RecordState'
    securityhub.batch-update-findings-workflow-status-transition:
      assert landed["Workflow"]["Status"] == "NEW"  ->  KeyError: 'Workflow'
    securityhub.update-findings-record-state-archive-has-no-effect:
      assert landed["RecordState"] == "ACTIVE"  ->  KeyError: 'RecordState'
  code: batch_import_findings.rs sets only finding.region / finding.processed_at (+ complete_severity) before insert; no record_state / workflow defaulting.

Cause 2 — expected vs actual:
  request: BatchImportFindings with two findings, the second identical to the first except `Severity` removed (so neither Severity nor FindingProviderFields is populated).
  expected (AWS): HTTP 200, SuccessCount=1, FailedCount=1, FailedFindings=[{Id: <bad id>, ErrorCode: "InvalidInput", ErrorMessage: naming the missing properties, e.g. "Finding does not adhere to Amazon Finding Format. FindingId: <bad id> Error: data should have required property 'Severity', data should have required property 'FindingProviderFields', data should match some schema in anyOf."}]; the good finding is retrievable via GetFindings and the bad one never is.
  actual (emulator): HTTP 200 with body {'FailedCount': 0, 'SuccessCount': 2, 'FailedFindings': []} — assertion "expected 1 success, got ... 'SuccessCount': 2".
  code: rejection_reason() in batch_import_findings.rs has no Severity/FindingProviderFields rule.

Cause 3 — expected vs actual:
  sequence: BatchImportFindings(v1) -> BatchUpdateFindings(Workflow.Status=NOTIFIED, Note) -> observed NOTIFIED via GetFindings -> BatchImportFindings(v2, same ProductArn+Id, Description changed).
  expected (AWS): the finding reads back with the v2 Description, Workflow.Status still "NOTIFIED" and the customer Note.Text intact (BatchImportFindings cannot update Note/UserDefinedFields/VerificationState/Workflow after creation, per the operation's modelled documentation).
  actual (emulator): after the re-import the Workflow member is gone from the finding:
      assert updated["Workflow"]["Status"] == "NOTIFIED"  ->  KeyError: 'Workflow'
  code: the re-import branch does `existing.finding = finding;`, a wholesale replacement of the stored record.

## Steps to Reproduce
Common setup: EnableSecurityHub(EnableDefaultStandards=False) (tolerate ResourceConflictException), account_id from STS GetCallerIdentity, product_arn = arn:aws:securityhub:<region>:<account>:product/<account>/default, findings built as valid ASFF 2018-10-08 records (SchemaVersion 2018-10-08, Id, ProductArn, GeneratorId, AwsAccountId, CreatedAt/UpdatedAt, Title, Description, Severity, one Resource).

Cause 1:
1. BatchImportFindings with one valid finding; assert SuccessCount=1.
2. GetFindings(Filters={"Id": [{"Value": <id>, "Comparison": "EQUALS"}]}).
3. Inspect the returned finding: AWS has RecordState="ACTIVE" and Workflow={"Status":"NEW"}; the emulator omits both keys.

Cause 2:
1. Build two valid findings `good` and `bad`, then `del bad["Severity"]` (leaving no FindingProviderFields either).
2. BatchImportFindings(Findings=[good, bad]).
3. AWS: SuccessCount=1, FailedCount=1, FailedFindings[0].ErrorCode == "InvalidInput" for the bad id; emulator: SuccessCount=2, FailedCount=0, FailedFindings=[].
4. GetFindings on the bad id: AWS returns []; the emulator returns the stored malformed finding.

Cause 3:
1. BatchImportFindings with a finding whose Description is "v1"; wait until GetFindings returns it.
2. BatchUpdateFindings(FindingIdentifiers=[{Id, ProductArn}], Workflow={"Status":"NOTIFIED"}, Note={"Text": <note>, "UpdatedBy": ...}); confirm via GetFindings that Workflow.Status is NOTIFIED.
3. BatchImportFindings the same Id+ProductArn with Description "v2"; assert FailedCount=0.
4. GetFindings again once Description is "v2": AWS still shows Workflow.Status="NOTIFIED" and the Note; the emulator has dropped Workflow and Note entirely.
