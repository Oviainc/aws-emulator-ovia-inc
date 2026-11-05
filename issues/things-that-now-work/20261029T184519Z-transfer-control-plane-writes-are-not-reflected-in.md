Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-as2-agreement-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-as2-agreement-lifecycle transfer-as2-certificate-and-profile-lifecycle transfer-workflow-definition-and-execution-introspection
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|DescribeAgreement|2xx->2xx|$.Agreement.Status
Issue-Key: transfer|ListCertificates|2xx->2xx|$.Certificates[*]
Issue-Key: transfer|ListWorkflows|2xx->2xx|$.Workflows[*]
Issue-Related: 20261029T184519Z-transfer-reads-omit-empty-collection-members-and-the.md
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# Transfer control-plane writes are not reflected in later reads: agreement status, certificates and workflows

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAgreement

## Description
Three net-new divergences share one fault: a successful write is acknowledged but the emulator's state is not updated, so the read that follows in the same scenario shows the pre-write world. All three reads answer HTTP 200, which is what makes this dangerous — nothing signals the loss. THREE causes must be closed before submitting.

Cause 1 of 3 — an agreement update to INACTIVE is acknowledged but DescribeAgreement still reports ACTIVE.
AWS: Smithy: DescribedAgreement.Status -> AgreementStatusType, documented as 'The current status of the agreement, either ACTIVE or INACTIVE'; DescribeAgreement returns the agreement's CURRENT state. The recorded scenario sets the agreement inactive and the following read returns Status 'INACTIVE'.
Emulator: the read returns Status 'ACTIVE' — the status transition was accepted and discarded. (The same response also omits the empty Tags member; that serializer fault is filed separately.)
Why: deactivating an AS2 agreement is the supported way to stop accepting a partner's traffic; a caller that deactivates and then verifies reads back ACTIVE, so it cannot tell whether the agreement is still live, and automation that waits for INACTIVE never converges.
Fix: the agreement state — the update path must persist Status, and DescribeAgreement must project the stored value instead of the creation-time one.
Traces: transfer-as2-agreement-lifecycle#task#8.

Cause 2 of 3 — an imported certificate does not appear in ListCertificates.
AWS: Smithy: ListCertificates -> Certificates (ListedCertificate entries); an imported certificate is listed with its CertificateId, its ARN under the account/region, Usage ('SIGNING' in the recorded scenario), Status ('ACTIVE'), Type ('CERTIFICATE' when no private key was supplied — DescribedCertificate.Type documents CERTIFICATE_WITH_PRIVATE_KEY vs CERTIFICATE) and its ActiveDate/InactiveDate.
Emulator: the listing has no entry at index 0 at all — the import is invisible to the listing. Note the open issue 'Implement transfer ImportCertificate: unimplemented scaffold stub' covers ImportCertificate ITSELF answering NotImplemented; it does not cover the listing projection, and this replay step was not reported as blocked by that stub, so the listing side must be closed here too.
Why: the certificate id/ARN a caller needs for CreateProfile.CertificateIds and for agreement setup cannot be discovered, and a caller that lists to verify its import concludes the import silently failed.
Fix: the certificate state plus the ListCertificates handler — persist imported certificates and project them (with Usage/Status/Type and the active/inactive dates) into the listing.
Traces: transfer-as2-certificate-and-profile-lifecycle#task#3.

Cause 3 of 3 — a created workflow does not appear in ListWorkflows.
AWS: Smithy: ListWorkflows -> Workflows (ListedWorkflow entries); a created workflow is listed with its WorkflowId, its workflow ARN and the Description supplied at creation.
Emulator: the listing has no entry at index 0 — the created workflow is invisible.
Why: ListWorkflows is how a caller enumerates workflows to attach one to a server's WorkflowDetails or to clean up; a workflow that exists (it is describable in the same scenario) but is unlistable makes enumeration-driven automation skip it and makes idempotent 'create if absent' logic create duplicates.
Fix: the workflow state/listing — ListWorkflows must project the stored workflows (Arn, WorkflowId, Description) rather than an empty set.
Traces: transfer-workflow-definition-and-execution-introspection#task#4.

## Evidence
From replay of simparity-wire-aws-v1. All three steps answered HTTP 200 on both sides, so the loss is invisible to status-only checks. Expected = recorded AWS response, Actual = emulator.

1. DescribeAgreement / transfer-as2-agreement-lifecycle#task#8 — '$.Agreement.Status: value_mismatch (expected INACTIVE, got ACTIVE)' (the cluster's detail also lists '$.Agreement.Tags: missing (expected [], got None)', the empty-collection serializer fault filed under the projection issue).
2. ListCertificates / transfer-as2-certificate-and-profile-lifecycle#task#3 — '$.Certificates[0]: missing', expected an entry carrying ActiveDate, Arn (arn:aws:transfer:<region>:<account>:certificate/<cert id>), CertificateId, InactiveDate, Status 'ACTIVE', Type 'CERTIFICATE', Usage 'SIGNING'; got None (empty listing).
3. ListWorkflows / transfer-workflow-definition-and-execution-introspection#task#4 — '$.Workflows[0]: missing', expected an entry carrying Arn (arn:aws:transfer:<region>:<account>:workflow/<workflow id>), Description as supplied at CreateWorkflow and WorkflowId; got None (empty listing).

Model grounding (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): DescribedAgreement.Status targets AgreementStatusType and is documented as the agreement's current ACTIVE/INACTIVE status; DescribedCertificate documents Usage (SIGNING/ENCRYPTION/TLS), Status (ACTIVE/INACTIVE) and Type (CERTIFICATE vs CERTIFICATE_WITH_PRIVATE_KEY); DescribeWorkflow/ListWorkflows are the read side of CreateWorkflow. The concrete ids, ARNs and dates above are per-run values from the recordings, not requirements.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Re-replay the three bound steps in their fixtures' order so the preceding write runs first:
   - transfer-as2-agreement-lifecycle#task#8 — after the scenario sets the agreement INACTIVE, DescribeAgreement must report Status 'INACTIVE'.
   - transfer-as2-certificate-and-profile-lifecycle#task#3 — after the certificate import, ListCertificates must contain that certificate with its CertificateId/Arn/Usage/Status/Type.
   - transfer-workflow-definition-and-execution-introspection#task#4 — after CreateWorkflow, ListWorkflows must contain that workflow with its Arn/WorkflowId/Description.
