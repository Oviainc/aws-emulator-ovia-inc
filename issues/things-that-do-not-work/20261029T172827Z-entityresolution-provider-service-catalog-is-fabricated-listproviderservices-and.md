Issue-Source: trace-sweep-divergence
Issue-Trace: entityresolution-catalog-list-provider-services
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: entityresolution-catalog-list-provider-services entityresolution-provider-services-catalog
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution|ListProviderServices|2xx->2xx|$.providerServiceSummaries[*]
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.anonymizedOutput
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerComponentSchema
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerConfigurationDefinition
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerEndpointConfiguration
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerEntityOutputDefinition
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerIntermediateDataAccessConfiguration
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerJobConfiguration
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerName
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerServiceArn
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerServiceDisplayName
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerServiceName
Issue-Key: entityresolution|GetProviderService|2xx->4xx|$.providerServiceType
Issue-Key: entityresolution|GetProviderService|2xx->4xx|status
Issue-Key: entityresolution|GetProviderService|4xx->4xx|$.message
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# entityresolution provider-service catalog is fabricated: ListProviderServices and GetProviderService do not serve AWS's catalog

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListProviderServices

## Description
The entityresolution emulator serves an invented provider-service catalog instead of the catalog AWS Entity Resolution publishes, so every read of that catalog diverges. There are 3 distinct causes in this issue and ALL THREE must be closed. No open issue covers them: the three open entityresolution issues (20261022T101024Z-entityresolution-write-path-skips-validation-and-drops-schema.md, 20261022T153803Z-entityresolution-control-plane-missing-reference-duplicate-validation-no.md, 20261022T191952Z-entityresolution-rule-based-matching-jobs-never-succeed-no.md) are about the schema-mapping/matching-workflow write path, control-plane validation/authorization and the matching data plane; none of them touches the provider-service catalog.

Cause 1 of 3 — ListProviderServices returns a catalog whose entries are not AWS's provider services.
AWS: GET /providerservices (@readonly, @paginated items=providerServiceSummaries, maxResults @range 15..25, optional providerName filter) answers the service's published third-party provider catalog. Each ProviderServiceSummary carries providerName, providerServiceName, providerServiceDisplayName, providerServiceType and providerServiceArn (pattern ^arn:(aws|aws-us-gov|aws-cn):(entityresolution):<region>::providerservice/<provider>/<service>$ — note the empty account-id segment). The catalog's MEMBERSHIP is not in the model, so it is recorded behavior only: the recording shows entries keyed by lowercase provider tokens with per-capability service names, e.g. providerName 'liveramp' with providerServiceName 'assignment' (display 'Assignment Email', type ASSIGNMENT), 'assignment-pii' (display 'Assignment PII', type ASSIGNMENT) and 'idmapping' (display 'ID Mapping', type ID_MAPPING), and providerName 'transunion' with providerServiceName 'assignment' (display 'TruAudience Identity Resolution & Enrichment'). The recorded page also shows the ARN region segment coming from the catalog entry itself (one page carries both us-east-2 and us-east-1 entries), not from the caller's region — that ARN-region derivation is recorded behavior only, and the per-entry region values are catalog data, not per-run values.
Emulator: serves its own seeded list — providerName 'LiveRamp' with providerServiceName 'IDENTITY_RESOLUTION' (display 'LiveRamp Identity Resolution') and 'TRANSCODE' (display 'LiveRamp Transcode', typed ID_MAPPING), providerName 'TransUnion' with 'IDENTITY_RESOLUTION' (display 'TransUnion TruAudience Identity Resolution', typed ASSIGNMENT), etc. Every element of every entry differs: provider token casing, service name, display name, type, and the ARN (built from the request region, e.g. us-west-2, rather than from the entry).
Why: a client discovers a provider service here and then addresses it by providerName/providerServiceName or by providerServiceArn in GetProviderService, CreateIdNamespace or CreateIdMappingWorkflow. Against the emulator it discovers identifiers AWS has never heard of, so every downstream call is built on names that do not exist in AWS, and code that looks a known provider service up by name finds nothing.
Fix: the seeded provider-service catalog in the entityresolution service crate's state (crate::state::provider_services, which crate::ops::list_provider_services reads at services/service-com-amazonaws-entityresolution-awsveniceservice/src/ops/list_provider_services.rs:181-210) — seed AWS's actual provider/service tokens, display names and types, and derive each entry's ARN from the entry (including its region segment) rather than from the request region.
Traces: entityresolution-catalog-list-provider-services#task#1 and entityresolution-provider-services-catalog#task#1 (same cluster).

