Issue-Source: trace-sweep-divergence
Issue-Trace: glue-catalog-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-catalog-lifecycle glue-catalog-read-and-error-paths
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetCatalog|2xx->2xx|$.Catalog.CatalogIdentifier
Issue-Key: glue|GetCatalog|2xx->2xx|$.Catalog.CatalogProperties
Issue-Key: glue|GetCatalog|2xx->2xx|$.Catalog.CatalogType
Issue-Key: glue|GetCatalog|2xx->2xx|$.Catalog.CreateDatabaseDefaultPermissions
Issue-Key: glue|GetCatalog|2xx->2xx|$.Catalog.CreateTableDefaultPermissions
Issue-Key: glue|GetCatalog|2xx->2xx|$.Catalog.Description
Issue-Commit: 4ad8e7b52fc0fe74ab6af4437a1188a6a348c83e

# Glue GetCatalog returns a Catalog object stripped of its identity, type, properties and default-permission members

> _Note: this issue was identified against the code base at commit `4ad8e7b52fc0fe74ab6af4437a1188a6a348c83e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCatalog

## Description
1 cause.

Cause 1 of 1: GetCatalog answers 200 but the Catalog structure it serializes carries only a bare subset of its members — the catalog's identity (CatalogIdentifier), its kind (CatalogType), its properties map (CatalogProperties), its description and both Lake-Formation default-permission lists are all absent from the body.

AWS: Smithy (com.amazonaws.glue#AWSGlue, GetCatalog -> GetCatalogResponse.Catalog -> com.amazonaws.glue#Catalog) models Description (DescriptionString, length 0..2048), CatalogProperties (CatalogPropertiesOutput), CreateTableDefaultPermissions (PrincipalPermissionsList) and CreateDatabaseDefaultPermissions (PrincipalPermissionsList) on the Catalog structure; AWS returns each of them on a read, emitting the empty map / empty list rather than omitting the member when nothing is configured. CatalogIdentifier and CatalogType are not in the on-disk model, so they are recorded behavior only: a read of the account's default catalog returns CatalogType with the fixed enum value 'NATIVE', CatalogIdentifier set to the catalog's own identifier (the caller's 12-digit account id for the default catalog), and Description set to Glue's fixed default-catalog text 'This is the Default Catalog.'. In that recorded response the FIXED text is 'This is the Default Catalog.' and the fixed enum is 'NATIVE'; the only request/state-derived value is CatalogIdentifier (the account id, rendered as 000000000000 in this environment) — the account id itself is environment-specific and is not part of the requirement.

Emulator: the response is 200 with the Catalog structure present but every one of those members missing — $.Catalog.CatalogIdentifier, $.Catalog.CatalogType, $.Catalog.CatalogProperties, $.Catalog.Description, $.Catalog.CreateTableDefaultPermissions and $.Catalog.CreateDatabaseDefaultPermissions all come back absent.

Why: a client cannot tell which catalog it read (no identifier), cannot tell a native catalog from a federated or Redshift-target one (no CatalogType), and cannot distinguish 'no default permissions configured' from 'not reported' — SDK code that dereferences catalog.catalogProperties() or iterates createTableDefaultPermissions() without a null check breaks on exactly the read it needs. Tooling that round-trips a catalog (read, modify, UpdateCatalog) silently drops the description and both permission lists.

Fix: the GetCatalog handler's Catalog serializer (and the shared catalog projection it uses for the account's implicit default catalog) must emit the full modeled Catalog structure, including empty collections and the default catalog's identifier, CatalogType 'NATIVE' and its fixed description.

Traces: glue-catalog-lifecycle#task#1 (same cluster: glue-catalog-read-and-error-paths#task#2).

Related but NOT covering this: open issue 20261029T201859Z-glue-responses-omit-modeled-members-whose-value-is.md bundles the same class of omission for GetClassifier, GetSecurityConfiguration(s), GetDataCatalogEncryptionSettings, GetPartition, BatchGetPartition, DeleteSchemaVersions and GetWorkflow, but it does not carry GetCatalog's replay key and none of its causes touch the catalog surface.

## Evidence
Replay of simparity-wire-aws-v1, fixture glue-catalog-lifecycle step task#1 (GetCatalog), expected status 200, actual status 200 — body mismatch:
  $.Catalog.CatalogIdentifier: missing (expected '000000000000', got None)
  $.Catalog.CatalogProperties: missing (expected {}, got None)
  $.Catalog.CatalogType: missing (expected 'NATIVE', got None)
  $.Catalog.CreateDatabaseDefaultPermissions: missing (expected [], got None)
  $.Catalog.CreateTableDefaultPermissions: missing (expected [], got None)
  $.Catalog.Description: missing (expected 'This is the Default Catalog.', got None)

Same cluster (divergence_key service=glue|GetCatalog|status=200->200|keys=$.Catalog.CatalogIdentifier,$.Catalog.CatalogProperties,$.Catalog.CatalogType,$.Catalog.CreateDatabaseDefaultPermissions,$.Catalog.CreateTableDefaultPermissions,$.Catalog.Description), also affecting glue-catalog-read-and-error-paths#task#2.

Expected per model: Catalog.Description, Catalog.CatalogProperties, Catalog.CreateTableDefaultPermissions, Catalog.CreateDatabaseDefaultPermissions are modeled members of com.amazonaws.glue#Catalog. CatalogType ('NATIVE') and CatalogIdentifier are recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Re-replay glue-catalog-lifecycle#task#1 and glue-catalog-read-and-error-paths#task#2.
3. Each step calls GetCatalog for the account's catalog and asserts the returned Catalog structure carries CatalogIdentifier, CatalogType ('NATIVE'), CatalogProperties, Description ('This is the Default Catalog.') and both CreateTableDefaultPermissions / CreateDatabaseDefaultPermissions lists (empty when unconfigured).
4. The emulator answers 200 with all six members absent.
