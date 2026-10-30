Issue-Source: trace-sweep-divergence
Issue-Trace: rds-engine-versions-pagination
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-engine-versions-pagination rds-create-db-instance-catalog-triple rds-global-cluster-mutation-envelopes
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.MinorEngineVersion
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsCluster
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsDataGroups
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsRDSProxy
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsChangeDataCaptureStreams
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsEnhancedBinlog
Issue-Related: 20261029T214632Z-rds-describedbengineversions-serves-a-stale-catalog-for-aurora.md
Issue-Commit: 4fee60863f3ac3b34c6722c9235a070909377b2f

# rds DescribeDBEngineVersions drops capability flags and omits the pagination Marker

> _Note: this issue was identified against the code base at commit `4fee60863f3ac3b34c6722c9235a070909377b2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
DescribeDBEngineVersions answers 200 but its DBEngineVersion projection is narrower than the one RDS serves, and its paginated form does not carry the continuation token RDS returns. Three independent replay clusters diverge on this operation; they are bundled here because they share one root cause - the DescribeDBEngineVersions projection/pagination path - and one service. There are 3 causes to close.

AWS (common to all three causes): Smithy: DescribeDBEngineVersions -> DBEngineVersionMessage {Marker: String, DBEngineVersions: DBEngineVersionList}; @paginated inputToken=Marker, outputToken=Marker, items=DBEngineVersions, pageSize=MaxRecords. The checked-in model's DBEngineVersion structure declares Engine, EngineVersion, MajorEngineVersion, DBParameterGroupFamily, DBEngineVersionDescription, ValidUpgradeTarget, SupportedCACertificateIdentifiers, SupportsReadReplica, SupportsParallelQuery, SupportsGlobalDatabases, SupportsBabelfish, SupportsLimitlessDatabase, SupportsCertificateRotationWithoutRestart, SupportsIntegrations and friends; it does NOT declare MinorEngineVersion, SupportsRDSProxy, SupportsCluster, SupportsDataGroups, SupportsEnhancedBinlog or SupportsChangeDataCaptureStreams, so for those members this is recorded behavior only: the recorded RDS wire response carries them on every DBEngineVersion entry, with MinorEngineVersion being the minor part of EngineVersion for that entry and each Supports* member being the engine version's own capability as a boolean string. The rule, not the recorded values, is the requirement: every engine-version entry the emulator serves must carry the full member set RDS emits for that engine version, and a truncated page must carry a continuation Marker.

Cause 1 of 3: the paginated form serves a different first page and does not reproduce the result-level Marker. Fixture rds-engine-versions-pagination, step task#1.
AWS: for a MaxRecords-limited DescribeDBEngineVersions the recorded response returns the catalog's own first page and a result-level Marker (DescribeDBEngineVersionsResult.Marker) - an opaque continuation token the caller echoes back as the Marker input member to fetch the next page. The recorded first entry is a fully populated mysql 5.7 row (Engine mysql, EngineVersion 5.7.44-rds.<date>, MajorEngineVersion 5.7, MinorEngineVersion the matching minor suffix, DBParameterGroupFamily mysql5.7, DBEngineVersionDescription 'MySQL <EngineVersion>', SupportedCACertificateIdentifiers {rds-ca-rsa4096-g1, rds-ca-rsa2048-g1}, SupportsReadReplica/SupportsRDSProxy true, SupportsBabelfish/SupportsCluster/SupportsDataGroups/SupportsGlobalDatabases/SupportsParallelQuery/SupportsLimitlessDatabase/SupportsCertificateRotationWithoutRestart false, and a ValidUpgradeTarget list enumerating the real upgrade paths with Engine/EngineVersion/MajorEngineVersion/MinorEngineVersion/Description/AutoUpgrade/IsMajorVersionUpgrade/SupportsSnapshotUpgrade per target). Those literal versions are one recording's catalog content, not the requirement; the requirement is that the page is the catalog's page and the Marker is present when more entries remain.
Emulator: 200, but the cluster diverges on two keys - $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0] (value_mismatch: the emulator's first page entry is not the recorded one) and $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker (the result-level continuation token does not match the recorded one).
Why: a client paginating the engine-version catalog terminates after the first page because there is no usable Marker, so it never sees the rest of the catalog; and because the first page is not the catalog's page, version-selection code that takes the first match picks an engine version the service does not publish.
Fix: the pagination and page-projection in the DescribeDBEngineVersions handler (it must emit DBEngineVersionMessage.Marker whenever the MaxRecords cut leaves entries unserved, and page over the catalog in the catalog's own order).
Traces: rds-engine-versions-pagination#task#1.

Cause 2 of 3: the DBEngineVersion projection omits MinorEngineVersion and the proxy/cluster/data-group capability flags. Fixture rds-create-db-instance-catalog-triple, step task#6.
AWS: recorded behavior only for these members (the on-disk DBEngineVersion shape does not declare them). The recorded response carries MinorEngineVersion (the minor component of that row's EngineVersion), SupportsRDSProxy, SupportsCluster and SupportsDataGroups on the engine-version entry - here SupportsRDSProxy true and SupportsCluster/SupportsDataGroups false for the queried engine version.
Emulator: 200 with those four members absent from the DBEngineVersion element: $...DBEngineVersions.DBEngineVersion.MinorEngineVersion, .SupportsRDSProxy, .SupportsDataGroups and .SupportsCluster are all reported missing (got None).
Why: these flags are exactly what a client consults before CreateDBProxy against a version, before choosing a cluster-capable version, or when rendering an engine-capability matrix. Absent members deserialize as unset/false in every SDK, so capability gating that works against AWS silently takes the unsupported branch, and MinorEngineVersion-based version comparison has nothing to compare.
Fix: the DBEngineVersion projection/serializer used by the DescribeDBEngineVersions handler, and the engine-version catalog fields it reads.
Traces: rds-create-db-instance-catalog-triple#task#6.

Cause 3 of 3: the same projection also omits the binlog/CDC capability flags on the cluster-capable engines. Fixture rds-global-cluster-mutation-envelopes, step task#1.
AWS: recorded behavior only (members undeclared in the on-disk model). The recorded entry carries MinorEngineVersion plus SupportsRDSProxy, SupportsEnhancedBinlog, SupportsChangeDataCaptureStreams, SupportsCluster and SupportsDataGroups - here SupportsRDSProxy true and the other four false for the queried engine version.
Emulator: 200 with $...DBEngineVersions.DBEngineVersion.SupportsRDSProxy, .SupportsDataGroups, .MinorEngineVersion, .SupportsChangeDataCaptureStreams, .SupportsCluster and .SupportsEnhancedBinlog all missing (got None).
Why: same client impact as Cause 2, extended to the Aurora/MySQL binlog and change-data-capture capability flags a client checks before enabling enhanced binlog or a CDC stream on a version; the member set the emulator emits must not vary with which fixture queries it.
Fix: as Cause 2 - one complete DBEngineVersion projection covering the whole member set RDS emits, rather than a per-engine subset.
Traces: rds-global-cluster-mutation-envelopes#task#1.

Related but NOT covering these: open issue 20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md (Cause 1) reports that the engine-version catalog CONTENT is a synthesized placeholder, keyed on the whole $...DBEngineVersions member for a sqlserver-dev-se filtered query. The three clusters above are keyed on individual DBEngineVersion members and on DescribeDBEngineVersionsResult.Marker, and the replay reports them as unowned, so they are filed here; a fix for the catalog seed alone will not restore the omitted members or the Marker.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 933 steps replayed, 344 passed. The rds service answers, so these are fidelity divergences, not a bootstrap gap. All three clusters below were reported with an empty owned_by and an empty stub_path, i.e. net-new.

Cause 1 - rds-engine-versions-pagination#task#1, DescribeDBEngineVersions, expected 200 / actual 200, verdict mismatch.
divergence_key: service=rds|DescribeDBEngineVersions|status=200->200|keys=$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0],$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker
detail: $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0]: value_mismatch - expected a fully populated mysql 5.7 row {Engine mysql, EngineVersion 5.7.44-rds.20250213, MajorEngineVersion 5.7, MinorEngineVersion 44-rds.20250213, DBParameterGroupFamily mysql5.7, DBEngineVersionDescription 'MySQL 5.7.44-rds.20250213', SupportedCACertificateIdentifiers {member: [rds-ca-rsa4096-g1, rds-ca-rsa2048-g1]}, SupportsReadReplica true, SupportsRDSProxy true, SupportsBabelfish/SupportsCluster/SupportsDataGroups/SupportsGlobalDatabases/SupportsParallelQuery/SupportsLimitlessDatabase/SupportsCertificateRotationWithoutRestart false, ValidUpgradeTarget {UpgradeTarget: [5.7.44-rds.20250508, 5.7.44-rds.20250818, 5.7.44-rds.20251212, 5.7.44-rds.20260212, ...] each with Engine/MajorEngineVersion/MinorEngineVersion/Description/AutoUpgrade false/IsMajorVersionUpgrade false/SupportsSnapshotUpgrade}}; the emulator served a different entry. The cluster's second diverging key is the result-level Marker, so the continuation token for the truncated page does not match the recorded one either. (The recorded version strings and dates are that recording's catalog content; the requirement is the member set and the presence of the Marker.)

Cause 2 - rds-create-db-instance-catalog-triple#task#6, DescribeDBEngineVersions, expected 200 / actual 200, verdict mismatch, affected_count 1.
detail: $...DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.MinorEngineVersion: missing (expected a shaped value, got None); .SupportsRDSProxy: missing (expected 'true', got None); .SupportsDataGroups: missing (expected 'false', got None); .SupportsCluster: missing (expected 'false', got None).

Cause 3 - rds-global-cluster-mutation-envelopes#task#1, DescribeDBEngineVersions, expected 200 / actual 200, verdict mismatch, affected_count 1.
detail: $...DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsRDSProxy: missing (expected 'true', got None); .SupportsDataGroups: missing (expected 'false', got None); .MinorEngineVersion: missing (expected a shaped value, got None); .SupportsChangeDataCaptureStreams: missing (expected 'false', got None); .SupportsCluster: missing (expected 'false', got None); .SupportsEnhancedBinlog: missing (expected 'false', got None).

Smithy consulted (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): operation DescribeDBEngineVersions (input DescribeDBEngineVersionsMessage {Engine, EngineVersion, DBParameterGroupFamily, Filters, MaxRecords, Marker, DefaultOnly, ListSupportedCharacterSets, ListSupportedTimezones, IncludeAll}; output DBEngineVersionMessage {Marker, DBEngineVersions}; paginated inputToken=Marker/outputToken=Marker/items=DBEngineVersions/pageSize=MaxRecords; no modeled errors) and shape DBEngineVersion. The shape declares no MinorEngineVersion, SupportsRDSProxy, SupportsCluster, SupportsDataGroups, SupportsEnhancedBinlog or SupportsChangeDataCaptureStreams, which is why those members are grounded in the recording rather than the model. Handler under review: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_engine_versions.rs (declared the only file to touch for this operation).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, and point an rds client at its data listener.
2. Cause 1: call DescribeDBEngineVersions with a MaxRecords small enough to truncate the catalog (as fixture rds-engine-versions-pagination does) and inspect the XML response: DescribeDBEngineVersionsResult.Marker is not the recorded continuation token, and DBEngineVersions.DBEngineVersion[0] is not the catalog's first row. Feeding the emulator's Marker back in does not continue the recorded page sequence.
3. Cause 2: call DescribeDBEngineVersions for the engine/version the rds-create-db-instance-catalog-triple fixture queries at task#6 and observe that each DBEngineVersion element has no MinorEngineVersion, SupportsRDSProxy, SupportsDataGroups or SupportsCluster child element.
4. Cause 3: call DescribeDBEngineVersions for the cluster-capable engine the rds-global-cluster-mutation-envelopes fixture queries at task#1 and observe that each DBEngineVersion element additionally has no SupportsEnhancedBinlog or SupportsChangeDataCaptureStreams child element.
5. Re-replay all three listed fixture steps (rds-engine-versions-pagination#task#1, rds-create-db-instance-catalog-triple#task#6, rds-global-cluster-mutation-envelopes#task#1) and confirm every one of them passes before submitting - the pointer for this issue covers all three.
