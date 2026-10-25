Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.lf-list-permissions-principal-filter-attributes-iam-allowed-principals-grant-to-the-principal
Issue-Key: glue__iam__lakeformation__s3.lf-list-permissions-principal-filter-attributes-iam-allowed-principals-grant-to-the-principal|run#17|AssertionError|assert ("IAM_ALLOWED_PRINCIPALS", "Database", ("ALL",)) in everyone, (

# Lake Formation ListPermissions does not report the grant a Data Catalog database is created carrying

## Operation
ListPermissions

## Description
A Data Catalog DATABASE is created carrying the account's "use only IAM access control" default grant — `ALL` to `IAM_ALLOWED_PRINCIPALS` — and `ListPermissions` on that database reports it as an entry: `{Principal:{DataLakePrincipalIdentifier:'IAM_ALLOWED_PRINCIPALS'}, Resource:{Database:{...}}, Permissions:['ALL']}`. The emulator reports an empty list for such a database: nothing granted that default, so there is no record in `permission_grants` for the listing to project.

The default itself IS modelled in this emulator — `crate::state::default_database_grant` renders it, and `RevokePermissions` honours it (and remembers a revoke in `revoked_default_grants`, which is what makes it revocable exactly once) — so what is missing is only the READ: `services/service-com-amazonaws-lakeformation-awslakeformation/src/ops/list_permissions.rs` enumerates stored records and never renders the default for the database a request names. The handler's own doc records the reasoning that was adopted when nothing could resolve a database name (`ListPermissions` "returns only those permissions that have been explicitly granted", and with no view of the catalog every database name would otherwise carry a default); that constraint is gone — `crate::state::missing_grant_entity` now resolves a database against the Data Catalog by reading Glue's own store in the shared engine, so the default can be rendered for a database the catalog actually holds and for no other.

Things a fix has to settle, none of which the grant/revoke paths answer today:
* WHICH requests render it: the resource filter has to name a database (there is no set of databases to enumerate, so an unfiltered listing cannot report one), the principal filter has to be absent or name `IAM_ALLOWED_PRINCIPALS`, and a `ResourceType` filter has to admit `DATABASE`;
* that a revoked default (`revoked_default_grants`) is NOT reported;
* how the synthetic entry takes part in pagination, whose continuation token names the stored key a page stopped on.

This issue was separated from `20261023T123230Z-lakeformation-lake-formation-grantpermissions-stores-a-table-grant-verbatim.md`, which fixed the two causes it described (the Table/TableWithColumns scope split and the refusal of a grant on a resource the catalog does not hold). The replay unit above gets past both of those and then failed on this assertion.

RESOLVED: the read landed — `crate::state::default_database_grants` renders the default each addressed database was created carrying and `ListPermissions` projects it beside the stored records, skipping one a revoke took away (`revoked_default_grants`); `tests/all/default_database_grant.rs` covers which requests render it.

## Evidence
Replaying `glue__iam__lakeformation__s3.lf-list-permissions-principal-filter-attributes-iam-allowed-principals-grant-to-the-principal`: `AssertionError: the database must carry the default IAM_ALLOWED_PRINCIPALS grant: []` — the listing of the governed database the unit created answered an empty `PrincipalResourcePermissions`.

## Steps to Reproduce
1. `glue:CreateDatabase` a database.
2. `lakeformation:ListPermissions` with `Resource={Database:{Name:<that database>}}`.
3. Observe an empty `PrincipalResourcePermissions` where AWS reports the `IAM_ALLOWED_PRINCIPALS` / `Database` / `['ALL']` entry.
