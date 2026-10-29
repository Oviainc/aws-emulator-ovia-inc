Issue-Source: trace-sweep-divergence
Issue-Trace: rds-create-db-instance-catalog-triple
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-create-db-instance-catalog-triple rds-global-cluster-mutation-envelopes rds-engine-versions-pagination
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.MinorEngineVersion
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsCluster
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsDataGroups
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsRDSProxy
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.ServerlessV2FeaturesSupport.MaxCapacity
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.ServerlessV2FeaturesSupport.MinCapacity
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsChangeDataCaptureStreams
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsEnhancedBinlog
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker
Issue-Related: 20261029T214632Z-rds-describedbengineversions-serves-a-stale-catalog-for-aurora.md
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS DBEngineVersion rows omit support flags and mis-serialize ServerlessV2 capacity

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
Independently of which versions the catalog holds, the DBEngineVersion serializer drops per-version capability flags and emits decimal capacity values in integer form. 3 causes.

Cause 1 of 3: DescribeDBEngineVersions narrowed to one engine version during instance creation.
AWS: recorded behavior only — the DBEngineVersion record always carries MinorEngineVersion, SupportsRDSProxy, SupportsDataGroups and SupportsCluster.
Emulator: all four members are absent from the record.
Why: a client deciding whether a version supports RDS Proxy, data groups or cluster mode has nothing to read, so it cannot pre-validate a create.
Fix: the DescribeDBEngineVersions handler's DBEngineVersion serializer (populate the capability flags).
Traces: rds-create-db-instance-catalog-triple#task#6.

Cause 2 of 3: DescribeDBEngineVersions for an Aurora PostgreSQL version.
AWS: the record carries SupportsRDSProxy, SupportsDataGroups, SupportsChangeDataCaptureStreams, SupportsCluster, SupportsEnhancedBinlog and MinorEngineVersion, and serializes ServerlessV2FeaturesSupport.MinCapacity in decimal form ('0.0').
Emulator: those six members are absent, and ServerlessV2FeaturesSupport.MinCapacity is '0' while MaxCapacity is '256' where the recording's value differs.
Why: the Serverless v2 capacity bounds a client reads are the wrong type (integer where AWS sends a decimal) and the capability flags are missing.
Fix: the same serializer plus the ServerlessV2FeaturesSupport double formatting.
Traces: rds-global-cluster-mutation-envelopes#task#1.

Cause 3 of 3: DescribeDBEngineVersions first page with MaxRecords set.
AWS: recorded 200 with the recorded row 0 (mysql 5.7.44-rds.20250213) and a Marker positioning the next page.
Emulator: 200 but row 0 and the Marker both diverge, so the paginated walk starts from a different element of a different catalog.
Why: a paginated enumeration cannot be resumed against the recorded ordering.
Fix: the DescribeDBEngineVersions handler's page ordering and Marker derivation.
Traces: rds-engine-versions-pagination#task#1.

## Evidence
Replay cluster details (status 200 matched):
1. rds-create-db-instance-catalog-triple#task#6: DBEngineVersion.MinorEngineVersion missing; .SupportsRDSProxy missing (expected 'true'); .SupportsDataGroups missing (expected 'false'); .SupportsCluster missing (expected 'false').
2. rds-global-cluster-mutation-envelopes#task#1: .SupportsRDSProxy missing (expected 'true'); .ServerlessV2FeaturesSupport.MinCapacity expected '0.0' got '0'; .ServerlessV2FeaturesSupport.MaxCapacity value_mismatch (got '256'); .SupportsDataGroups, .MinorEngineVersion, .SupportsChangeDataCaptureStreams, .SupportsCluster, .SupportsEnhancedBinlog all missing.
3. rds-engine-versions-pagination#task#1: DBEngineVersion[0] value_mismatch (expected the mysql 5.7.44-rds.20250213 record) and DescribeDBEngineVersionsResult.Marker diverged.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-create-db-instance-catalog-triple#task#6 and the steps in trace_ids; each reads a single DBEngineVersion (or the first catalog page) and diffs the record's members.
