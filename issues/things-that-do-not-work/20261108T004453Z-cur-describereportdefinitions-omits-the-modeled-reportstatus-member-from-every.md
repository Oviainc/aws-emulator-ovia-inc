Issue-Source: trace-sweep-divergence
Issue-Trace: cur-additional-artifacts-schema-versioning-and-name-pattern-errors
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cur-additional-artifacts-schema-versioning-and-name-pattern-errors cur-create-describe-modify-roundtrip cur-idempotent-put-vs-duplicate-name-conflict
Issue-Shape-Id: com.amazonaws.costandusagereportservice#AWSOrigamiServiceGatewayService
Issue-Key: cur|DescribeReportDefinitions|2xx->2xx|$.ReportDefinitions[*]
Issue-Commit: 06ab0242402a8d038c4677fdb4da18af84e7f2d9

# DescribeReportDefinitions omits the modeled ReportStatus member from every returned ReportDefinition

> _Note: this issue was identified against the code base at commit `06ab0242402a8d038c4677fdb4da18af84e7f2d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReportDefinitions

## Description
Cause 1 of 1: DescribeReportDefinitions serializes a stored report definition without its ReportStatus member, so no read of a report definition ever exposes the delivery status of the report.

AWS: Smithy: ReportDefinition.ReportStatus targets structure ReportStatus ("The status of the report."), whose members are lastDelivery (LastDelivery, @length min 16 max 20, @pattern ^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$, "A timestamp that gives the date of a report delivery.") and lastStatus (enum LastStatus, "An enum that gives the status of a report delivery."). The ReportStatus doc states it is "A two element dictionary with a lastDelivery and lastStatus key whose values describe the date and status of the last delivered report for a particular report definition." Recorded behavior: the service returns the ReportStatus key on every ReportDefinition it serves from DescribeReportDefinitions, including for a definition that has never had a delivery, where both members are present as empty strings (ReportStatus = {"lastDelivery": "", "lastStatus": ""}). The two-element dictionary is therefore unconditional on this read path; only the member VALUES depend on whether and when a report was last delivered.

Emulator: HTTP 200 with the right ReportDefinitions list length and every other member correct (ReportName, TimeUnit, Format, Compression, AdditionalSchemaElements, S3Bucket, S3Prefix, S3Region, AdditionalArtifacts, RefreshClosedReports, ReportVersioning, BillingViewArn, ReportCreationData), but $.ReportDefinitions[0] carries NO ReportStatus key at all. Replay detail: "$.ReportDefinitions[0]: missing (expected {... 'ReportStatus': {'lastDelivery': '', 'lastStatus': ''} ...}, got {...})" — the only difference between expected and actual object is the absent ReportStatus. The omission is unconditional: it shows up on the describe after create, after each modify, and on the describe following the duplicate-name conflict path, i.e. on every DescribeReportDefinitions response in the corpus.

Why: a client that reads the delivery status of its report through DescribeReportDefinitions — the only operation that exposes it — sees the member as absent rather than as an empty-valued two-element dictionary. Generated SDKs model ReportStatus as an optional structure, so the client cannot distinguish "never delivered" (AWS: present with empty lastDelivery/lastStatus) from "the field was not returned", and any code that reads reportStatus.lastStatus to decide whether delivery has happened dereferences a null / raises instead of observing the empty sentinel. Delivery-monitoring and report-health polling built on this read silently never progresses against the emulator.

Fix: the DescribeReportDefinitions response projection in the cost-and-usage-report-service crate — the code that maps a stored report definition to the wire ReportDefinition structure must always emit ReportStatus with both lastDelivery and lastStatus (empty-string sentinels until a delivery has occurred), which in turn means the stored definition's state must carry last-delivery/last-status fields rather than dropping them. Because the same projection is reused, apply it wherever a ReportDefinition is serialized, not only on the first list element.

Traces: cur-additional-artifacts-schema-versioning-and-name-pattern-errors#task#4; cur-create-describe-modify-roundtrip#task#4, #task#6, #task#8; cur-idempotent-put-vs-duplicate-name-conflict#task#5 (5 steps, one cluster, divergence_key service=cur|DescribeReportDefinitions|status=200->200|keys=$.ReportDefinitions[0]).

Not covered by any open issue: the open cur issues concern DescribeReportDefinitions pagination-token scope (20261104T190814Z-cost-and-usage-report-service-describereportdefinitions-pagination.md), the invalid-NextToken error class (20261105T033247Z-cost-and-usage-report-service-describereportdefinitions-error-response.md), ModifyReportDefinition rename-collision wording, and DeleteReportDefinition ResponseMessage — none of them touch the ReportDefinition response shape on the success path.

## Evidence
Step cur-additional-artifacts-schema-versioning-and-name-pattern-errors#task#4 (DescribeReportDefinitions), verdict=mismatch, expected_status=200, actual_status=200.

Expected $.ReportDefinitions[0] (recorded): {'AdditionalArtifacts': ['REDSHIFT'], 'AdditionalSchemaElements': ['RESOURCES', 'SPLIT_COST_ALLOCATION_DATA', 'MANUAL_DISCOUNT_COMPATIBILITY'], 'BillingViewArn': 'arn:aws:billing::<account-id>:billingview/primary', 'Compression': 'ZIP', 'Format': 'textORcsv', 'RefreshClosedReports': False, 'ReportCreationDate': '<shaped-not-asserted>', 'ReportName': '<report-name>', 'ReportStatus': {'lastDelivery': '', 'lastStatus': ''}, 'ReportVersioning': 'OVERWRITE_REPORT', 'S3Bucket': '<bucket>', 'S3Prefix': 'cur-reports', 'S3Region': 'us-west-2', 'TimeUnit': 'DAILY'}

Actual $.ReportDefinitions[0] (emulator): the same object WITHOUT the 'ReportStatus' key. Every other member, including ReportCreationDate, matched.

Fixed vs variable parts of the expected value: the key names 'ReportStatus', 'lastDelivery' and 'lastStatus' and the two-element shape are fixed requirements from the model; the empty-string values are the recorded sentinel for a definition with no delivery yet; the report name, bucket name, account id in BillingViewArn and the ReportCreationDate timestamp are per-run values from the request/state and are NOT requirements.

Same cluster, same missing key: cur-create-describe-modify-roundtrip#task#4, #task#6, #task#8 (describe after create and after each modify) and cur-idempotent-put-vs-duplicate-name-conflict#task#5. affected_count=5. stub_path empty, owned_by empty.

## Steps to Reproduce
1. Replay the wire corpus against the emulator built from this checkout (replay_wire_corpus).
2. Fixture cur-additional-artifacts-schema-versioning-and-name-pattern-errors, step task#4: after PutReportDefinition creates a report definition, call DescribeReportDefinitions.
3. Inspect $.ReportDefinitions[0] in the HTTP 200 body: the 'ReportStatus' key is absent. AWS returns ReportStatus as a two-element dictionary {"lastDelivery": "", "lastStatus": ""} for a definition that has not yet had a delivery.
4. Repeat with cur-create-describe-modify-roundtrip steps task#4/task#6/task#8 (describe after create and after each ModifyReportDefinition) and cur-idempotent-put-vs-duplicate-name-conflict step task#5 — ReportStatus is missing from the returned definition in every case.
