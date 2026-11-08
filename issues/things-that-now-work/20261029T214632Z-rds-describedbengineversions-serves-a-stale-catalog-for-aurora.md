Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[*]
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS DescribeDBEngineVersions serves a stale catalog for Aurora MySQL, Db2 and SQL Server

> _Resolution: the per-entry and per-release-line causes are fixed — an Aurora MySQL entry
> is described by the two releases its version string carries (`Aurora (MySQL 5.7) 2.11.1`)
> instead of the product name joined to the whole string, the Aurora MySQL 2.11 line
> publishes its successive builds (2.11.2 … 2.11.5 beside 2.11.1 and 2.11.6) so a client
> can create them and read them as that entry's minor upgrade targets, and Aurora
> PostgreSQL publishes the 12 family (12.9), so an 11 entry's upgrade targets are 11.21 and
> 12.9. The members each row carries — the Db2 exportable log types and
> `S3_INTEGRATION` feature name, the RDS Custom for SQL Server `DefaultCharacterSet`, the
> CA certificate identifiers, every `Supports*` flag and each upgrade target's
> classification — and the `db2-ce` 12.1 line were already served and are now pinned by
> `tests/all/engine_version_release_lines.rs`. DECLINED, and so still divergent on replay:
> the IDENTITY, COUNT and ORDER of the recorded listings (the `DBEngineVersion[*]` key).
> Those are a snapshot of AWS's live release catalogue on the day of the recording — it
> moves with every RDS release and no rule produces it — so matching it entry-for-entry
> would be fitting the recorded instance, exactly as resolved for the same key in
> `20261022T162132Z-rds-describedbengineversions-engine-catalog-listings-do-not-match.md`._

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
DescribeDBEngineVersions answers 200 but the per-engine catalog it serves is not the recorded one: whole DBEngineVersion rows differ, each row is missing the support flags AWS publishes, and the ValidUpgradeTarget trees do not match. Five engine families are affected in this bundle. 5 causes.

Shared AWS behavior: recorded behavior only — for each Engine the response lists the engine's versions in the recorded order, each DBEngineVersion carrying DBParameterGroupFamily, MajorEngineVersion, MinorEngineVersion, DBEngineVersionDescription, SupportedCACertificateIdentifiers, SupportsBabelfish, SupportsRDSProxy, SupportsCertificateRotationWithoutRestart, SupportsGlobalDatabases, SupportsParallelQuery, SupportsDataGroups, SupportsReadReplica, SupportsCluster, SupportsLimitlessDatabase, SupportedFeatureNames, ExportableLogTypes, SupportsLogExportsToCloudwatchLogs, SupportsIntegrations, SupportsLocalWriteForwarding, Status, and a ValidUpgradeTarget list whose UpgradeTarget entries carry AutoUpgrade, IsMajorVersionUpgrade, SupportsSnapshotUpgrade and the same support flags.
Emulator: serves a different, shorter version list per engine; the rows it does serve carry only a subset of those members.
Why: a client that picks an engine version from the catalog, or asserts that a version it intends to create is offered, gets a set RDS does not publish — and CreateDB* then accepts/refuses versions inconsistently with the catalog.
Fix: the RDS engine-version catalog data used by the DescribeDBEngineVersions handler, and that handler's DBEngineVersion serializer.

Cause 1 of 5: Engine aurora-mysql — the recorded first row is 5.7.mysql_aurora.2.11.1 (family aurora-mysql5.7) with its 2.11.2/2.11.3 upgrade targets; the emulator's list differs from row 0 onward across 35 rows. Traces: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#1.
Cause 2 of 5: Engine aurora-postgresql — recorded first row 11.9 (family aurora-postgresql11) with 11.21/12.9 upgrade targets; the emulator's 46-row list differs from row 0. Traces: ...-aurora-mysql-custom-sqlserver-se#task#2.
Cause 3 of 5: Engine custom-sqlserver-ee — recorded first row 15.00.4073.23.v1 (family custom-sqlserver-ee-15.0) with DefaultCharacterSet SQL_Latin1_General_CP1_CI_AS; the emulator's 37-row list differs from row 0. Traces: ...-aurora-mysql-custom-sqlserver-se#task#3; also #task#4 and rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#1.
Cause 4 of 5: Engine db2-ae — recorded first row 11.5.9.0.sb00000000.r1 (family db2-ae-11.5) with ExportableLogTypes diag.log/notify.log and SupportedFeatureNames S3_INTEGRATION; the emulator's 17-row list differs from row 0. Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#2; also #task#4.
Cause 5 of 5: Engine db2-ce — recorded first row 12.1.4.0.sb00080714.r1 (family db2-ce-12.1); the emulator serves 11.5.9.0.sb00000000.r1 (family db2-ce-11.5) as row 0 and the 12.1 line is absent. Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#3.

## Evidence
Replay cluster details (status 200 matched; DBEngineVersion rows diverged):
1. rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#1: DBEngineVersion[0] missing — expected the aurora-mysql 5.7.mysql_aurora.2.11.1 record (DBParameterGroupFamily 'aurora-mysql5.7', SupportsRDSProxy 'true', SupportedCACertificateIdentifiers [rds-ca-ecc384-g1, rds-ca-rsa4096-g1, rds-ca-rsa2048-g1], ValidUpgradeTarget 2.11.2/2.11.3); divergence spans rows [0]..[34].
2. ...#task#2: DBEngineVersion[0] missing — expected aurora-postgresql 11.9 (family 'aurora-postgresql11'); divergence spans rows [0]..[45].
3. ...#task#3: DBEngineVersion[0] missing — expected custom-sqlserver-ee 15.00.4073.23.v1 (family 'custom-sqlserver-ee-15.0'); rows [0]..[36].
4. ...-custom-sqlserver-web-oracle-ee#task#2: DBEngineVersion[0] missing — expected db2-ae 11.5.9.0.sb00000000.r1 (family 'db2-ae-11.5'); rows [0]..[16].
5. ...-custom-sqlserver-web-oracle-ee#task#3: DBEngineVersion[0] expected db2-ce 12.1.4.0.sb00080714.r1 (family 'db2-ce-12.1'), got db2-ce 11.5.9.0.sb00000000.r1 (family 'db2-ce-11.5'); rows [0]..[2].
(The version strings are the recorded catalog's contents, which is exactly what this operation must publish.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#1 and the steps in trace_ids; each calls DescribeDBEngineVersions for one Engine and diffs the DBEngineVersions list against the recording.
