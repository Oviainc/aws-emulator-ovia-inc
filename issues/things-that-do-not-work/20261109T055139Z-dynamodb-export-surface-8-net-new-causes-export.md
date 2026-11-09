Issue-Source: trace-sweep-divergence
Issue-Trace: ddbctl-export-time-window-and-token-conflict
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbctl-export-time-window-and-token-conflict ddbctl-list-exports-paginated-by-table-arn dynamodb-export-lifecycle dynamodb-point-in-time-export-pipeline dynamodb-point-in-time-recovery-and-export
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ExportTableToPointInTime|4xx->4xx|$.__type
Issue-Key: dynamodb|ExportTableToPointInTime|2xx->2xx|$.ExportDescription.S3SseAlgorithm
Issue-Key: dynamodb|ExportTableToPointInTime|4xx->2xx|$.__type
Issue-Key: dynamodb|ExportTableToPointInTime|4xx->2xx|$.message
Issue-Key: dynamodb|ExportTableToPointInTime|4xx->2xx|status
Issue-Key: dynamodb|DescribeExport|2xx->2xx|$.ExportDescription.DestinationType
Issue-Key: dynamodb|DescribeExport|2xx->2xx|$.ExportDescription.S3SseAlgorithm
Issue-Key: dynamodb|ListExports|2xx->2xx|$.ExportSummaries[*].DestinationType
Issue-Key: dynamodb|DescribeExport|2xx->2xx|$.ExportDescription.BilledSizeBytes
Issue-Key: dynamodb|DescribeExport|4xx->4xx|$.message
Issue-Related: 20261030T060227Z-dynamodb-pitr-and-export-state-no-export-summaries.md
Issue-Related: 20261030T060227Z-dynamodb-validationexception-messages-diverge-from-the-wording-aws.md
Issue-Commit: bdb5184d827fec951c886c257ec0292728205109

# dynamodb export surface: 8 net-new causes - export descriptions omit fields and export refusals diverge

> _Note: this issue was identified against the code base at commit `bdb5184d827fec951c886c257ec0292728205109`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportTableToPointInTime

## Description
Eight net-new root causes in the DynamoDB table-export surface (ExportTableToPointInTime / DescribeExport / ListExports). All eight must be closed before submitting.

Cause 1 of 8: ExportTableToPointInTime picks the wrong error class for an ExportTime outside the recovery window.
AWS: Smithy: ExportTableToPointInTime errors include InvalidExportTimeException (400, doc "The specified ExportTime is outside of the point in time recovery window") and PointInTimeRecoveryUnavailableException (400, doc "Point in time recovery has not yet been enabled for this source table"). The recorded response is 400 with __type 'com.amazonaws.dynamodb.v20120810#InvalidExportTimeException' and message 'Export Time is invalid' - that message is entirely fixed text with no variable part.
Emulator: 400 with __type 'com.amazonaws.dynamodb.v20120810#PointInTimeRecoveryUnavailableException' and message "Point in time recovery is not enabled for table '<tableName>'" (fixed text "Point in time recovery is not enabled for table '...'" with the table name taken from the request). It reports the source table as PITR-unavailable where AWS reports the requested time as invalid.
Why: a client branches on the error class to decide between "enable PITR on the source table first" and "choose an ExportTime inside the window". It takes the first branch, re-enables PITR, retries with the same ExportTime and loops.
Fix: the ExportTableToPointInTime handler's pre-condition ordering - the PITR-availability guard must not fire for a table whose PITR is enabled; the window comparison must raise InvalidExportTimeException with AWS's fixed text.
Traces: ddbctl-export-time-window-and-token-conflict#task#4.

Cause 2 of 8: the ExportTableToPointInTime response omits S3SseAlgorithm.
AWS: Smithy: ExportDescription.S3SseAlgorithm (enum S3SseAlgorithm, doc "Type of encryption used on the bucket where export data is stored. Valid values ... AES256 - server-side encryption with Amazon S3 managed keys; KMS"). The recording shows AWS echoing 'AES256' for an export that named no S3SseAlgorithm and no KMS key, i.e. the S3-managed default is resolved and reported.
Emulator: the member is absent from ExportDescription.
Why: a client that reads back the export's encryption setting (to assert compliance, or to copy the setting onto a follow-up export) sees nothing and cannot tell an encrypted export from an unencrypted one.
Fix: the ExportTableToPointInTime handler's export state plus the ExportDescription serializer - resolve and persist the effective SSE algorithm at export creation.
Traces: ddbctl-export-time-window-and-token-conflict#task#9 (cluster also covers ddbctl-export-time-window-and-token-conflict#task#10, ddbctl-list-exports-paginated-by-table-arn#task#6/#task#7, dynamodb-export-lifecycle#task#8, dynamodb-point-in-time-export-pipeline#task#7).

