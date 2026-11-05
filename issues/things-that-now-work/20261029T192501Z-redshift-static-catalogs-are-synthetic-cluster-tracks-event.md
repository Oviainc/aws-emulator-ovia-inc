Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-catalog-describe-reserved-node-offerings
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-catalog-describe-reserved-node-offerings redshift-account-attributes-and-reserved-node-offerings redshift-catalog-describe-cluster-tracks redshift-catalog-describe-event-categories redshift-catalog-describe-orderable-cluster-options redshift-static-catalog-metadata-reads
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeReservedNodeOfferings|2xx->2xx|$.DescribeReservedNodeOfferingsResponse.DescribeReservedNodeOfferingsResult.ReservedNodeOfferings.ReservedNodeOffering[*]
Issue-Key: redshift|DescribeClusterTracks|2xx->2xx|$.DescribeClusterTracksResponse.DescribeClusterTracksResult.MaintenanceTracks.MaintenanceTrack[*]
Issue-Key: redshift|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap[*]
Issue-Key: redshift|DescribeOrderableClusterOptions|2xx->2xx|$.DescribeOrderableClusterOptionsResponse.DescribeOrderableClusterOptionsResult.OrderableClusterOptions.OrderableClusterOption[*]
Issue-Key: redshift|DescribeClusterTracks|2xx->2xx|$.DescribeClusterTracksResponse.DescribeClusterTracksResult.MaintenanceTracks.MaintenanceTrack
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift static catalogs are synthetic: cluster tracks, event categories, orderable options and reserved-node offerings

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReservedNodeOfferings

## Description
6 root causes: the four Redshift read-only catalog operations serve emulator-invented inventories instead of the service's catalog, and one of them also serializes a multi-entry list as a single structure.

Cause 1 of 6: DescribeReservedNodeOfferings (unfiltered) serves a placeholder catalog. AWS returns the region's reserved-node offering catalog: dozens of ReservedNodeOffering entries spanning the current node-type families (rg./ra3./dc2. classes), with Duration values of both 31536000 and 94608000 seconds, OfferingType values from the current vocabulary ("All Upfront", "Partial Upfront", "No Upfront"), ReservedNodeOfferingType "Regular", and a populated RecurringCharges structure containing a RecurringCharge with RecurringChargeFrequency "Hourly" and a RecurringChargeAmount. Emulator: a short synthetic list of dc2.large-only entries with Duration fixed at 31536000, retired OfferingType values "Light Utilization"/"Medium Utilization", ReservedNodeOfferingType "Upgradable", and RecurringCharges serialized as an empty string instead of a structure. Why: a client that selects an offering by node type, term length or offering type finds nothing usable, and PurchaseReservedNodeOffering can never be driven from the catalog the emulator advertises; code reading RecurringCharges.RecurringCharge crashes on a string. Fix: the DescribeReservedNodeOfferings state/catalog seed must carry the real offering inventory, and the serializer must emit RecurringCharges as a structure. Traces: redshift-catalog-describe-reserved-node-offerings#task#1.

Cause 2 of 6: DescribeReservedNodeOfferings constrained to a single offering id returns an entry from the same synthetic catalog instead of the requested offering's real attributes (node type, duration, fixed/usage price, offering type, recurring charges). Fix: as cause 1, on the by-id lookup path. Traces: redshift-account-attributes-and-reserved-node-offerings#task#2.

Cause 3 of 6: DescribeClusterTracks serves placeholder DatabaseVersion values. AWS returns a MaintenanceTrack per track name ("current", "trailing") where each track's DatabaseVersion is the real engine build for that track and the two tracks carry DIFFERENT versions, and each UpdateTarget's DatabaseVersion is the version of the track it points at. Emulator: both tracks and every UpdateTarget report one constant placeholder version, so the tracks are indistinguishable; the SupportedOperations entries are also emitted in a different order from AWS's. Why: a client choosing a maintenance track by engine version, or diffing current vs trailing, cannot. Fix: the DescribeClusterTracks catalog seed/state. Traces: redshift-catalog-describe-cluster-tracks#task#1.

Cause 4 of 6: DescribeEventCategories serves none of the service's event catalog. AWS returns an EventCategoriesMap per source type (cluster, cluster-parameter-group, cluster-security-group, cluster-snapshot, scheduled-action, reserved-node), each with an Events list of EventInfoMap entries carrying EventId (the REDSHIFT-EVENT-nnnn identifiers), EventDescription (the service's templated description text), Severity (INFO/ERROR) and EventCategories (management, monitoring, pending, security, ...). The emulator's response does not contain these maps at all. Why: any client that enumerates event ids/categories — notably to build a CreateEventSubscription EventCategories list — gets an empty or wrong vocabulary. Fix: seed the DescribeEventCategories catalog with the real per-source-type event inventory. Traces: redshift-catalog-describe-event-categories#task#1.

