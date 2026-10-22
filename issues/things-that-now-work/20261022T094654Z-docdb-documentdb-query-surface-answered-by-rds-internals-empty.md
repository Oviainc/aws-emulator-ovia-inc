Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-catalog-describe-db-engine-versions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-catalog-describe-db-engine-versions docdb-catalog-describe-orderable-db-instance-options docdb-instance-not-found-envelope-variants docdb-query-envelope-shared-with-rds
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions
Issue-Key: docdb|DescribeOrderableDBInstanceOptions|2xx->2xx|$.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions
Issue-Key: docdb|DescribeDBInstances|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|DescribeDBInstances|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: docdb|DeleteDBInstance|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: docdb|DeleteDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|DeleteDBInstance|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: docdb|DeleteDBInstance|4xx->4xx|status
Issue-Key: docdb|CreateDBInstance|4xx->2xx|$.ErrorResponse
Issue-Key: docdb|CreateDBInstance|4xx->2xx|status
Issue-Key: docdb|DescribeDBEngineVersions|4xx->2xx|$.ErrorResponse
Issue-Key: docdb|DescribeDBEngineVersions|4xx->2xx|status
Issue-Commit: 9cc0add35ee0fa09b41f1226e545932e7bbe5c43

# DocumentDB Query surface answered by RDS internals: empty engine/instance catalogs, missing not-found errors, wrong validation order

