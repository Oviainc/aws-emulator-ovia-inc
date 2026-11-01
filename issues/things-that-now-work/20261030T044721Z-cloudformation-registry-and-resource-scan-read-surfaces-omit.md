Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-resource-scan-workflow
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-resource-scan-workflow cloudformation-type-registry-reads
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeResourceScan|2xx->2xx|$.DescribeResourceScanResponse.DescribeResourceScanResult.ResourceTypeSummaries
Issue-Key: cloudformation|ListTypes|2xx->2xx|$.ListTypesResponse.ListTypesResult.NextToken
Issue-Key: cloudformation|ListTypes|2xx->2xx|$.ListTypesResponse.ListTypesResult.TypeSummaries.member[*]
Issue-Key: cloudformation|DescribeType|2xx->2xx|$.DescribeTypeResponse.DescribeTypeResult.SchemaHandlerPermissions
Issue-Commit: c6cd310496053acfb8677287c8f8c63a1ffba3db

# cloudformation registry and resource-scan read surfaces omit modelled members and seed a RESOURCE-only type catalog

> _Note: this issue was identified against the code base at commit `c6cd310496053acfb8677287c8f8c63a1ffba3db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeResourceScan

## Description
Three net-new causes, all in CloudFormation's read-only registry / resource-scan surfaces. Every one answers HTTP 200 with a response document that is missing members the recording shows AWS returning, or (for ListTypes) with a type catalog of the wrong shape and kind. No open issue carries these replay keys: the replay report lists owned_by=[] for all three. The nearest open issues are '20261029T214321Z-cloudformation-resource-scans-find-nothing-hook-handler-progress.md' and '20261028T235427Z-cloudformation-2-net-new-root-causes-in-the.md' (resource-scan discovery and refusal wording) and '20261029T214321Z-cloudformation-type-registry-reads.md'-family issues such as '20261029T214321Z-cloudformation-type-registry-testtype-s-publisher-gate-refuses.md' and '20261028T094216Z-cloudformation-the-public-extension-registry-exposes-no-activatable.md' — none of them covers the three response-shape causes below, so all three must be closed independently.

Cause 1 of 3: DescribeResourceScan omits ResourceTypeSummaries from DescribeScanResult.
AWS: recorded behavior only. The checked-in Smithy model's DescribeResourceScanOutput declares only ResourceScanId, Status, StatusReason, StartTime, EndTime, PercentageCompleted, ResourceTypes, ResourcesScanned, ResourcesRead and ScanFilters — it says nothing about ResourceTypeSummaries. The recording shows AWS answering 200 and including a ResourceTypeSummaries element on $.DescribeResourceScanResponse.DescribeResourceScanResult for a scan in any status (empty/self-closing when the scan has found nothing per-type yet).
Emulator: answers 200 but $.DescribeResourceScanResponse.DescribeResourceScanResult.ResourceTypeSummaries is absent entirely (replay: 'missing (expected \'\', got None)'), on both the in-progress read and the post-completion read of the same scan.
Why: a client that reads the per-resource-type breakdown of a scan (how many resources of each type were read/scanned) gets no element at all rather than an empty/populated list, so SDK deserialization yields null and any code that iterates the summaries to render scan progress by type breaks or silently reports nothing.
Fix: the DescribeResourceScan handler's response serializer, plus the resource-scan state that must record per-type counters so the element can be emitted.
Traces: cloudformation-resource-scan-workflow#task#2 (representative), cloudformation-resource-scan-workflow#task#4.

Cause 2 of 3: ListTypes returns a RESOURCE-only, metadata-free catalog in a single unpaginated page.
AWS: Smithy ListTypes -> ListTypesOutput{TypeSummaries: list<TypeSummary>, NextToken @length min 1 max 1024}, @paginated(inputToken=NextToken, outputToken=NextToken, items=TypeSummaries, pageSize=MaxResults); RegistryType enum = RESOURCE | MODULE | HOOK; TypeSummary models Type, TypeName, TypeArn, DefaultVersionId, LastUpdated, Description, PublisherId, OriginalTypeName, PublicVersionNumber, LatestPublicVersion, PublisherIdentity, PublisherName, IsActivated. Recorded behavior shows the default (PUBLIC/visibility-default) listing containing HOOK entries as well as RESOURCE entries, each summary carrying the publisher and version metadata (Type, TypeName, TypeArn, Description, PublisherId, PublisherName, PublisherIdentity, PublisherCategory, LatestPublicVersion, IsActivated, LastUpdated — PublisherCategory is recorded-only, it is not in the model), and a NextToken present because the catalog is larger than one page.
Emulator: the first TypeSummaries member is a bare RESOURCE entry carrying only Type, TypeName and TypeArn (replay: expected a HOOK summary with publisher/version metadata, got {'Type': 'RESOURCE', 'TypeName': 'AWS::ACMPCA::Certificate', 'TypeArn': '...type/resource/AWS-ACMPCA-Certificate'}), and $.ListTypesResponse.ListTypesResult.NextToken is absent ('missing (expected <shaped-not-asserted>, got None)'). The recorded type names/ARNs/versions themselves are not the requirement — the requirement is that the seeded registry contains MODULE and HOOK extensions in addition to RESOURCE ones, that each summary carries the modelled publisher/description/version members, and that NextToken is emitted whenever more results remain than the effective page size.
Why: a client enumerating the extension registry sees only resource types, so any flow that filters Type=HOOK or Type=MODULE finds nothing; and because NextToken is never emitted, a paginating client stops after the first page and treats the truncated catalog as complete, with no way to detect the truncation.
Fix: the ListTypes handler — the seeded type-registry state (catalog contents and per-type metadata) and the handler's pagination/NextToken emission.
Traces: cloudformation-type-registry-reads#task#2.

Cause 3 of 3: DescribeType omits SchemaHandlerPermissions from DescribeTypeResult.
AWS: recorded behavior only. The checked-in Smithy model's DescribeTypeOutput does not declare SchemaHandlerPermissions (it declares Arn, Type, TypeName, DefaultVersionId, IsDefaultVersion, TypeTestsStatus, TypeTestsStatusDescription, Description, Schema, ProvisioningType, DeprecatedStatus, LoggingConfig, RequiredActivatedTypes, ExecutionRoleArn, Visibility, SourceUrl, DocumentationUrl, LastUpdated, TimeCreated, ConfigurationSchema, PublisherId, OriginalTypeName, OriginalTypeArn, PublicVersionNumber, LatestPublicVersion, IsActivated, AutoUpdate). The recording shows AWS answering 200 with a SchemaHandlerPermissions element present on $.DescribeTypeResponse.DescribeTypeResult (empty for a type with no handler permissions).
Emulator: answers 200 but the element is absent entirely (replay: 'missing (expected \'\', got None)').
Why: a client reading the handler permissions a registered extension requires gets no element rather than an empty list, so tooling that inspects or renders an extension's handler permissions sees null and either errors or reports nothing where AWS reports an (empty) set.
Fix: the DescribeType handler's response serializer, plus the type-registry state that must carry the per-type handler-permission set.
Traces: cloudformation-type-registry-reads#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, score 77.1%, 60 divergence clusters; 57 already owned by open issues, these 3 net-new with owned_by=[] and stub_path="").

Cause 1 — cluster index 34, cloudformation / DescribeResourceScan, expected 200, actual 200:
  $.DescribeResourceScanResponse.DescribeResourceScanResult.ResourceTypeSummaries: missing (expected '', got None)
  affected: cloudformation-resource-scan-workflow#task#2, cloudformation-resource-scan-workflow#task#4

Cause 2 — cluster index 59, cloudformation / ListTypes, expected 200, actual 200:
  $.ListTypesResponse.ListTypesResult.TypeSummaries.member[0]: value_mismatch — expected a HOOK summary with full publisher/version metadata ({'Type': 'HOOK', 'TypeName': 'AWS::ControlTower::Hook', 'TypeArn': 'arn:aws:cloudformation:us-west-2::type/hook/aws-hooks/AWS-ControlTower-Hook', 'Description': ..., 'PublisherId': 'aws-hooks', 'PublisherName': 'aws-hooks', 'PublisherIdentity': 'AWS', 'PublisherCategory': 'VERIFIED_PUBLISHER', 'LatestPublicVersion': '1.2.31', 'IsActivated': 'false', 'LastUpdated': ...}); got {'Type': 'RESOURCE', 'TypeName': 'AWS::ACMPCA::Certificate', 'TypeArn': 'arn:aws:cloudformation:us-west-2::type/resource/AWS-ACMPCA-Certificate'}
  $.ListTypesResponse.ListTypesResult.NextToken: missing (expected '<shaped-not-asserted>', got None)
  affected: cloudformation-type-registry-reads#task#2
  (the specific recorded type name, ARN, version string and timestamp are per-recording values, not requirements; the requirements are the presence of non-RESOURCE kinds, the modelled summary members, and a NextToken when more results remain)

Cause 3 — cluster index 60, cloudformation / DescribeType, expected 200, actual 200:
  $.DescribeTypeResponse.DescribeTypeResult.SchemaHandlerPermissions: missing (expected '', got None)
  affected: cloudformation-type-registry-reads#task#3

Model evidence used (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): DescribeResourceScanOutput has no ResourceTypeSummaries member and DescribeTypeOutput has no SchemaHandlerPermissions member, so both are recorded-behavior-only requirements; ListTypes is @paginated with outputToken=NextToken and TypeSummary models the publisher/description/version members listed above; RegistryType = RESOURCE|MODULE|HOOK.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Cause 1: replay fixture cloudformation-resource-scan-workflow and inspect step task#2 (and task#4) — StartResourceScan, then DescribeResourceScan(ResourceScanId=<id from step 1>); observe the 200 DescribeResourceScanResult has no ResourceTypeSummaries element.
3. Cause 2: replay fixture cloudformation-type-registry-reads step task#2 — ListTypes with the default visibility/type filters; observe every TypeSummaries member is Type=RESOURCE carrying only Type/TypeName/TypeArn (no Description, PublisherId, PublisherName, PublisherIdentity, LatestPublicVersion, IsActivated, LastUpdated) and that ListTypesResult carries no NextToken even though the whole catalog is returned.
4. Cause 3: replay fixture cloudformation-type-registry-reads step task#3 — DescribeType for a registry type; observe the 200 DescribeTypeResult has no SchemaHandlerPermissions element.
5. Re-replay all three fixture steps (cloudformation-resource-scan-workflow#task#2, cloudformation-type-registry-reads#task#2, cloudformation-type-registry-reads#task#3) to confirm a fix; all three must pass before this issue is closed.
