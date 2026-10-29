Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-catalog-describe-db-engine-versions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-catalog-describe-db-engine-versions neptune-catalog-describe-event-categories neptune-catalog-describe-orderable-db-instance-options
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[*]
Issue-Key: neptune|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap[*]
Issue-Key: neptune|DescribeOrderableDBInstanceOptions|2xx->2xx|$.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# Neptune catalog describes serve RDS-wide data instead of the Neptune engine catalog

> _Resolution: all three catalogs are served out of the Amazon RDS front door, which is
> where a Neptune client's reads land (all three models publish one wire identity). The
> structural causes are fixed: the graph engine now publishes its own release line across
> its four DB parameter group families, with its own product name in each version
> description, its own per-version global-database capability and an upgrade chain within
> and across its families; every release that report calls available is now ORDERABLE, under
> that engine's own `amazon-license` licence model (cause 3's `type_mismatch` is gone — the
> member is a list of `OrderableDBInstanceOption` elements); and the event-category report
> now publishes `custom-engine-version` and the two categories only a DB cluster publishes,
> so the category SETS agree with the recording. DECLINED, and so still divergent on
> replay: the IDENTITY, COUNT and ORDER of the recorded listings (the `DBEngineVersion[*]`,
> `EventCategoriesMap[*]` and `OrderableDBInstanceOption[*]` keys), the release-revision
> suffix of a recorded version description, and the three source types no citable category
> table exists for. Those are a snapshot of AWS's live catalogue on the day of the recording
> plus a per-response rendering of unordered sets — no rule produces either, so matching
> them entry-for-entry would be fitting the recorded instance. This emulator publishes its
> own reference data in AWS's forms, shapes and invariants instead._

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
The three read-only Neptune catalog operations answer 200 but their payloads are not the Neptune catalog: the engine-version list is empty of Neptune entries, the event-category map is the RDS/Aurora map in a different order, and the orderable-instance-options list is not even serialized as a list. THIS ISSUE HAS 3 CAUSES — all three must be closed before submitting.

Cause 1 of 3: DescribeDBEngineVersions returns no Neptune DBEngineVersion entries. AWS: Smithy com.amazonaws.neptune#DBEngineVersion models Engine, EngineVersion, DBParameterGroupFamily, DBEngineVersionDescription, ValidUpgradeTarget, SupportsReadReplica and SupportsGlobalDatabases, and the recording returns the full set of Neptune engine versions — each with Engine 'neptune', its DBParameterGroupFamily, its version description, and a ValidUpgradeTarget list of the versions it can be upgraded to (each target carrying EngineVersion, Description, AutoUpgrade and IsMajorVersionUpgrade). Emulator: every DBEngineVersion element of the recorded list is absent from the response. Why: a client that discovers which engine versions exist, or computes an upgrade path from ValidUpgradeTarget, gets nothing and cannot pick a version to create a cluster with. Fix: the engine-version catalog behind the DescribeDBEngineVersions handler. Traces: neptune-catalog-describe-db-engine-versions#task#1.

Cause 2 of 3: DescribeEventCategories serves the RDS/Aurora category map, not Neptune's, and in a different order. AWS: the recording returns an EventCategoriesMapList whose entries pair each SourceType Neptune publishes with that source type's category list (e.g. db-snapshot with notification/creation/deletion/restoration, db-cluster with failover/migration/failure/notification/serverless/creation/deletion/maintenance/configuration change/global-failover, custom-engine-version with failure), in the service's own ordering. Emulator: the list is populated from a different catalog — source types appear in other positions and with other category sets (e.g. position 0 is db-instance with the 16-category RDS list, position 6 is db-proxy, which the recording does not report there). Why: a client that builds an event-subscription request from this map requests categories Neptune does not publish for that source type, and the subscription is then refused. Fix: the event-category catalog behind the DescribeEventCategories handler. Traces: neptune-catalog-describe-event-categories#task#1.

Cause 3 of 3: DescribeOrderableDBInstanceOptions serializes OrderableDBInstanceOptions as the wrong type. AWS: the recording returns OrderableDBInstanceOptions as a list of OrderableDBInstanceOption elements, each carrying Engine 'neptune', EngineVersion, DBInstanceClass, StorageType, LicenseModel 'amazon-license', Vpc, the AvailabilityZones list, SupportedNetworkTypes and the Supports* capability flags. Emulator: the member is not a list of those elements at all (type_mismatch against the whole OrderableDBInstanceOptions value). Why: a client that enumerates orderable instance classes before creating an instance cannot parse the response, let alone choose a class. Fix: the DescribeOrderableDBInstanceOptions handler's serializer and the orderable-options catalog it reads. Traces: neptune-catalog-describe-orderable-db-instance-options#task#1.

## Evidence
Replay of simparity-wire-aws-v1; all three answered the expected 200 with a diverging body.
Cause 1: $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[0..28] all reported 'missing' — e.g. index 0 expected {Engine 'neptune', EngineVersion '1.1.1.0', DBParameterGroupFamily 'neptune1', DBEngineVersionDescription 'Neptune 1.1.1.0.R7', ValidUpgradeTarget: {UpgradeTarget: [...]}}, got nothing at that position.
Cause 2: EventCategoriesMap[0] expected {SourceType 'db-snapshot', EventCategories: [notification, creation, deletion, restoration]}, got {SourceType 'db-instance', EventCategories: [availability, backtrack, backup, configuration change, ...]}; [5] expected db-cluster, got db-cluster-snapshot; [6] expected custom-engine-version, got db-proxy; 10 positions diverged.
Cause 3: $.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions: type_mismatch — expected {OrderableDBInstanceOption: [ {Engine 'neptune', EngineVersion '1.4.8.0', DBInstanceClass 'db.r5.12xlarge', LicenseModel 'amazon-license', AvailabilityZones: {...}, ...}, ... ]}.

## Steps to Reproduce
1. Replay fixture neptune-catalog-describe-db-engine-versions step task#1 and diff DBEngineVersions against the recording.
2. Replay neptune-catalog-describe-event-categories step task#1 and diff the EventCategoriesMapList entries and their order.
3. Replay neptune-catalog-describe-orderable-db-instance-options step task#1 and observe OrderableDBInstanceOptions is not serialized as a list of OrderableDBInstanceOption.
