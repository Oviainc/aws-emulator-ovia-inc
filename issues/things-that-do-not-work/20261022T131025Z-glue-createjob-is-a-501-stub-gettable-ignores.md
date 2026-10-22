Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.glue-job-round-trips-worker-config-and-derives-capacity glue__iam__lakeformation__s3.glue-job-catalog-read-denied-until-lf-grant glue__iam__lakeformation__s3.glue-job-denied-on-governed-table-without-lf-data-admin-until-lf-grant glue__iam__lakeformation__s3.lf-governed-table-get-table-denied-without-lf-grant-and-describe-grant-restores-it glue__iam__lakeformation__s3.glue-governed-table-round-trips-partitioned-parquet-schema-and-unregistered-location glue__iam__lakeformation__s3.e2e-troubleshoot-glue-job-table-read-failure-lifecycle
Issue-Key: glue__iam__lakeformation__s3|CreateJob|NotImplemented
Issue-Key: glue__iam__lakeformation__s3.lf-governed-table-get-table-denied-without-lf-grant-and-describe-grant-restores-it|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS refuses it, so there is no error")
Issue-Key: glue__iam__lakeformation__s3.glue-governed-table-round-trips-partitioned-parquet-schema-and-unregistered-location|run#19|AssertionError|assert t.get("CreatedBy") == me, f"CreatedBy {t.get('CreatedBy')!r}, the creator is {me!r}"
Issue-Key: glue__iam__lakeformation__s3|GetDatabase|EntityNotFoundException
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# Glue: CreateJob is a 501 stub, GetTable ignores Lake Formation governance, Table.CreatedBy is not the caller identity, and a CFN-declared database is absent from the catalog

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Four distinct root causes in the Glue emulator, found by replaying the behavioral suite. Each must be closed before this issue is done.

Cause 1 of 4: CreateJob answers 501 NotImplemented (unimplemented scaffold stub).
AWS: Smithy com.amazonaws.glue#CreateJob takes Name (NameString, required, len 1..255), Role (RoleString, required) and Command (JobCommand, required) plus WorkerType (enum G.025X|G.1X|G.2X|G.4X|G.8X|Standard|Z.2X), NumberOfWorkers, GlueVersion, Timeout (range 1..), MaxRetries (default 0), DefaultArguments, ExecutionProperty, and returns 200 with output member Name. Its only modeled errors are AlreadyExistsException, ConcurrentModificationException, IdempotentParameterMismatchException, InternalServiceException, InvalidInputException, OperationTimeoutException, ResourceNumberLimitExceededException (all HTTP 400 except InternalServiceException 500). HTTP 501 / error code NotImplemented is not a modeled outcome of this operation.
Emulator: every CreateJob call fails with HTTP 501, error code NotImplemented, message 'operation CreateJob is scaffolded but not implemented in this emulator'. The handler is an untouched scaffold: services/service-com-amazonaws-glue-awsglue/src/ops/create_job.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() body ends in Err(crate::scaffolded::Scaffolded::not_implemented("CreateJob")) (line 272).
Why: no client can define a Glue job, so nothing downstream of a job definition exists either — GetJob, StartJobRun, GetJobRuns and every job-driven data-plane flow are unreachable. Three suite checks never get past their first call.
Fix: implement the handler in services/service-com-amazonaws-glue-awsglue/src/ops/create_job.rs — persist the job definition in the service state (crate::state) keyed by Name, reject a duplicate Name with AlreadyExistsException, and return the created Name. Jobs must then be readable by GetJob/GetJobs and runnable so GetJobRuns reports runs; derived capacity (MaxCapacity from WorkerType x NumberOfWorkers) belongs in the job projection.
Traces: glue__iam__lakeformation__s3.glue-job-round-trips-worker-config-and-derives-capacity, glue__iam__lakeformation__s3.glue-job-catalog-read-denied-until-lf-grant, glue__iam__lakeformation__s3.glue-job-denied-on-governed-table-without-lf-data-admin-until-lf-grant.

