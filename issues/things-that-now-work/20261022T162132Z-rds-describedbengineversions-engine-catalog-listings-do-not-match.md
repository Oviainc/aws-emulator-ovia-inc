Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se rds-catalog-describe-db-engine-versions-oracle-ee-cdb-oracle-se2
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[*]
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# RDS DescribeDBEngineVersions engine catalog listings do not match the AWS catalog

> _Resolution: the structural and per-entry causes are fixed — the RDS Custom for SQL
> Server engines are served as catalogue data (cause 3), every engine version is published
> in its engine family's form (Db2 `…sbNNNNNNNN.rN`, the dated RDS Extended Support builds),
> the `docdb 8.0` and `neptune1` families exist, Db2 publishes its `SupportedFeatureNames`
> and every Oracle entry its `SupportedNcharCharacterSets`. DECLINED, and so still
> divergent on replay: the IDENTITY, COUNT and ORDER of the recorded listings (causes 1, 2,
> 7 and the residue of 4-6 and 8-10, i.e. the `DBEngineVersion[*]` key). Those are a
> snapshot of AWS's live catalogue on the day of the recording — it moves with every RDS
> release and no rule produces it — so matching it entry-for-entry would be fitting the
> recorded instance. This emulator publishes its own reference data in AWS's forms, shapes
> and invariants instead._

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
DescribeDBEngineVersions answers 200 for every engine filter the corpus exercises, but the list it serves is a small emulator-invented table rather than the AWS engine-version catalog. For each engine the returned DBEngineVersion entries differ from AWS in identity (which EngineVersion values exist at all), in cardinality (AWS returns tens of versions per engine where the emulator returns a handful), and in the per-entry envelope. The ten causes below are ten distinct engine/filter combinations that each diverge; they share one root cause, the catalog data the handler reads.

Cause 1 of 10: DescribeDBEngineVersions for Engine=aurora-mysql.
AWS: Smithy: DescribeDBEngineVersions -> output DBEngineVersionMessage {Marker, DBEngineVersions: DBEngineVersionList}, no modeled errors; DBEngineVersion models Engine, EngineVersion, MajorEngineVersion, DBParameterGroupFamily, DBEngineVersionDescription, SupportedEngineModes, SupportedCACertificateIdentifiers, SupportsCertificateRotationWithoutRestart, SupportsGlobalDatabases, SupportsParallelQuery, SupportsReadReplica, SupportsLimitlessDatabase, ValidUpgradeTarget. Recorded behavior only for the exact version set: AWS lists aurora-mysql 5.7.mysql_aurora.2.11.x (DBParameterGroupFamily aurora-mysql5.7) with a populated ValidUpgradeTarget chain.
Emulator: returns a different first entry entirely; every expected DBEngineVersion index 0..45 is reported 'missing' against what the emulator served.
Why: a client enumerating supported engine versions to pick a CreateDBInstance EngineVersion, or to compute a minor-version upgrade path from ValidUpgradeTarget, sees versions that do not exist on AWS and misses the ones that do.
Fix: the DescribeDBEngineVersions engine-version catalog data the operation's handler reads.
Traces: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#1.

Cause 2 of 10: DescribeDBEngineVersions for Engine=aurora-postgresql.
AWS: same model citation; recorded behavior only for the version set - AWS lists aurora-postgresql 11.9 (DBParameterGroupFamily aurora-postgresql11) and its upgrade targets 11.21, 12.9, ... .
Emulator: entries 0..45 all diverge; the emulator's aurora-postgresql list is a different, shorter set.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#2.

Cause 3 of 10: DescribeDBEngineVersions for the custom-sqlserver-ee family.
AWS: recorded behavior only - AWS returns a non-empty DBEngineVersions list for custom-sqlserver-ee (e.g. 15.00.4073.23.v1, DBParameterGroupFamily custom-sqlserver-ee-15.0, DefaultCharacterSet SQL_Latin1_General_CP1_CI_AS).
Emulator: $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions is a type_mismatch - the emulator does not serve a list of DBEngineVersion members here at all for this filter.
Why: a client that filters on a custom engine gets a structurally different result and cannot read DBEngineVersions as a list.
Fix: same catalog data plus the DBEngineVersions list serializer for the empty/absent case.
Traces: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#3 (cluster also covers rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#4, rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#1, rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se#task#4).

