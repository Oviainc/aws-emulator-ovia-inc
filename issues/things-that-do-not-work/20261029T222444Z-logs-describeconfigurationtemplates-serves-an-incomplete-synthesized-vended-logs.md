Issue-Source: trace-sweep-divergence
Issue-Trace: logs-catalog-describe-configuration-templates
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-catalog-describe-configuration-templates
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeConfigurationTemplates|2xx->2xx|$.configurationTemplates[*]
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs DescribeConfigurationTemplates serves an incomplete, synthesized vended-logs template catalog (10 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationTemplates

## Description
DescribeConfigurationTemplates is a read-only AWS-managed catalog: for a given `service` (and optional logTypes/resourceTypes/deliveryDestinationTypes filters) it returns the fixed set of ConfigurationTemplate entries that AWS publishes for vended log delivery. The emulator does not carry that catalog. For most services it returns an EMPTY list (or far fewer entries than AWS), and where it does return an entry the entry is a synthesized CloudWatch-Logs-shaped placeholder (deliveryDestinationType 'CWL', allowedFields fixed to the five @-prefixed Insights fields @ingestionTime/@log/@logStream/@message/@timestamp, allowedOutputFormats ['json'], no allowedFieldDelimiters, no allowedSuffixPathFields, no s3TablesIntegration, defaultDeliveryConfigValues.recordFields set to those same five fields) rather than the per-service/per-logType template AWS publishes.

Smithy: DescribeConfigurationTemplates input {service, logTypes, resourceTypes, deliveryDestinationTypes, nextToken, limit}, output configurationTemplates: ConfigurationTemplates, paginated by nextToken/limit. The model does not enumerate the catalog contents, so the required contents are recorded behavior only — the fixtures record what AWS returns.

Cause count: 10. Each cause below is one filter/service slice of the catalog that the emulator answers wrongly; all ten must be closed. The general requirement in every case: for the queried service/logType/resourceType/destination-type combination the emulator must return the SAME template entries AWS returns, each with the real allowedFields (and their per-field `mandatory` flags), allowedFieldDelimiters ['\t',' ',','], the real allowedOutputFormats (plain/json/w3c/parquet subsets per destination type), allowedSuffixPathFields, defaultDeliveryConfigValues (fieldDelimiter plus the ordered recordFields, and s3DeliveryConfiguration.suffixPath/enableHiveCompatiblePath for S3 destinations), the allowedActionForAllowVendedLogsDeliveryForResource of the form '<service>:AllowVendedLogDeliveryForResource', the s3TablesIntegration block where AWS publishes one, and one entry per (logType, resourceType, deliveryDestinationType) triple. Concrete field lists in the Evidence section are the recorded catalog, not per-run values.

=== Cause 1 of 10: catalog slice for service=aidevops (and the unfiltered first page) ===
AWS: returns the aidevops templates, e.g. logType-bearing entries with mandatory resource_arn/event_timestamp plus the optional_* field family, allowedOutputFormats ['plain','json','w3c','parquet'], allowedSuffixPathFields ['accountid','region','resource_id','yyyy','MM','dd','HH'] and allowedActionForAllowVendedLogsDeliveryForResource 'aidevops:AllowVendedLogDeliveryForResource'.
Emulator: configurationTemplates[0] is missing entirely (six steps of this fixture read the same slice).
Why: a client enumerating what it may configure for aidevops delivery sees no template at all and concludes vended delivery is unsupported.
Fix: the static catalog behind the DescribeConfigurationTemplates handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-catalog-describe-configuration-templates#task#1 (also task#6, task#13, task#17, task#21, task#24).

=== Cause 2 of 10: catalog slice for service=apigateway ===
AWS: three templates for resourceType 'stages'/logType 'EXECUTION_LOGS' across S3/CWL/FH destinations, allowedFields timestamp/resource_arn/resource_id/event_timestamp/api_id/stage/resource_path/http_method plus mandatory 'payload', allowedSuffixPathFields including api_id and stage, s3 suffixPath '{region}/{api_id}/{stage}/'.
Emulator: all three entries missing.
Why: API Gateway execution-log delivery cannot be configured from the catalog.
Fix: same catalog.
Traces: logs-catalog-describe-configuration-templates#task#2 (also task#7, task#8, task#9, task#12, task#15, task#16, task#23, task#25, task#26, task#28, task#29).

=== Cause 3 of 10: catalog slice for service=autoloop ===
AWS: 21 templates (ERROR_LOGS on resourceType 'subscription' etc.) with the transformer_arn/object_identifier/event_identifier/trace_identifier/log_message field family.
Emulator: all 21 entries missing.
Why: same as above for autoloop.
Fix: same catalog.
Traces: logs-catalog-describe-configuration-templates#task#3 (also task#22).

=== Cause 4 of 10: catalog slice for service=bedrock ===
AWS: 17 templates (EVENT_LOGS on resourceType 'agent-alias' etc.) whose allowedFields are all mandatory (timestamp, resourceId, traceId, spanId, sessionId, requestId, operation, attributes, body, eventType, eventVersion, eventName, level).
Emulator: all entries missing.
Why: Bedrock vended-log delivery is unconfigurable.
Fix: same catalog.
Traces: logs-catalog-describe-configuration-templates#task#4.

=== Cause 5 of 10: catalog slice for service=bedrock-agentcore ===
AWS: 48 templates (APPLICATION_LOGS on resourceType 'workload-identity' etc.) with the OTel-style field family (timeUnixNano, severityNumber, severityText, body, traceId, spanId) and allowedSuffixPathFields including workload_identity_id.
Emulator: all entries missing.
Why: same.
Fix: same catalog.
Traces: logs-catalog-describe-configuration-templates#task#5.

=== Cause 6 of 10: catalog slice for service=eks ===
AWS: 31 templates (e.g. AUTO_MODE_LOAD_BALANCING_LOGS on resourceType 'cluster') with mandatory resource_arn/event_timestamp and the controller/reconcileID field family, allowedSuffixPathFields including cluster_id.
Emulator: all entries missing.
Why: same.
Fix: same catalog.
Traces: logs-catalog-describe-configuration-templates#task#10.

=== Cause 7 of 10: catalog slice for service=elasticloadbalancing ===
AWS: 12 templates including ALB_CONNECTION_LOGS on resourceType 'loadbalancer' with the TLS connection-log field set (all mandatory) and an s3TablesIntegration {datasourceName 'aws_alb', datasourceType 'connection'}.
Emulator: returns a single synthesized CWL ALB_ACCESS_LOGS placeholder whose allowedFields are the five @-prefixed Insights fields, and omits the other 11 entries, the s3TablesIntegration block and every S3-destination variant.
Why: the one entry a client does see advertises fields that are not the ALB log schema, so a CreateDelivery built from it specifies recordFields AWS will reject.
Fix: same catalog.
Traces: logs-catalog-describe-configuration-templates#task#11 (also task#20).

=== Cause 8 of 10: catalog slice for service=events ===
AWS: 18 templates (ERROR_LOGS on resourceType 'subscriber' etc.) with the subscriber_name/event_id/invocation_id/message_type/log_level field family and allowedSuffixPathFields including subscriber_name.
Emulator: all entries missing.
Why: same.
Fix: same catalog.
Traces: logs-catalog-describe-configuration-templates#task#14.

=== Cause 9 of 10: catalog slice for service=mediatailor ===
AWS: 9 templates with the MediaTailor ad-insertion field family (eventTimestamp/awsAccountId mandatory, vast/ads/beacon fields optional).
Emulator: all entries missing.
Why: same.
Fix: same catalog.
Traces: logs-catalog-describe-configuration-templates#task#18 (also task#19, task#27).

=== Cause 10 of 10: catalog slice for service=workmail ===
AWS: 15 templates for ACCESS_CONTROL_LOGS on resourceType 'organization' across destination types, with the organization_arn/user_id/protocol/source_ip/scope/access_granted field family (several mandatory) and S3 suffixPath '{region}/{yyyy}/{MM}/{dd}/{HH}/'.
Emulator: one synthesized CWL placeholder with the five @-prefixed Insights fields and allowedOutputFormats ['json']; the other 14 entries missing.
Why: same as cause 7 — the advertised field set is not WorkMail's.
Fix: same catalog.
Traces: logs-catalog-describe-configuration-templates#task#30.

## Evidence
All ten clusters replay at HTTP 200 -> 200 with body divergences only; the status and error-code contract is correct and only the catalog content is wrong.
Cause 1: "$.configurationTemplates[0]: missing (expected {'allowedActionForAllowVendedLogsDeliveryForResource': 'aidevops:AllowVendedLogDeliveryForResource', 'allowedFieldDelimiters': ['\t', ' ', ','], 'allowedFields': [{'mandatory': False, 'name': 'timestamp'}, {'mandatory': True, 'name': 'resource_arn'}, ...], 'allowedOutputFormats': ['plain', 'json', 'w3c', 'parquet'], 'allowedSuffixPathFields': ['accountid', 'region', 'resource_id', 'yyyy', 'MM', 'dd', 'HH'], ...}, got None)" — divergence keys cover configurationTemplates[0..5].
Cause 2: same 'missing' verdict on configurationTemplates[0..2] for the apigateway stages/EXECUTION_LOGS templates (expected s3DeliveryConfiguration.suffixPath '{region}/{api_id}/{stage}/').
Cause 3: 'missing' on configurationTemplates[0..20] for autoloop.
Cause 4: 'missing' on configurationTemplates[0..16] for bedrock.
Cause 5: 'missing' on configurationTemplates[0..47] for bedrock-agentcore.
Cause 6: 'missing' on configurationTemplates[0..30] for eks.
Cause 7: "expected {... 'logType': 'ALB_CONNECTION_LOGS', 'resourceType': 'loadbalancer', 's3TablesIntegration': {'datasourceName': 'aws_alb', 'datasourceType': 'connection'}, 'service': 'elasticloadbalancing'}, got {'service': 'elasticloadbalancing', 'logType': 'ALB_ACCESS_LOGS', 'resourceType': 'loadbalancer', 'deliveryDestinationType': 'CWL', 'defaultDeliveryConfigValues': {'recordFields': ['@ingestionTime', '@log', '@logStream', '@message', '@timestamp']}, ...}" — keys configurationTemplates[0..11].
Cause 8: 'missing' on configurationTemplates[0..17] for events.
Cause 9: 'missing' on configurationTemplates[0..8] for mediatailor.
Cause 10: "expected {... 'allowedFields': [{'mandatory': True, 'name': 'organization_arn'}, ...], 'allowedOutputFormats': ['plain', 'json', 'w3c', 'parquet'] ...}, got {... 'allowedFields': [{'name': '@ingestionTime', 'mandatory': False}, ... {'name': '@timestamp', 'mandatory': True}], 'allowedOutputFormats': ['json'] ...}" — keys configurationTemplates[0..14].

## Steps to Reproduce
For each cause call logs DescribeConfigurationTemplates with the corresponding filter and compare the returned list, element by element, against the recorded AWS catalog:
1. DescribeConfigurationTemplates(service='aidevops') — replay logs-catalog-describe-configuration-templates#task#1.
2. service='apigateway' — #task#2.
3. service='autoloop' — #task#3.
4. service='bedrock' — #task#4.
5. service='bedrock-agentcore' — #task#5.
6. service='eks' — #task#10.
7. service='elasticloadbalancing' — #task#11.
8. service='events' — #task#14.
9. service='mediatailor' — #task#18.
10. service='workmail' — #task#30.
In each case assert the entry count, and for every entry the logType/resourceType/deliveryDestinationType triple, allowedFields (names, order and mandatory flags), allowedFieldDelimiters, allowedOutputFormats, allowedSuffixPathFields, defaultDeliveryConfigValues and s3TablesIntegration.
