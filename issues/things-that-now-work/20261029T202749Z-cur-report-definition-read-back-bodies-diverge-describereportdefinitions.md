Issue-Source: trace-sweep-divergence
Issue-Trace: cur-additional-artifacts-schema-versioning-and-name-pattern-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cur-additional-artifacts-schema-versioning-and-name-pattern-errors cur-create-describe-modify-roundtrip cur-idempotent-put-vs-duplicate-name-conflict cur-delete-idempotency-and-post-delete-error-propagation cur-tag-untag-list-lifecycle
Issue-Shape-Id: com.amazonaws.costandusagereportservice#AWSOrigamiServiceGatewayService
Issue-Key: cur|DescribeReportDefinitions|2xx->2xx|$.ReportDefinitions[*]
Issue-Key: cur|DeleteReportDefinition|2xx->2xx|$
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# cur report-definition read-back bodies diverge: DescribeReportDefinitions omits service-populated members and DeleteReportDefinition returns a ResponseMessage AWS omits

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReportDefinitions

## Description
Two net-new root causes in the CUR emulator's SUCCESS-path response bodies: a report definition is echoed back exactly as it was stored (so every member the service itself populates is absent), and a delete answers with an output member AWS leaves out. Both are 200/200 body-shape divergences — status and operation routing are correct.

Cause 1 of 2: DescribeReportDefinitions returns only the members the caller supplied, omitting BillingViewArn, ReportStatus and ReportCreationDate.
  AWS: Smithy (com.amazonaws.costandusagereportservice, awsJson1_1) DescribeReportDefinitions -> DescribeReportDefinitionsResponse { ReportDefinitions: ReportDefinitionList, NextToken }. ReportDefinition models, besides the required create-time members, the OPTIONAL BillingViewArn (@length 0..128, @pattern ^(arn:aws(-cn)?:billing::[0-9]{12}:billingview/)?[a-zA-Z0-9_\+=\.\-@].{1,30}$ -- "The Amazon resource name of the billing view") and ReportStatus (-> ReportStatus { lastDelivery: LastDelivery @pattern ^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$, lastStatus: LastStatus enum }, "The status of the report"). Recorded behavior only for the rest: every recorded DescribeReportDefinitions of a report created WITHOUT a BillingViewArn comes back carrying (a) BillingViewArn set to the account's primary billing view, i.e. arn:aws:billing::<caller-account-id>:billingview/primary -- fixed shape, the 12-digit account id is the variable part; (b) ReportStatus present with both members empty for a report that has never had a delivery, i.e. {"lastDelivery": "", "lastStatus": ""}; and (c) ReportCreationDate, a member the Smithy model does not declare at all, whose value the fixture records as shaped-not-asserted (a per-run timestamp -- its presence is the requirement, its value is not). So AWS's read-back of a report definition is the caller's definition PLUS these three service-populated members.
  Emulator: handle() clones the stored definition verbatim (services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/describe_report_definitions.rs:168-171, `store.reports[start..end].iter().map(|report| report.definition.clone())`), and PutReportDefinition stores the input definition member for member with nothing derived (.../src/ops/put_report_definition.rs:194-209). The replayed response therefore contains exactly the 11 members the create request sent and no BillingViewArn, no ReportStatus and no ReportCreationDate.
  Why: a client that reads ReportStatus to find out whether a report has ever been delivered (the usual console/monitoring flow) gets a missing field instead of the empty two-member dictionary AWS always sends, and must special-case the emulator; a client that reads BillingViewArn to learn which billing view a report is scoped to sees none and concludes the report is unscoped, where AWS reports the primary billing view; a client that orders or ages report definitions by ReportCreationDate has no value to read. Each is a KeyError/None on a field AWS returns on every call, not an edge case.
  Fix: the DescribeReportDefinitions handler's projection of a stored report into a wire ReportDefinition, backed by the state record -- the store must hold the creation instant and the delivery status alongside the definition (crate::state::StoredReport) and the handler must project BillingViewArn (defaulted to the primary billing view of the caller's account when the create named none), ReportStatus and ReportCreationDate, instead of cloning the stored definition struct.
  Traces: cur-additional-artifacts-schema-versioning-and-name-pattern-errors#task#4 (representative); same cluster also at cur-create-describe-modify-roundtrip#task#4, #task#6, #task#8 and cur-idempotent-put-vs-duplicate-name-conflict#task#5.

Cause 2 of 2: DeleteReportDefinition answers with a ResponseMessage member where AWS returns an empty body, on both the deleted-something and the nothing-to-delete path.
  AWS: Smithy models DeleteReportDefinitionResponse { ResponseMessage: DeleteResponseMessage } (optional) and the operation's only error is ValidationException -- deleting a name the account does not hold is NOT refused. Recorded behavior only for the wire body: every recorded DeleteReportDefinition -- the one that removed an existing report AND the repeat against the already-deleted name -- answers HTTP 200 with the body {} , i.e. the optional ResponseMessage member is NOT serialized. The requirement is a 200 with an empty JSON object; there is no message text and nothing from the request is echoed.
  Emulator: .../src/ops/delete_report_definition.rs:158-168 always sets `response_message: Some(...)`, composing "Successfully deleted report <ReportName>" when the store held the report and "The report <ReportName> in the request doesn't exist." when it did not -- both emulator-authored, with the request's ReportName spliced in.
  Why: the response advertises a per-call diagnostic AWS never sends, so a client that branches on `ResponseMessage` being present (or on its text) takes a branch that can never be taken against AWS, and a client that treats the delete as idempotent sees two different bodies for the two legs where AWS sends one. It also leaks the caller's report name into a body AWS leaves empty.
  Fix: the DeleteReportDefinition handler -- return DeleteReportDefinitionOutput with `response_message: None` on both legs (keep the store mutation and the no-op-on-missing semantics, which are already correct).
  Traces: cur-additional-artifacts-schema-versioning-and-name-pattern-errors#task#7; same cluster also at cur-create-describe-modify-roundtrip#task#9, cur-delete-idempotency-and-post-delete-error-propagation#task#4 and #task#6, cur-idempotent-put-vs-duplicate-name-conflict#task#6, cur-tag-untag-list-lifecycle#task#11.

