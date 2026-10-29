Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-resource-scan-workflow
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-resource-scan-workflow cloudformation-generated-template-and-resource-scan cloudformation-template-generator-and-hook-result-errors cloudformation-list-type-registrations-invalid-filter-rejected cloudformation-type-registry-reads
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ListResourceScans|2xx->2xx|$.ListResourceScansResponse.ListResourceScansResult.ResourceScanSummaries
Issue-Key: cloudformation|DescribeResourceScan|2xx->2xx|$.DescribeResourceScanResponse.DescribeResourceScanResult.PercentageCompleted
Issue-Key: cloudformation|DescribeResourceScan|2xx->2xx|$.DescribeResourceScanResponse.DescribeResourceScanResult.ResourceTypeSummaries
Issue-Key: cloudformation|DescribeResourceScan|2xx->2xx|$.DescribeResourceScanResponse.DescribeResourceScanResult.ResourceTypes
Issue-Key: cloudformation|DescribeResourceScan|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|DescribeResourceScan|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ListResourceScanResources|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|ListResourceScanResources|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ListResourceScanRelatedResources|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudformation|ListResourceScanRelatedResources|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ListTypeRegistrations|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ListTypes|2xx->2xx|$.ListTypesResponse.ListTypesResult.NextToken
Issue-Key: cloudformation|ListTypes|2xx->2xx|$.ListTypesResponse.ListTypesResult.TypeSummaries.member[*]
Issue-Key: cloudformation|DescribeType|2xx->2xx|$.DescribeTypeResponse.DescribeTypeResult.SchemaHandlerPermissions
Issue-Key: cloudformation|DescribeType|2xx->2xx|$.DescribeTypeResponse.DescribeTypeResult.SourceUrl
Issue-Key: cloudformation|BatchDescribeTypeConfigurations|2xx->2xx|$.BatchDescribeTypeConfigurationsResponse.BatchDescribeTypeConfigurationsResult.Errors.member.ErrorMessage
Issue-Key: cloudformation|BatchDescribeTypeConfigurations|2xx->2xx|$.BatchDescribeTypeConfigurationsResponse.BatchDescribeTypeConfigurationsResult.TypeConfigurations
Issue-Key: cloudformation|BatchDescribeTypeConfigurations|2xx->2xx|$.BatchDescribeTypeConfigurationsResponse.BatchDescribeTypeConfigurationsResult.UnprocessedTypeConfigurations
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudformation resource-scan and type-registry reads return wrong payloads and emulator-authored refusals

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeResourceScan

## Description
The two read-only discovery surfaces — resource scans and the extension type registry — answer 200/400 with payloads and refusals that do not match the recording: a completed scan is invisible to ListResourceScans, scan summaries are missing, a malformed scan id is refused as a lookup failure instead of an input-constraint violation, and the registry reads drop members and use emulator wording. 9 net-new root causes, one service.

The replay reported owned_by=[] and no stub for all nine. Open issues "cloudformation: 2 net-new root causes in the resource-scan surface — the ResourceScanInProgress message is the model's doc sentence, and a scan never ..." and "cloudformation: 7 net-new root causes in the resource-scan, generated-template and type-registry surfaces ..." are the nearest neighbours and carry none of these fixture#step bindings.

Cause 1 of 9: ListResourceScans reports no scans at all.
AWS: Smithy: ListResourceScans output ResourceScanSummaries (list). After a scan completes, the recording returns one summary per scan carrying ResourceScanId (the resourceScan ARN), Status, ScanType, PercentageCompleted, StartTime and EndTime.
Emulator: 200 with ResourceScanSummaries serialized as an empty value (type_mismatch: a list of summaries was expected, an empty string was returned).
Why: the scan a client just started and polled to COMPLETE cannot be found again, so the generator flow (scan -> list -> pick a scan -> create template) cannot be resumed.
Fix: the ListResourceScans handler and the resource-scan state it must enumerate.
Traces: cloudformation-resource-scan-workflow#task#5.

Cause 2 of 9: DescribeResourceScan omits the scanned-type breakdown and renders the percentage as an integer.
AWS: Smithy: DescribeResourceScan output members ResourceTypes (list), ResourceTypeSummaries (list) and PercentageCompleted (double). For a completed scan the recording carries both list members (present, possibly empty) and PercentageCompleted serialized as '100.0'.
Emulator: ResourceTypes and ResourceTypeSummaries absent; PercentageCompleted serialized as '100'.
Why: a client cannot see which resource types a scan covered, and an SDK that round-trips the double sees a different literal from AWS's.
Fix: the DescribeResourceScan response serializer (emit the modelled list members and serialize the double as a double).
Traces: cloudformation-resource-scan-workflow#task#2, cloudformation-resource-scan-workflow#task#4.

