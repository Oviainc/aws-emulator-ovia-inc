Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# DatabaseArn is persisted but never surfaces anywhere: databases are derived only from Credentials, so an application registered with a DatabaseArn and no credentials has no databases

_Filed: 2026-10-24T15:17:25Z_
_Operation: RegisterApplication | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

RegisterApplication's DatabaseArn member is documented as "The Amazon Resource Name of the SAP HANA database" of the application being registered. The emulator stores it on StoredApplication.database_arn and then never reads it: StoredApplication::databases() builds the application's database set exclusively from the DatabaseName of each ApplicationCredential, and no implemented projection (Application view, ApplicationSummary, DatabaseSummary, Operation) carries the stored ARN.

Consequences a caller can observe through implemented operations: (a) an application registered with DatabaseArn and no Credentials (legal — Credentials has default [] and is only conditionally required for HANA) reports an EMPTY ListDatabases result even though the registration named its HANA database; (b) the database set and the DatabaseId/Arn values are synthesized from credential names rather than from the database the registration pointed at, so the ARNs ListDatabases reports for such an application do not agree with the DatabaseArn that was registered.

## Evidence

Smithy model: RegisterApplicationInput$DatabaseArn (SsmSapArn) — "The Amazon Resource Name of the SAP HANA database"; ListDatabases returns DatabaseSummary{ApplicationId, ComponentId, DatabaseId, DatabaseType, Arn, Tags}. Handler/state: src/ops/register_application.rs stores `database_arn: input.database_arn`; src/state.rs StoredApplication::databases() iterates `self.credentials` only and mints ARNs with database_arn(region, app_id, database_id_from_name(credential.database_name)) — `database_arn` field is never read by any projection (grep: it is written once and never consumed). Tests: tests/all/application_lifecycle.rs registers a case WITH DatabaseArn but asserts nothing about it; tests/all/list_databases.rs pins the credential-derived listing.

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"ABAP-DB","ApplicationType":"SAP_ABAP","Instances":["i-00000000000000001"],"DatabaseArn":"arn:aws:ssm-sap:us-east-1:000000000000:database/HDB-TEST/HDB"} (no Credentials)
2. POST /list-databases {"ApplicationId":"ABAP-DB"}
3. Observe Databases: [] — the registered DatabaseArn is nowhere in any response, whereas the real service associates that HANA database with the application and reports it
