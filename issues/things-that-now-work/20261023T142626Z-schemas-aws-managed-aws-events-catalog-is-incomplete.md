Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-catalog-list-schemas
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-catalog-list-schemas schemas-export-aws-managed-schema
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|ListSchemas|2xx->2xx|$.Schemas[*]
Issue-Key: schemas|DescribeSchema|2xx->2xx|$.Content
Issue-Key: schemas|DescribeSchema|2xx->2xx|$.SchemaArn
Issue-Key: schemas|ExportSchema|2xx->2xx|$.Content
Issue-Commit: 17c220c4f82dc0c00315f2111baf55aa4ec61156

# schemas AWS-managed aws.events catalog is incomplete and its entries carry synthesized metadata

> _Note: this issue was identified against the code base at commit `17c220c4f82dc0c00315f2111baf55aa4ec61156`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSchemas

## Description
The service-owned `aws.events` discovered-schema registry is served from a small hand-curated seed rather than the AWS-managed catalog, so reads of it diverge in four distinct ways. 51 of the 57 replayed divergence clusters are ListSchemas over this registry. This issue bundles FIVE causes; all five must be closed.

Cause 1 of 5: ListSchemas over the AWS-managed registry omits nearly every managed schema.
AWS: Smithy: ListSchemas GET /v1/registries/name/{RegistryName}/schemas -> ListSchemasResponse.Schemas (list of SchemaSummary), 200, @paginated. The catalog's membership is recorded behavior only: for every event source AWS publishes, the registry holds the whole CloudTrail family (`<source>@AWSAPICallviaCloudTrail`, `@AWSConsoleActionviaCloudTrail`, `@AWSConsoleSignInviaCloudTrail`, `@AWSInsightviaCloudTrail`, `@AWSNetworkActivityEventviaCloudTrail`, `@AWSServiceEventviaCloudTrail`) plus that service's own event schemas (e.g. `aws.a4b@RoomStateChange`, `aws.acm@ACMCertificateAvailable`, the whole `aws.aidevops@Investigation*`/`@Mitigation*` family).
Emulator: for most sources it serves only `<source>@AWSAPICallviaCloudTrail` and nothing else, and for some sources (aws.act, aws.aidevops, aws.amazon.connect, aws.amazonmq, aws.amazonq) it serves no schema at all. Replay reports whole expected elements as `missing` with `got None` — e.g. aws.a4b: 8 of 9 expected summaries missing; aws.aidevops: all 15 missing.
Why: a client enumerating or prefix-searching the managed catalog (the normal way to find the schema for an event it must consume, and what code-binding and export flows start from) sees an almost empty registry, so schema discovery and every downstream DescribeSchema/ExportSchema on a real managed schema name fails with NotFound against a registry AWS says contains it.
Fix: the AWS-managed catalog seed in the schemas service state (the `AWS_EVENT_SOURCES` / `AWS_SERVICE_EVENT_TYPES` tables feeding `aws_managed_registries`), not the ListSchemas handler's paging.
Traces: schemas-catalog-list-schemas#task#1 (aws.a4b), plus 49 further clusters of the same fixture; seed tables at services/service-com-amazonaws-schemas-schemas/src/state.rs:374-388.

Cause 2 of 5: the same omission applies to the `aws.events` source itself.
AWS: recorded behavior only — the registry holds `aws.events@APIDestinationActivated`, `@APIDestinationDeactivated`, `@AWSConsoleActionviaCloudTrail`, `@AWSConsoleSignInviaCloudTrail`, `@AWSInsightviaCloudTrail`, `@AWSNetworkActivityEventviaCloudTrail`, `@AWSServiceEventviaCloudTrail`, `@ConnectionActivated`, `@ConnectionAuthorizationStarted` and more, status 200.
Emulator: serves only `aws.events@AWSAPICallviaCloudTrail` and `aws.events@ScheduledEvent`; every other expected summary comes back `missing (... got None)`.
Why: the EventBridge-bus event schemas are the ones a schema-registry user is most likely to resolve first, so the gap is hit on the very first call of an export flow.
Fix: same seed table as cause 1.
Traces: schemas-export-aws-managed-schema#task#1.

