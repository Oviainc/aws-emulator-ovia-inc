Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee rds-catalog-describe-db-engine-versions-oracle-ee-cdb-oracle-se2 rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se rds-catalog-describe-db-engine-versions-sqlserver-ee-sqlserver-se rds-catalog-describe-db-engine-versions-sqlserver-web rds-create-db-instance-catalog-triple rds-engine-versions-pagination rds-global-cluster-mutation-envelopes
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[*]
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.MinorEngineVersion
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportedFeatureNames
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsCertificateRotationWithoutRestart
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsCluster
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsDataGroups
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsIntegrations
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsLimitlessDatabase
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsLocalWriteForwarding
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsRDSProxy
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.DBEngineDescription
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.DBEngineVersionDescription
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.ExportableLogTypes.member
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.ServerlessV2FeaturesSupport
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportedCACertificateIdentifiers
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportedEngineModes.member
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsChangeDataCaptureStreams
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsEnhancedBinlog
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsReadReplica
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# RDS DescribeDBEngineVersions serves a stub catalog missing engines, versions and members

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
Ten clusters, one fault: the DBEngineVersion catalog behind DescribeDBEngineVersions is a stub. For a given Engine/EngineVersion/DBParameterGroupFamily filter AWS returns the full set of DBEngineVersion entries with their ValidUpgradeTarget trees; the emulator returns a single synthesized entry (or none), with most DBEngineVersion members omitted. Smithy: DescribeDBEngineVersions -> DBEngineVersionMessage{Marker, DBEngineVersions: DBEngineVersionList}; the operation models no errors, so every one of these is a 200 whose body is wrong. Each cause below is one filter/engine family that must be served; all share the same fix site.

Cause 1 of 10 — Engine=aurora-mysql. AWS: a DBEngineVersions list (e.g. 5.7.mysql_aurora.2.11.1, DBParameterGroupFamily aurora-mysql5.7, SupportsRDSProxy, SupportedCACertificateIdentifiers, SupportedEngineModes, ValidUpgradeTarget entries). Emulator: a single element where a list is expected — the other catalog entries are not served. Traces: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#1.

Cause 2 of 10 — Engine=aurora-postgresql (11.9 and the rest of the family). Emulator: the whole expected entry set is missing (indices 0..45 absent). Traces: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#2.

Cause 3 of 10 — Engine=custom-sqlserver-ee / custom-sqlserver-se. AWS: entries with DBParameterGroupFamily custom-sqlserver-ee-15.0, DefaultCharacterSet, ValidUpgradeTarget. Emulator: DBEngineVersions is the wrong type/shape (catalog not served). Traces: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#3.

Cause 4 of 10 — Engine=mariadb (10.5.x family with ValidUpgradeTarget to 10.6.x). Emulator: all 27 expected entries missing. Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#6.

Cause 5 of 10 — Engine=mysql (5.7.44-rds.* family). Emulator: all 22 expected entries missing. Traces: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#7.

Cause 6 of 10 — Engine=postgres (11.22-rds.* family). Emulator: all 50 expected entries missing. Traces: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se#task#2.

Cause 7 of 10 — Engine=sqlserver-ee / sqlserver-se (14.00/15.00 families with DefaultCharacterSet). Emulator: all 78 expected entries missing. Traces: rds-catalog-describe-db-engine-versions-sqlserver-ee-sqlserver-se#task#1.

Cause 8 of 10 — single-version lookup during instance creation (mysql 8.0.x). AWS: the entry carries MinorEngineVersion, SupportsRDSProxy, SupportsCertificateRotationWithoutRestart, SupportsDataGroups, SupportsCluster, SupportsLimitlessDatabase, SupportedFeatureNames, SupportsIntegrations, SupportsLocalWriteForwarding. Emulator: all nine members absent from the single entry it does return. Traces: rds-create-db-instance-catalog-triple#task#6.

Cause 9 of 10 — paginated listing. AWS: the first page's entry set and a Marker for the next page. Emulator: wrong first entry and a diverging Marker, so the catalog cannot be walked. Traces: rds-engine-versions-pagination#task#1.

