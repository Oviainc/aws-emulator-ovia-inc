Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.deploy-cfn-glue-database-table-s3-location-positive athena__glue__s3.deploy-cfn-stack-update-glue-table-schema-positive glue.concurrent-catalog-create-conflict-rejected glue.session-description-and-tags-round-trip glue.create-table-optimizer-nonexistent-table-rejected glue.create-table-optimizer-on-nonexistent-table-fails glue.asset-type-lifecycle-put-get-delete glue.integration-resource-property-lifecycle glue.get-unfiltered-table-metadata-nonexistent-table-rejected glue.get-column-statistics-task-run-invalid-input-exact-code
Issue-Key: athena__glue__s3|GetTable|EntityNotFoundException
Issue-Key: glue.concurrent-catalog-create-conflict-rejected|run#16|AssertionError|assert errors == [plain_refusal, plain_refusal], (
Issue-Key: glue|GetTags|InvalidInputException
Issue-Key: glue.create-table-optimizer-nonexistent-table-rejected|_error_code#16|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: glue.create-table-optimizer-on-nonexistent-table-fails|_error_code#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: glue.asset-type-lifecycle-put-get-delete|run#9|AssertionError|assert created["Id"] == asset_type_name, f"expected PutAssetType Id {asset_type_name!r}, got {created['Id']!r}"
Issue-Key: glue.integration-resource-property-lifecycle|run#9|AssertionError|assert created["ResourcePropertyArn"].endswith(f"integrationresourceproperty/database/{db_name}"), (
Issue-Key: glue.get-unfiltered-table-metadata-nonexistent-table-rejected|run#13|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: glue.get-column-statistics-task-run-invalid-input-exact-code|run#4|AssertionError|assert malformed_code == "ValidationException", (
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Glue: 6 net-new root causes — CFN-declared Glue databases/tables are absent under their declared names, CreateTableOptimizer and a plain CatalogInput are accepted where AWS refuses, GetTags rejects a session ARN, and three responses carry emulator-minted identities

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
6 causes; all six must be closed before submitting.

Related open issues, named for the fixer, none of which carries the replay keys listed here: 20261025T022820Z-glue-cfn-declared-resources-and-job-run-execution.md, 20261022T131025Z-glue-createjob-is-a-501-stub-gettable-ignores.md and 20261029T175551Z-glue-job-runs-reached-through-the-emr-glue.md describe the same CloudFormation defect as Cause 1 but are grounded in other checks; 20261027T124611Z-glue-3-net-new-root-causes-createcatalog-accepts.md describes the native-catalog acceptance of Cause 2 but is grounded in glue.create-catalog-native-unsupported-rejected / glue.catalog-federation-parameters-round-trip, not in the concurrency check listed here.

Cause 1 of 6: a stack declaring AWS::Glue::Database + AWS::Glue::Table reaches CREATE_COMPLETE but the database does not exist under its declared name.
AWS: Smithy: GetTable models EntityNotFoundException (client, HTTP 400) and takes DatabaseName+Name. A CloudFormation stack that declares AWS::Glue::Database with DatabaseInput.Name=<N> and an AWS::Glue::Table in it creates a Glue database whose name IS <N> (the declared name is the physical id for this resource type), so after CREATE_COMPLETE glue:GetTable(DatabaseName=<N>, Name=<T>) returns the table with its StorageDescriptor.Location and Columns.
Emulator: the stack reports CREATE_COMPLETE and exports the names, but glue:GetTable(DatabaseName=<N>, ...) is refused with EntityNotFoundException `Database <N> not found.` — nothing was created under the declared name (a sibling check shows the stack output carrying a synthesized logical id such as `<stack>-CatalogDatabase-17118343A41F` instead of the declared database name).
Why: the deploy's own response reports success while the resource is unreachable, so every read-after-deploy flow (Athena/EMR/Glue clients resolving the catalog by its declared name, and a stack UPDATE that changes the table's schema) fails against a stack CloudFormation says is complete.
Fix: the CloudFormation resource provisioning for AWS::Glue::Database / AWS::Glue::Table must create the Glue resources through the Glue catalog state under the declared DatabaseInput.Name / TableInput.Name and use that as the physical id (the CFN resource-type implementation for the Glue types, not the Glue handlers themselves).
Traces: athena__glue__s3.deploy-cfn-glue-database-table-s3-location-positive, athena__glue__s3.deploy-cfn-stack-update-glue-table-schema-positive.

