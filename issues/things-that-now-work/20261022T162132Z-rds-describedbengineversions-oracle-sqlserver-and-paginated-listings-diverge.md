Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-db-engine-versions-oracle-ee-cdb-oracle-se2
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-db-engine-versions-oracle-ee-cdb-oracle-se2 rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se rds-catalog-describe-db-engine-versions-sqlserver-ee-sqlserver-se rds-catalog-describe-db-engine-versions-sqlserver-web rds-engine-versions-pagination
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker
Issue-Related: 20261022T162132Z-rds-describedbengineversions-engine-catalog-listings-do-not-match.md
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# RDS DescribeDBEngineVersions oracle sqlserver and paginated listings diverge from AWS

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
Continuation of the same DescribeDBEngineVersions catalog fault for the remaining engine filters the corpus exercises, plus the paginated first page. Seven further engine/filter combinations each diverge in the body while the status stays 200. Filed separately from the first DescribeDBEngineVersions bundle only because a bundle carries at most ten causes; a fixer must close all seven here as well.

Cause 1 of 7: DescribeDBEngineVersions for Engine=oracle-ee-cdb.
AWS: Smithy: DescribeDBEngineVersions -> DBEngineVersionMessage {Marker, DBEngineVersions}, no modeled errors. Recorded behavior only for the version set - AWS lists oracle-ee-cdb 19.0.0.0.ru-2021-04.rur-2021-04.r1, DBParameterGroupFamily oracle-ee-cdb-19, DefaultCharacterSet AL32UTF8, SupportsReadReplica true, and a ru-/rur- upgrade chain.
Emulator: every expected DBEngineVersion index 0..54 is reported missing against the served list.
Why: a client enumerating CDB-capable Oracle versions for CreateDBInstance gets versions that do not exist on AWS.
Fix: the DescribeDBEngineVersions engine-version catalog data the operation's handler reads.
Traces: rds-catalog-describe-db-engine-versions-oracle-ee-cdb-oracle-se2#task#1.

Cause 2 of 7: DescribeDBEngineVersions for Engine=oracle-se2-cdb.
AWS: recorded behavior only - AWS lists oracle-se2-cdb 19.0.0.0.ru-2021-04.rur-2021-04.r1, DBParameterGroupFamily oracle-se2-cdb-19, SupportsReadReplica false.
Emulator: entries 0..50 diverge.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se#task#1.

Cause 3 of 7: DescribeDBEngineVersions for Engine=postgres.
AWS: recorded behavior only - AWS lists postgres 11.22-rds.<date> builds, DBParameterGroupFamily postgres11, with dated minor versions and SupportsCertificateRotationWithoutRestart true.
Emulator: entries 0..49 diverge; no dated -rds.<date> minor versions are served.
Why: a client pinning a dated RDS PostgreSQL minor version cannot find it, and the upgrade chain it would read is wrong.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se#task#2.

Cause 4 of 7: DescribeDBEngineVersions for Engine=sqlserver-dev-ee.
AWS: recorded behavior only - AWS lists sqlserver-dev-ee 15.00.4455.2.v1, DBParameterGroupFamily sqlserver-dev-ee-15.0, DefaultCharacterSet SQL_Latin1_General_CP1_CI_AS, with 15.00/16.00 upgrade targets.
Emulator: entries 0..9 diverge.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se#task#3.

Cause 5 of 7: DescribeDBEngineVersions for Engine=sqlserver-ee.
AWS: recorded behavior only - AWS lists sqlserver-ee 14.00.3281.6.v1, DBParameterGroupFamily sqlserver-ee-14.0, SupportsReadReplica true, SupportedCACertificateIdentifiers [rds-ca-ecc384-g1, rds-ca-rsa4096-g1, rds-ca-rsa2048-g1].
Emulator: entries 0..77 diverge - the AWS listing is roughly 78 entries and the emulator's is a different, shorter set.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-sqlserver-ee-sqlserver-se#task#1 (cluster also covers task#2 and task#3 of the same fixture, i.e. the sqlserver-se filter).

Cause 6 of 7: DescribeDBEngineVersions for Engine=sqlserver-web.
AWS: recorded behavior only - AWS lists sqlserver-web 14.00.3281.6.v1, DBParameterGroupFamily sqlserver-web-14.0, SupportsReadReplica false.
Emulator: entries 0..73 diverge.
Why: as cause 1.
Fix: same catalog data.
Traces: rds-catalog-describe-db-engine-versions-sqlserver-web#task#1.

Cause 7 of 7: DescribeDBEngineVersions first page with MaxRecords, where the catalog fault also moves the pagination token.
AWS: Smithy: DescribeDBEngineVersions is @paginated {inputToken: Marker, outputToken: Marker, items: DBEngineVersions, pageSize: MaxRecords}; the first page is the head of the AWS catalog and Marker points at the next element of that same catalog.
Emulator: DBEngineVersion[0] is a value_mismatch (expected the mysql 5.7.44-rds.20250213 head entry) and DescribeDBEngineVersionsResult.Marker diverges with it.
Why: a client paginating DescribeDBEngineVersions walks a different sequence, so page boundaries and the resume token do not line up with AWS; a resumed enumeration yields a different set.
Fix: same catalog data, and the Marker the handler derives from its ordering.
Traces: rds-engine-versions-pagination#task#1.

## Evidence
All seven clusters replay expected_status 200 / actual_status 200; the divergence is in the body. Cause 1: $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0]: missing (expected Engine=oracle-ee-cdb, EngineVersion=19.0.0.0.ru-2021-04.rur-2021-04.r1, DBParameterGroupFamily=oracle-ee-cdb-19), repeated for indices up to [54]. Cause 5 reports divergent indices up to DBEngineVersion[77]. Cause 7 reports both $...DBEngineVersions.DBEngineVersion[0] and $...DescribeDBEngineVersionsResult.Marker as divergent keys.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture rds-catalog-describe-db-engine-versions-oracle-ee-cdb-oracle-se2, step task#1 (awsQuery Action=DescribeDBEngineVersions&Engine=oracle-ee-cdb) and diff DBEngineVersions against the recording.
3. Repeat for each fixture#step in trace_ids.
4. For cause 7 replay rds-engine-versions-pagination#task#1 and check both DBEngineVersion[0] and the returned Marker.
