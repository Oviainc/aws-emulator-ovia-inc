Issue-Source: trace-sweep-divergence
Issue-Trace: es-domain-creation-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-domain-creation-validation-errors opensearch-domain-creation-and-deletion-errors opensearch-domain-lifecycle-and-config-inspection es-application-extension-errors opensearch-insight-feedback-invalid-thumbs-enum opensearch-insight-feedback-invalid-thumbs-validation es-direct-query-datasource-errors opensearch-saved-objects-migration-attempt opensearch-describe-offering-invalid-id-format-error es-engine-version-and-instance-catalog es-index-and-domain-datasource-errors opensearch-domain-maintenance-and-vpc-endpoint-listing opensearch-list-insights-domain-not-found es-application-defaults-and-insights-errors
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateDomain|4xx->4xx|$.message
Issue-Key: es|GetApplication|4xx->4xx|$.message
Issue-Key: es|ListMigrations|4xx->4xx|$.message
Issue-Key: es|GetCapability|4xx->4xx|$.message
Issue-Key: es|InsightFeedback|4xx->4xx|$.message
Issue-Key: es|AddDirectQueryDataSource|4xx->4xx|$.message
Issue-Key: es|StartMigration|4xx->4xx|$.message
Issue-Key: es|DescribeReservedInstanceOfferings|4xx->4xx|$.message
Issue-Key: es|DescribeInstanceTypeLimits|4xx->4xx|$.message
Issue-Key: es|ListInstanceTypeDetails|4xx->4xx|$.message
Issue-Key: es|GetDirectQueryDataSource|4xx->4xx|$.message
Issue-Key: es|DeleteDirectQueryDataSource|4xx->4xx|$.message
Issue-Key: es|GetDataSource|4xx->4xx|$.message
Issue-Key: es|DeleteDataSource|4xx->4xx|$.message
Issue-Key: es|ListDomainMaintenances|4xx->4xx|$.message
Issue-Key: es|ListInsights|4xx->4xx|$.message
Issue-Key: es|ListDataSourceAttachments|4xx->4xx|$.message
Issue-Key: es|UpdateApplication|4xx->4xx|$.message
Issue-Key: es|DeleteApplication|4xx->4xx|$.message
Issue-Key: es|AttachDataSource|4xx->4xx|$.message
Issue-Key: es|DescribeDataSourceAttachment|4xx->4xx|$.message
Issue-Key: es|DetachDataSource|4xx->4xx|$.message
Issue-Key: es|DeregisterCapability|4xx->4xx|$.message
Issue-Key: es|UpdateDirectQueryDataSource|4xx->4xx|$.message
Issue-Key: es|AddDataSource|4xx->4xx|$.message
Issue-Key: es|UpdateDataSource|4xx->4xx|$.message
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: constraint-violation refusal messages are emulator-composed, with wrong patterns, ordering and enum sets

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
10 net-new root causes, all in the layer that renders a 400 ValidationException body for this service. Status (400) and error code (ValidationException) are already right in every one; the message text is wrong.

AWS: the refusal message for a constraint violation is a fixed template: "<N> validation error(s) detected: " followed by "; "-joined clauses, each "Value '<the request's value>' at '<member path>' failed to satisfy constraint: Member must <constraint phrase>". The variable parts (value, member path, the pattern or value set) come from the request and the model; the rest is fixed text. The pattern is echoed as the model's literal @pattern string, NOT a normalized or re-anchored form.

Cause 1 of 10 - pattern anchors are added. AWS echoes the pattern with its leading ^ and trailing $ stripped, e.g. "Member must satisfy regular expression pattern: [a-z][a-z0-9\\-]+" for DomainName (Smithy: DomainName @pattern "^[a-z][a-z0-9\\-]+$", @length 3..28). Emulator emits "^[a-z][a-z0-9\\-]+$". Same defect for dataSourceName/name (@pattern "^[a-z][a-z0-9_]+$" -> AWS prints [a-z][a-z0-9_]+) and for ListInsights entity.value (AWS prints ([a-z][a-z0-9\\-]+|\\d{12})). Traces: es-domain-creation-validation-errors#task#1 (representative), es-direct-query-datasource-errors#task#3, es-direct-query-datasource-errors#task#5, es-index-and-domain-datasource-errors#task#6, es-index-and-domain-datasource-errors#task#8, opensearch-domain-maintenance-and-vpc-endpoint-listing#task#1, opensearch-list-insights-domain-not-found#task#1.

Cause 2 of 10 - the application `id` pattern is stale. AWS validates application ids against ^(?!-)[a-z0-9\\-]{3,30}$ and prints that pattern. Smithy's Id member says @pattern "^[a-z0-9]{3,30}$", which is what the emulator prints; the recordings show the service has moved on, so the recording is the requirement. Traces: es-application-extension-errors#task#1 (GetApplication), es-application-defaults-and-insights-errors#task#3, es-application-extension-errors#task#2/#task#3/#task#6/#task#7/#task#8.

Cause 3 of 10 - `applicationId` reports only one of its two failures. AWS reports both the pattern clause (pattern printed as [a-z0-9-]{3,30}) and a second clause "Member must have length less than or equal to 30", so the header reads "2 validation errors detected". The emulator reports one clause and "1 validation error detected". Trace: es-application-extension-errors#task#10 (ListMigrations).

Cause 4 of 10 - multi-error clause ordering. AWS orders the clauses by the operation's input-member declaration order (capabilityName before applicationId for GetCapability/DeregisterCapability) and the emulator orders them by its own evaluation order, so even when the clause set is right the message differs. Traces: es-application-extension-errors#task#4 (GetCapability), es-application-extension-errors#task#5 (DeregisterCapability).

