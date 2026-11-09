Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue.deploy-glue-catalog-encryption-settings-singleton-round-trip-and-delete cloudformation__glue.deploy-glue-connection-type-lifecycle cloudformation__glue.deploy-glue-custom-entity-type-lifecycle cloudformation__glue.deploy-glue-data-quality-ruleset-and-udf-lifecycle cloudformation__glue.deploy-glue-integration-resource-property-round-trip-and-delete cloudformation__glue.deploy-glue-registry-schema-schemaversion-and-metadata-round-trip-and-delete cloudformation__glue.deploy-glue-classifier-lifecycle cloudformation__glue.deploy-glue-classifier-invalid-grok-pattern-rejected cloudformation__glue.deploy-glue-connection-lifecycle cloudformation__glue.deploy-glue-connection-invalid-connection-type-rejected cloudformation__glue.deploy-glue-database-table-partition-lifecycle cloudformation__glue.deploy-glue-schema-duplicate-name-rejected-and-rolled-back
Issue-Key: cloudformation__glue|CreateStack|ValidationError
Issue-Key: cloudformation__glue.deploy-glue-classifier-lifecycle|run#11|AssertionError|assert (ref, listed) == (name, [(name, classification, _GROK)]), (
Issue-Key: cloudformation__glue.deploy-glue-classifier-invalid-grok-pattern-rejected|run#9|AssertionError|assert control == [("sim2real-log", _GROK)], f"control classifier reads {control!r}"
Issue-Key: cloudformation__glue.deploy-glue-connection-lifecycle|run#12|AssertionError|assert (ref, listed) == (name, [(name, "JDBC", jdbc_url, username)]), (
Issue-Key: cloudformation__glue.deploy-glue-connection-invalid-connection-type-rejected|run#11|AssertionError|assert control == ["JDBC"], f"control connection reads {control!r}"
Issue-Key: cloudformation__glue.deploy-glue-database-table-partition-lifecycle|run#23|AssertionError|assert part_ref[:3] == [ctx.account, db_name, tbl_name] and len(part_ref) == 4, (
Issue-Key: cloudformation__glue.deploy-glue-schema-duplicate-name-rejected-and-rolled-back|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__glue: 4 net-new causes — eight AWS::Glue::* types unregistered, classifiers/connections absent from Glue after CREATE_COMPLETE, wrong Partition Ref, duplicate schema name accepted

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four distinct root causes in the CloudFormation+Glue surface. Each is numbered with its own failing check ids; all four must be closed before this issue is done.

Cause 1 of 4: CloudFormation does not register eight modelled AWS::Glue::* resource types.
AWS: AWS::Glue::DataCatalogEncryptionSettings, ConnectionType, CustomEntityType, DataQualityRuleset, UserDefinedFunction, IntegrationResourceProperty, SchemaVersion and SchemaVersionMetadata are public CloudFormation resource types, so CreateStack accepts templates declaring them and provisions them.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [<the bracketed list of types from the request>]`.
Why: Glue catalog encryption, connection types, custom entity types, data-quality rulesets, UDFs, integration resource properties and schema versions/metadata cannot be deployed through CloudFormation at all.
Fix: register those types in the CloudFormation resource-type registry and add their resource handlers.
Traces: cloudformation__glue.deploy-glue-catalog-encryption-settings-singleton-round-trip-and-delete, .deploy-glue-connection-type-lifecycle, .deploy-glue-custom-entity-type-lifecycle, .deploy-glue-data-quality-ruleset-and-udf-lifecycle, .deploy-glue-integration-resource-property-round-trip-and-delete, .deploy-glue-registry-schema-schemaversion-and-metadata-round-trip-and-delete.

Cause 2 of 4: classifiers and connections a stack reports CREATE_COMPLETE do not exist in Glue.
AWS: after CREATE_COMPLETE, GetClassifiers returns the classifier the stack created and GetConnections returns its connection, each findable by the name the stack's Ref reports.
Emulator: GetClassifiers and GetConnections both return an empty list for the stack's Ref, and the "control" (valid) resource of each negative check also reads back as [].
Why: a client that provisions Glue classifiers/connections with CloudFormation and then uses them from a crawler or job finds nothing in the Data Catalog.
Fix: the AWS::Glue::Classifier and AWS::Glue::Connection resource handlers must create the entity in Glue state.
Traces: cloudformation__glue.deploy-glue-classifier-lifecycle, .deploy-glue-classifier-invalid-grok-pattern-rejected, .deploy-glue-connection-lifecycle, .deploy-glue-connection-invalid-connection-type-rejected.

Cause 3 of 4: AWS::Glue::Partition Ref is a generated stack name rather than the partition's identifier.
AWS: Ref on AWS::Glue::Partition returns the composite partition identifier (catalog id, database, table and partition values), not a stack-generated name, and the partition is then findable through GetPartition/GetPartitions.
Emulator: Ref returns `<stack-name>-<logical-id>-<suffix>`, which identifies no partition in the Data Catalog.
Why: a template that references its partition — or a client that reads the Ref to look the partition up — is handed a value Glue cannot resolve.
Fix: the AWS::Glue::Partition resource handler's physical-resource-id/Ref resolution and its Data Catalog write.
Traces: cloudformation__glue.deploy-glue-database-table-partition-lifecycle.

Cause 4 of 4: a duplicate Glue schema name is accepted and the stack reaches CREATE_COMPLETE.
AWS: CreateSchema refuses a schema name already registered in the registry (AlreadyExistsException), so CloudFormation marks the resource CREATE_FAILED and the stack ROLLBACK_COMPLETE.
Emulator: the stack ends `CREATE_COMPLETE`.
Why: two stacks can silently claim the same schema name, so a client believes it owns a schema that another stack also owns.
Fix: the AWS::Glue::Schema resource handler's uniqueness check / propagation of the Glue refusal.
Traces: cloudformation__glue.deploy-glue-schema-duplicate-name-rejected-and-rolled-back.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Glue::DataCatalogEncryptionSettings] (and the analogous list for ConnectionType, CustomEntityType, DataQualityRuleset+UserDefinedFunction, IntegrationResourceProperty, SchemaVersion+SchemaVersionMetadata).
Cause 2: AssertionError: "Ref '<stack>-Cls-<suffix>', GetClassifiers []"; "Ref '<stack>-Conn-<suffix>', GetConnections []"; 'control classifier reads []'; 'control connection reads []'.
Cause 3: AssertionError: "Ref(Part) 'sim2real-behav-cfn-glucat-<id>-Part-<suffix>'" — expected the partition identifier, not a generated name.
Cause 4: AssertionError: "stack '<name>' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE".

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: CreateStack with each of the eight listed AWS::Glue::* types.
3. Cause 2: deploy AWS::Glue::Classifier and AWS::Glue::Connection, then call GetClassifiers / GetConnections.
4. Cause 3: deploy AWS::Glue::Database + Table + Partition and read the Partition Ref.
5. Cause 4: deploy an AWS::Glue::Schema whose name is already registered and watch the final stack status.