Cause 2 of 3 — GetProviderService answers 404 for a provider service that exists in AWS's catalog, and serves none of the modeled detail members.
AWS: GET /providerservices/{providerName}/{providerServiceName} (@readonly, providerServiceName is the provider-service ARN) answers 200 with the full provider-service descriptor. Smithy: GetProviderServiceOutput requires providerName, providerServiceName, providerServiceDisplayName, providerServiceType, providerServiceArn, providerEndpointConfiguration, anonymizedOutput (Boolean) and providerEntityOutputDefinition (Document), and may carry providerConfigurationDefinition (Document), providerJobConfiguration (Document), providerIntermediateDataAccessConfiguration, providerIdNameSpaceConfiguration and providerComponentSchema; errors include ResourceNotFoundException (404) and ValidationException (400). The recorded response for a catalog entry shows all of those populated (e.g. anonymizedOutput true, providerComponentSchema with providerSchemaAttributes, providerConfigurationDefinition as a JSON-schema document with client_id/client_secret/resolution_flow, providerEndpointConfiguration.marketplaceConfiguration, providerEntityOutputDefinition and providerIntermediateDataAccessConfiguration.requiredBucketActions).
Emulator: answers 404 for that same provider service — the ARN names an entry its fabricated catalog does not contain — so all of the output members above are absent.
Why: a client cannot read any provider service's configuration definition, endpoint configuration or entity-output definition, which is exactly what it needs before creating a provider-backed ID mapping workflow or namespace; the whole provider integration path is unreachable and the 200 branch of GetProviderService is never exercised.
Fix: the same seeded catalog plus the GetProviderService handler in the entityresolution service crate — resolve providerName/providerServiceName (ARN form) against the seeded catalog and serve the entry's full descriptor, including the required providerEndpointConfiguration, anonymizedOutput and providerEntityOutputDefinition members.
Traces: entityresolution-provider-services-catalog#task#2.

Cause 3 of 3 — GetProviderService's not-found message is not AWS's text.
AWS: for a providerName/providerServiceName pair that is not in the catalog the call fails with HTTP 404 ResourceNotFoundException whose message is the fixed string 'PartnerComponent Not Found' — the recorded message has no variable parts (it does not echo the requested provider or service name).
Emulator: status 404 and the error type are right, but the message is 'ProviderService does not exist'.
Why: clients and operators match or log this message; a different string makes the emulator's not-found indistinguishable from an arbitrary error and breaks message-based assertions written against AWS.
Fix: the error construction for GetProviderService in the entityresolution service crate (the shared not-found message helpers in crate::state::messages) — emit the fixed text 'PartnerComponent Not Found' for the catalog lookup miss.
Traces: entityresolution-provider-services-catalog#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false, 36/55 steps passed, so entityresolution is serving).

Cause 1 — fixture entityresolution-catalog-list-provider-services, step task#1 (cluster also covers entityresolution-provider-services-catalog#task#1), ListProviderServices, status 200 expected and 200 got; body mismatch on $.providerServiceSummaries[0..5]:
  expected [0] {'providerName': 'liveramp', 'providerServiceArn': 'arn:aws:entityresolution:us-east-2::providerservice/liveramp/assignment', 'providerServiceDisplayName': 'Assignment Email', 'providerServiceName': 'assignment', 'providerServiceType': 'ASSIGNMENT'}
  got      [0] {'providerName': 'LiveRamp', 'providerServiceArn': 'arn:aws:entityresolution:us-west-2::providerservice/LiveRamp/IDENTITY_RESOLUTION', 'providerServiceDisplayName': 'LiveRamp Identity Resolution', 'providerServiceName': 'IDENTITY_RESOLUTION', 'providerServiceType': 'ASSIGNMENT'}
  expected [1] liveramp/assignment-pii 'Assignment PII' ASSIGNMENT; got LiveRamp/TRANSCODE 'LiveRamp Transcode' ID_MAPPING
  expected [2] liveramp/idmapping 'ID Mapping' ID_MAPPING; got TransUnion/IDENTITY_RESOLUTION 'TransUnion TruAudience Identity Resolution' ASSIGNMENT
  expected [3] transunion/assignment 'TruAudience Identity Resolution & Enrichment' (arn region us-east-1)
  (the provider/service tokens, display names and types are catalog data; the per-entry ARN region segments are catalog data too, not per-run values)