Cause 2 of 6: a CreateCatalog carrying a plain CatalogInput is accepted, so a name race produces a winner instead of two validation refusals.
AWS: Smithy: CreateCatalog takes required Name + CatalogInput and models InvalidInputException and AlreadyExistsException (client, HTTP 400). A CatalogInput with no FederatedCatalog/federation configuration is not a supported catalog: real Glue refuses it with InvalidInputException, and validation precedes any name-uniqueness lock — so two concurrent identical plain-CatalogInput creates BOTH fail with InvalidInputException and no catalog is created.
Emulator: one racer gets 200 (an empty-body success) and the other gets AlreadyExistsException `Catalog already exists.`, i.e. the unsupported input was accepted and the name lock answered instead of the validator.
Why: an unsupported catalog shape becomes creatable, the validation-before-lock ordering is inverted, and a caller racing two identical creates gets a catalog AWS would never have made.
Fix: the CreateCatalog handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_catalog.rs) must validate CatalogInput before taking the name lock and refuse a non-federated CatalogInput with InvalidInputException.
Traces: glue.concurrent-catalog-create-conflict-rejected.

Cause 3 of 6: GetTags refuses a session ARN as untaggable.
AWS: Smithy: GetTags takes required ResourceArn (GlueResourceArn, @pattern ^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:) and models EntityNotFoundException/InvalidInputException; CreateSession accepts Tags (TagsMap) and Glue sessions are taggable resources whose tags are read back through the generic tagging system. So GetTags(ResourceArn=arn:aws:glue:<region>:<account>:session/<session-id>) returns the Tags supplied on CreateSession.
Emulator: the call is refused with InvalidInputException `ResourceArn must be the ARN of a taggable Glue resource naming one of connection, crawler, database, job; got "arn:aws:glue:<region>:<account>:session/<session-id>"` — the taggable-resource allow-list omits session (and the message is emulator-authored prose that echoes the request).
Why: tags supplied on CreateSession are unreadable, so the tag half of the session representation is lost and tag-based cost allocation/discovery for sessions is impossible.
Fix: the GetTags handler's resource-type allow-list and ARN parsing (services/service-com-amazonaws-glue-awsglue/src/ops/get_tags.rs) must accept session ARNs (and return the tags stored by CreateSession).
Traces: glue.session-description-and-tags-round-trip.

Cause 4 of 6: CreateTableOptimizer succeeds for a table that does not exist.
AWS: Smithy: CreateTableOptimizer takes required CatalogId/DatabaseName/TableName/Type/TableOptimizerConfiguration and models EntityNotFoundException (client, HTTP 400) among its errors. A CreateTableOptimizer naming a table that does not exist is refused with EntityNotFoundException; nothing is created.
Emulator: the call returns HTTP 200 with an empty body, so the optimizer is accepted against a nonexistent table.
Why: an overly-permissive accept — a caller gets no signal that the target table is missing, and a client's negative branch (catch EntityNotFoundException) never runs, so a typo in DatabaseName/TableName silently 'succeeds'.
Fix: the CreateTableOptimizer handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_table_optimizer.rs) must resolve the table first and refuse with EntityNotFoundException.
Traces: glue.create-table-optimizer-nonexistent-table-rejected, glue.create-table-optimizer-on-nonexistent-table-fails.

Cause 5 of 6: PutAssetType and CreateIntegrationResourceProperty mint opaque ids instead of the identities AWS derives.
AWS: Smithy: PutAssetType output carries Id (AssetTypeId) and Name (AssetTypeName); the registered asset type is addressed by its Name, and the Id real Glue returns IS that name, so a subsequent GetAssetType(Identifier=<Id>) and the Id echoed by PutAssetType agree with the submitted Name. CreateIntegrationResourceProperty output carries ResourcePropertyArn (String512) built from the resource it describes: for a database resource it ends with `integrationresourceproperty/database/<database-name>`, derived from the request's ResourceArn.
Emulator: PutAssetType returns a random 32-hex id (`71c17df761c935e059d3540ff2338e90`) instead of the asset type name, and CreateIntegrationResourceProperty returns `arn:aws:glue:<region>:<account>:integrationresourceproperty/<32-hex>` instead of the resource-derived suffix.
Why: both identities are how a client addresses the resource afterwards; an opaque per-call hash is not reconstructible by a caller that knows only the name/ARN it submitted, so cross-references (and any ARN-based policy) break.
Fix: the PutAssetType and CreateIntegrationResourceProperty handlers (services/service-com-amazonaws-glue-awsglue/src/ops/put_asset_type.rs, create_integration_resource_property.rs) must derive Id/ResourcePropertyArn from the request rather than minting a random id.
Traces: glue.asset-type-lifecycle-put-get-delete, glue.integration-resource-property-lifecycle.