Cause 10 of 10 — Engine=aurora-postgresql single version (16.4) read before a global-cluster mutation. AWS: SupportsRDSProxy, ServerlessV2FeaturesSupport{MinCapacity,MaxCapacity}, SupportedCACertificateIdentifiers, SupportsChangeDataCaptureStreams, SupportsDataGroups, MinorEngineVersion, SupportsCluster present; SupportedEngineModes = 'provisioned' only; SupportsReadReplica = 'false'. Emulator: those members are absent, SupportedEngineModes is ['provisioned','serverless'] and SupportsReadReplica is 'true' — the capability flags are invented rather than read from a catalog. Traces: rds-global-cluster-mutation-envelopes#task#1.

Why (all causes): DescribeDBEngineVersions is how clients choose an engine version, decide upgrade paths (ValidUpgradeTarget) and gate on capability flags; a stub catalog makes every such decision wrong while still answering 200, and it is also why CreateDBInstance/CreateOptionGroup cannot validate Engine/EngineVersion (see the companion issue on missing parameter guards). Fix: the RDS engine-version catalog data and the DescribeDBEngineVersions handler's filtering/serialization (Engine, EngineVersion, DBParameterGroupFamily, DefaultOnly, Marker/MaxRecords paging).

## Evidence
Cause 1: rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se#task#1 (12 steps in cluster) — $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion type_mismatch: expected a list of DBEngineVersion entries (aurora-mysql 5.7.mysql_aurora.2.11.1 with ValidUpgradeTarget), got a single element.
Cause 2: same fixture task#2 — DBEngineVersion[0..45] all 'missing'.
Cause 3: same fixture task#3 (6 steps) — DBEngineVersions type_mismatch (expected custom-sqlserver-ee 15.00.* entries).
Cause 4: rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee#task#6 — DBEngineVersion[0..26] missing (mariadb 10.5.28 ...).
Cause 5: same fixture task#7 — DBEngineVersion[0..21] missing (mysql 5.7.44-rds.20250213 ...).
Cause 6: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se#task#2 — DBEngineVersion[0..49] missing (postgres 11.22-rds.20241121 ...).
Cause 7: rds-catalog-describe-db-engine-versions-sqlserver-ee-sqlserver-se#task#1 — DBEngineVersion[0..77] missing (sqlserver-ee 14.00.3281.6.v1 ...).
Cause 8: rds-create-db-instance-catalog-triple#task#6 — MinorEngineVersion, SupportsRDSProxy, SupportsCertificateRotationWithoutRestart, SupportsDataGroups, SupportsCluster, SupportsLimitlessDatabase, SupportedFeatureNames, SupportsIntegrations, SupportsLocalWriteForwarding all missing.
Cause 9: rds-engine-versions-pagination#task#1 — DBEngineVersion[0] value_mismatch and DescribeDBEngineVersionsResult.Marker diverges.
Cause 10: rds-global-cluster-mutation-envelopes#task#1 — SupportedEngineModes.member expected 'provisioned', got ['provisioned','serverless']; SupportsReadReplica expected 'false', got 'true'; ServerlessV2FeaturesSupport / SupportedCACertificateIdentifiers / SupportsRDSProxy / MinorEngineVersion missing.

## Steps to Reproduce
1) Replay the wire corpus (replay_wire_corpus).
2) For each engine family, call DescribeDBEngineVersions with the fixture's filter and diff DBEngineVersions against the recording: aurora-mysql and aurora-postgresql and custom-sqlserver (fixture rds-catalog-describe-db-engine-versions-aurora-mysql-custom-sqlserver-se, task#1/#2/#3), mariadb and mysql (rds-catalog-describe-db-engine-versions-custom-sqlserver-web-oracle-ee, task#6/#7), postgres (rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se task#2), sqlserver-ee/se (rds-catalog-describe-db-engine-versions-sqlserver-ee-sqlserver-se task#1).
3) Single-version lookups: rds-create-db-instance-catalog-triple task#6 and rds-global-cluster-mutation-envelopes task#1.
4) Pagination: rds-engine-versions-pagination task#1 — walk the Marker.
