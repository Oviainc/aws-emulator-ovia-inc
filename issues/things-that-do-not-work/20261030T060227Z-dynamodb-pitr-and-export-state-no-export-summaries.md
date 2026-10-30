Issue-Source: trace-sweep-divergence
Issue-Trace: ddbctl-backup-on-creating-table-and-restore-name-collision
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbctl-backup-on-creating-table-and-restore-name-collision ddbctl-export-time-window-and-token-conflict ddbctl-list-exports-paginated-by-table-arn ddbctl-restore-time-outside-pitr-window dynamodb-backup-create-list-restore-cleanup dynamodb-backup-lifecycle dynamodb-point-in-time-export-pipeline dynamodb-point-in-time-recovery-and-export dynamodb-point-in-time-recovery-and-restore incremental-export-invalid-windows-rejected dynamodb-export-lifecycle
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|UpdateContinuousBackups|2xx->2xx|$.ContinuousBackupsDescription.PointInTimeRecoveryDescription.PointInTimeRecoveryWindowDays
Issue-Key: dynamodb|DescribeContinuousBackups|2xx->2xx|$.ContinuousBackupsDescription.PointInTimeRecoveryDescription.PointInTimeRecoveryWindowDays
Issue-Key: dynamodb|ListExports|2xx->2xx|$.ExportSummaries[*]
Issue-Key: dynamodb|ListExports|2xx->2xx|$.NextToken
Issue-Key: dynamodb|ListExports|4xx->4xx|$.message
Issue-Key: dynamodb|RestoreTableToPointInTime|4xx->4xx|$.message
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# DynamoDB PITR and export state: no export summaries, recovery window omitted, wrong refusal messages

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateContinuousBackups

## Description
Point-in-time recovery and table export report an incomplete picture of their own state: the continuous-backups description omits the recovery window, ListExports never shows the exports that were started (and therefore never paginates), and both the export token refusal and the restore-window refusal carry messages that drop the information AWS puts in them. 6 causes, all dynamodb.

