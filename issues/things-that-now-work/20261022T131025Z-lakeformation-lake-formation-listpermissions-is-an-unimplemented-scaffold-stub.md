Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.lf-revoke-of-absent-iam-allowed-principals-grant-rejected-and-grant-is-idempotent
Issue-Key: glue__iam__lakeformation__s3|ListPermissions|NotImplemented
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# Lake Formation ListPermissions is an unimplemented scaffold stub: HTTP 501 NotImplemented instead of the principal/resource permission list

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPermissions

## Description
Cause 1 of 1: ListPermissions is never served.
AWS: Smithy com.amazonaws.lakeformation#ListPermissions is an HTTP POST /ListPermissions returning 200 with PrincipalResourcePermissions (PrincipalResourcePermissionsList) and NextToken; it is @paginated (inputToken NextToken / outputToken NextToken / pageSize MaxResults, MaxResults range 1..1000) and accepts Principal, Resource, ResourceType (CATALOG|DATABASE|DATA_LOCATION|LF_TAG|LF_TAG_POLICY|...|TABLE) and IncludeRelated as filters. Its doc states it returns only explicitly granted permissions, filtered by the caller's own permissions. Its only modeled errors are InternalServiceException (500), InvalidInputException (400) and OperationTimeoutException (400) — HTTP 501 / NotImplemented is not a modeled outcome.
Emulator: every ListPermissions call fails with HTTP 501, error code NotImplemented, message 'operation ListPermissions is scaffolded but not implemented in this emulator'. services/service-com-amazonaws-lakeformation-awslakeformation/src/ops/list_permissions.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() returns Err(crate::scaffolded::Scaffolded::not_implemented("ListPermissions")) (line 185).
Why: ListPermissions is the only way a client reads back Lake Formation grant state, so no grant can be verified, no idempotence of GrantPermissions can be observed (grant twice -> exactly one entry), and no diagnosis that filters permissions by principal can run. Every caller that grants and then confirms is blocked.
Fix: implement the handler in services/service-com-amazonaws-lakeformation-awslakeformation/src/ops/list_permissions.rs — project the grant records held in the service state (crate::state) as PrincipalResourcePermissions, honour the Principal/Resource/ResourceType filters, and paginate with NextToken/MaxResults.
Traces: glue__iam__lakeformation__s3.lf-revoke-of-absent-iam-allowed-principals-grant-rejected-and-grant-is-idempotent. (The same check also exercises RevokePermissions/GrantPermissions, whose stub issues are filed separately by this run; this issue is the ListPermissions read path only.)

## Evidence
Expected: HTTP 200 with PrincipalResourcePermissions listing the explicit grants on the named resource (one entry after granting IAM_ALLOWED_PRINCIPALS twice, none after the revoke). Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListPermissions operation: operation ListPermissions is scaffolded but not implemented in this emulator — HTTP 501, a status/code the model does not declare for this operation. Handler: services/service-com-amazonaws-lakeformation-awslakeformation/src/ops/list_permissions.rs:185.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite check glue__iam__lakeformation__s3.lf-revoke-of-absent-iam-allowed-principals-grant-rejected-and-grant-is-idempotent.
2. Or directly: create a Glue database and table, then call lakeformation:ListPermissions with Resource={"Table":{"DatabaseName":<db>,"Name":<table>}} -> HTTP 501 NotImplemented instead of 200 with PrincipalResourcePermissions.
