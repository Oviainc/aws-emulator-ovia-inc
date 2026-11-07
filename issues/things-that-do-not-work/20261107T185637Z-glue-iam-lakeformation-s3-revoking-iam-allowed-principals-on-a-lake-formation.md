Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.lf-database-iap-revoked-get-database-denied-until-a-table-grant
Issue-Key: glue__iam__lakeformation__s3.lf-database-iap-revoked-get-database-denied-until-a-table-grant|run#11|AssertionError|assert denied == expected, (
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# Revoking IAM_ALLOWED_PRINCIPALS on a Lake Formation database does not make GetDatabase refuse

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDatabase

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issues 20261102T091623Z-lakeformation-listpermissions-returns-nothing-for-the-creator-of.md and 20261025T000319Z-glue-iam-lakeformation-s3-a-glue-job-run.md own other checks of this service and do not carry this replay key.

Cause 1 of 1: Glue GetDatabase does not consult Lake Formation permissions.
AWS: once the IAM_ALLOWED_PRINCIPALS grant is revoked from a database, a principal holding no Lake Formation grant on it is refused GetDatabase with HTTP 400, __type AccessDeniedException and message "Insufficient Lake Formation permission(s): Required Describe on <database-name>" — the fixed text is "Insufficient Lake Formation permission(s): Required Describe on " and the variable part is the database name from the request. Access is restored by a Lake Formation grant (a table grant on a table in that database is enough to make the database describable).
Emulator: GetDatabase keeps returning 200 with the database after the revoke — the check records that it "returned 'OK'" where the refusal above was required. Lake Formation's permission state has no effect on the Glue catalog read path.
Why: Lake Formation governance is not enforced on the catalog: revoking IAM_ALLOWED_PRINCIPALS, the first step of locking a database down, changes nothing, so a client validating a least-privilege data-lake setup gets a false pass and a principal that should be blind to a database can enumerate it.
Fix: the Glue GetDatabase (and the sibling catalog read) handler's authorization step — it must evaluate the caller's Lake Formation permissions on the database (Describe, or an inherited grant from a table) and emit AccessDeniedException with the message above when none applies.
Traces: glue__iam__lakeformation__s3.lf-database-iap-revoked-get-database-denied-until-a-table-grant.

## Evidence
Expected 'AccessDeniedException: Insufficient Lake Formation permission(s): Required Describe on <database>' after revoking IAM_ALLOWED_PRINCIPALS; got 200 'OK' with the database.

## Steps to Reproduce
1) Create a Glue database; confirm the IAM_ALLOWED_PRINCIPALS grant exists.
2) RevokePermissions of ALL from IAM_ALLOWED_PRINCIPALS on that database.
3) As a principal with no Lake Formation grant, call GetDatabase; then grant SELECT/DESCRIBE on a table in it and call again.
