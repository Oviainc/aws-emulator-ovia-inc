Issue-Source: trace-sweep-divergence
Issue-Trace: logs-integration-and-s3table-notfound-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-integration-and-s3table-notfound-errors logs-integration-setup-and-s3table-source-errors logs-s3-table-integration-associate-source-error logs-s3-table-integration-association-error-workflow logs-s3-table-integration-edge-cases logs-s3-table-integration-source-association-errors logs-s3-table-integration-source-association-not-found logs-s3-table-integration-source-error-paths logs-s3-table-integration-sources logs-s3table-integration-not-found-workflow logs-b2-lookup-tables-and-integrations logs-s3table-associate-malformed-arn logs-integration-lifecycle logs-integration-lifecycle-error-paths logs-put-integration-validation-errors logs-delete-integration-nonexistent-name logs-put-integration-invalid-retention-error logs-put-integration-malformed-config-error logs-put-integration-malformed-role-arn-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|AssociateSourceToS3TableIntegration|4xx->2xx|$.__type
Issue-Key: logs|AssociateSourceToS3TableIntegration|4xx->2xx|$.message
Issue-Key: logs|AssociateSourceToS3TableIntegration|4xx->2xx|status
Issue-Key: logs|ListSourcesForS3TableIntegration|4xx->2xx|$.__type
Issue-Key: logs|ListSourcesForS3TableIntegration|4xx->2xx|$.message
Issue-Key: logs|ListSourcesForS3TableIntegration|4xx->2xx|status
Issue-Key: logs|AssociateSourceToS3TableIntegration|4xx->4xx|$.message
Issue-Key: logs|ListIntegrations|2xx->2xx|$.integrations
Issue-Key: logs|GetIntegration|4xx->4xx|$.message
Issue-Key: logs|DeleteIntegration|4xx->4xx|$.message
Issue-Key: logs|PutIntegration|2xx->2xx|$.integrationConfig
Issue-Key: logs|GetIntegration|2xx->2xx|$.integration
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.accessPolicy.status.statusMessage
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.application.applicationEndpoint
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.application.status.statusMessage
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.collection.collectionEndpoint
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.collection.collectionName
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.collection.status.statusMessage
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.dataSource.status.statusMessage
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.encryptionPolicy.status.statusMessage
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.lifecyclePolicy.status.statusMessage
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.networkPolicy.status.statusMessage
Issue-Key: logs|GetIntegration|2xx->2xx|$.integrationDetails.openSearchIntegrationDetails.workspace.status.statusMessage
Issue-Key: logs|GetIntegration|2xx->2xx|$.name
Issue-Key: logs|GetIntegration|2xx->2xx|$.status
Issue-Key: logs|GetIntegration|2xx->2xx|$.type
Issue-Key: logs|ListIntegrations|2xx->2xx|$.integrationSummaries[*]
Issue-Key: logs|PutIntegration|4xx->4xx|$.message
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs integrations and S3-table sources skip validation and drop response members (10 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateSourceToS3TableIntegration

## Description
Ten root causes on the integrations / S3-table-integration surface: input validation that AWS performs is absent or wrong, and the integration read paths omit response members AWS always sends. Cause count: 10 — all ten must be closed. The open behavioral issue 20261029T182816Z-logs-ten-cloudwatch-logs-refusal-messages-are-emulator.md (cause 6) describes the same integration not-found wording from behavioral checks; the wire steps below are not covered by its replay keys.

=== Cause 1 of 10: AssociateSourceToS3TableIntegration does not validate the dataSource source name ===
AWS: refuses with HTTP 400 ValidationException and the fixed text 'Source name must only contain letters, numbers, and underscores' (no variable parts) when the dataSource's source name contains any other character. Smithy: AssociateSourceToS3TableIntegration binds ValidationException (httpError 400); the character rule is recorded behavior.
Emulator: answers HTTP 200 and associates the source.
Why: the client believes an illegally-named source was associated; on AWS the association does not exist, so every later ListSourcesForS3TableIntegration diverges. Ten fixtures hit this one check.
Fix: the dataSource name validation branch of the AssociateSourceToS3TableIntegration handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-integration-and-s3table-notfound-errors#task#2.

=== Cause 2 of 10: ListSourcesForS3TableIntegration accepts an ARN that is not an s3tableintegration ARN ===
AWS: refuses with HTTP 400 ValidationException whose message is 'Invalid S3 table integration ARN format. Expected: s3tableintegration/{integration-id}: <integrationArn>' — fixed text plus one variable part, the integrationArn from the request. Smithy: ListSourcesForS3TableIntegration.integrationArn is a required Arn and the op binds ValidationException (httpError 400).
Emulator: answers HTTP 200.
Why: passing a plain integration ARN silently 'works', so a caller never learns it must use the s3tableintegration ARN form.
Fix: the integrationArn shape check in the ListSourcesForS3TableIntegration handler.
Traces: logs-b2-lookup-tables-and-integrations#task#6.

=== Cause 3 of 10: AssociateSourceToS3TableIntegration's too-short-ARN refusal is emulator-authored ===
AWS: a too-short integrationArn is refused with the standard constraint message "1 validation error detected: Value at 'integrationArn' failed to satisfy constraint: Member must have length greater than or equal to 20" — fixed template whose variable parts are the member name and the bound.
Emulator: '1 validation error detected: Unable to parse integrationArn ' — the constraint template is replaced by a parse complaint.
Why: the status and code are right, but tooling that parses the standard constraint message to report which member and which bound failed gets nothing usable.
Fix: the integrationArn length validation in the AssociateSourceToS3TableIntegration handler — use the shared constraint renderer before attempting to parse.
Traces: logs-s3-table-integration-association-error-workflow#task#2.

=== Cause 4 of 10: ListIntegrations omits the `integrations` member when there are none ===
AWS: the response carries integrations as an EMPTY list alongside integrationSummaries (recorded behavior only — the Smithy output models only integrationSummaries, so `integrations` is an undocumented member AWS returns on the wire).
Emulator: the member is absent.
Why: clients built against the recorded wire shape dereference response.integrations and get null instead of an empty list.
Fix: the ListIntegrations serializer.
Traces: logs-b2-lookup-tables-and-integrations#task#2.

=== Cause 5 of 10: GetIntegration's not-found message does not name the integration ===
AWS: ResourceNotFoundException (HTTP 400) with 'Integration with name <integrationName> does not exist.' — fixed text plus one variable part taken from the request. Smithy: GetIntegration binds ResourceNotFoundException (httpError 400).
Emulator: 'The specified integration does not exist.'
Why: a caller managing several integrations cannot tell which name was missing.
Fix: the not-found branch of the GetIntegration handler.
Traces: logs-b2-lookup-tables-and-integrations#task#4.

=== Cause 6 of 10: DeleteIntegration's not-found message does not name the integration ===
AWS: same text as cause 5, 'Integration with name <integrationName> does not exist.'
Emulator: 'The specified integration does not exist.' (six fixtures).
Why: same; delete-if-exists loops cannot attribute the failure.
Fix: the not-found branch of the DeleteIntegration handler.
Traces: logs-b2-lookup-tables-and-integrations#task#5.

=== Cause 7 of 10: PutIntegration omits the `integrationConfig` member ===
AWS: the PutIntegration response carries integrationConfig (an empty structure when the integration has no extra config) in addition to integrationName/integrationStatus — recorded behavior only; the Smithy output models only integrationName and integrationStatus.
Emulator: the member is absent.
Why: a client reading back the accepted configuration gets null and cannot confirm what was stored.
Fix: the PutIntegration serializer.
Traces: logs-integration-lifecycle#task#3.

=== Cause 8 of 10: GetIntegration omits the whole OPENSEARCH integration projection ===
AWS: returns, for an ACTIVE OpenSearch integration, the `integration` structure (integrationConfig.openSearchIntegrationConfig, integrationName, integrationType), the duplicate top-level `name`/`status`/`type` members, and integrationDetails.openSearchIntegrationDetails with a status.statusMessage ('' when healthy) on every sub-resource (accessPolicy, application, collection, dataSource, encryptionPolicy, lifecyclePolicy, networkPolicy, workspace) plus application.applicationEndpoint and collection.collectionEndpoint/collectionName. Recorded behavior only for `integration`/`name`/`status`/`type`; Smithy models integrationDetails: IntegrationDetails.
Emulator: all of those members are absent.
Why: a client that polls GetIntegration for ACTIVE status and then reads the application endpoint to drive dashboards has nothing to read.
Fix: the integration state record and the GetIntegration serializer.
Traces: logs-integration-lifecycle#task#4.

=== Cause 9 of 10: ListIntegrations' summaries lack the duplicate name/status/type members (and `integrations`) ===
AWS: each integrationSummaries entry carries BOTH the documented integrationName/integrationType/integrationStatus and the undocumented name/status/type aliases, and the response also carries the `integrations` list with the full integration records (recorded behavior only).
Emulator: only the three documented members per summary, and no `integrations` list.
Why: clients reading summary.name/status/type (as the recorded wire allows) see nothing.
Fix: the ListIntegrations serializer (same component as cause 4).
Traces: logs-integration-lifecycle#task#5.

=== Cause 10 of 10: PutIntegration enforces the wrong retentionDays upper bound ===
AWS: refuses retentionDays above 3650 with "1 validation error detected: Value at 'resourceConfig.openSearchResourceConfig.retentionDays' failed to satisfy constraint: Member must have value less than or equal to 3650" — the variable parts are the member path and the bound, and the recorded bound is 3650. Note the Smithy model declares OpenSearchResourceConfig.retentionDays @range min 1 max 30; the recorded service enforces 3650, so the recording is the wire truth and the model's bound is stale.
Emulator: refuses with the same template but the bound 30, so values in 31..3650 that AWS accepts are rejected.
Why: a client requesting a realistic retention (e.g. several hundred days) is refused outright, and the refusal advertises a limit AWS does not have.
Fix: the retentionDays range check in the PutIntegration handler — use the service's 3650 bound rather than the model trait.
Traces: logs-put-integration-invalid-retention-error#task#1.

## Evidence
Cause 1: "status: expected 400, got 200; $.__type: missing (expected 'ValidationException', got None); $.message: missing (expected 'Source name must only contain letters, numbers, and underscores', got None)" — 10 affected steps.
Cause 2: "status: expected 400, got 200; $.message: missing (expected 'Invalid S3 table integration ARN format. Expected: s3tableintegration/{integration-id}: <integrationArn>', got None)" — the ARN is the variable part.
Cause 3: "$.message: value_mismatch (expected \"1 validation error detected: Value at 'integrationArn' failed to satisfy constraint: Member must have length greater than or equal to 20\", got '1 validation error detected: Unable to parse integrationArn ')".
Cause 4: "$.integrations: missing (expected [], got None)" at HTTP 200 -> 200 (4 affected steps).
Cause 5: "$.message: value_mismatch (expected 'Integration with name <name> does not exist.', got 'The specified integration does not exist.')" at 400 -> 400.
Cause 6: same mismatch on DeleteIntegration, 6 affected steps.
Cause 7: "$.integrationConfig: missing (expected {}, got None)".
Cause 8: "$.integration: missing (expected {'integrationConfig': {'openSearchIntegrationConfig': {}}, 'integrationName': <name>, 'integrationType': 'OPENSEARCH'}, got None); $.integrationDetails.openSearchIntegrationDetails.<accessPolicy|application|collection|dataSource|encryptionPolicy|lifecyclePolicy|networkPolicy|workspace>.status.statusMessage: missing (expected '', got None); $.integrationDetails.openSearchIntegrationDetails.application.applicationEndpoint: missing; $.name / $.status ('ACTIVE') / $.type ('OPENSEARCH'): missing" — the endpoint host and the integration name are per-run values.
Cause 9: "$.integrationSummaries[0]: type_mismatch (expected {... 'name': <name>, 'status': 'ACTIVE', 'type': 'OPENSEARCH'}, got {'integrationName': <name>, 'integrationType': 'OPENSEARCH', 'integrationStatus': 'ACTIVE'}); $.integrations: missing (expected [{...}], got None)".
Cause 10: "$.message: value_mismatch (expected \"... Member must have value less than or equal to 3650\", got \"... Member must have value less than or equal to 30\")" — 4 affected steps.

## Steps to Reproduce
1: AssociateSourceToS3TableIntegration with a dataSource whose source name contains a '-' or '.' — expect 400 ValidationException 'Source name must only contain letters, numbers, and underscores' (logs-integration-and-s3table-notfound-errors#task#2).
2: ListSourcesForS3TableIntegration(integrationArn='arn:aws:logs:<region>:<acct>:integration:<name>') — expect 400 ValidationException naming the expected s3tableintegration/{integration-id} form (logs-b2-lookup-tables-and-integrations#task#6).
3: AssociateSourceToS3TableIntegration(integrationArn='arn:aws') — expect the standard length-constraint message for 'integrationArn' (logs-s3-table-integration-association-error-workflow#task#2).
4: ListIntegrations with no integrations present — expect integrations: [] in the body (logs-b2-lookup-tables-and-integrations#task#2).
5/6: GetIntegration / DeleteIntegration with an unknown integrationName — expect 'Integration with name <name> does not exist.' (#task#4, #task#5).
7/8/9: PutIntegration(integrationType='OPENSEARCH'), then GetIntegration and ListIntegrations — assert integrationConfig on the put response, and the integration/name/status/type/integrationDetails projection and `integrations` list on the reads (logs-integration-lifecycle#task#3, #task#4, #task#5).
10: PutIntegration with resourceConfig.openSearchResourceConfig.retentionDays = 4000 — expect the constraint message to name the 3650 bound; also verify retentionDays = 365 is ACCEPTED (logs-put-integration-invalid-retention-error#task#1).