Cause 6 of 6: two operations classify a bad request as 'not found' instead of the error class AWS raises.
AWS: Smithy: GetUnfilteredTableMetadata models EntityNotFoundException, PermissionTypeMismatchException and InvalidInputException but is a Lake-Formation-credential-vended operation: a caller without the required LF authorization is refused with AccessDeniedException BEFORE the table's existence is considered, so a nonexistent table under an unauthorized caller yields AccessDeniedException, not EntityNotFoundException. GetColumnStatisticsTaskRun models ValidationException and EntityNotFoundException: a ColumnStatisticsTaskRunId that violates the member's constraints (here an id with embedded newlines) is a malformed request and is refused with ValidationException, again before any lookup.
Emulator: both answer EntityNotFoundException — authorization and input validation are performed after (or instead of) the existence check.
Why: the error code drives SDK error-type branching; a client is told 'this does not exist' when AWS says 'you are not allowed' or 'your request is malformed', which sends retry/remediation logic down the wrong path and can mask a permission problem as a missing resource.
Fix: order the checks correctly in the two handlers (services/service-com-amazonaws-glue-awsglue/src/ops/get_unfiltered_table_metadata.rs — LF authorization before lookup; get_column_statistics_task_run.rs — request validation before lookup).
Traces: glue.get-unfiltered-table-metadata-nonexistent-table-rejected, glue.get-column-statistics-task-run-invalid-input-exact-code.

## Evidence
Cause 1 — athena__glue__s3.deploy-cfn-glue-database-table-s3-location-positive:
  botocore.errorfactory.EntityNotFoundException: An error occurred (EntityNotFoundException) when calling the GetTable operation: Database <declared database name> not found.
  (same for athena__glue__s3.deploy-cfn-stack-update-glue-table-schema-positive; the stack itself was CREATE_COMPLETE. A sibling glue check shows the stack output as '<stack>-CatalogDatabase-17118343A41F' where the declared name was expected.)
Cause 2 — glue.concurrent-catalog-create-conflict-rejected:
  expected: both concurrent plain-CatalogInput creates refused with InvalidInputException (validation precedes the name lock)
  actual:   [(HTTP 200 empty body, None), (None, ('AlreadyExistsException', 'Catalog already exists.'))]
Cause 3 — glue.session-description-and-tags-round-trip:
  botocore.errorfactory.InvalidInputException: An error occurred (InvalidInputException) when calling the GetTags operation: ResourceArn must be the ARN of a taggable Glue resource naming one of connection, crawler, database, job; got "arn:aws:glue:us-west-2:000000000000:session/<session-id>"
  expected: the Tags supplied on CreateSession, returned by GetTags on the session ARN
Cause 4 — glue.create-table-optimizer-nonexistent-table-rejected and glue.create-table-optimizer-on-nonexistent-table-fails:
  expected: EntityNotFoundException; actual: HTTP 200 with an empty body
Cause 5 — glue.asset-type-lifecycle-put-get-delete: expected PutAssetType Id '<submitted asset type name>', got '71c17df761c935e059d3540ff2338e90'
        glue.integration-resource-property-lifecycle: expected ResourcePropertyArn ending with 'integrationresourceproperty/database/<database-name>', got '…:integrationresourceproperty/c63b823394cb75122c34602cc02c21e7'
Cause 6 — glue.get-unfiltered-table-metadata-nonexistent-table-rejected: expected AccessDeniedException, got EntityNotFoundException
        glue.get-column-statistics-task-run-invalid-input-exact-code: expected ValidationException for the run id 'bad\nrun\nid', got EntityNotFoundException

## Steps to Reproduce
Cause 1: deploy a stack with AWS::Glue::Database (DatabaseInput.Name=N) and AWS::Glue::Table (TableInput.Name=T, StorageDescriptor.Location=s3://<bucket>/data/). After CREATE_COMPLETE, glue:GetTable(DatabaseName=N, Name=T) must return the table; today it is EntityNotFoundException 'Database N not found.'. Then UPDATE the stack adding a column and re-read GetTable.
Cause 2: fire two concurrent CreateCatalog(Name=C, CatalogInput={'Description':'x'}) calls from a barrier -> BOTH must fail InvalidInputException and GetCatalog(C) must not find a catalog.
Cause 3: CreateSession(Id=S, Role=R, Command={'Name':'glueetl','PythonVersion':'3.9'}, Description=<text>, Tags={'team':'data-eng','purpose':'x'}), then GetTags(ResourceArn='arn:aws:glue:<region>:<account>:session/S') -> must return those tags.
Cause 4: CreateTableOptimizer(CatalogId=<account>, DatabaseName=<existing db>, TableName=<no such table>, Type='compaction', TableOptimizerConfiguration={'roleArn':R,'enabled':True}) -> EntityNotFoundException.
Cause 5: PutFormType(Name=F, Schema=...) then PutAssetType(Name=A, Forms={...}) -> the returned Id must equal A; CreateIntegrationResourceProperty(ResourceArn=<glue database ARN>, SourceProcessingProperties={...}) -> ResourcePropertyArn must end with 'integrationresourceproperty/database/<database name>'.
Cause 6: GetUnfilteredTableMetadata(CatalogId=<account>, DatabaseName=D, Name=<no such table>, SupportedPermissionTypes=['COLUMN_PERMISSION']) as a caller without the LF grant -> AccessDeniedException; GetColumnStatisticsTaskRun(ColumnStatisticsTaskRunId='bad\nrun\nid') -> ValidationException.