> _Note: this issue was identified against the code base at commit `9cc0add35ee0fa09b41f1226e545932e7bbe5c43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
SIX causes, all on the docdb (DocDB) awsQuery surface. DocDB shares RDS's wire identity (Smithy service_info: sdkId DocDB, endpointPrefix 'rds', sigv4Name 'rds', protocol awsQuery, version 2014-10-31), and the docdb crate's own operations are still 501 scaffolds (services/service-com-amazonaws-docdb-amazonrdsv19/src/ops/describe_db_engine_versions.rs is marked 'SCAFFOLDED, NOT IMPLEMENTED'), so docdb-addressed Query traffic is answered by the RDS implementation and inherits RDS-shaped catalogs, RDS validation order and the RDS error serializer. Each cause below must be closed before this issue is submitted. Related OPEN issues exist on the RDS side (20261022T092913Z-rds-describedbengineversions-serves-a-stub-catalog-missing-engines, 20261022T092913Z-rds-describeorderabledbinstanceoptions-returns-a-fabricated-option-set, 20261022T092913Z-rds-awsquery-error-envelopes-omit-requestid-and-misclassify, 20261022T092913Z-rds-writes-and-reads-skip-existence-and-parameter) and the older docdb AmbiguousService issues (20261022T060002Z-docdb-*) describe a routing failure that no longer reproduces; none of them carries the replay keys below, so none covers these steps.

Cause 1 of 6: DescribeDBEngineVersions reports no DocumentDB engine version at all.
AWS: Smithy (docdb) DescribeDBEngineVersions -> DBEngineVersionMessage.DBEngineVersions, a DBEngineVersionList of DBEngineVersion (Engine, EngineVersion, DBParameterGroupFamily, DBEngineVersionDescription, SupportedCACertificateIdentifiers, ValidUpgradeTarget, SupportsCertificateRotationWithoutRestart). The awsQuery serializer emits the list member-wrapped as DBEngineVersions/DBEngineVersion. Recorded behavior shows the docdb engine populated (engine 'docdb', versions 3.6.0 and 5.0.x, family 'docdb3.6', description 'Amazon DocumentDB (with MongoDB compatibility)', ValidUpgradeTarget entries to 5.0.x/8.0.x).
Emulator: HTTP 200, but $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions is not the member-wrapped structure at all (replay verdict type_mismatch). The answering handler is the RDS one, whose static ENGINES table (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_engine_versions.rs) lists only aurora-mysql, aurora-postgresql, db2-*, mariadb, mysql, oracle-*, postgres and sqlserver-* — there is no 'docdb' entry, so a docdb-filtered query resolves to an empty result.
Why: a client that enumerates engine versions before creating a cluster (console, CDK, CLI version checks) sees zero available DocumentDB versions and cannot resolve an EngineVersion or DBParameterGroupFamily, so cluster creation is blocked or proceeds with an unvalidated version.
Fix: the docdb DescribeDBEngineVersions handler and its engine-version catalog data (implement the docdb op rather than letting the request fall through to the RDS catalog).
Traces: docdb-catalog-describe-db-engine-versions#task#1.

Cause 2 of 6: DescribeOrderableDBInstanceOptions reports no orderable DocumentDB instance option.
AWS: Smithy (docdb) DescribeOrderableDBInstanceOptions takes a REQUIRED Engine and returns OrderableDBInstanceOptionsMessage.OrderableDBInstanceOptions, a list of OrderableDBInstanceOption (Engine, EngineVersion, DBInstanceClass, LicenseModel, AvailabilityZones, Vpc, StorageType), member-wrapped as OrderableDBInstanceOptions/OrderableDBInstanceOption. Recorded behavior for Engine=docdb shows db.r5.* classes with populated AvailabilityZones, Vpc=true, StorageType 'standard', LicenseModel 'na' and the DocDB capability flags.
Emulator: HTTP 200, but $.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions is not the member-wrapped structure (type_mismatch) — same root cause as Cause 1: the RDS orderable-options source knows no docdb engine.
Why: a client cannot discover any valid DBInstanceClass for a DocumentDB instance, so instance sizing and pre-flight validation in deployment tooling fail.
Fix: the docdb DescribeOrderableDBInstanceOptions handler and its orderable-option catalog data.
Traces: docdb-catalog-describe-orderable-db-instance-options#task#1.

Cause 3 of 6: the awsQuery ErrorResponse envelope omits RequestId.
AWS: Smithy (docdb) DescribeDBInstances models DBInstanceNotFoundFault with httpError 404 and awsQueryError code DBInstanceNotFound; the awsQuery error envelope is ErrorResponse{Error{Type,Code,Message}, RequestId}, and the recording asserts a RequestId is present (shaped, value not asserted).
Emulator: the status 404 and code DBInstanceNotFound are right, but $.ErrorResponse.RequestId is absent entirely. (The Message also differs only by a trailing period — recorded example 'DBInstance <id> not found.' vs emulator 'DBInstance <id> not found' — reported here as an example, not as the requirement.)
Why: every awsQuery error from this service is unattributable: SDK retry/telemetry layers and support correlation read RequestId out of the error envelope, and strict XML consumers that require the element fail to parse the fault.
Fix: the shared awsQuery error serializer used by the docdb service — emit RequestId on every ErrorResponse.
Traces: docdb-instance-not-found-envelope-variants#task#1.

Cause 4 of 6: DeleteDBInstance on a nonexistent instance refuses with the wrong status and error identity.
AWS: Smithy (docdb) DeleteDBInstance input models exactly ONE member, DBInstanceIdentifier — docdb models no SkipFinalSnapshot and no FinalDBSnapshotIdentifier, so no snapshot parameter-combination rule can apply here. It models DBInstanceNotFoundFault (httpError 404, awsQueryError code DBInstanceNotFound), and the recording shows instance existence resolved first: 404 / DBInstanceNotFound.
Emulator: HTTP 400 with Error.Code 'ValidationException' and message 'FinalDBSnapshotIdentifier must be specified unless SkipFinalSnapshot is enabled' (and again no RequestId) — the RDS delete handler's snapshot validation runs ahead of the existence check, on members docdb does not model.
Why: delete-if-exists logic keys on 404/DBInstanceNotFound to treat the resource as already gone; a 400 ValidationException naming a parameter the DocDB API has no member for is unrecognizable, so teardown aborts or retries forever.
Fix: the docdb DeleteDBInstance handler — resolve DBInstanceIdentifier against state and return DBInstanceNotFoundFault before any parameter-combination validation, and drop validation of members docdb does not model.
Traces: docdb-instance-not-found-envelope-variants#task#2.

Cause 5 of 6: CreateDBInstance against a nonexistent cluster succeeds with 200.
AWS: Smithy (docdb) CreateDBInstance requires DBClusterIdentifier and models DBClusterNotFoundFault with httpError 404 and awsQueryError code DBClusterNotFoundFault ('DBClusterIdentifier doesn't refer to an existing cluster'). Recorded response: 404, Error{Type 'Sender', Code 'DBClusterNotFoundFault', Message 'The source cluster could not be found or cannot be accessed: <clusterId>'}.
Emulator: HTTP 200 with a success envelope and no ErrorResponse — the instance is created against a cluster that does not exist.
Why: a client that mistypes a cluster id, or races cluster creation, gets a phantom instance instead of a refusal; later describes and deletes then disagree with the real topology, and the error path the client wrote for DBClusterNotFoundFault is never exercised.
Fix: the docdb CreateDBInstance handler validation/state — resolve DBClusterIdentifier in the service store before admitting the instance.
Traces: docdb-instance-not-found-envelope-variants#task#4.

Cause 6 of 6: a default-engine-version probe that AWS refuses is answered 200.
AWS: recorded behavior only — the model declares no errors for DescribeDBEngineVersions. The recording shows this request refused with HTTP 400 and Error{Type 'Sender', Code 'InvalidParameterCombination', Message "RDS can't find a default engine version."} when the default version is requested and no default can be resolved for the named engine.
Emulator: HTTP 200 with a DescribeDBEngineVersionsResponse and no ErrorResponse at all.
Why: a caller probing for a default engine version reads 200 as 'a default exists' and then launches with an unresolved/unusable EngineVersion instead of handling InvalidParameterCombination.
Fix: the docdb DescribeDBEngineVersions validation path — refuse the unresolvable default-version combination with InvalidParameterCombination/400 instead of returning an empty success.
Traces: docdb-query-envelope-shared-with-rds#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, state_reset_failures 0, bootstrap_signal false).

Cause 1 — docdb-catalog-describe-db-engine-versions#task#1, DescribeDBEngineVersions: expected 200, got 200; $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions: type_mismatch. Expected a member-wrapped {'DBEngineVersion': [{'Engine': 'docdb', 'EngineVersion': '3.6.0', 'DBParameterGroupFamily': 'docdb3.6', 'DBEngineVersionDescription': 'Amazon DocumentDB (with MongoDB compatibility)', 'SupportedCACertificateIdentifiers': {'member': ['rds-ca-rsa4096-g1','rds-ca-rsa2048-g1']}, 'ValidUpgradeTarget': {'UpgradeTarget': [...5.0.x/8.0.x...]}, ...}]}; the emulator's value is of a different type.

Cause 2 — docdb-catalog-describe-orderable-db-instance-options#task#1, DescribeOrderableDBInstanceOptions: expected 200, got 200; $.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions: type_mismatch. Expected {'OrderableDBInstanceOption': [{'Engine': 'docdb', 'EngineVersion': '5.0.0', 'DBInstanceClass': 'db.r5.12xlarge', 'Vpc': 'true', 'StorageType': 'standard', 'LicenseModel': 'na', 'AvailabilityZones': {'AvailabilityZone': [{'Name':'us-west-2a'},...]}, 'SupportsClusters': 'true', ...}, ...]}.

Cause 3 — docdb-instance-not-found-envelope-variants#task#1, DescribeDBInstances: status 404 as expected; $.ErrorResponse.RequestId: missing (expected a shaped value, got None). $.ErrorResponse.Error.Message: 'DBInstance sim2real-probe-docdb-noinst-1b7fe834 not found.' (recorded example) vs 'DBInstance sim2real-probe-docdb-noinst-1b7fe834 not found'.

Cause 4 — docdb-instance-not-found-envelope-variants#task#2, DeleteDBInstance: status expected 404, got 400. $.ErrorResponse.Error.Code: expected 'DBInstanceNotFound', got 'ValidationException'. $.ErrorResponse.Error.Message: expected 'DBInstance sim2real-probe-docdb-noinst-1b7fe834 not found.', got 'FinalDBSnapshotIdentifier must be specified unless SkipFinalSnapshot is enabled'. $.ErrorResponse.RequestId: missing.

Cause 5 — docdb-instance-not-found-envelope-variants#task#4, CreateDBInstance: status expected 404, got 200. $.ErrorResponse: missing — expected {'Error': {'Type': 'Sender', 'Code': 'DBClusterNotFoundFault', 'Message': 'The source cluster could not be found or cannot be accessed: sim2real-probe-docdb-nocluster-1b7fe834'}, 'RequestId': <shaped>}.

Cause 6 — docdb-query-envelope-shared-with-rds#task#4, DescribeDBEngineVersions: status expected 400, got 200. $.ErrorResponse: missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterCombination', 'Message': "RDS can't find a default engine version."}, 'RequestId': <shaped>}.

Supporting code evidence: services/service-com-amazonaws-docdb-amazonrdsv19/src/ops/describe_db_engine_versions.rs:4 ('SCAFFOLDED, NOT IMPLEMENTED') and :168 (returns not_implemented); services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_engine_versions.rs:190-208 (serves from catalogue()) and :356-568 (ENGINES table with no 'docdb' entry). Smithy model: services/service-com-amazonaws-docdb-amazonrdsv19/smithy-model/com-amazonaws-docdb-amazonrdsv19.json.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1: replay docdb-catalog-describe-db-engine-versions step task#1 — awsQuery DescribeDBEngineVersions (Version=2014-10-31) against the docdb/rds endpoint with Engine=docdb; observe the result list is not the member-wrapped DBEngineVersions/DBEngineVersion structure (no docdb versions).
4. Cause 2: replay docdb-catalog-describe-orderable-db-instance-options step task#1 — DescribeOrderableDBInstanceOptions with Engine=docdb; observe OrderableDBInstanceOptions is not the member-wrapped list of docdb options.
5. Cause 3: replay docdb-instance-not-found-envelope-variants step task#1 — DescribeDBInstances with a DBInstanceIdentifier that does not exist; observe 404/DBInstanceNotFound with no ErrorResponse/RequestId element.
6. Cause 4: replay docdb-instance-not-found-envelope-variants step task#2 — DeleteDBInstance with the same nonexistent DBInstanceIdentifier; observe 400 ValidationException about FinalDBSnapshotIdentifier instead of 404 DBInstanceNotFound.
7. Cause 5: replay docdb-instance-not-found-envelope-variants step task#4 — CreateDBInstance naming a DBClusterIdentifier that does not exist; observe 200 instead of 404 DBClusterNotFoundFault.
8. Cause 6: replay docdb-query-envelope-shared-with-rds step task#4 — the DescribeDBEngineVersions default-version probe; observe 200 instead of 400 InvalidParameterCombination.