Cause 3 of 8: an S3Prefix shorter than the service minimum is accepted instead of refused.
AWS: recorded behavior only for the length floor - the on-disk Smithy model constrains ExportTableToPointInTimeInput.S3Prefix only to @length min 0 max 1024, but the recorded response is 400 with __type 'com.amazon.coral.validate#ValidationException' and message "Invalid Request: 1 validation error detected: Value '<s3Prefix>' at 's3Prefix' failed to satisfy constraint: Member must have length greater than or equal to 3". Fixed text: "Invalid Request: 1 validation error detected: Value '", "' at 's3Prefix' failed to satisfy constraint: Member must have length greater than or equal to 3"; the only variable part is the prefix value echoed from the request.
Emulator: 200, with no __type and no message - the export is created and starts.
Why: a caller with a too-short prefix gets a successful export at AWS-invalid coordinates, so the client-side defect is never surfaced and the objects land where AWS would never have written them.
Fix: ExportTableToPointInTime input validation (the s3Prefix length check), which must run before any export state is created.
Traces: ddbctl-export-time-window-and-token-conflict#task#12 (cluster also covers #task#13).

Cause 4 of 8: DescribeExport omits DestinationType and S3SseAlgorithm.
AWS: S3SseAlgorithm is modeled on ExportDescription (see cause 2). DestinationType is recorded behavior only - it is not a member of ExportDescription in the on-disk model, but the recording shows AWS returning 'S3' for an export whose destination is an S3 bucket.
Emulator: both members are absent from the DescribeExport ExportDescription.
Why: a client polling DescribeExport to learn where and how an export landed cannot determine the destination kind or the encryption, so it cannot construct the follow-up S3 reads or assert the export's settings.
Fix: the DescribeExport serializer plus the persisted export record it reads from.
Traces: ddbctl-export-time-window-and-token-conflict#task#14 (cluster also covers ddbctl-list-exports-paginated-by-table-arn#task#8/#task#9, dynamodb-point-in-time-export-pipeline#task#8).

Cause 5 of 8: ListExports summaries omit DestinationType.
AWS: recorded behavior only - the on-disk ExportSummary models ExportArn, ExportStatus and ExportType, but each recorded ExportSummaries entry also carries DestinationType 'S3'.
Emulator: the member is absent from every ExportSummaries entry.
Why: a client listing a table's exports to pick the S3-destined ones has no field to filter on and must DescribeExport each ARN (which also omits it - cause 4).
Fix: the ListExports handler's ExportSummary projection.
Traces: ddbctl-list-exports-paginated-by-table-arn#task#10.

Cause 6 of 8: ListExports does not paginate - NextToken is never returned.
AWS: Smithy: ListExports is @paginated with inputToken/outputToken NextToken and pageSize MaxResults, and MaxResults targets ListExportsMaxLimit @range min 1 max 25. The recording, for a table with more exports than the requested page size, returns a NextToken (its value is opaque and not asserted).
Emulator: the response carries no NextToken, so the first page is presented as the whole result set.
Why: a client paging a table's exports silently sees only the first page and concludes exports beyond it do not exist.
Fix: the ListExports handler's pagination - emit a continuation token whenever more exports remain after the page, and accept it on the next call.
Traces: ddbctl-list-exports-paginated-by-table-arn#task#11.

Cause 7 of 8: DescribeExport reports a billed size derived from the exported item bytes where AWS reports none billed.
AWS: Smithy: ExportDescription.BilledSizeBytes (long, @range min 0, doc "The billable size of the table export"). The recording of a completed export shows BilledSizeBytes 0.
Emulator: a nonzero value equal to the raw byte size of the exported items.
Why: cost-attribution and billing-assertion clients read a charge for an export AWS did not bill, so any test or report that compares expected to reported billed size disagrees for every export.
Fix: the export state's billed-size accounting, read by the DescribeExport serializer. Hypothesis: AWS bills a table export in rounded units and reports 0 below the first billable unit, rather than reporting the summed item bytes.
Traces: dynamodb-export-lifecycle#task#10.