Cause 3 of 9: DescribeResourceScan refuses a malformed scan id as a lookup failure.
AWS: a ResourceScanId that does not match the modelled pattern is an input-constraint violation, not a missing resource: Code ValidationError with '1 validation error detected: Value '<supplied value>' at 'resourceScanId' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z0-9-]+:cloudformation:[a-z0-9-]+:[0-9]{12}:resourceScan/[a-z0-9-]+' — fixed text apart from the rejected value, the member name and the pattern.
Emulator: 400 with Code ResourceScanNotFound and 'There is no ResourceScan found for the ResourceScanId.'
Why: a client cannot distinguish a typo in the id (retry with a corrected id) from a scan that does not exist (start a new scan); error-code-driven handling branches the wrong way.
Fix: the DescribeResourceScan input validation, ahead of the state lookup.
Traces: cloudformation-generated-template-and-resource-scan#task#6.

Cause 4 of 9: ListResourceScanResources has the same missing input validation.
AWS: as cause 3, Code ValidationError with the resourceScanId pattern message.
Emulator: 400 ResourceScanNotFound / 'There is no ResourceScan found for the ResourceScanId.'
Why/Fix: as cause 3, in the ListResourceScanResources input validation.
Traces: cloudformation-resource-scan-workflow#task#7, cloudformation-template-generator-and-hook-result-errors#task#3.

Cause 5 of 9: ListResourceScanRelatedResources has the same missing input validation.
AWS: as cause 3.
Emulator: 400 ResourceScanNotFound / 'There is no ResourceScan found for the ResourceScanId.'
Why/Fix: as cause 3, in the ListResourceScanRelatedResources input validation.
Traces: cloudformation-template-generator-and-hook-result-errors#task#4.

Cause 6 of 9: ListTypeRegistrations renders the enum value set in the wrong order in its validation message.
AWS: recorded behavior only for the ordering — the refusal is '1 validation error detected: Value '<supplied value>' at 'registrationStatusFilter' failed to satisfy constraint: Member must satisfy enum value set: [IN_PROGRESS, COMPLETE, FAILED]'. Note the order is the service's, not the model's: Smithy RegistrationStatus declares [COMPLETE, IN_PROGRESS, FAILED].
Emulator: 400 with the same sentence but '[COMPLETE, IN_PROGRESS, FAILED]' — it renders the model's declaration order.
Why: the message is a requirement of the error class; a client or test matching AWS's exact refusal text does not match.
Fix: the enum-value-set rendering used by this operation's input validation (a per-member value order, not the model's).
Traces: cloudformation-list-type-registrations-invalid-filter-rejected#task#1.

Cause 7 of 9: ListTypes returns a different registry than AWS and no NextToken.
AWS: Smithy: ListTypes output TypeSummaries (list) and NextToken. The recorded first page's first summary is a HOOK entry carrying TypeArn, TypeName, Description, IsActivated, Type, PublisherIdentity, PublisherId, PublisherName, PublisherCategory, LatestPublicVersion and LastUpdated, and the page carries a NextToken because the registry is larger than one page.
Emulator: the first summary is a RESOURCE entry with only Type, TypeName and TypeArn, and NextToken is absent.
Why: the publisher/activation metadata a caller filters and paginates on does not exist, so no registry browse or third-party activation flow can proceed past the first page.
Fix: the ListTypes handler and the seeded type-registry state (publisher identity/category, activation flag, versions, pagination).
Traces: cloudformation-type-registry-reads#task#2.

Cause 8 of 9: DescribeType drops SchemaHandlerPermissions and truncates SourceUrl.
AWS: Smithy: DescribeType output members SourceUrl and SchemaHandlerPermissions. For an AWS-vendored resource type the recorded SourceUrl is the provider repository URL including its '.git' suffix and SchemaHandlerPermissions is present (possibly empty).
Emulator: SourceUrl is the same URL with the '.git' suffix stripped; SchemaHandlerPermissions is absent.
Why: a caller that resolves the provider source or inspects the handler permissions of a type gets a URL that does not clone and a member that does not exist.
Fix: the DescribeType response serializer and the type-registry seed data.
Traces: cloudformation-type-registry-reads#task#3.