Cause 2 — fixture entityresolution-provider-services-catalog, step task#2, GetProviderService: status expected 200, got 404; every output member missing, including the @required ones — $.anonymizedOutput (expected True), $.providerEndpointConfiguration (expected {'marketplaceConfiguration': {...}}), $.providerEntityOutputDefinition (expected a JSON-schema document requiring 'entity' with a RAMPID property), $.providerComponentSchema (expected {'providerSchemaAttributes': [{'hashing': True, 'type': 'EMAIL_ADDRESS'}], 'schemas': [['EMAIL_ADDRESS']]}), $.providerConfigurationDefinition (client_id/client_secret/resolution_flow, required [client_id, client_secret]), $.providerIntermediateDataAccessConfiguration (requiredBucketActions s3:PutObject/GetObject/ListBucket/DeleteObject/GetBucketLocation/GetBucketAcl), $.providerJobConfiguration, $.providerName, $.providerServiceArn, $.providerServiceDisplayName, $.providerServiceName, $.providerServiceType.

Cause 3 — fixture entityresolution-provider-services-catalog, step task#3, GetProviderService, status 404 expected and 404 got; $.message value_mismatch: expected 'PartnerComponent Not Found', got 'ProviderService does not exist'.

Smithy citations (services/service-com-amazonaws-entityresolution-awsveniceservice/smithy-model/com-amazonaws-entityresolution-awsveniceservice.json): ListProviderServices GET /providerservices @readonly @paginated(items=providerServiceSummaries, pageSize=maxResults @range 15..25, inputToken/outputToken=nextToken), input providerName @httpQuery EntityName; GetProviderService GET /providerservices/{providerName}/{providerServiceName} @readonly, providerServiceName @httpLabel targets ProviderServiceArn (pattern ^arn:(aws|aws-us-gov|aws-cn):(entityresolution):([a-z]{2}-[a-z]{1,10}-[0-9])::providerservice/([a-zA-Z0-9_-]{1,255})/([a-zA-Z0-9_-]{1,255})$), output @required providerName/providerServiceName/providerServiceDisplayName/providerServiceType/providerServiceArn/providerEndpointConfiguration/anonymizedOutput/providerEntityOutputDefinition, errors ResourceNotFoundException(404), ValidationException(400), AccessDeniedException(403), ThrottlingException(429), InternalServerException(500).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 and look at the entityresolution ListProviderServices / GetProviderService steps.
3. Cause 1: GET /providerservices (entityresolution ListProviderServices, no providerName filter) — compare providerServiceSummaries with fixture entityresolution-catalog-list-provider-services step task#1: the emulator returns its own seeded entries (LiveRamp/IDENTITY_RESOLUTION, LiveRamp/TRANSCODE, TransUnion/IDENTITY_RESOLUTION, ...) with request-region ARNs instead of AWS's catalog (liveramp/assignment, liveramp/assignment-pii, liveramp/idmapping, transunion/assignment, ... with per-entry ARN regions).
4. Cause 2: GET /providerservices/{providerName}/{providerServiceArn} for a provider service AWS's catalog contains (fixture entityresolution-provider-services-catalog step task#2) — emulator answers 404; expected 200 with the full descriptor including anonymizedOutput, providerEndpointConfiguration, providerEntityOutputDefinition, providerComponentSchema, providerConfigurationDefinition, providerJobConfiguration and providerIntermediateDataAccessConfiguration.
5. Cause 3: GET /providerservices/{providerName}/{providerServiceArn} for a provider service that is in no catalog (fixture entityresolution-provider-services-catalog step task#3) — status 404 is right but the message is 'ProviderService does not exist'; expected the fixed text 'PartnerComponent Not Found'.