Cause 8 of 8: the ExportNotFoundException message drops AWS's repeated prefix.
AWS: Smithy: DescribeExport error ExportNotFoundException (400, doc "The specified export was not found"). The recorded 400 body carries message "Export not found: Export not found: <exportArn>" - AWS's wording repeats the prefix, so the fixed text is "Export not found: Export not found: " and the only variable part is the export ARN echoed from the request.
Emulator: message "Export not found: <exportArn>" - a single prefix. Status (400) and __type agree; only the message diverges.
Why: a client that matches or logs the refusal text against AWS's wording does not recognise the emulator's message, so negative-path assertions on a deleted or never-created export fail.
Fix: the DescribeExport error formatter for ExportNotFoundException.
Traces: dynamodb-point-in-time-export-pipeline#task#10 (cluster also covers dynamodb-point-in-time-recovery-and-export#task#6).

Related open issue, which does NOT carry these replay keys: 20261108T051530Z-dynamodb-8-net-new-causes-exporttabletopointintime-answers-notimplemented.md (ClientToken-conflict NotImplemented and a different InvalidExportTime wording) and 20261030T060227Z-dynamodb-pitr-and-export-state-no-export-summaries.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 (242 fixtures, 2631 steps, score 90.5%), 16 net-new divergence clusters, 8 of them in the export surface:

1. ddbctl-export-time-window-and-token-conflict#task#4 ExportTableToPointInTime - expected 400 __type com.amazonaws.dynamodb.v20120810#InvalidExportTimeException / message 'Export Time is invalid'; got 400 __type com.amazonaws.dynamodb.v20120810#PointInTimeRecoveryUnavailableException / message "Point in time recovery is not enabled for table '<table>'".
2. ddbctl-export-time-window-and-token-conflict#task#9 ExportTableToPointInTime - $.ExportDescription.S3SseAlgorithm: missing (expected 'AES256', got None). 6 steps affected.
3. ddbctl-export-time-window-and-token-conflict#task#12 ExportTableToPointInTime - status: expected 400, got 200; $.__type missing (expected 'com.amazon.coral.validate#ValidationException'); $.message missing (expected "Invalid Request: 1 validation error detected: Value '<prefix>' at 's3Prefix' failed to satisfy constraint: Member must have length greater than or equal to 3"). 2 steps affected.
4. ddbctl-export-time-window-and-token-conflict#task#14 DescribeExport - $.ExportDescription.DestinationType: missing (expected 'S3'); $.ExportDescription.S3SseAlgorithm: missing (expected 'AES256'). 4 steps affected.
5. ddbctl-list-exports-paginated-by-table-arn#task#10 ListExports - $.ExportSummaries[0].DestinationType: missing (expected 'S3').
6. ddbctl-list-exports-paginated-by-table-arn#task#11 ListExports - $.NextToken: missing (expected a shaped token, got None) (plus the cause-5 DestinationType gap).
7. dynamodb-export-lifecycle#task#10 DescribeExport - $.ExportDescription.BilledSizeBytes: value_mismatch (expected 0, got a byte sum of the exported items).
8. dynamodb-point-in-time-export-pipeline#task#10 DescribeExport - $.message: value_mismatch (expected 'Export not found: Export not found: <exportArn>', got 'Export not found: <exportArn>'). 2 steps affected.

Model evidence: smithy_query operation ExportTableToPointInTime/DescribeExport/ListExports and shape ExportDescription/ExportSummary/InvalidExportTimeException/PointInTimeRecoveryUnavailableException/ExportNotFoundException on services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: fixture ddbctl-export-time-window-and-token-conflict, step task#4 - ExportTableToPointInTime on a PITR-enabled table with an ExportTime outside the recovery window.
4. Cause 2: same fixture, step task#9 - a successful ExportTableToPointInTime; inspect ExportDescription.S3SseAlgorithm.
5. Cause 3: same fixture, step task#12 - ExportTableToPointInTime with an S3Prefix of fewer than 3 characters.
6. Cause 4: same fixture, step task#14 - DescribeExport on that export ARN; inspect DestinationType and S3SseAlgorithm.
7. Cause 5/6: fixture ddbctl-list-exports-paginated-by-table-arn, steps task#10 and task#11 - ListExports by TableArn with MaxResults below the export count; inspect ExportSummaries[].DestinationType and NextToken.
8. Cause 7: fixture dynamodb-export-lifecycle, step task#10 - DescribeExport on a completed export; inspect BilledSizeBytes.
9. Cause 8: fixture dynamodb-point-in-time-export-pipeline, step task#10 - DescribeExport on a nonexistent export ARN; compare the message text.
