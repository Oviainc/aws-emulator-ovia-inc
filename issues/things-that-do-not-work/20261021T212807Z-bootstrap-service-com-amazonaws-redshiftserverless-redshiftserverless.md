Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the redshift-serverless emulator

_Filed: 2026-10-21T21:28:07Z_
_Operation: service-com-amazonaws-redshiftserverless-redshiftserverless | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 65 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Redshift Serverless client can do anything. Implement a FIRST, SMALL slice: the namespace + workgroup lifecycle, which is the core resource pair of Redshift Serverless (a namespace holds the database/users, a workgroup holds the compute and is attached to a namespace).

Wire protocol (from OPERATIONS.md): awsJson1_1, `POST /` with `X-Amz-Target: RedshiftServerless.<OperationName>`, JSON body using the modeled member names. sigv4 signing name `redshift-serverless`.

### Happy path to implement, in order

1. **CreateNamespace** — input `namespaceName` (required, len 3..64, `^[a-z0-9-]+$`), plus optional `adminUsername`, `adminUserPassword`, `dbName`, `kmsKeyId`, `defaultIamRoleArn`, `iamRoles`, `logExports`, `tags`, `manageAdminPassword`. Output: `namespace` (Namespace). Must INSERT a new namespace record and return the stored record, with server-derived fields filled in: `namespaceId` (uuid), `namespaceArn` (`arn:aws:redshift-serverless:<region>:<account>:namespace/<namespaceId>`), `status` = `AVAILABLE`, `creationDate` = now, `dbName` defaulting to `dev` and `adminUsername` echoed from input only because it was stored. Re-creating an existing `namespaceName` must fail `ConflictException` (409); a name violating the length/pattern constraint must fail `ValidationException` (400).
2. **CreateWorkgroup** — input `workgroupName` (required, len 3..64, `^[a-z0-9-]+$`), `namespaceName` (required), optional `baseCapacity`, `maxCapacity`, `port`, `publiclyAccessible`, `enhancedVpcRouting`, `configParameters`, `securityGroupIds`, `subnetIds`, `tags`. Output: `workgroup` (Workgroup). Must look the namespace up in state and fail `ResourceNotFoundException` (404) when `namespaceName` does not exist; duplicate `workgroupName` -> `ConflictException` (409). Store and return `workgroupId` (uuid), `workgroupArn`, `status` = `AVAILABLE`, `creationDate` = now, `baseCapacity` defaulting to 128, `port` defaulting to 5439, and a derived `endpoint` (`address` = `<workgroupName>.<account>.<region>.redshift-serverless.amazonaws.com`, `port`).
3. **GetWorkgroup** — input `workgroupName` (required). Output `workgroup` (required). Must return the record stored by CreateWorkgroup/UpdateWorkgroup byte-for-byte (same id, arn, capacity, port, endpoint, creationDate); unknown name -> `ResourceNotFoundException` (404).
4. **ListWorkgroups** — `@paginated(inputToken=nextToken, outputToken=nextToken, items=workgroups)`, input `nextToken`, `maxResults`. Output `workgroups` (required), `nextToken`. Must enumerate the stored workgroups, honour `maxResults` as a page bound and return an opaque `nextToken` only when more remain; the token must resume where the previous page ended. Must also reflect deletions.
5. **UpdateWorkgroup** — input `workgroupName` (required) plus the mutable fields `baseCapacity`, `maxCapacity`, `port`, `publiclyAccessible`, `enhancedVpcRouting`, `configParameters`, `securityGroupIds`, `subnetIds`. Output `workgroup` (required). Must mutate the persisted record in place so a following GetWorkgroup observes the new values; unknown name -> `ResourceNotFoundException` (404).
6. **DeleteWorkgroup** / **DeleteNamespace** — each takes the required name, returns the deleted `workgroup` / `namespace` record (required output), and REMOVES it from state so a following Get returns `ResourceNotFoundException` (404) and the List no longer contains it. DeleteNamespace while a workgroup still references that namespace must fail `ConflictException` (409).

Also wire **GetNamespace** / **ListNamespaces** (same read/pagination rules as 3 and 4) since they are trivially the read side of the same store and clients poll them to confirm creation.

Every one of these must compute its response from persisted state. A hardcoded `AVAILABLE` workgroup, an echo of the input struct, or a generated id that is not stored is a failure of this issue — a create followed by a get/list must return exactly what was created, and a delete must be observable.

## State Model