Cause 5 of 6: DescribeOrderableClusterOptions serves a dc2.large-only catalog. AWS returns one OrderableClusterOption per (ClusterVersion, ClusterType, NodeType) combination that can actually be ordered — including the ra3. family (e.g. ra3.16xlarge, ra3.4xlarge) with ClusterType multi-node — each with its AvailabilityZones list. The emulator emits only dc2.large rows, so every ra3./rg. row AWS returns is absent and the dc2.large rows it does return are extra. Why: a client that validates a requested node type against the orderable catalog rejects node types AWS permits. Fix: the DescribeOrderableClusterOptions catalog seed. Traces: redshift-catalog-describe-orderable-cluster-options#task#1.

Cause 6 of 6: DescribeClusterTracks serializes the MaintenanceTracks collection with the wrong cardinality. For the recorded request AWS returns a single MaintenanceTrack element (one structure) under MaintenanceTracks; the emulator always emits a two-element array, so the deserialized type at $...MaintenanceTracks.MaintenanceTrack is a list where AWS gives a structure (type_mismatch), independent of the version values in cause 3. Why: a client reading MaintenanceTracks.MaintenanceTrack as the single returned track gets a list and fails to deserialize. Fix: the DescribeClusterTracks handler must return only the tracks the request selects, and the serializer must follow awsQuery single-element flattening as AWS does. Traces: redshift-static-catalog-metadata-reads#task#2.

## Evidence
Replay of simparity-wire-aws-v1; all six clusters verdict=mismatch with expected_status == actual_status == 200 (purely payload divergences).

DescribeReservedNodeOfferings (redshift-catalog-describe-reserved-node-offerings#task#1): 60 ReservedNodeOffering positions diverge. Example entry [0]: expected {'Duration': '94608000', 'OfferingType': 'All Upfront', 'ReservedNodeOfferingType': 'Regular', 'RecurringCharges': {'RecurringCharge': {'RecurringChargeFrequency': 'Hourly', 'RecurringChargeAmount': '0.0'}}, 'NodeType': 'rg.large', ...}; got {'NodeType': 'dc2.large', 'Duration': '31536000', 'OfferingType': 'Light Utilization', 'RecurringCharges': '', 'ReservedNodeOfferingType': 'Upgradable', ...}.
DescribeReservedNodeOfferings by id (redshift-account-attributes-and-reserved-node-offerings#task#2): expected NodeType 'rg.12xlarge', OfferingType 'Partial Upfront', ReservedNodeOfferingType 'Regular', RecurringCharges a structure; got NodeType 'dc2.large', OfferingType 'Light Utilization', ReservedNodeOfferingType 'Upgradable', RecurringCharges ''.
DescribeClusterTracks (redshift-catalog-describe-cluster-tracks#task#1): expected track 'current' DatabaseVersion '1.0.434008' with UpdateTarget 'trailing' at '1.0.436211', and track 'trailing' at '1.0.436211' with UpdateTarget 'current' at '1.0.434008'; got every version as the single constant '1.0.49795', and SupportedOperation entries in the opposite order.
DescribeEventCategories (redshift-catalog-describe-event-categories#task#1): EventCategoriesMap[0..5] all reported missing; expected maps include SourceType 'cluster' with EventInfoMap entries REDSHIFT-EVENT-2000/2001/3000/3001/3014/3015/3016/4000/4502 and their Severity and EventCategories values.
DescribeOrderableClusterOptions (redshift-catalog-describe-orderable-cluster-options#task#1): expected rows include NodeType 'ra3.16xlarge' and 'ra3.4xlarge' (ClusterType 'multi-node'); got only 'dc2.large' rows, reported as extra at positions 0,1,2,3,7,9,11,17,19.
DescribeClusterTracks (redshift-static-catalog-metadata-reads#task#2): $...MaintenanceTracks.MaintenanceTrack: type_mismatch — expected a single structure (track 'current', DatabaseVersion '1.0.384821'), got a two-element array.

Individual version strings, prices and offering ids in the recording are one region's catalog snapshot; the requirement is that the catalog be the service's real inventory with the modeled value vocabularies and structure shapes, not a single synthetic row family.

## Steps to Reproduce
1. Replay fixture redshift-catalog-describe-reserved-node-offerings step task#1 and compare the ReservedNodeOfferings list: node-type families, Duration values, OfferingType/ReservedNodeOfferingType vocabularies and the RecurringCharges shape.
2. Replay redshift-account-attributes-and-reserved-node-offerings step task#2 (DescribeReservedNodeOfferings for one offering id).
3. Replay redshift-catalog-describe-cluster-tracks step task#1 and compare per-track DatabaseVersion values and SupportedOperations order.
4. Replay redshift-catalog-describe-event-categories step task#1 and check that EventCategoriesMapList carries a map per source type with EventInfoMap entries.
5. Replay redshift-catalog-describe-orderable-cluster-options step task#1 and compare the (ClusterVersion, ClusterType, NodeType) rows.
6. Replay redshift-static-catalog-metadata-reads step task#2 and check the cardinality/type of MaintenanceTracks.MaintenanceTrack.
