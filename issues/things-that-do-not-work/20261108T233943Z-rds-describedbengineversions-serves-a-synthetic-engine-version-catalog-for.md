Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee rds-catalog-describe-db-engine-versions-oracle-ee-cdb-oracle-se2
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[*]
Issue-Commit: 0d0b5d4835067c793fae3be07582601671bbaf82

# DescribeDBEngineVersions serves a synthetic engine-version catalog for Aurora MySQL, Aurora PostgreSQL, custom SQL Server, Db2, DocumentDB, MariaDB, MySQL, Neptune and Oracle EE

> _Note: this issue was identified against the code base at commit `0d0b5d4835067c793fae3be07582601671bbaf82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
This bundle carries 10 net-new causes. They share ONE fault: the emulator answers DescribeDBEngineVersions 200 from a hand-written placeholder catalog rather than from the RDS engine-version catalog, so for every `Engine` filter the rows it returns are not the rows AWS returns. The per-engine causes are listed separately because each is a distinct `Engine` the catalog must carry and each must be re-replayed.

Shared statement of the fault:

AWS: DescribeDBEngineVersions returns, for the requested `Engine` (and optional `EngineVersion`/`DBParameterGroupFamily`), one DBEngineVersion per release that engine actually offers in the region. Each row carries Engine, EngineVersion, MajorEngineVersion, MinorEngineVersion, DBParameterGroupFamily, DBEngineDescription, DBEngineVersionDescription, Status, the capability flags (SupportsReadReplica, SupportsGlobalDatabases, SupportsParallelQuery, SupportsRDSProxy, SupportsBabelfish, SupportsCertificateRotationWithoutRestart, SupportsLimitlessDatabase, SupportsLogExportsToCloudwatchLogs, ...), SupportedCACertificateIdentifiers, SupportedEngineModes, ExportableLogTypes, SupportedFeatureNames, DefaultCharacterSet where the engine has one, and the full ValidUpgradeTarget list with each target's AutoUpgrade/IsMajorVersionUpgrade/SupportsSnapshotUpgrade flags. Smithy: DescribeDBEngineVersions -> DBEngineVersionMessage/DBEngineVersion (the model defines all of the above members; the *contents* of the catalog are not modelled, so for the row values this is recorded behavior only). Status is 200 in every recording.

Emulator: status matches (200) but the list contents do not. In every cause below the recorded row at index 0 (and typically every other index) is reported `missing`, i.e. the emulator emits a list of different, mostly placeholder rows for the same filter — e.g. for `Engine=db2-ce` the recording's first row is EngineVersion '12.1.4.0.sb00080714.r1' / DBParameterGroupFamily 'db2-ce-12.1' while the emulator emits EngineVersion '11.5.9.0.sb00000000.r1' / DBParameterGroupFamily 'db2-ce-11.5' with the obviously synthetic build suffix `sb00000000`. Capability flags, SupportedCACertificateIdentifiers, DefaultCharacterSet and ValidUpgradeTarget are absent or wrong on the rows it does emit.

Why: clients use this call to discover what they may ask for. A console or IaC tool that lists upgrade targets, picks a DBParameterGroupFamily for CreateDBParameterGroup, or validates an EngineVersion before CreateDBInstance/CreateDBCluster gets an answer that does not describe the engines the emulator itself then accepts, so version selection, upgrade planning and parameter-group family derivation all mis-resolve against the emulator while they work against AWS.

Fix: the rds service crate's DescribeDBEngineVersions handler and the static engine-version catalog it reads. The catalog needs one real row per engine/version pair, with the flags, CA-certificate identifiers, character set and ValidUpgradeTarget list populated; the handler then filters it by Engine/EngineVersion/DBParameterGroupFamily. Do not special-case the fixtures.

Cause 1 of 10: Engine=aurora-mysql. Recorded first row EngineVersion '5.7.mysql_aurora.2.11.1', DBParameterGroupFamily 'aurora-mysql5.7', SupportedEngineModes [provisioned], SupportsGlobalDatabases true, SupportsParallelQuery true, with a 3-entry ValidUpgradeTarget; the emulator's rows do not match it (all 35 indices reported missing). Trace: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#1.

Cause 2 of 10: Engine=aurora-postgresql. Recorded first row EngineVersion '11.9', DBParameterGroupFamily 'aurora-postgresql11', description 'Aurora PostgreSQL (Compatible with PostgreSQL 11.9)'; all 46 recorded indices reported missing. Trace: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#2.

Cause 3 of 10: Engine=custom-sqlserver-ee. Recorded first row EngineVersion '15.00.4073.23.v1', DBParameterGroupFamily 'custom-sqlserver-ee-15.0', DefaultCharacterSet SQL_Latin1_General_CP1_CI_AS; all 37 recorded indices reported missing. Trace: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#3.

Cause 4 of 10: Engine=db2-ae. Recorded first row EngineVersion '11.5.9.0.sb00000000.r1', DBParameterGroupFamily 'db2-ae-11.5', ExportableLogTypes [diag.log, notify.log], SupportedFeatureNames [S3_INTEGRATION]; all 17 recorded indices reported missing. Trace: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#2.

Cause 5 of 10: Engine=db2-ce. Recorded first row EngineVersion '12.1.4.0.sb00080714.r1' / family 'db2-ce-12.1'; emulator emits '11.5.9.0.sb00000000.r1' / family 'db2-ce-11.5'. Trace: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#3.

Cause 6 of 10: Engine=docdb. Recorded first row EngineVersion '3.6.0', DBParameterGroupFamily 'docdb3.6', ValidUpgradeTarget listing the 5.0.x and 8.0.x major upgrades; all 8 recorded indices reported missing. Trace: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#5.

Cause 7 of 10: Engine=mariadb. Recorded first row EngineVersion '10.5.28', family 'mariadb10.5', SupportsRDSProxy true, SupportsCertificateRotationWithoutRestart true; all 27 recorded indices reported missing. Trace: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#6.

Cause 8 of 10: Engine=mysql. Recorded first row EngineVersion '5.7.44-rds.20250213', family 'mysql5.7', SupportedCACertificateIdentifiers [rds-ca-rsa4096-g1, rds-ca-rsa2048-g1]; all 22 recorded indices reported missing. Trace: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#7.

Cause 9 of 10: Engine=neptune. Recorded first row EngineVersion '1.1.1.0', family 'neptune1', SupportedEngineModes [provisioned]; all 29 recorded indices reported missing. Trace: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#8.

Cause 10 of 10: Engine=oracle-ee. Recorded first row EngineVersion '19.0.0.0.ru-2020-04.rur-2020-04.r1', family 'oracle-ee-19', DefaultCharacterSet AL32UTF8; all 40 recorded indices reported missing. Trace: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#9.

Related but NOT covering this: open issue 20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md owns the DescribeDBMajorEngineVersions, DescribeEventCategories and DescribeOrderableDBInstanceOptions catalog clusters; the replay reports every cause above as unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (96 fixtures, 977 steps, 483 passed). All 10 clusters: verdict=mismatch, expected_status=200, actual_status=200, divergence_key 'service=rds|DescribeDBEngineVersions|status=200->200|keys=$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0..N]'.

Example (Cause 5, db2-ce): detail '$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0]: missing (expected {... EngineVersion 12.1.4.0.sb00080714.r1, DBParameterGroupFamily db2-ce-12.1, MajorEngineVersion 12.1, SupportsReadReplica true, ExportableLogTypes [diag.log, notify.log], SupportedFeatureNames [S3_INTEGRATION], Status available ...}, got {MinorEngineVersion 9.0.sb00000000.r1, Engine db2-ce, MajorEngineVersion 11.5, EngineVersion 11.5.9.0.sb00000000.r1, DBParameterGroupFamily db2-ce-11.5, ...})'.

Example (Cause 1, aurora-mysql): expected row {Engine aurora-mysql, EngineVersion 5.7.mysql_aurora.2.11.1, DBEngineVersionDescription 'Aurora (MySQL 5.7) 2.11.1', DBParameterGroupFamily aurora-mysql5.7, SupportsGlobalDatabases true, SupportsParallelQuery true, SupportedCACertificateIdentifiers [rds-ca-ecc384-g1, rds-ca-rsa4096-g1, rds-ca-rsa2048-g1], ValidUpgradeTarget -> 2.11.2 / 2.11.3} is reported missing from the emulator's list, as are indices 1..34.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se, step task#1 (rds DescribeDBEngineVersions with Engine=aurora-mysql) and compare DescribeDBEngineVersionsResult.DBEngineVersions against the recording.
3. Repeat for each trace_ids entry: steps task#2 and task#3 of the same fixture (aurora-postgresql, custom-sqlserver-ee) and steps task#2, task#3, task#5, task#6, task#7, task#8, task#9 of rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee (db2-ae, db2-ce, docdb, mariadb, mysql, neptune, oracle-ee).
4. Each returns 200 with a list whose rows are not the recorded rows.