Cause 3 of 5: the managed entries that ARE served carry made-up LastModified and VersionCount.
AWS: Smithy: SchemaSummary.LastModified is `@timestampFormat("date-time")` "The date and time that schema was modified" and SchemaSummary.VersionCount is `__long` "The number of versions available for the schema". Recorded: each managed schema reports its OWN modification instant at whole-second precision (e.g. `2026-07-08T18:12:15Z`, `2025-07-19T02:19:20Z`) and a per-schema version count that is frequently 2 or 3 (`aws.acm@ACMCertificateAvailable` VersionCount 3, `aws.a4b@RoomStateChange` VersionCount 3, the `aws.aidevops@*` family VersionCount 2).
Emulator: every entry in a response reports the SAME timestamp, which is the moment the service state was seeded, at sub-second precision (`2026-10-23T14:22:06.917567Z` in one fixture and `2026-10-23T14:22:16.459841Z` in another — i.e. it moves with the emulator's clock rather than with the schema), and VersionCount is always 1.
Why: LastModified is how a client decides whether its cached copy of a managed schema is stale; a value that equals "now" at every boot invalidates every cache on every run, and a constant VersionCount of 1 hides the existence of versions 2 and 3, so a client that pins a version cannot see one to pin.
Fix: the seeded catalog entries' stored metadata in the schemas service state — give each managed schema its own fixed last-modified instant and version count instead of deriving them from seed time and a single stored version.
Traces: schemas-catalog-list-schemas#task#6 (aws.acm, VersionCount 3 expected, 1 returned); same metadata visible in every other cluster of that fixture.

Cause 4 of 5: DescribeSchema on a managed schema invents a SchemaArn and returns the wrong Content.
AWS: Smithy: DescribeSchema GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName} -> 200 with Content, SchemaArn, SchemaName, SchemaVersion, Type, LastModified. Recorded: for an AWS-managed schema, SchemaArn is the empty string (the managed catalog's schemas are not customer resources and carry no ARN — correspondingly, ListSchemas summaries for this registry carry no SchemaArn member at all), and Content is the OpenApi3 document AWS publishes for that event (`{"openapi":"3.0.0","info":{"title":"AWS API Call via CloudTrail published by aws.events",...},"paths":{},"components":{"schemas":{"AWSAPICallviaCloudTrail":{...full CloudTrail record: addendum, additionalEventData, eventCategory, resources[], tlsDetails, userIdentity.sessionContext ...}}}}`).
Emulator: returns `SchemaArn: arn:aws:schemas:<region>:<account>:schema/aws.events/aws.events@AWSAPICallviaCloudTrail` where AWS returns `''`, and a Content document that does not match the published one.
Why: a client that stores the returned ARN builds a resource identifier AWS never issues (and that no TagResource/policy call will accept), and code generation or validation driven off Content works from a document whose properties differ from the events EventBridge actually delivers.
Fix: the DescribeSchema handler's managed-registry branch and the seeded schema content — suppress ARN synthesis for `RegistryScope::Aws` (see `Registry::schema_arn_in`) and serve the published document.
Traces: schemas-export-aws-managed-schema#task#2; ARN synthesis at services/service-com-amazonaws-schemas-schemas/src/state.rs:171-176.

Cause 5 of 5: ExportSchema returns a JSONSchemaDraft4 body unlike AWS's.
AWS: Smithy: ExportSchema GET .../export?type=, Type enum values OpenApi3|JSONSchemaDraft4 -> 200 with Content. Recorded: for `type=JSONSchemaDraft4` the body is the draft-04 translation of the managed schema — `{"$schema":"http://json-schema.org/draft-04/schema#","title":"AWS API Call via CloudTrail published by aws.events","definitions":{"AWSAPICallviaCloudTrail":{...}}}` with nullable members rendered as `"type":["object","null"]` and object members carrying an explicit `"type":"object"`.
Emulator: Content is a different document (value_mismatch on $.Content).
Why: ExportSchema exists so a client can feed a managed event schema to a draft-04 validator or generator; a body that differs in structure makes generated models reject real events.
Fix: the ExportSchema handler's OpenApi3 -> draft-04 conversion, together with the seeded source document from cause 4.
Traces: schemas-export-aws-managed-schema#task#3.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, 13 fixtures, 289 steps, score 18.3%).

Cause 1 — schemas-catalog-list-schemas#task#1, ListSchemas, expected 200 / actual 200:
  $.Schemas[0]: missing (expected {'SchemaName': 'aws.a4b@AWSAPICallviaCloudTrail', 'LastModified': '2026-07-08T18:12:15Z', 'VersionCount': 1, 'tags': {}}, got {'LastModified': '2026-10-23T14:22:06.917567Z', 'SchemaArn': 'arn:aws:schemas:us-west-2:000000000000:schema/aws.events/aws.a4b@AWSAPICallviaCloudTrail', ...})
  $.Schemas[1..7]: missing (aws.a4b@AWSConsoleActionviaCloudTrail, @AWSConsoleSignInviaCloudTrail, @AWSInsightviaCloudTrail, @AWSNetworkActivityEventviaCloudTrail, @AWSServiceEventviaCloudTrail, @RoomDeviceCalendarErrorStatus, @RoomStateChange) — got None
  Same shape in 49 further clusters, e.g. task#7 (aws.act@TrustBundleAssociationVersionUpdated missing, nothing served) and task#10 (all 15 aws.aidevops@* missing).

Cause 2 — schemas-export-aws-managed-schema#task#1, ListSchemas, 200/200: expected aws.events@APIDestinationActivated, @APIDestinationDeactivated, @AWSConsoleActionviaCloudTrail, @AWSConsoleSignInviaCloudTrail, @AWSInsightviaCloudTrail, @AWSNetworkActivityEventviaCloudTrail, @AWSServiceEventviaCloudTrail, @ConnectionActivated, @ConnectionAuthorizationStarted ...; got only aws.events@AWSAPICallviaCloudTrail and aws.events@ScheduledEvent.

Cause 3 — schemas-catalog-list-schemas#task#6, ListSchemas, 200/200: expected aws.acm@ACMCertificateAvailable LastModified '2026-07-08T18:06:56Z' VersionCount 3; every served entry instead reports LastModified '2026-10-23T14:22:06.917567Z' and VersionCount 1. The same member in schemas-export-aws-managed-schema reports '2026-10-23T14:22:16.459841Z' — the value tracks the emulator's state-reset clock, not the schema.

Cause 4 — schemas-export-aws-managed-schema#task#2, DescribeSchema, 200/200:
  $.SchemaArn: value_mismatch (expected '', got 'arn:aws:schemas:us-west-2:000000000000:schema/aws.events/aws.events@AWSAPICallviaCloudTrail')
  $.Content: value_mismatch (expected the openapi 3.0.0 'AWS API Call via CloudTrail published by aws.events' document with components.schemas.AWSAPICallviaCloudTrail)

Cause 5 — schemas-export-aws-managed-schema#task#3, ExportSchema, 200/200:
  $.Content: value_mismatch (expected the draft-04 document: {"$schema":"http://json-schema.org/draft-04/schema#","title":"AWS API Call via CloudTrail published by aws.events","definitions":{...}})

Note: the resolved issue 20261022T164853Z-schemas-does-not-serve-the-aws-managed-aws.md stood the catalog up at all; these causes are about what it now contains and reports.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture schemas-catalog-list-schemas, step task#1 — GET /v1/registries/name/aws.events/schemas?schemaNamePrefix=aws.a4b and compare the Schemas list against the recorded summaries.
3. Cause 2: fixture schemas-export-aws-managed-schema, step task#1 — same call with prefix aws.events.
4. Cause 3: fixture schemas-catalog-list-schemas, step task#6 — inspect LastModified/VersionCount of the returned aws.acm summaries, then reset state and repeat: LastModified moves with the clock.
5. Cause 4: fixture schemas-export-aws-managed-schema, step task#2 — GET /v1/registries/name/aws.events/schemas/name/aws.events@AWSAPICallviaCloudTrail and compare SchemaArn and Content.
6. Cause 5: fixture schemas-export-aws-managed-schema, step task#3 — GET the same path with /export?type=JSONSchemaDraft4 and compare Content.
