Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.lf-list-permissions-principal-filter-attributes-iam-allowed-principals-grant-to-the-principal
Issue-Key: glue__iam__lakeformation__s3.lf-list-permissions-principal-filter-attributes-iam-allowed-principals-grant-to-the-principal|run#17|AssertionError|assert ("IAM_ALLOWED_PRINCIPALS", "Database", ("ALL",)) in everyone, (
Issue-Commit: 2638cac857b16c670648ae4436fc785f6120a46b

# A freshly created Glue database carries no default IAM_ALLOWED_PRINCIPALS grant, so ListPermissions on the Database resource is empty and the Principal filter has nothing to attribute

> _Note: this issue was identified against the code base at commit `2638cac857b16c670648ae4436fc785f6120a46b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPermissions

## Description
Cause 1 of 1: CreateDatabase does not seed the Lake Formation default database grant, so Lake Formation's view of a brand-new Glue database has no permissions at all.

AWS: An account's Lake Formation DataLakeSettings carry CreateDatabaseDefaultPermissions, which by default holds a single entry granting Permissions=["ALL"] to the DataLakePrincipalIdentifier "IAM_ALLOWED_PRINCIPALS" (Smithy: lakeformation GetDataLakeSettings -> output DataLakeSettings, POST /GetDataLakeSettings, 200). When a Glue database is created in such an account, Lake Formation records that database-level grant for the new database, so ListPermissions with Resource={"Database":{"Name":<db>}} returns a PrincipalResourcePermissions entry whose Principal.DataLakePrincipalIdentifier is "IAM_ALLOWED_PRINCIPALS", whose Resource is the Database, and whose Permissions are ["ALL"] (Smithy: lakeformation ListPermissions, input Principal/Resource/ResourceType/MaxResults(range 1..1000), output PrincipalResourcePermissionsList + NextToken, POST /ListPermissions, 200; modeled errors InvalidInputException 400, OperationTimeoutException 400, InternalServiceException 500). Passing DatabaseInput.CreateTableDefaultPermissions=[] on CreateDatabase suppresses only the default grant inherited by TABLES created in that database; it does not remove the DATABASE-level IAM_ALLOWED_PRINCIPALS grant. Recorded behavior additionally shows the two reading routes diverging while that grant exists: ListPermissions WITH a Principal filter naming an ungranted role resolves the group grant onto that role and answers exactly one Database/ALL entry under the role's own ARN, while the unfiltered listing filtered client-side by that role's ARN finds nothing; after RevokePermissions of ["ALL"] from IAM_ALLOWED_PRINCIPALS on the database, the filtered route goes empty too.

Emulator: immediately after glue CreateDatabase (DatabaseInput.CreateTableDefaultPermissions=[]), ListPermissions with Resource={"Database":{"Name":<db>}} returns an empty PrincipalResourcePermissions list - no IAM_ALLOWED_PRINCIPALS/Database/ALL entry is present. The check fails at the assertion that the database must carry the default grant, with the observed listing []. The governed-table assertions that run BEFORE it all passed (the granted role's table grant came back correctly normalised into one Table/DESCRIBE entry plus one TableWithColumns/SELECT entry, and the ungranted role held nothing), so this is not the verbatim-table-grant defect of the open issue 20261023T123230Z-lakeformation-lake-formation-grantpermissions-stores-a-table-grant-verbatim.md - that one is already past. Because the default grant never exists, the two downstream behaviors of the same cause are also unreachable: the server-side Principal filter has no group grant to attribute to the queried role, and the revoke of IAM_ALLOWED_PRINCIPALS on the database is a no-op.

Why: a client that reads effective Lake Formation permissions sees a newly created database as fully locked down instead of open to every IAM principal, which is the opposite of the real default. Governance tooling that enumerates grants to decide what to revoke before switching a database to Lake-Formation-managed access concludes there is nothing to revoke and silently skips the hardening step; code that asks "what does this role hold on this database?" through the server-side Principal filter gets an empty answer where AWS reports Database/ALL, so it wrongly decides the role needs a grant it already has.

Fix: the Lake Formation permission state seeded at database creation - the glue CreateDatabase handler must register the account DataLakeSettings' CreateDatabaseDefaultPermissions (default IAM_ALLOWED_PRINCIPALS -> ALL) as a Database-level grant, and DatabaseInput.CreateTableDefaultPermissions must only govern table-level defaults. The lakeformation ListPermissions handler must then (a) return that grant under the IAM_ALLOWED_PRINCIPALS principal in the unfiltered listing and (b) resolve group grants onto the queried principal when a Principal filter is supplied, rather than matching the principal identifier literally, with RevokePermissions on IAM_ALLOWED_PRINCIPALS removing both views.

Traces: glue__iam__lakeformation__s3.lf-list-permissions-principal-filter-attributes-iam-allowed-principals-grant-to-the-principal

## Evidence
Expected (real AWS): ListPermissions(Resource={"Database":{"Name":<db>}}) right after CreateDatabase contains the entry (Principal="IAM_ALLOWED_PRINCIPALS", Resource key "Database", Permissions=("ALL",)); ListPermissions(Resource=<same>, Principal={"DataLakePrincipalIdentifier":<ungranted role arn>}) returns exactly [(<role arn>, "Database", ("ALL",))]; the unfiltered listing contains no entry naming that role; after RevokePermissions(Principal=IAM_ALLOWED_PRINCIPALS, Resource=<database>, Permissions=["ALL"]) the filtered listing becomes [].

Actual (emulator): the unfiltered Database listing is []. Replay failure:
  AssertionError: the database must carry the default IAM_ALLOWED_PRINCIPALS grant: []
  at check_glue__iam__lakeformation__s3_lf_list_permissions_principal_filter_attributes_iam_allowed_principals_grant_to_the_principal.py line 76:
  assert ("IAM_ALLOWED_PRINCIPALS", "Database", ("ALL",)) in everyone

The preceding governed-table assertions in the same check passed, which bounds the defect to the database default grant and the Principal-filter resolution rather than to grant normalisation.

## Steps to Reproduce
1. glue CreateDatabase with DatabaseInput={"Name": "<sql_safe_db>", "CreateTableDefaultPermissions": []} (the account DataLakeSettings are left at their defaults).
2. glue CreateTable in that database (any EXTERNAL_TABLE; the check uses a parquet table "usage_events").
3. iam CreateRole for two roles trusted by glue.amazonaws.com, one of which is never granted anything.
4. lakeformation GrantPermissions to the first role on Resource={"Table":{"DatabaseName":<db>,"Name":"usage_events"}} with Permissions=["SELECT","DESCRIBE"].
5. lakeformation ListPermissions with Resource={"Database":{"Name":<db>}} and no Principal -> AWS includes (IAM_ALLOWED_PRINCIPALS, Database, ["ALL"]); the emulator returns an empty list.
6. lakeformation ListPermissions with the same Resource plus Principal={"DataLakePrincipalIdentifier": <ungranted role arn>} -> AWS returns one Database/ALL entry under that role's ARN; the emulator returns an empty list.
7. lakeformation RevokePermissions(Principal=IAM_ALLOWED_PRINCIPALS, Resource={"Database":{"Name":<db>}}, Permissions=["ALL"]) and repeat step 6 -> AWS now returns [].