Cause 2 of 4: GetTable on a Lake Formation governed table succeeds for a principal with no Lake Formation grant.
AWS: recorded behavior only — com.amazonaws.glue#GetTable does not model AccessDeniedException (its modeled errors are EntityNotFoundException, FederationSourceException, FederationSourceRetryableException, GlueEncryptionException, InternalServiceException, InvalidInputException, OperationTimeoutException, ResourceNotReadyException), but the recording shows the catalog read being refused by Lake Formation: a caller holding glue:GetTable and lakeformation:GetDataAccess in IAM, reading a table in a database with no IAM_ALLOWED_PRINCIPALS grant and no Lake Formation grant, gets error code AccessDeniedException (example message: 'Insufficient Lake Formation permission(s): Required Describe on usage_events'). The same session reads a table that does carry IAM_ALLOWED_PRINCIPALS, and after lakeformation:GrantPermissions grants DESCRIBE on the governed table the same GetTable succeeds. The requirement is the decision and its error code, not the message text.
Emulator: the GetTable call returns the table normally — the check's helper raises 'the call succeeded; real AWS refuses it, so there is no error'. Glue's catalog read is not gated on Lake Formation state at all, so governed and un-governed tables are indistinguishable to a non-admin principal.
Why: a client cannot test or reproduce Lake Formation governance. Code that expects AccessDeniedException until a grant exists sees unauthorized metadata disclosed instead, and the grant-flips-the-outcome transition the scenario depends on can never be observed.
Fix: services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs (and the equivalent read path for GetTables/GetDatabase) must consult the Lake Formation permission state for the caller's principal — allow when IAM_ALLOWED_PRINCIPALS is granted on the resource or the principal holds DESCRIBE/SELECT, otherwise refuse with AccessDeniedException naming the missing Lake Formation permission.
Traces: glue__iam__lakeformation__s3.lf-governed-table-get-table-denied-without-lf-grant-and-describe-grant-restores-it.

