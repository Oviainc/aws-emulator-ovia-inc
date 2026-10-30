Issue-Source: trace-sweep-divergence
Issue-Trace: rds-engine-versions-pagination
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-engine-versions-pagination
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker
Issue-Related: 20261029T214632Z-rds-describedbengineversions-serves-a-stale-catalog-for-aurora.md
Issue-Commit: 301afeafbd8a408bf1460387900a1fccabe4ba25

# RDS DescribeDBEngineVersions paged mysql sweep returns wrong rows and a diverging Marker

> _Note: this issue was identified against the code base at commit `301afeafbd8a408bf1460387900a1fccabe4ba25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
The paginated DescribeDBEngineVersions sweep answers 200 but neither the page's contents nor its continuation token match the recorded service: the replay cluster reports BOTH $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0] and $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker diverging on the same step. 1 cause.

Cause 1 of 1: a MaxRecords-limited DescribeDBEngineVersions page for the mysql engine family serves a page whose first row is not the catalogue's first row, and a Marker that is not the recorded continuation token.
AWS: Smithy: DescribeDBEngineVersions -> DBEngineVersionMessage {Marker, DBEngineVersions}, @paginated {inputToken=Marker, outputToken=Marker, items=DBEngineVersions, pageSize=MaxRecords}; DescribeDBEngineVersionsMessage$MaxRecords is documented "Default: 100 Constraints: Minimum 20, maximum 100" and "If more than the MaxRecords value is available, a pagination token called a marker is included in the response so you can retrieve the remaining results". The model constrains no catalogue VALUES, so for the content this is recorded behavior only: a page lists the engine's published versions in the service's own order, each DBEngineVersion carrying DBParameterGroupFamily, the MajorEngineVersion/MinorEngineVersion split, DBEngineVersionDescription, SupportedCACertificateIdentifiers and the Supports* capability flags, plus a ValidUpgradeTarget list whose UpgradeTarget entries carry AutoUpgrade, IsMajorVersionUpgrade and SupportsSnapshotUpgrade; and the response carries a Marker whenever versions remain beyond the page. In the recording the first row of the mysql page is the 5.7.44 line (DBParameterGroupFamily mysql5.7, MajorEngineVersion 5.7, MinorEngineVersion 44-rds.<build>, SupportsRDSProxy/SupportsReadReplica true, SupportsCluster/SupportsGlobalDatabases/SupportsParallelQuery false, ValidUpgradeTarget enumerating the later 5.7.44 builds with AutoUpgrade false and IsMajorVersionUpgrade false) — those version strings and build dates are the recorded catalogue's contents, i.e. the content this operation must publish, not a per-run value.
Emulator: answers 200 and the same envelope, but the replay reports DBEngineVersion[0] as a value_mismatch against that recorded row AND the sibling DescribeDBEngineVersionsResult.Marker diverging in the same cluster (divergence_key "service=rds|DescribeDBEngineVersions|status=200->200|keys=$...DBEngineVersions.DBEngineVersion[0],$...Marker"). The captured detail is truncated after the DBEngineVersion[0] expectation, so the two Marker values are in that step's own diff. Hypothesis: the handler pages over its own synthesized catalogue (crate::state::paginate over the matching rows, with the page key derived from the catalogue POSITION), so a shorter/differently ordered mysql row set both shifts row 0 and changes — or, when the short list fits in one page, omits — the next-page Marker that RDS still returns.
Why: a client that pages DescribeDBEngineVersions to enumerate selectable mysql versions, or that feeds the returned Marker back as the next request's Marker, gets a first page RDS never serves and a continuation that does not correspond to it: the sweep either stops early (no further page) or resumes at a boundary in a different total order, so versions are silently skipped and a version picked from the page may not exist for CreateDBInstance.
Fix: the RDS engine-version catalogue data the DescribeDBEngineVersions handler reads, and that handler's paging/projection (the MaxRecords page assembly and next-Marker computation in the operation's handler, services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_engine_versions.rs around the crate::state::paginate call at lines 294-301, and the catalogue it selects from).
Traces: rds-engine-versions-pagination#task#1.

Related open issues that do NOT cover this step: "RDS DescribeDBEngineVersions serves a stale catalog for Aurora MySQL, Db2 and SQL Server" (20261029T214632Z-rds-describedbengineversions-serves-a-stale-catalog-for-aurora.md) covers unpaged per-engine catalogues for aurora-mysql, aurora-postgresql, custom-sqlserver-ee, db2-ae and db2-ce; "rds engine, parameter and event-category catalogs are synthesized placeholders instead of the real RDS catalog" (20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md) covers the sqlserver-dev-se filter and other catalogues. Neither carries this fixture/step, neither names the mysql family, and neither covers the DescribeDBEngineVersionsResult.Marker key, which is why this cluster replays unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false — rds answers, so this is a fidelity divergence, not a bootstrap gap).

Cluster (verdict mismatch, expected_status 200, actual_status 200, affected_count 1):
- fixture rds-engine-versions-pagination, step task#1, operation DescribeDBEngineVersions
- divergence_key: service=rds|DescribeDBEngineVersions|status=200->200|keys=$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0],$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker
- detail: "$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0]: value_mismatch" — expected the recorded mysql row {Engine 'mysql', EngineVersion '5.7.44-rds.20250213', DBEngineVersionDescription 'MySQL 5.7.44-rds.20250213', DBParameterGroupFamily 'mysql5.7', MajorEngineVersion '5.7', MinorEngineVersion '44-rds.20250213', SupportedCACertificateIdentifiers {member: [rds-ca-rsa4096-g1, rds-ca-rsa2048-g1]}, SupportsRDSProxy 'true', SupportsReadReplica 'true', SupportsBabelfish 'false', SupportsCluster 'false', SupportsGlobalDatabases 'false', SupportsParallelQuery 'false', SupportsDataGroups 'false', SupportsLimitlessDatabase 'false', SupportsCertificateRotationWithoutRestart 'false', ValidUpgradeTarget {UpgradeTarget: [5.7.44-rds.20250508, 5.7.44-rds.20250818, 5.7.44-rds.20251212, 5.7.44-rds.20260212 …], each with MajorEngineVersion '5.7', AutoUpgrade 'false', IsMajorVersionUpgrade 'false', SupportsSnapshotUpgrade and SupportsIntegrations flags}}. The captured detail string is truncated mid-ValidUpgradeTarget, so the Marker expected/actual pair is not in the capture; the key list above records that Marker diverged on the same step.
- owned_by: [] (no open issue carries this replay key); stub_path: "" (not blocked by an unimplemented operation).

Status parity: 200 vs 200 — no error code or message is involved in this divergence; the defect is the page's contents and its continuation token.

Smithy consulted (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): DescribeDBEngineVersions input {Engine, EngineVersion, DBParameterGroupFamily, Filters, MaxRecords, Marker, DefaultOnly, ListSupportedCharacterSets, ListSupportedTimezones, IncludeAll}, output DBEngineVersionMessage {Marker, DBEngineVersions}, paginated {inputToken=Marker, outputToken=Marker, items=DBEngineVersions, pageSize=MaxRecords}, no modeled errors; docs for MaxRecords (default 100, min 20, max 100, marker returned when more records remain) and for the Marker member on input and output.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout; point an rds client at its data listener.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-replay rds-engine-versions-pagination#task#1: it calls DescribeDBEngineVersions for the mysql engine family with MaxRecords set and diffs the response against the recording.
4. Observe DBEngineVersions.DBEngineVersion[0] is not the recorded 5.7.44 mysql row (wrong version/family/capability flags/ValidUpgradeTarget) and that DescribeDBEngineVersionsResult.Marker does not match the recorded continuation token.
5. Feed the returned Marker back as the next request's Marker and compare the second page with the recording to see the sweep skip or stop early.
