Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-automation-filter-steps-and-complete
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-automation-filter-steps-and-complete ssm-document-permission-share-and-revoke ssm-document-sharing-workflow ssm-cloud-connector-create-invalid-role ssm-cloudconnector-create-invalid-input ssm-cloudconnector-list-and-invalid-create ssm-create-cloud-connector-invalid-role-arn
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetDocument|4xx->2xx|$.Message
Issue-Key: ssm|GetDocument|4xx->2xx|$.__type
Issue-Key: ssm|GetDocument|4xx->2xx|status
Issue-Key: ssm|CreateCloudConnector|4xx->2xx|$.__type
Issue-Key: ssm|CreateCloudConnector|4xx->2xx|$.message
Issue-Key: ssm|CreateCloudConnector|4xx->2xx|status
Issue-Commit: 685d7dc1b36320cc963844050fe238af8df0451a

# SSM skips existence checks: GetDocument on a deleted document and CreateCloudConnector with an absent IAM role both answer 200 (2 causes)

> _Note: this issue was identified against the code base at commit `685d7dc1b36320cc963844050fe238af8df0451a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDocument

## Description
Two SSM entry points serve a success where AWS refuses the request, because neither checks that the resource it is handed exists. Both causes are in service `ssm`.

Cause 1 of 2 — GetDocument keeps serving a document after DeleteDocument (representative; fixture ssm-automation-filter-steps-and-complete, step task#7).
AWS: Smithy: GetDocument input Name (target DocumentARN, required, pattern ^[a-zA-Z0-9_\\-.:/]{3,128}$); modelled errors InvalidDocument ("The specified SSM document doesn't exist.") and InvalidDocumentVersion. Once DeleteDocument has removed a document, GetDocument on that name answers status 400 with __type 'InvalidDocument' and Message of the form 'Document with name <name> does not exist.' — fixed text 'Document with name ' + the Name taken from the request + ' does not exist.'
Emulator: status 200 with the full document payload — the deleted document is still readable, and no __type/Message is produced.
Why: deletion is unobservable through the document read path, so teardown can never be confirmed, a recreate-after-delete flow sees a stale document, and negative tests asserting the InvalidDocument refusal pass vacuously.
Fix: the GetDocument handler's lookup against the document store must honour deletion and take the InvalidDocument error path (the store must actually remove the record on DeleteDocument).
Traces: ssm-automation-filter-steps-and-complete#task#7, ssm-document-permission-share-and-revoke#teardown#9, ssm-document-sharing-workflow#teardown#8.
Related but not covering: open issue 20261103T173026Z-ssm-document-payloads-and-document-store-error-paths owns the same failure on DescribeDocument (it carries that replay key); the GetDocument cluster above is unowned and is filed here.

Cause 2 of 2 — CreateCloudConnector accepts a RoleArn that names no role (fixture ssm-cloud-connector-create-invalid-role, step task#1).
AWS: Smithy: CreateCloudConnector requires DisplayName, RoleArn (CloudConnectorIamRoleArn, length 20..2048, pattern ^arn:aws[a-z0-9-]*:iam::\\d{12}:role\\/[\\w-\\/.@+=,]{1,1017}$), Configuration and ConfigConnectorArn; its modelled error list (ConflictException, InternalServerError, ServiceQuotaExceededException) does not include the refusal AWS actually sends, so for this cause: recorded behavior only — AWS answers status 400 with __type 'ValidationException' and Message of the form 'Nonexistent role or missing ssm service principal in trust policy: <roleArn>' — fixed text 'Nonexistent role or missing ssm service principal in trust policy: ' followed by the RoleArn value taken verbatim from the request. The role must exist and its trust policy must allow the ssm service principal; a syntactically valid ARN is not enough.
Emulator: status 200 — the connector is created and a CloudConnectorId returned, with no __type and no Message, for a role ARN that resolves to no IAM role.
Why: a misconfigured connector is created and reported healthy; the operator never learns the role is missing or untrusted at create time, and IAM-wiring regression tests that assert the ValidationException pass vacuously.
Fix: the CreateCloudConnector handler's validation — resolve RoleArn against IAM state and check the ssm service principal in its trust policy before creating the connector, emitting ValidationException with the message above otherwise.
Traces: ssm-cloud-connector-create-invalid-role#task#1, ssm-cloudconnector-create-invalid-input#task#2, ssm-cloudconnector-list-and-invalid-create#task#2, ssm-create-cloud-connector-invalid-role-arn#task#1.
Related but not covering: open issue 20261103T215115Z-ssm-createcloudconnector-stub is an unimplemented-stub issue, yet this replay shows CreateCloudConnector serving a 200 (the replay reported stub_path empty for the cluster), so the stub issue does not describe or gate this divergence; and 20261104T023744Z-ssm-accepts-two-inputs-aws-refuses-createactivation-with covers the same trust-policy rule for CreateActivation only.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (service ssm, 2 net-new clusters, stub_path empty, owned_by empty):
- ssm-automation-filter-steps-and-complete#task#7 (GetDocument, expected 400 -> actual 200): $.__type missing (expected 'InvalidDocument', got None); $.Message missing (expected 'Document with name <documentName> does not exist.', got None). Cluster also covers ssm-document-permission-share-and-revoke#teardown#9 and ssm-document-sharing-workflow#teardown#8.
- ssm-cloud-connector-create-invalid-role#task#1 (CreateCloudConnector, expected 400 -> actual 200): $.__type missing (expected 'ValidationException', got None); $.message missing (expected 'Nonexistent role or missing ssm service principal in trust policy: <roleArn>', got None). Cluster also covers ssm-cloudconnector-create-invalid-input#task#2, ssm-cloudconnector-list-and-invalid-create#task#2 and ssm-create-cloud-connector-invalid-role-arn#task#1.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Deleted document: replay fixture ssm-automation-filter-steps-and-complete through step task#7 — CreateDocument, use it, DeleteDocument, then GetDocument on the same Name; the emulator answers 200 with the document instead of 400 InvalidDocument.
4. Absent role: replay fixture ssm-cloud-connector-create-invalid-role step task#1 — CreateCloudConnector with a well-formed arn:aws:iam::<account>:role/<name> that no IAM role matches; the emulator answers 200 with a CloudConnectorId instead of 400 ValidationException.