In `src/state.rs` (`ComAmazonawsRedshiftserverlessRedshiftserverlessState`, reached via `crate::state::service_state(&*self.engine)` and its lock guard):

- `namespaces: BTreeMap<String /* namespaceName */, NamespaceRecord>` where `NamespaceRecord { namespace_name, namespace_id: Uuid, namespace_arn: String, admin_username: Option<String>, admin_user_password: Option<String> (never echoed back), db_name: String (default "dev"), kms_key_id: Option<String>, default_iam_role_arn: Option<String>, iam_roles: Vec<String>, log_exports: Vec<String>, status: NamespaceStatus, creation_date: DateTime, tags: Vec<Tag> }`
- `workgroups: BTreeMap<String /* workgroupName */, WorkgroupRecord>` where `WorkgroupRecord { workgroup_name, workgroup_id: Uuid, workgroup_arn: String, namespace_name: String, base_capacity: i32 (default 128), max_capacity: Option<i32>, port: i32 (default 5439), publicly_accessible: bool, enhanced_vpc_routing: bool, config_parameters: Vec<ConfigParameter>, security_group_ids: Vec<String>, subnet_ids: Vec<String>, status: WorkgroupStatus, endpoint: Endpoint { address, port, vpc_endpoints }, creation_date: DateTime, workgroup_version: String, tags: Vec<Tag> }`

BTreeMap keying gives the deterministic ordering that the two paginated lists need; pagination tokens encode the last-returned key. All remaining operations (snapshots, recovery points, endpoint access, usage limits, scheduled actions, reservations, custom domains, resource policy, restore/table-restore, credentials, tags) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-redshiftserverless-redshiftserverless/OPERATIONS.md` lines 162-194 (CreateNamespace/GetNamespace/UpdateNamespace/DeleteNamespace/ListNamespaces) and 426-458 (CreateWorkgroup/GetWorkgroup/UpdateWorkgroup/DeleteWorkgroup/ListWorkgroups) give the exact required members, outputs (`namespace` (Namespace), `workgroup` (Workgroup)), paginated traits and error sets (`ConflictException` 409, `ResourceNotFoundException` 404, `ValidationException` 400) cited above. Field lists for `Namespace` and `Workgroup` come from `smithy_query shape=Namespace,Workgroup` on `smithy-model/com-amazonaws-redshiftserverless-redshiftserverless.json`. All handler files under `src/ops/` are scaffolds: e.g. `src/ops/create_workgroup.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 211 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateWorkgroup"))` (HTTP 501). No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `redshift-serverless` client at it.
2. `aws redshift-serverless create-namespace --namespace-name bootstrap-ns --db-name dev --admin-username admin --admin-user-password Passw0rd!` — expect a `namespace` with `namespaceId`, `namespaceArn`, `status=AVAILABLE`, `creationDate`; currently HTTP 501 NotImplemented.
3. `aws redshift-serverless get-namespace --namespace-name bootstrap-ns` — must return the same id/arn/dbName just created.
4. `aws redshift-serverless create-workgroup --workgroup-name bootstrap-wg --namespace-name bootstrap-ns --base-capacity 32` — expect a `workgroup` with `workgroupId`, `workgroupArn`, `status=AVAILABLE`, `baseCapacity=32`, `port=5439`, a derived `endpoint.address`.
5. `aws redshift-serverless create-workgroup --workgroup-name wg2 --namespace-name does-not-exist` — expect `ResourceNotFoundException` (404).
6. `aws redshift-serverless get-workgroup --workgroup-name bootstrap-wg` — must return exactly the stored record (same id, arn, baseCapacity 32, endpoint, creationDate).
7. `aws redshift-serverless list-workgroups --max-results 1` — must return one workgroup plus a `nextToken`; passing that token must return the remaining ones and no duplicates.
8. `aws redshift-serverless update-workgroup --workgroup-name bootstrap-wg --base-capacity 64` then `get-workgroup` — must report `baseCapacity=64`.
9. `aws redshift-serverless delete-namespace --namespace-name bootstrap-ns` while the workgroup still exists — expect `ConflictException` (409).
10. `aws redshift-serverless delete-workgroup --workgroup-name bootstrap-wg`, then `get-workgroup --workgroup-name bootstrap-wg` — expect `ResourceNotFoundException` (404) and the workgroup absent from `list-workgroups`.
11. `aws redshift-serverless delete-namespace --namespace-name bootstrap-ns`, then `get-namespace` — expect `ResourceNotFoundException` (404) and absence from `list-namespaces`.
