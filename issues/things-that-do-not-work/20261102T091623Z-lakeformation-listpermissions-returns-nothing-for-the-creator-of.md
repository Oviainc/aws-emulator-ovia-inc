Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.lf-governed-database-table-grants-creator-only-and-omits-iam-allowed-principals
Issue-Key: glue__iam__lakeformation__s3.lf-governed-database-table-grants-creator-only-and-omits-iam-allowed-principals|run#3|AssertionError|assert len(perms) >= 2, (
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# lakeformation: ListPermissions returns nothing for the creator of a governed database's table

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPermissions

## Description
Cause 1 of 1: the implicit permissions Lake Formation records for the creator of a table in a governed database are never stored, so ListPermissions is empty.
AWS: when a table is created in a database whose CreateTableDefaultPermissions are empty (a governed database), Lake Formation grants the creating principal the table permissions implicitly, and ListPermissions for that resource returns at least two entries for the creator — the table permission and the column-wildcard (ALL table columns) permission — and does NOT list IAMAllowedPrincipals.
Emulator: ListPermissions returns [] for that resource, so neither the creator's table permission nor its column-wildcard permission exists.
Why: an operator auditing who holds access to a governed table is told nobody does, and any flow that revokes or diffs the creator's implicit grants has nothing to act on.
Fix: the Lake Formation permission store must record the creator's implicit table and column-wildcard grants at table creation and surface them through ListPermissions.
Traces: glue__iam__lakeformation__s3.lf-governed-database-table-grants-creator-only-and-omits-iam-allowed-principals.

## Evidence
'AssertionError: the creator must hold table and column-wildcard permissions, ListPermissions returned []'.

## Steps to Reproduce
1. Create a database with empty CreateTableDefaultPermissions (governed) and create a table in it.
2. Call lakeformation ListPermissions for that table resource and observe an empty list where the creator's table and column-wildcard permissions are expected.
3. Or replay glue__iam__lakeformation__s3.lf-governed-database-table-grants-creator-only-and-omits-iam-allowed-principals.