Neither cause is covered by an open issue: the only open CUR issue, 20261029T172101Z-cost-and-usage-report-service-2-net-new.md, is about Message text on DuplicateReportNameException/ResourceNotFoundException refusals and says nothing about success-path bodies. Neither cluster was reported with a stub_path or an owned_by.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; 54 steps, 22 mismatched).

Cause 1 -- divergence_key service=cur|DescribeReportDefinitions|status=200->200|keys=$.ReportDefinitions[0]; expected_status 200, actual_status 200; affected_count 5.
  Expected (AWS) $.ReportDefinitions[0]: {'AdditionalArtifacts': ['REDSHIFT'], 'AdditionalSchemaElements': ['RESOURCES', 'SPLIT_COST_ALLOCATION_DATA', 'MANUAL_DISCOUNT_COMPATIBILITY'], 'BillingViewArn': 'arn:aws:billing::000000000000:billingview/primary', 'Compression': 'ZIP', 'Format': 'textORcsv', 'RefreshClosedReports': False, 'ReportCreationDate': '<shaped-not-asserted>', 'ReportName': '<report name from the request>', 'ReportStatus': {'lastDelivery': '', 'lastStatus': ''}, 'ReportVersioning': 'OVERWRITE_REPORT', 'S3Bucket': '<bucket from the request>', 'S3Prefix': 'cur-reports', 'S3Region': 'us-west-2', 'TimeUnit': 'DAILY'}
  Actual (emulator): the same element WITHOUT BillingViewArn, ReportCreationDate and ReportStatus.
  Requirement vs per-run values: the three missing members are the requirement. The 12-digit account id inside BillingViewArn is a per-environment value (recorded as 000000000000) and the ReportCreationDate value is explicitly shaped-not-asserted; the report name, bucket, prefix, region, TimeUnit, Format, Compression and artifact/schema lists are echoes of the create request and already match.

Cause 2 -- divergence_key service=cur|DeleteReportDefinition|status=200->200|keys=$; expected_status 200, actual_status 200; affected_count 6.
  Expected (AWS) $: {}
  Actual (emulator) $: {'ResponseMessage': 'Successfully deleted report <ReportName>'} (and, on the re-delete leg of cur-delete-idempotency-and-post-delete-error-propagation, 'The report <ReportName> in the request doesn't exist.').
  Requirement vs per-run values: the requirement is the ABSENCE of the ResponseMessage member on a 200. The report name inside the emulator's text is a per-run value from the request and is not part of any requirement -- AWS sends no text at all.

Source of both behaviors:
  services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/describe_report_definitions.rs:168-171 (verbatim clone of the stored definition)
  services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/put_report_definition.rs:194-209 (stores the definition member for member, derives nothing)
  services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/delete_report_definition.rs:158-168 (always Some(response_message))

Smithy citations: ReportDefinition members BillingViewArn (@length 0..128, @pattern ^(arn:aws(-cn)?:billing::[0-9]{12}:billingview/)?[a-zA-Z0-9_\+=\.\-@].{1,30}$) and ReportStatus (-> structure ReportStatus { lastDelivery, lastStatus }); ReportCreationDate is NOT in the model (recorded behavior only); DeleteReportDefinitionResponse { ResponseMessage: DeleteResponseMessage } optional, errors [ValidationException].

## Steps to Reproduce
Cause 1 (DescribeReportDefinitions projection):
1. Point a cur client at the emulator (awsJson1_1, region us-west-2).
2. PutReportDefinition with ReportDefinition {ReportName: R, TimeUnit: DAILY, Format: textORcsv, Compression: ZIP, AdditionalSchemaElements: [RESOURCES, SPLIT_COST_ALLOCATION_DATA, MANUAL_DISCOUNT_COMPATIBILITY], S3Bucket: B, S3Prefix: cur-reports, S3Region: us-west-2, AdditionalArtifacts: [REDSHIFT], RefreshClosedReports: false, ReportVersioning: OVERWRITE_REPORT} -- i.e. WITHOUT BillingViewArn.
3. DescribeReportDefinitions().
4. Read ReportDefinitions[0]: BillingViewArn, ReportStatus and ReportCreationDate are all absent. AWS returns BillingViewArn = arn:aws:billing::<account>:billingview/primary, ReportStatus = {lastDelivery: "", lastStatus: ""} and a ReportCreationDate timestamp.
5. Replay: cur-additional-artifacts-schema-versioning-and-name-pattern-errors step task#4.

Cause 2 (DeleteReportDefinition body):
1. Create report R as above.
2. DeleteReportDefinition(ReportName=R) -> 200 with {'ResponseMessage': 'Successfully deleted report R'}; AWS answers 200 with {}.
3. DeleteReportDefinition(ReportName=R) again (now unknown) -> 200 with {'ResponseMessage': "The report R in the request doesn't exist."}; AWS again answers 200 with {}.
4. Replay: cur-additional-artifacts-schema-versioning-and-name-pattern-errors step task#7 (and the re-delete leg at cur-delete-idempotency-and-post-delete-error-propagation task#4/task#6).