Cause 1 of 6: UpdateContinuousBackups' response omits the point-in-time recovery window.
AWS: recorded behavior — the ContinuousBackupsDescription.PointInTimeRecoveryDescription returned by UpdateContinuousBackups carries PointInTimeRecoveryWindowDays alongside the status (the recording shows 35, DynamoDB's default retention). The on-disk Smithy model names this concept RecoveryPeriodInDays (@range 1..35) on PointInTimeRecoveryDescription; the wire member the recording asserts is PointInTimeRecoveryWindowDays, so the requirement is that the enabled window be reported, with the service default when the caller did not choose one.
Emulator: PointInTimeRecoveryWindowDays absent from the response.
Why: a client cannot learn how far back it may restore, so it cannot validate a RestoreDateTime before issuing a restore and cannot report retention in a compliance check.
Fix: the UpdateContinuousBackups response projection and the continuous-backups state — store the recovery window on enable and serialize it.
Traces: ddbctl-backup-on-creating-table-and-restore-name-collision#task#14 (cluster of 10, incl. ddbctl-export-time-window-and-token-conflict#task#6, dynamodb-backup-create-list-restore-cleanup#task#5, dynamodb-point-in-time-recovery-and-restore#task#7).

Cause 2 of 6: DescribeContinuousBackups omits the same member.
AWS: recorded behavior — DescribeContinuousBackups returns the identical PointInTimeRecoveryDescription, including PointInTimeRecoveryWindowDays (35 in the recordings).
Emulator: the member is absent on the read path too.
Why: as cause 1, on the operation clients actually poll.
Fix: same state and the DescribeContinuousBackups projection.
Traces: dynamodb-backup-lifecycle#task#11 (cluster of 5, incl. dynamodb-point-in-time-export-pipeline#task#6, dynamodb-point-in-time-recovery-and-export#task#8).

Cause 3 of 6: ListExports returns no ExportSummaries for an export that was started.
AWS: Smithy: ListExports output is ExportSummaries (list of ExportSummary) plus a NextToken, and it is @paginated (inputToken NextToken / outputToken NextToken / pageSize MaxResults). The recording shows one summary for the started export with DestinationType 'S3', the export's ExportArn, an ExportStatus and ExportType 'FULL_EXPORT'.
Emulator: ExportSummaries[0] absent — the list is empty although ExportTableToPointInTime was accepted earlier in the fixture.
Why: exports are undiscoverable: a client that lists exports to find the one it started, or to reconcile exports it does not hold the ARN for, sees nothing.
Fix: the ListExports handler and the export state — register each accepted export so it is enumerable.
Traces: dynamodb-export-lifecycle#task#9 (cluster of 3, also dynamodb-export-lifecycle#task#11 and dynamodb-point-in-time-export-pipeline#task#9).

Cause 4 of 6: ListExports with MaxResults produces no second page and no NextToken.
AWS: Smithy: ListExportsMaxLimit is @range 1..25 and the operation is paginated; the recording shows a page carrying one ExportSummary plus a NextToken for the remaining export.
Emulator: neither ExportSummaries[0] nor NextToken is present.
Why: SDK paginators terminate immediately and report zero exports, so a caller with more exports than a page never reaches the rest.
Fix: the ListExports handler's paging — slice the registered exports by MaxResults and mint a continuation NextToken when more remain.
Traces: ddbctl-list-exports-paginated-by-table-arn#task#10.

Cause 5 of 6: the invalid-NextToken refusal is a bare invented sentence instead of AWS's constraint message.
AWS: the recording shows HTTP 400 with the message "Invalid Request: 1 validation error detected: Value '<the submitted token>' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ([0-9a-f]{16})+". The 'Invalid Request: ', the '1 validation error detected: ' envelope, the "Value '...' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ([0-9a-f]{16})+" frame and the pattern itself are fixed text; the quoted token comes from the request.
Emulator: HTTP 400 with the message 'Invalid NextToken'.
Whey a client relying on the message cannot tell a malformed token from an expired one, and loses the pattern that says what a valid token looks like.
Why: a client relying on the message cannot tell a malformed token from an expired one, and loses the pattern that documents a valid token.
Fix: the ListExports NextToken validation — refuse a token that does not match ([0-9a-f]{16})+ with the full constraint message.
Traces: ddbctl-list-exports-paginated-by-table-arn#task#14.

Cause 6 of 6: the out-of-window restore refusal drops the table name and the window bounds.
AWS: the recording shows HTTP 400 with "RestoreDateTime '<requested instant>' must be within EarliestRestorableDateTime '<earliest>' and LatestRestorableDateTime '<latest>' for table '<table name>'". The sentence frame is fixed text; the three timestamps and the table name are taken from the request and the table's current PITR window.
Emulator: the generic 'An invalid restore time was specified. RestoreDateTime must be between EarliestRestorableDateTime and LatestRestorableDateTime' — no interpolated values at all.
Why: the whole diagnostic value of the refusal is the bounds it names; without them a caller cannot correct the RestoreDateTime without a separate DescribeContinuousBackups call, and cannot tell whether it asked too early or too late.
Fix: the RestoreTableToPointInTime validation — build the message from the requested instant, the table's Earliest/LatestRestorableDateTime and the table name.
Traces: ddbctl-restore-time-outside-pitr-window#task#6 (cluster of 3, also task#7 and task#8 of the same fixture).

Related open issues: '20261025T015306Z-dynamodb-exporttabletopointintime-is-an-unimplemented-scaffold-stub-it.md' and '20261025T024002Z-dynamodb-exporttabletopointintime-stub.md' file ExportTableToPointInTime as a stub; the export now starts, so the ListExports registration, paging and token refusal above are the next layer and are not covered by them.

## Evidence
Divergence 4 (UpdateContinuousBackups, 200->200): $.ContinuousBackupsDescription.PointInTimeRecoveryDescription.PointInTimeRecoveryWindowDays missing (expected 35), cluster of 10.
Divergence 18 (DescribeContinuousBackups, 200->200): same member missing (expected 35), cluster of 5.
Divergence 25 (ListExports, 200->200): $.ExportSummaries[0] missing (expected {DestinationType: S3, ExportArn: <export arn>, ExportStatus: IN_PROGRESS, ExportType: FULL_EXPORT}), cluster of 3.
Divergence 5 (ListExports, 200->200): $.ExportSummaries[0] missing and $.NextToken missing on a MaxResults-limited page.
Divergence 6 (ListExports, 400->400): $.message expected "Invalid Request: 1 validation error detected: Value 'zzzz...' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ([0-9a-f]{16})+", got 'Invalid NextToken'.
Divergence 11 (RestoreTableToPointInTime, 400->400): expected "RestoreDateTime '<t>' must be within EarliestRestorableDateTime '<e>' and LatestRestorableDateTime '<l>' for table '<name>'", got 'An invalid restore time was specified. RestoreDateTime must be between EarliestRestorableDateTime and LatestRestorableDateTime' (cluster of 3).
Model: smithy_query(operation=ListExports) -> output ExportSummaries + NextToken, paginated{inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults}, MaxResults @range 1..25; smithy_query(shape=PointInTimeRecoveryDescription) -> PointInTimeRecoveryStatus, RecoveryPeriodInDays @range 1..35, EarliestRestorableDateTime, LatestRestorableDateTime.

## Steps to Reproduce
1. Replay ddbctl-backup-on-creating-table-and-restore-name-collision#task#14 and dynamodb-backup-lifecycle#task#11: enable PITR, then read it back -> PointInTimeRecoveryWindowDays absent from both the Update and the Describe response.
2. Replay dynamodb-export-lifecycle#task#9: ExportTableToPointInTime then ListExports -> no ExportSummaries.
3. Replay ddbctl-list-exports-paginated-by-table-arn#task#10: ListExports with MaxResults -> no summary and no NextToken.
4. Replay ddbctl-list-exports-paginated-by-table-arn#task#14: ListExports with a malformed NextToken -> message 'Invalid NextToken' instead of the nextToken pattern constraint message.
5. Replay ddbctl-restore-time-outside-pitr-window#task#6: RestoreTableToPointInTime outside the window -> generic message with no bounds or table name.