Cause 3 of 4: Table.CreatedBy is a synthesized principal, not the caller's identity.
AWS: Smithy com.amazonaws.glue#Table member CreatedBy — 'The person or entity who created the table.' The recording asserts it equals the creating caller's ARN as STS GetCallerIdentity reports it for that same session, i.e. the two services must agree on the caller's principal ARN.
Emulator: after CreateTable by the default caller, GetTable reports CreatedBy 'arn:aws:iam::000000000000:user/EMULATOR8A2A8977D364' while sts:GetCallerIdentity for the same credentials reports 'arn:aws:iam::000000000000:user/emulator'. Glue invents its own per-request principal name instead of using the request's resolved caller identity. (Every other assertion in that check — partition keys, serde, Compressed, the NumberOfBuckets/StoredAsSubDirectories/Retention defaults and IsRegisteredWithLakeFormation false — passed, so this is the sole divergence there.)
Why: a client that correlates catalog ownership with the identity it authenticated as (audit flows, 'did I create this table' logic, the scenario's diagnosis) sees a principal that exists nowhere else in the account.
Fix: in the Glue table create/projection path (services/service-com-amazonaws-glue-awsglue/src/ops/create_table.rs and the Table projection used by get_table.rs) take CreatedBy from the request's resolved caller principal ARN — the same value the STS caller-identity path returns — rather than generating a name.
Traces: glue__iam__lakeformation__s3.glue-governed-table-round-trips-partitioned-parquet-schema-and-unregistered-location.

Cause 4 of 4: a Glue database created through CloudFormation is not present in the Glue catalog (same symptom surfaced from the Glue side; see the companion cloudformation issue filed in this sweep for the resource-provisioning half).
AWS: recorded behavior only — after a stack reaches CREATE_COMPLETE, the stack Output that Refs the AWS::Glue::Database resource names a database that glue:GetDatabase resolves, and that database carries the template's declared DatabaseInput (here an empty CreateTableDefaultPermissions). glue#GetDatabase models EntityNotFoundException for a database that does not exist.
Emulator: the stack reaches CREATE_COMPLETE and emits outputs, but glue.get_database(Name=<the stack's GlueDatabaseName output>) fails with EntityNotFoundException: 'Database sim2real-gluelf-e2e-<id>-gluedatabase-<id> not found in catalog 000000000000'. The output value also has the shape of an auto-generated physical id (<stack>-<logicalid>-<suffix>) rather than the DatabaseInput name the template declares, so nothing in the catalog answers under either name.
Why: every CloudFormation-deployed Glue estate is a phantom — a client that deploys a stack and then uses the catalog it declared gets not-found on resources CloudFormation reported as created, and the whole deploy/setup/solve lifecycle stops at its first catalog read.
Fix: wire the CloudFormation AWS::Glue::Database (and the sibling Table/Job resources in the same template) provisioning to actually create the resource in the Glue service state, using the declared DatabaseInput.Name as the physical id so Ref/Outputs name a resolvable database.
Traces: glue__iam__lakeformation__s3.e2e-troubleshoot-glue-job-table-read-failure-lifecycle (phase 1/5 deploy).

## Evidence
Cause 1 — expected: HTTP 200 {"Name":"<job name>"} and a job readable by GetJob. Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateJob operation: operation CreateJob is scaffolded but not implemented in this emulator (HTTP 501). Handler: services/service-com-amazonaws-glue-awsglue/src/ops/create_job.rs:272.

Cause 2 — expected: error code AccessDeniedException (example message 'Insufficient Lake Formation permission(s): Required Describe on usage_events') for GetTable on the governed table, then success after GrantPermissions DESCRIBE. Actual: GetTable returned the table; harness raised AssertionError: the call succeeded; real AWS refuses it, so there is no error.

Cause 3 — expected: Table.CreatedBy == sts:GetCallerIdentity Arn of the creating session ('arn:aws:iam::000000000000:user/emulator' in this run). Actual: AssertionError: CreatedBy 'arn:aws:iam::000000000000:user/EMULATOR8A2A8977D364', the creator is 'arn:aws:iam::000000000000:user/emulator'.

Cause 4 — expected: GetDatabase(Name=<GlueDatabaseName output>) returns the database with CreateTableDefaultPermissions == []. Actual: _PhaseError: [phase 1/5 deploy] EntityNotFoundException: An error occurred (EntityNotFoundException) when calling the GetDatabase operation: Database sim2real-gluelf-e2e-96142c85be50-gluedatabase-098453aa36cc not found in catalog 000000000000 — raised after the stack had reached CREATE_COMPLETE and produced outputs.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: glue:CreateJob with Name/Role/Command (glueetl, Glue 4.0, WorkerType G.1X, NumberOfWorkers 2) -> observe HTTP 501 NotImplemented. Checks: glue-job-round-trips-worker-config-and-derives-capacity, glue-job-catalog-read-denied-until-lf-grant, glue-job-denied-on-governed-table-without-lf-data-admin-until-lf-grant.
3. Cause 2: CreateDatabase with no IAM_ALLOWED_PRINCIPALS (governed) + CreateTable; assume a role whose inline policy allows glue:GetDatabase/GetTable/GetTables and lakeformation:GetDataAccess; call glue:GetTable on the governed table -> it succeeds instead of AccessDeniedException. Check: lf-governed-table-get-table-denied-without-lf-grant-and-describe-grant-restores-it.
4. Cause 3: CreateTable as the default caller, then GetTable and compare Table.CreatedBy with sts:GetCallerIdentity Arn. Check: glue-governed-table-round-trips-partitioned-parquet-schema-and-unregistered-location.
5. Cause 4: create the suite's glue-job-lf-governed-table CloudFormation stack, wait for CREATE_COMPLETE, read the GlueDatabaseName output and call glue:GetDatabase with it -> EntityNotFoundException. Check: e2e-troubleshoot-glue-job-table-read-failure-lifecycle.
