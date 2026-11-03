Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.attach-data-source-nonexistent-resource-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.attach-data-source-nonexistent-resource-rejected|run#20|AssertionError|assert retry["attachmentId"] == attach_result["attachmentId"], (
Issue-Commit: c30ef51fcd05b9a0beb076a6507e64cfdd13ef56

# opensearch: AttachDataSource is not idempotent after its attachment settles — a repeat attach of the same (application id, dataSourceArn) mints a NEW attachmentId instead of returning the existing attachment

> _Note: this issue was identified against the code base at commit `c30ef51fcd05b9a0beb076a6507e64cfdd13ef56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachDataSource

## Description
This issue carries 1 root cause, in the opensearch service. It must be closed before submitting.

Cause 1 of 1: a repeat AttachDataSource for an (application id, dataSourceArn) pair the application already holds an attachment for returns a different attachmentId, so one pair ends up with two attachment records.

AWS: Smithy: AttachDataSource(id @required @httpLabel pattern ^[a-z0-9]{3,30}$, dataSourceArn @required ARN @length(20..2048), clientToken, workspaceId, workspaceConfiguration) -> {attachmentId, id, arn, dataSourceArn, status: DataSourceAttachmentStatus one of ATTACHED|FAILED|PENDING}, errors AccessDeniedException(403), ConflictException(409), DisabledOperationException(409), ResourceNotFoundException(409), ValidationException(400), InternalException(500). The operation's own modeled documentation states: "This operation is idempotent: If the data source is already attached or pending, the operation returns the existing attachment." DescribeDataSourceAttachment is keyed by exactly (id @required, dataSourceArn @required) and returns a single attachmentId, so an (application, data source) pair names AT MOST ONE attachment and therefore at most one attachmentId. Recorded behavior for a dataSourceArn that names a domain which does not exist: the attach is ACCEPTED (status PENDING/not ATTACHED, never ATTACHED), the attachment is validated asynchronously, the pair keeps ONE stable attachmentId across DescribeDataSourceAttachment reads and across a repeat AttachDataSource, and the attachment does NOT reach a terminal state quickly (not within a 5-minute bounded poll on real AWS), so a client retrying the attach always gets the same attachmentId back.

Emulator: the first AttachDataSource is accepted and the pair reads back with a stable attachmentId while the check polls DescribeDataSourceAttachment for 15 s (that leg passes). A repeat AttachDataSource with the IDENTICAL id and dataSourceArn then answers with a different attachmentId: `repeat AttachDataSource returned a different attachmentId ('dsa-3ibp6ed2aaaa' != 'dsa-4kubnyc2aaaa'), implying a duplicate record` (the two ids are per-run generated values, not requirements; the requirement is that the two calls report the SAME id). The mechanism is visible in the handler: an attachment whose dataSourceArn resolves to no data source is given an owed FAILED outcome a short provisioning span from the attach, the next call materialises that FAILED, and the idempotency short-circuit deliberately excludes a FAILED record (`held.filter(|held| held.status != ATTACHMENT_STATUS_FAILED)`), so the retry falls through and `store.mint_attachment_id()` issues a brand-new id for the same pair. Two divergences compose here: the attachment is driven to a terminal FAILED far sooner than AWS resolves it at all, and a FAILED record is then replaced under a new identity rather than returned.

Why: a client that retries an attach — the documented safe thing to do for an asynchronous, idempotent operation — believes it created a second attachment for a data source it already attached. Any caller that captured attachmentId from the first call and keys state, logs or polling on it now holds an id the service no longer reports, while DescribeDataSourceAttachment (which has no attachmentId input) silently answers about the newest record. Retry wrappers and converge-loops therefore churn attachment records instead of converging, and no client can treat attachmentId as the stable identity AWS's model gives it.

Fix: the idempotency and identity rule in the AttachDataSource handler and the attachment state it reads — an (application id, dataSourceArn) pair must keep ONE attachment record and ONE attachmentId for repeat attaches regardless of the record's current status, and the attachment-settlement schedule for a dataSourceArn that resolves to no data source must not drive the record to a terminal state within the window in which a client is still retrying.

Traces: opensearch.attach-data-source-nonexistent-resource-rejected.

Note on neighbouring filed work: open issue 20261029T182036Z-opensearch-index-reads-writes-and-data-source-attachment.md covers other attachment causes (a valid attachment settling FAILED instead of ATTACHED; AttachDataSource's validation-vs-lookup error precedence for opensearch.attach_data_source_duplicate_rejected; DetachDataSource's race verdict). None of them covers attachmentId identity across a repeat attach, which is what this check fails on, and the replay report does not list this check as owned by it.

## Evidence
Failing check opensearch.attach-data-source-nonexistent-resource-rejected, [mismatch].

Passing legs (so the failure is isolated): AttachDataSource with an unknown application id correctly answers ResourceNotFoundException with `Application <id> not found for account <accountId>`; the first attach for an ARN with no backing domain is accepted with a non-ATTACHED status; DescribeDataSourceAttachment reports the same attachmentId and a non-ATTACHED status on every read for 15 s.

Failing assertion (check_opensearch_attach_data_source_nonexistent_resource_rejected.py line 68):
  assert retry["attachmentId"] == attach_result["attachmentId"]
  AssertionError: repeat AttachDataSource returned a different attachmentId ('dsa-3ibp6ed2aaaa' != 'dsa-4kubnyc2aaaa'), implying a duplicate record

Expected: the second AttachDataSource(id=<app>, dataSourceArn=<same ARN>) returns HTTP 200 with attachmentId equal to the one the first attach returned, and a status that is not ATTACHED.
Actual: HTTP 200 with a freshly minted attachmentId for the same (id, dataSourceArn) pair. The concrete id strings above are per-run generated values; the requirement is their equality, not their text.

Handler evidence (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/attach_data_source.rs): line 231-232 materialise owed status moves; lines 266-269 return the existing attachment ONLY when its status is not ATTACHMENT_STATUS_FAILED; lines 286-296 schedule an absent data source's attachment to fall due one PROVISIONING span after the attach; line 318 mints a new attachment id on the fall-through path.

## Steps to Reproduce
1. CreateApplication(name=<unique>) and capture its id.
2. Build a syntactically valid ARN for a domain that does NOT exist: arn:aws:es:<region>:<accountId>:domain/<unused-name>.
3. AttachDataSource(id=<appId>, dataSourceArn=<that ARN>) — note the returned attachmentId and status (status must not be ATTACHED).
4. Poll DescribeDataSourceAttachment(id=<appId>, dataSourceArn=<that ARN>) every 3 s for ~15 s — attachmentId stays stable and status never becomes ATTACHED (this leg already passes).
5. Call AttachDataSource(id=<appId>, dataSourceArn=<the same ARN>) again and compare attachmentId with the one from step 3: the emulator returns a different id, AWS returns the same one.
6. Read DescribeDataSourceAttachment(id=<appId>, dataSourceArn=<that ARN>) once more and compare its attachmentId with step 3's as an independent confirmation.
