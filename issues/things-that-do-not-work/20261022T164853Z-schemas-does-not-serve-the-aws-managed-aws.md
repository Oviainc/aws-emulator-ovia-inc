Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-catalog-list-schemas
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-catalog-list-schemas schemas-schema-search-and-list schemas-export-aws-managed-schema
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|ListSchemas|2xx->2xx|$.Schemas[*]
Issue-Key: schemas|DescribeSchema|2xx->4xx|$.Content
Issue-Key: schemas|DescribeSchema|2xx->4xx|$.Description
Issue-Key: schemas|DescribeSchema|2xx->4xx|$.LastModified
Issue-Key: schemas|DescribeSchema|2xx->4xx|$.SchemaArn
Issue-Key: schemas|DescribeSchema|2xx->4xx|$.SchemaName
Issue-Key: schemas|DescribeSchema|2xx->4xx|$.SchemaVersion
Issue-Key: schemas|DescribeSchema|2xx->4xx|$.Type
Issue-Key: schemas|DescribeSchema|2xx->4xx|$.VersionCreatedDate
Issue-Key: schemas|DescribeSchema|2xx->4xx|$.tags
Issue-Key: schemas|DescribeSchema|2xx->4xx|status
Issue-Key: schemas|ExportSchema|2xx->4xx|$.Content
Issue-Key: schemas|ExportSchema|2xx->4xx|$.SchemaName
Issue-Key: schemas|ExportSchema|2xx->4xx|$.SchemaVersion
Issue-Key: schemas|ExportSchema|2xx->4xx|$.Type
Issue-Key: schemas|ExportSchema|2xx->4xx|status
Issue-Commit: e556c3079ace9bb74a6c235df6f1f92b13f370bd

# schemas does not serve the AWS-managed aws.events discovered-schema catalog

> _Note: this issue was identified against the code base at commit `e556c3079ace9bb74a6c235df6f1f92b13f370bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSchemas

## Description
AWS ships a read-only registry named `aws.events` that holds the discovered schema for every AWS service event type (`aws.<source>@<DetailType>`, e.g. `aws.acm@ACMCertificateExpired`, `aws.ec2@EC2InstanceStateChangeNotification`, `aws.events@AWSAPICallviaCloudTrail`). The emulator's `aws.events` registry contains only a handful of hand-seeded entries, so paging the catalog returns almost nothing and the managed schemas cannot be described or exported.

This is ONE root cause observed as 52 divergence clusters, all of which must be re-replayed before closing: 50 ListSchemas clusters in fixture schemas-catalog-list-schemas (one per page of the catalog) plus the DescribeSchema and ExportSchema clusters in fixture schemas-export-aws-managed-schema. Every cluster's fixture#step is listed in trace_ids so the fix is verified across the whole catalog walk, not just the representative page.

  AWS: recorded behavior (the catalog's membership is AWS-managed and not described by the model). ListSchemas on RegistryName=aws.events returns 200 and pages, via NextToken, through thousands of SchemaSummary entries whose SchemaName is `aws.<source>@<DetailType>`, each with LastModified, VersionCount and tags. DescribeSchema and ExportSchema on any of those names (e.g. SchemaName=aws.events@AWSAPICallviaCloudTrail, Type=OpenApi3 / JSONSchemaDraft4) return 200 with the schema document, Description "Schema for event type <DetailType>, published by AWS service <source>", SchemaVersion, LastModified/VersionCreatedDate and tags. Smithy: ListSchemas GET /v1/registries/name/{RegistryName}/schemas, paginated on NextToken/Schemas, output __listOfSchemaSummary; DescribeSchema and ExportSchema both model NotFoundException(404) only for names that do not exist.
  Emulator: ListSchemas answers 200 but with an essentially empty page set — in each recorded page every expected SchemaSummary is reported missing, and the only entries it does produce are a few seeded names (observed: aws.autoscaling@EC2InstanceLaunchSuccessful, aws.ec2@EC2InstanceStateChangeNotification). DescribeSchema and ExportSchema for aws.events@AWSAPICallviaCloudTrail answer 404 instead of 200.
  Why: the discovery workflow a client actually performs against this service — enumerate the AWS-managed catalog, pick an event type, describe it, export it as OpenApi3/JSONSchemaDraft4 and generate code bindings — produces an empty catalog and then a 404, so nothing downstream of discovery can be exercised. Pagination over the catalog is also untestable because there is no content to page.
  Fix: the schemas service's seeded state for the AWS-managed `aws.events` registry (the catalog of `aws.<source>@<DetailType>` schemas plus their documents), which the ListSchemas/DescribeSchema/ExportSchema handlers read. Note that the per-entry LastModified values and ARNs in the recordings are environment-specific examples; the requirement is the catalog's membership and that describe/export of a catalog member answers 200 with its document.

Traces: representative schemas-catalog-list-schemas#task#1; the other 49 ListSchemas pages and schemas-export-aws-managed-schema#task#2 (DescribeSchema) / #task#3 (ExportSchema) are listed in trace_ids.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, 0 state_reset_failures):
- schemas-catalog-list-schemas#task#1 ListSchemas 200->200: $.Schemas[0..7] all 'missing', e.g. expected {'SchemaName': 'aws.a4b@AWSAPICallviaCloudTrail', 'VersionCount': 1, 'tags': {}}, got None (cluster covers 10 steps)
- schemas-catalog-list-schemas#task#2 ListSchemas: $.Schemas[0..5] missing (aws.abuse@* ) — cluster of 73 steps
- schemas-catalog-list-schemas#task#32 ListSchemas: expected aws.autoscaling@AWSAPICallViaCloudTrail..., got only {'SchemaName': 'aws.autoscaling@EC2InstanceLaunchSuccessful', 'SchemaArn': 'arn:aws:schemas:us-west-2:000000000000:schema/aws.events/aws.autoscaling@EC2InstanceLaunchSuccessful'} — i.e. a single seeded entry in place of the whole aws.autoscaling block
- schemas-catalog-list-schemas#task#70 ListSchemas: expected the aws.ec2@* block, got only aws.ec2@EC2InstanceStateChangeNotification
- schemas-export-aws-managed-schema#task#2 DescribeSchema: status expected 200, got 404; $.SchemaName missing (expected 'aws.events@AWSAPICallviaCloudTrail'), $.Description missing (expected 'Schema for event type AWSAPICallviaCloudTrail, published by AWS service aws.events'), $.Content missing (the OpenApi3 document)
- schemas-export-aws-managed-schema#task#3 ExportSchema: status expected 200, got 404; $.Content missing (the JSONSchemaDraft4 document)
In total 50 of the 76 divergence clusters in this replay are ListSchemas pages of this one catalog.

## Steps to Reproduce
1. Boot the emulator from mainline and call ListSchemas with RegistryName=aws.events, paging with NextToken (fixture schemas-catalog-list-schemas replays ~206 such pages).
2. Compare each page to the recording: the AWS-managed `aws.<source>@<DetailType>` entries are absent; only a couple of seeded names come back.
3. Call DescribeSchema RegistryName=aws.events SchemaName=aws.events@AWSAPICallviaCloudTrail (fixture schemas-export-aws-managed-schema step task#2): the emulator answers 404 instead of 200 with the schema document.
4. Call ExportSchema for the same name with Type=JSONSchemaDraft4 (step task#3): 404 instead of 200.
5. Re-replay all 52 fixture#step bindings listed in trace_ids after seeding the catalog.