Cause 9 of 9: BatchDescribeTypeConfigurations omits its empty result members and uses emulator wording in Errors.
AWS: Smithy: BatchDescribeTypeConfigurations output members Errors, UnprocessedTypeConfigurations and TypeConfigurations; all three are present in the recorded 200 body even when empty. For a type that has no configuration the Errors entry's ErrorMessage is 'TypeConfiguration not found'.
Emulator: UnprocessedTypeConfigurations and TypeConfigurations absent; ErrorMessage is "The type '<type name>' cannot be found."
Why: a client that iterates the three result lists hits a missing member, and the per-entry error text (a requirement of that error class) does not match AWS's.
Fix: the BatchDescribeTypeConfigurations response serializer and its per-entry error message.
Traces: cloudformation-type-registry-reads#task#7, cloudformation-type-registry-reads#task#9.

## Evidence
Replay of simparity-wire-aws-v1, 9 mismatch clusters:
- cloudformation-resource-scan-workflow#task#5 ListResourceScans 200: $.ListResourceScansResponse.ListResourceScansResult.ResourceScanSummaries type_mismatch — expected a member with PercentageCompleted '100.0', ResourceScanId 'arn:aws:cloudformation:us-west-2:000000000000:resourceScan/<uuid>', StartTime/EndTime, ScanType PARTIAL, Status COMPLETE; got ''.
- cloudformation-resource-scan-workflow#task#2 DescribeResourceScan 200: .ResourceTypes missing, .ResourceTypeSummaries missing, .PercentageCompleted value_mismatch (expected '100.0', got '100') — 2 affected steps.
- cloudformation-generated-template-and-resource-scan#task#6 DescribeResourceScan 400: Code expected 'ValidationError', got 'ResourceScanNotFound'; Message expected "1 validation error detected: Value 'not-a-valid-resource-scan-id' at 'resourceScanId' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z0-9-]+:cloudformation:[a-z0-9-]+:[0-9]{12}:resourceScan/[a-z0-9-]+", got 'There is no ResourceScan found for the ResourceScanId.'
- cloudformation-resource-scan-workflow#task#7 ListResourceScanResources 400: same Code/Message divergence (2 affected steps).
- cloudformation-template-generator-and-hook-result-errors#task#4 ListResourceScanRelatedResources 400: same Code/Message divergence.
- cloudformation-list-type-registrations-invalid-filter-rejected#task#1 ListTypeRegistrations 400: Message expected "... enum value set: [IN_PROGRESS, COMPLETE, FAILED]", got "... enum value set: [COMPLETE, IN_PROGRESS, FAILED]".
- cloudformation-type-registry-reads#task#2 ListTypes 200: .TypeSummaries.member[0] value_mismatch — expected the AWS::ControlTower::Hook HOOK summary with PublisherIdentity/PublisherId/PublisherName/PublisherCategory/IsActivated/LatestPublicVersion/LastUpdated/Description, got {Type: RESOURCE, TypeName: AWS::ACMPCA::Certificate, TypeArn: ...}; .NextToken missing.
- cloudformation-type-registry-reads#task#3 DescribeType 200: .SourceUrl value_mismatch (expected '...aws-cloudformation-resource-providers-iam.git', got the same URL without '.git'); .SchemaHandlerPermissions missing.
- cloudformation-type-registry-reads#task#7 BatchDescribeTypeConfigurations 200: .UnprocessedTypeConfigurations missing, .TypeConfigurations missing, .Errors.member.ErrorMessage value_mismatch (expected 'TypeConfiguration not found', got "The type 'AWS::IAM::Role' cannot be found.") — 2 affected steps.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. StartResourceScan, poll DescribeResourceScan to COMPLETE (observe PercentageCompleted '100' and no ResourceTypes/ResourceTypeSummaries), then ListResourceScans — the summaries list comes back empty.
3. Call DescribeResourceScan / ListResourceScanResources / ListResourceScanRelatedResources with ResourceScanId='not-a-valid-resource-scan-id' — all three answer 400 ResourceScanNotFound instead of 400 ValidationError with the resourceScanId pattern message.
4. Call ListTypeRegistrations(RegistrationStatusFilter='NOT_A_REAL_STATUS') and compare the enum value set ordering in the message.
5. Call ListTypes (first page), DescribeType for an AWS-vendored resource type, and BatchDescribeTypeConfigurations for a type with no configuration, and compare the members and messages against the recorded bodies.
