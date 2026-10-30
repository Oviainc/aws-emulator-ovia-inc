Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-catalog-list-schemas
Issue-Step: task#33
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-catalog-list-schemas
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|ListSchemas|2xx->2xx|$.Schemas[*]
Issue-Commit: 2a04615191c549bdab1e2f00e25668c6ae6e3d20

# schemas ListSchemas omits whole AWS event sources from the managed catalog

> _Note: this issue was identified against the code base at commit `2a04615191c549bdab1e2f00e25668c6ae6e3d20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSchemas

## Description
Walking the AWS-managed registry (aws.events) with ListSchemas returns a catalogue whose INVENTORY is not the published one: for a number of event sources the catalogue holds no entry at all, so every page from that point on is filled with the next source's entries and the names a client reads back are not the names AWS publishes. Ten causes, one per event source that is absent from the seeded catalogue.

AWS (common to every cause below): Smithy models ListSchemas as GET /v1/registries/name/{RegistryName}/schemas -> 200 ListSchemasResponse{Schemas: list<SchemaSummary>, NextToken}, @paginated(items: Schemas, pageSize: Limit), with SchemaSummary.SchemaName as __string, VersionCount as __long and Tags with jsonName 'tags'. WHICH entries the service-owned registry holds is recorded behavior only: the recording walks the whole catalogue page by page, and each page holds the published '<source>@<EventName>' entries in SchemaName order. The requirement is therefore structural, not a literal page: the catalogue must publish an entry for EVERY source AWS publishes events for, so that the page at any offset carries the same names. A source that is missing shifts every later page.

Emulator (common): the catalogue is generated in services/service-com-amazonaws-schemas-schemas/src/state.rs from the fixed source list AWS_EVENT_SOURCES crossed with CLOUD_TRAIL_EVENT_FAMILY plus the AWS_SERVICE_EVENT_TYPES table (aws_managed_event_types() -> aws_managed_schemas() -> aws_managed_registries()). Sources absent from AWS_EVENT_SOURCES contribute no entry, and the replay reports the page's entries as 'missing' with either a different source's entry or None in their place.

Why (common): ListSchemas is how a client discovers what it can describe, export or generate bindings for. A source the catalogue never lists is a schema that cannot be discovered at all, and the shift it causes makes every later page report names the caller did not ask for, so paging the registry yields a different universe of schemas than AWS.

Fix (common): the seed data of the service-owned catalogue in state.rs - AWS_EVENT_SOURCES and the AWS_SERVICE_EVENT_TYPES table that aws_managed_event_types() builds the catalogue from - not the ListSchemas handler's paging, which slices whatever the store holds.

Cause 1 of 10 - source aws.b2bi absent. AWS: the page holds aws.b2bi@AWSAPICallviaCloudTrail, aws.b2bi@AWSConsoleActionviaCloudTrail, aws.b2bi@AWSConsoleSignInviaCloudTrail, aws.b2bi@AWSInsightviaCloudTrail, aws.b2bi@AWSNetworkActivityEventviaCloudTrail, aws.b2bi@AWSServiceEventviaCloudTrail and the b2bi service events (e.g. aws.b2bi@AcknowledgementCompleted, VersionCount 2). Emulator: the same offsets carry aws.backup-gateway@* entries - no aws.b2bi entry exists. Traces: schemas-catalog-list-schemas#task#33.

Cause 2 of 10 - sources aws.chime-sdk-media-pipelines and aws.chime-sdk-voice absent. AWS: the page holds the CloudTrail family of aws.chime-sdk-media-pipelines followed by aws.chime-sdk-voice@*. Emulator: those offsets carry aws.chime@* instead. Traces: schemas-catalog-list-schemas#task#38.

Cause 3 of 10 - source aws.ec2fleet absent. AWS: the page holds exactly ten entries - aws.ec2fleet@EC2FleetError, @EC2FleetInformation, @EC2FleetInstanceChange, @EC2FleetSpotInstanceRequestChange, @EC2FleetStateChange, @EC2SpotFleetError, @EC2SpotFleetInformation, @EC2SpotFleetInstanceChange, @EC2SpotFleetSpotInstanceRequestChange, @EC2SpotFleetStateChange (the EC2Fleet* five carrying a published historical instant, 2020-12-17T18:43:39Z/40Z, not a derived one). Emulator: all ten are 'got None' - the page is short, because the source contributes nothing. Traces: schemas-catalog-list-schemas#task#71.

Cause 4 of 10 - source aws.epfs absent. AWS: aws.epfs@NodeIdentificationJobStateChanged, @PowerFlowJobStateChanged, @PowerFlowNodeJobStateChanged, @SessionStateChanged, @SubsystemAnalysisJobStateChanged. Emulator: all five 'got None'. Traces: schemas-catalog-list-schemas#task#88.

Cause 5 of 10 - sources aws.factstore and aws.finspace-api absent. AWS: aws.factstore@NewCommitPushed followed by the aws.finspace-api CloudTrail family. Emulator: those offsets carry aws.finspace@* entries. Traces: schemas-catalog-list-schemas#task#94.

Cause 6 of 10 - source aws.glue-autodebug absent. AWS: aws.glue-autodebug@AutoVersionJobStateChange, @AutodebugmetadatakeysharedfromOrca, @Autoversionjobruncompleted, @Autoversionjobversionupdated, @DataQualityEvaluationResultsAvailable, @DataQualityTaskStateChange, @EntityMeteringEvent and the rest of that source. Emulator: those offsets carry aws.glue@* entries, and later entries are 'got None'. Traces: schemas-catalog-list-schemas#task#100.

Cause 7 of 10 - source aws.goshawk absent. AWS: aws.goshawk@CodeArtifactPackageVersionStateChange and aws.goshawk@GoshawkRepositoryStateChange. Emulator: both 'got None'. Traces: schemas-catalog-list-schemas#task#101.

Cause 8 of 10 - source aws.health-agent absent (and aws.health's own service events incomplete). AWS: the aws.health-agent CloudTrail family, then aws.health entries including aws.health@AWSHealthAbuseEvent (VersionCount 4, published instant 2026-03-12T21:12:29Z) and aws.health@AWSHealthEvent. Emulator: the aws.health-agent offsets carry aws.health@* entries, and AWSHealthAbuseEvent is absent / reported with the wrong version count. Traces: schemas-catalog-list-schemas#task#106.

Cause 9 of 10 - source aws.inspector-scan absent. AWS: the aws.inspector-scan CloudTrail family precedes aws.inspector2@*. Emulator: the page starts at aws.inspector2@* - the inspector-scan entries do not exist. Traces: schemas-catalog-list-schemas#task#110.

Cause 10 of 10 - source aws.iotsecuredtunneling absent. AWS: the aws.iotsecuredtunneling CloudTrail family precedes aws.iotsitewise@*. Emulator: the page starts at aws.iotsitewise@*. Traces: schemas-catalog-list-schemas#task#119.

Not part of this issue: on these same steps every entry's LastModified also differs (the recording's instants fall in clock bands of the recording run - 2026-07-08T18:01Z-18:25Z and 2026-09-17T10:42Z-11:04Z - while the emulator derives an instant from a digest of the schema name). That is a per-run clock value, so it is not filed as a requirement here; the names, presence and VersionCount of the entries are.

Related open issues that do NOT cover this: 20261029T113448Z-schemas-listschemas-pagination.md is about NextToken decoding, and 20261029T203553Z-schemas-aws-managed-schema-content-is-wrong-for-both.md is about one entry's Content document. Neither carries these fixture steps or the catalogue inventory.

## Evidence
Replay of fixture schemas-catalog-list-schemas, service=schemas, operation ListSchemas, expected 200 / actual 200, key $.Schemas[*]:

- task#33: $.Schemas[0..] expected 'aws.b2bi@AWSAPICallviaCloudTrail', 'aws.b2bi@AWSConsoleActionviaCloudTrail', ... 'aws.b2bi@AcknowledgementCompleted' (VersionCount 2); got 'aws.backup-gateway@AWSAPICallviaCloudTrail', 'aws.backup-gateway@AWSConsoleActionviaCloudTrail', ... (every one of the ~100 indices of the page diverges).
- task#38: expected 'aws.chime-sdk-media-pipelines@AWSAPICallviaCloudTrail' ... 'aws.chime-sdk-voice@AWSAPICallviaCloudTrail'; got 'aws.chime@AWSAPICallviaCloudTrail' ...
- task#71: expected the ten 'aws.ec2fleet@*' summaries; got None for all ten.
- task#88: expected the five 'aws.epfs@*' summaries; got None for all five.
- task#94: expected 'aws.factstore@NewCommitPushed', 'aws.finspace-api@AWSAPICallviaCloudTrail' ...; got 'aws.finspace@AWSAPICallviaCloudTrail' ...
- task#100: expected 'aws.glue-autodebug@AutoVersionJobStateChange' ...; got 'aws.glue@AWSAPICallviaCloudTrail' ... and None past index 6.
- task#101: expected 'aws.goshawk@CodeArtifactPackageVersionStateChange', 'aws.goshawk@GoshawkRepositoryStateChange'; got None for both.
- task#106: expected 'aws.health-agent@AWSAPICallviaCloudTrail' ... 'aws.health@AWSHealthAbuseEvent' (VersionCount 4); got 'aws.health@AWSAPICallviaCloudTrail' ...
- task#110: expected 'aws.inspector-scan@AWSAPICallviaCloudTrail' ...; got 'aws.inspector2@AWSAPICallviaCloudTrail' ...
- task#119: expected 'aws.iotsecuredtunneling@AWSAPICallviaCloudTrail' ...; got 'aws.iotsitewise@AWSAPICallviaCloudTrail' ...

Smithy: ListSchemas GET /v1/registries/name/{RegistryName}/schemas -> 200, @paginated(items=Schemas, pageSize=Limit); SchemaSummary{SchemaName: __string, VersionCount: __long, LastModified: __timestampIso8601 (date-time), Tags jsonName 'tags'}.

Code: services/service-com-amazonaws-schemas-schemas/src/state.rs - AWS_EVENT_SOURCES (the source list ending at line 2466), CLOUD_TRAIL_EVENT_FAMILY and AWS_SERVICE_EVENT_TYPES consumed by aws_managed_event_types() (~line 2851), rendered by aws_managed_schemas() (~line 2872) and seeded by aws_managed_registries() (~line 2904).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture schemas-catalog-list-schemas and compare the Schemas list of steps task#33, task#38, task#71, task#88, task#94, task#100, task#101, task#106, task#110 and task#119 (each is a GET /v1/registries/name/aws.events/schemas page).
3. Equivalently by hand: page through GET /v1/registries/name/aws.events/schemas and grep the returned SchemaNames for the prefixes 'aws.b2bi@', 'aws.chime-sdk-media-pipelines@', 'aws.chime-sdk-voice@', 'aws.ec2fleet@', 'aws.epfs@', 'aws.factstore@', 'aws.finspace-api@', 'aws.glue-autodebug@', 'aws.goshawk@', 'aws.health-agent@', 'aws.inspector-scan@', 'aws.iotsecuredtunneling@'. Each returns nothing; AWS publishes entries for all of them.
4. Each of the ten steps must be re-replayed after the fix: they are different pages, and seeding one source only realigns the pages that source shifted.