Cause 5 of 10 - enum value-set ordering. AWS prints "Member must satisfy enum value set: [Down, Up]" for InsightFeedback.Thumbs; the emulator prints [Up, Down]. Trace: opensearch-insight-feedback-invalid-thumbs-enum#task#1.

Cause 6 of 10 - the role-ARN check must win over the generic constraint report. For AddDirectQueryDataSource / UpdateDirectQueryDataSource / AddDataSource / UpdateDataSource with an unparseable IAM role ARN, AWS answers the single fixed message "Invalid Role Arn : <the roleArn from the request>" and does not emit a constraint report at all. The emulator instead emits a 2-clause framework report naming dataSourceName and dataSourceType.*.roleArn. Traces: es-direct-query-datasource-errors#task#2, es-direct-query-datasource-errors#task#4, es-index-and-domain-datasource-errors#task#5, es-index-and-domain-datasource-errors#task#7.

Cause 7 of 10 - StartMigration validates only applicationId. AWS reports three clauses: migrationOptions.workspace.type against the enum value set [ALL, OBSERVABILITY, ESSENTIALS, SECURITY_ANALYTICS, SEARCH], migrationOptions.source.datasourceArn against arn:aws[a-z\\-]*:aoss:[a-z0-9\\-]+:[0-9]{12}:collection/[a-z0-9]+, and applicationId against [a-z0-9]+. The emulator reports only applicationId, against its own ^[a-z0-9]{3,30}$. Trace: opensearch-saved-objects-migration-attempt#task#1.

Cause 8 of 10 - GUID members get a hand-written message. For a malformed reservedInstanceOfferingId AWS emits the standard constraint report with pattern \\p{XDigit}{8}-\\p{XDigit}{4}-\\p{XDigit}{4}-\\p{XDigit}{4}-\\p{XDigit}{12} (Smithy: GUID @pattern, @length 36..36); the emulator emits its own prose "ReservedInstanceOfferingId must be a GUID of the form xxxxxxxx-...: <value>". Trace: opensearch-describe-offering-invalid-id-format-error#task#1.

Cause 9 of 10 - DescribeInstanceTypeLimits instanceType enum set. AWS prints the full modeled enum value set for instanceType in the service's own order; the emulator prints a different set/order. Trace: es-engine-version-and-instance-catalog#task#5.

Cause 10 of 10 - ListInstanceTypeDetails unsupported-version message. The fixed text "Unsupported Version: <requested>, allowed Versions are: [...]" is right but the version list must be newest-first (OpenSearch_3.7 ... Elasticsearch_1.5); the emulator lists it oldest-first. Trace: es-engine-version-and-instance-catalog#task#6.

Why: clients and test suites assert on these messages, and operators read them; a client that parses the member path or the echoed pattern out of the message (a common pattern for surfacing field-level errors in a UI) gets a path/pattern that does not match what the service documents, and a client counting "N validation errors detected" sees the wrong count.

Fix: the service's request-validation / ValidationException message builder (pattern echo, clause ordering, clause completeness, enum-set rendering) plus the per-operation validators named in causes 6, 7 and 9 where a semantic check must precede or replace the framework report.

## Evidence
Cause 1: expected "1 validation error detected: Value 'Sim2Real_Probe_BAD' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z][a-z0-9\\-]+"; actual "... pattern: ^[a-z][a-z0-9\\-]+$".
Cause 2: expected "... at 'id' ... pattern: ^(?!-)[a-z0-9\\-]{3,30}$"; actual "... pattern: ^[a-z0-9]{3,30}$".
Cause 3: expected "2 validation errors detected: ... pattern: [a-z0-9-]{3,30}; Value ... at 'applicationId' failed to satisfy constraint: Member must have length less than or equal to 30"; actual "1 validation error detected: ... pattern: ^[a-z0-9]{3,30}$".
Cause 4: expected clause order capabilityName then applicationId (3 clauses); actual applicationId then capabilityName (2 clauses).
Cause 5: expected "enum value set: [Down, Up]"; actual "[Up, Down]".
Cause 6: expected "Invalid Role Arn : arn:aws:iam::role/<role>"; actual a 2-clause constraint report.
Cause 7: expected "3 validation errors detected: ...workspace.type...; ...datasourceArn...; ...applicationId..."; actual "1 validation error detected: ...applicationId...".
Cause 8: expected the standard constraint report with the \\p{XDigit} pattern; actual "ReservedInstanceOfferingId must be a GUID of the form xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx: <value>".
Cause 10: expected allowed-version list newest-first starting OpenSearch_3.7; actual oldest-first starting Elasticsearch_1.5.
All clusters: status 400 and x-amzn-errortype ValidationException already match; only $.message diverges.

## Steps to Reproduce
1. Replay es-domain-creation-validation-errors step task#1: CreateDomain with DomainName 'Sim2Real_Probe_BAD' and compare $.message against the recording.
2. Replay es-application-extension-errors step task#1 (GetApplication with id '00000000-0000-0000-0000-000000000000') for the stale id pattern, and task#4/#task#10 for the clause-count and ordering causes.
3. Replay es-direct-query-datasource-errors step task#2 for the 'Invalid Role Arn : <arn>' precedence, opensearch-insight-feedback-invalid-thumbs-enum step task#1 for the enum order, and opensearch-saved-objects-migration-attempt step task#1 for the missing StartMigration clauses.