Cause 4 of 10: DescribeDBEngineVersions for Engine=db2-ae.
AWS: recorded behavior only - AWS lists db2-ae 11.5.9.0.sb00000000.r1, DBParameterGroupFamily db2-ae-11.5, ExportableLogTypes [diag.log, notify.log], SupportedFeatureNames [S3_INTEGRATION].
Emulator: entries 0..16 diverge; the emulator's db2 list is synthetic.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#2 (cluster also covers rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#4).

Cause 5 of 10: DescribeDBEngineVersions for Engine=db2-ce.
AWS: recorded behavior only - AWS lists db2-ce 12.1.4.0.sb00080714.r1, DBParameterGroupFamily db2-ce-12.1.
Emulator: serves db2-ce 11.5.9.0 with DBParameterGroupFamily db2-ce-11.5; entries 0..2 diverge.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#3.

Cause 6 of 10: DescribeDBEngineVersions for Engine=docdb.
AWS: recorded behavior only - AWS lists docdb 3.6.0 (DBParameterGroupFamily docdb3.6) with major-version upgrade targets 5.0.x and 8.0.x.
Emulator: entries 0..7 diverge.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#5.

Cause 7 of 10: DescribeDBEngineVersions for Engine=mariadb.
AWS: recorded behavior only - AWS lists mariadb 10.5.28 (DBParameterGroupFamily mariadb10.5) and a 10.6.x upgrade chain.
Emulator: entries 0..26 diverge.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#6.

Cause 8 of 10: DescribeDBEngineVersions for Engine=mysql.
AWS: recorded behavior only - AWS lists mysql 5.7.44-rds.<date> builds (DBParameterGroupFamily mysql5.7) with dated minor versions.
Emulator: entries 0..21 diverge; the emulator has no dated -rds.<date> minor versions.
Why: as cause 1; a client pinning a dated RDS minor version cannot find it.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#7.

Cause 9 of 10: DescribeDBEngineVersions for Engine=neptune.
AWS: recorded behavior only - AWS lists neptune 1.1.1.0 (DBParameterGroupFamily neptune1, SupportedEngineModes [provisioned]) with 1.1.2.0/1.2.x upgrade targets.
Emulator: entries 0..29 diverge.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#8.

Cause 10 of 10: DescribeDBEngineVersions for Engine=oracle-ee.
AWS: recorded behavior only - AWS lists oracle-ee 19.0.0.0.ru-YYYY-MM.rur-YYYY-MM.rN releases (DBParameterGroupFamily oracle-ee-19, DefaultCharacterSet AL32UTF8).
Emulator: entries 0..45 diverge.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#9 (cluster also covers rds-catalog-describe-db-engine-versions-oracle-ee-cdb-oracle-se2#task#2).

## Evidence
All ten clusters replay with expected_status 200 and actual_status 200, so the fault is purely in the body. Representative (cause 1): $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0]: missing - expected {'Engine': 'aurora-mysql', 'EngineVersion': '5.7.mysql_aurora.2.11.1', 'DBParameterGroupFamily': 'aurora-mysql5.7', 'DBEngineVersionDescription': 'Aurora (MySQL 5.7) 2.11.1', ...} and the same for DBEngineVersion[1]..[45]. Cause 3 is stronger: $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions: type_mismatch, i.e. the emulator did not serve a DBEngineVersion list for the custom-sqlserver-ee filter at all. Cause 5: expected Engine=db2-ce EngineVersion 12.1.4.0.sb00080714.r1 / family db2-ce-12.1, got EngineVersion 11.5.9.0 / family db2-ce-11.5.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se, step task#1 (awsQuery POST Action=DescribeDBEngineVersions&Engine=aurora-mysql).
3. Compare DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[*] against the recorded response: the version identities and the entry count differ.
4. Repeat for each fixture#step listed in trace_ids to exercise the other nine engine filters.
