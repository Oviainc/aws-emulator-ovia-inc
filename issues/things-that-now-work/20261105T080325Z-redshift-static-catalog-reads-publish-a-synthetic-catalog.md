Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-catalog-describe-cluster-tracks
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-catalog-describe-cluster-tracks redshift-catalog-describe-event-categories redshift-catalog-describe-orderable-cluster-options redshift-catalog-describe-reserved-node-offerings redshift-static-catalog-metadata-reads
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeClusterTracks|2xx->2xx|$.DescribeClusterTracksResponse.DescribeClusterTracksResult.MaintenanceTracks.MaintenanceTrack[*]
Issue-Key: redshift|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap[*]
Issue-Key: redshift|DescribeOrderableClusterOptions|2xx->2xx|$.DescribeOrderableClusterOptionsResponse.DescribeOrderableClusterOptionsResult.OrderableClusterOptions.OrderableClusterOption[*]
Issue-Key: redshift|DescribeReservedNodeOfferings|2xx->2xx|$.DescribeReservedNodeOfferingsResponse.DescribeReservedNodeOfferingsResult.ReservedNodeOfferings.ReservedNodeOffering[*]
Issue-Key: redshift|DescribeClusterTracks|2xx->2xx|$.DescribeClusterTracksResponse.DescribeClusterTracksResult.MaintenanceTracks.MaintenanceTrack
Issue-Commit: 349fac5dd238950f9954cc41b162705b83d96a37

# Redshift static catalog reads publish a synthetic catalog: cluster tracks, event categories, orderable options and reserved-node offerings

> _Note: this issue was identified against the code base at commit `349fac5dd238950f9954cc41b162705b83d96a37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterTracks

## Description
The four pure-read Redshift catalog operations — DescribeClusterTracks, DescribeEventCategories, DescribeOrderableClusterOptions and DescribeReservedNodeOfferings — are served from a catalog seeded into the service store at store creation (see the module header of services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/describe_cluster_tracks.rs: "the catalog is seeded into the store when the store is created"). That seed is emulator-invented rather than a transcription of the catalog AWS publishes, so every one of these reads answers 200 with content that does not match AWS. All five causes below are the same defect in different catalog tables; each must be closed before this issue is submitted. 5 causes.

Cause 1 of 5: DescribeClusterTracks publishes maintenance tracks whose release versions are not AWS's.
  AWS: Smithy: DescribeClusterTracks -> TrackListMessage.MaintenanceTracks (TrackList), errors InvalidClusterTrackFault (awsQueryError InvalidClusterTrack, 400) and UnauthorizedOperation (400); doc: "Returns a list of all the available maintenance tracks." The model does not enumerate the tracks, so for the track CONTENT this is recorded behavior only: the recording shows the two named tracks `current` and `trailing`, each carrying a DatabaseVersion drawn from the Redshift release stream (the recording shows 6-digit patch releases, e.g. current=1.0.434008 with UpdateTargets->UpdateTarget{MaintenanceTrackName=trailing, DatabaseVersion=1.0.436211}, and the mirrored entry for `trailing`), and each UpdateTarget carrying SupportedOperations = restore-from-cluster-snapshot, modify-cluster.
  Emulator: answers 200 with the right track NAMES and the right SupportedOperations, but with DatabaseVersion values from a synthetic low series (current=1.0.49795, trailing=1.0.49608) that belong to no Redshift release. Replay reports both MaintenanceTrack[0] and MaintenanceTrack[1] as missing/extra purely because of those version strings.
  Why: a client that reads DescribeClusterTracks to pick a cluster version (or to assert that the version it is about to pass to CreateCluster/ModifyCluster/RestoreFromClusterSnapshot exists on the track) is handed a version string AWS would never accept, so version-pinning and track-upgrade logic validated against the emulator is wrong against AWS. The exact digits of a patch release are release-dependent and not themselves a requirement; what is required is that the seeded catalog carries the Redshift release-stream versions the recording shows rather than an invented series, and that the two tracks cross-reference each other's version in UpdateTargets.
  Fix: the seeded maintenance-track catalog in the redshift service crate's state seed (crate::state, consumed by ops/describe_cluster_tracks.rs).
  Traces: redshift-catalog-describe-cluster-tracks#task#1.

Cause 2 of 5: DescribeClusterTracks ignores track selection / publishes the wrong number of tracks.
  AWS: Smithy: DescribeClusterTracks input MaintenanceTrackName (String), Marker, MaxRecords; paginated {inputToken Marker, outputToken Marker, items MaintenanceTracks, pageSize MaxRecords}. Recorded behavior only: in this fixture the response body carries MaintenanceTracks with a SINGLE MaintenanceTrack member (the `current` track, with UpdateTargets pointing at `trailing`), i.e. one element serialized as a single query-protocol member, not a two-element list.
  Emulator: answers 200 with a two-element MaintenanceTrack list (`current` and `trailing`), reported by replay as `$.DescribeClusterTracksResponse.DescribeClusterTracksResult.MaintenanceTracks.MaintenanceTrack: type_mismatch (expected <single object>, got <list of 2>)`, and again with the synthetic DatabaseVersions 1.0.49795 / 1.0.49608. Hypothesis: the handler returns the whole seeded table and never narrows it by the request's MaintenanceTrackName (or the seeded table's arity is simply fixed at two), so a single-track selection cannot be expressed.
  Why: the query protocol serializes a one-element list as one member and a two-element list as two, so a client that selects one track by name gets an extra track it did not ask for and its SDK deserializes a different shape than AWS returns; filtering and paging over this catalog cannot be exercised at all.
  Fix: DescribeClusterTracks' handler in the redshift service crate (apply the MaintenanceTrackName selection and the Marker/MaxRecords window to the seeded track list before serializing).
  Traces: redshift-static-catalog-metadata-reads#task#2.

Cause 3 of 5: DescribeEventCategories publishes neither AWS's source types nor AWS's event catalog.
  AWS: Smithy: DescribeEventCategories input SourceType (String), output EventCategoriesMessage.EventCategoriesMapList (list of EventCategoriesMap{SourceType, Events:[EventInfoMap{EventId, EventDescription, EventCategories, Severity}]}); no modeled errors. Recorded behavior only for the catalog content: the recording carries SIX EventCategoriesMap entries (one per source type, beginning with SourceType=cluster), and each entry lists the documented Redshift event ids with their canonical templated descriptions, category and severity — e.g. REDSHIFT-EVENT-2000 "Cluster <cluster name> created at <time in UTC>." (category management, severity INFO), REDSHIFT-EVENT-2001 "Cluster <cluster name> deleted at <time in UTC>." (management, INFO), REDSHIFT-EVENT-3015 (pending, INFO), REDSHIFT-EVENT-3016 / 3000 / 3014 / 3001 (monitoring, INFO), REDSHIFT-EVENT-4000 "Master credentials updated for cluster <cluster name> at <time in UTC>." (security, INFO), REDSHIFT-EVENT-4502 (ERROR). The `<cluster name>` / `<time in UTC>` angle-bracket placeholders are the catalog's FIXED text — this operation returns templates, not instantiated messages, so nothing here is a per-run value.
  Emulator: answers 200 but all six EventCategoriesMap entries diverge (replay reports EventCategoriesMap[0]..[5] missing), i.e. the seeded event catalog does not reproduce AWS's source types, event ids, severities, categories or description templates.
  Why: DescribeEventCategories is how a client discovers which categories it may pass to CreateEventSubscription/ModifyEventSubscription and how it renders event ids it later receives from DescribeEvents. A wrong catalog makes subscription-category validation and event rendering built on the emulator diverge from AWS, and makes the event ids a client whitelists unverifiable.
  Fix: the seeded event-category catalog in the redshift service crate's state seed, consumed by DescribeEventCategories' handler (and shared with DescribeEvents).
  Traces: redshift-catalog-describe-event-categories#task#1.

Cause 4 of 5: DescribeOrderableClusterOptions offers only one node type, so the orderable-shape catalog is wrong.
  AWS: Smithy: DescribeOrderableClusterOptions input ClusterVersion, NodeType, MaxRecords, Marker; output OrderableClusterOptionsMessage.OrderableClusterOptions (list of OrderableClusterOption{ClusterVersion, ClusterType, NodeType, AvailabilityZones}); no modeled errors; paginated on Marker/MaxRecords. Recorded behavior only for the content: the recording lists the current generation of orderable shapes — ra3.16xlarge, ra3.4xlarge and the other ra3 members — as `multi-node` options under ClusterVersion 1.0, each with the region's AvailabilityZones (us-west-2a..d) and SupportedPlatforms VPC.
  Emulator: answers 200 with a catalog in which every entry is NodeType `dc2.large` (alternating ClusterType multi-node/single-node); replay reports nine OrderableClusterOption positions ([0],[1],[2],[3],[7],[9],[11],[17],[19]) as extra/divergent because the published node types are not the ones AWS offers.
  Why: this is the catalog a client consults before CreateCluster to decide which NodeType/ClusterType pair is orderable. An emulator that only ever offers dc2.large makes every ra3-based provisioning path untestable and lets a client conclude a shape is unavailable (or available) contrary to AWS.
  Fix: the seeded orderable-cluster-option catalog in the redshift service crate's state seed, consumed by DescribeOrderableClusterOptions' handler (it must also stay consistent with the node types CreateCluster accepts).
  Traces: redshift-catalog-describe-orderable-cluster-options#task#1.

Cause 5 of 5: DescribeReservedNodeOfferings publishes invented offerings instead of AWS's reserved-node price list.
  AWS: Smithy: DescribeReservedNodeOfferings input ReservedNodeOfferingId, MaxRecords, Marker; output ReservedNodeOfferingsMessage.ReservedNodeOfferings (list of ReservedNodeOffering{ReservedNodeOfferingId, NodeType, Duration, FixedPrice, UsagePrice, CurrencyCode, OfferingType, RecurringCharges, ReservedNodeOfferingType}); errors DependentServiceUnavailableFault (400), ReservedNodeOfferingNotFoundFault (awsQueryError ReservedNodeOfferingNotFound, 404), UnsupportedOperationFault (400). Recorded behavior only for the content: the recording returns ~60 offerings spanning the reserved-node node types (rg.large, rg.xlarge, ...), BOTH term durations (31536000 = 1 year and 94608000 = 3 years), the three OfferingTypes (All Upfront / Partial Upfront / No Upfront) with FixedPrice/UsagePrice/RecurringCharges consistent with the chosen OfferingType (e.g. All Upfront => non-zero FixedPrice with RecurringChargeAmount 0.0; No Upfront => FixedPrice 0.0 with a non-zero hourly RecurringCharge), CurrencyCode USD and ReservedNodeOfferingType Regular.
  Emulator: answers 200 with a catalog in which every offering is NodeType dc2.large with Duration 31536000 only, and with price fields that do not follow the OfferingType (e.g. OfferingType 'No Upfront' with FixedPrice 0 and RecurringChargeAmount 0.175, 'Partial Upfront' with FixedPrice 766.5). Replay reports all 60 ReservedNodeOffering positions as divergent. The ReservedNodeOfferingId values are generated identifiers and are NOT part of the requirement; the node-type coverage, the 1-year and 3-year durations, the OfferingType set and the price/recurring-charge consistency are.
  Why: a client that lists offerings to pick one for PurchaseReservedNodeOffering (or to drive a cost estimate) can only ever see a single 1-year dc2.large shape, so 3-year and ra3/rg reservations are unreachable and any cost logic built on the emulator's prices is wrong; the self-inconsistent price fields also make an offering that AWS would never emit look valid.
  Fix: the seeded reserved-node-offering catalog in the redshift service crate's state seed, consumed by DescribeReservedNodeOfferings' handler (and by PurchaseReservedNodeOffering/GetReservedNodeExchangeOfferings, which read the same table).
  Traces: redshift-catalog-describe-reserved-node-offerings#task#1.

Not covered by any open issue: the open redshift issues 20261103T040731Z-redshift-parameter-group-validation-and-the-parameter-catalog.md (the cluster-PARAMETER catalog), 20261029T192501Z-redshift-read-payloads-omit-modeled-members-and-tagging.md (missing members on cluster/tag reads) and 20261030T022044Z-redshift-invalidclustertrack-refusal-message-omits-the-trailing-period.md (the InvalidClusterTrack refusal text) touch adjacent surfaces but none of them carries the replay key of any of these five clusters, and none of them is about the content of the maintenance-track, event-category, orderable-option or reserved-node-offering tables.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200). All five clusters are verdict=mismatch with expected_status 200 and actual_status 200 — the operations answer, with the wrong catalog.

Cause 1 — redshift-catalog-describe-cluster-tracks#task#1, DescribeClusterTracks, 200 -> 200:
  $.DescribeClusterTracksResponse.DescribeClusterTracksResult.MaintenanceTracks.MaintenanceTrack[0]: expected {'MaintenanceTrackName': 'current', 'DatabaseVersion': '1.0.434008', 'UpdateTargets': {'UpdateTarget': {'MaintenanceTrackName': 'trailing', 'DatabaseVersion': '1.0.436211', 'SupportedOperations': {'SupportedOperation': [{'OperationName': 'restore-from-cluster-snapshot'}, {'OperationName': 'modify-cluster'}]}}}}, got the same shape with DatabaseVersion '1.0.49795' and UpdateTarget DatabaseVersion '1.0.49608'.
  MaintenanceTrack[1]: expected trailing/1.0.436211 -> current/1.0.434008, got trailing/1.0.49608 -> current/1.0.49795.

Cause 2 — redshift-static-catalog-metadata-reads#task#2, DescribeClusterTracks, 200 -> 200:
  $.DescribeClusterTracksResponse.DescribeClusterTracksResult.MaintenanceTracks.MaintenanceTrack: type_mismatch — expected a single object {'MaintenanceTrackName': 'current', 'DatabaseVersion': '1.0.384821', 'UpdateTargets': {'UpdateTarget': {'MaintenanceTrackName': 'trailing', 'DatabaseVersion': '1.0.369760', 'SupportedOperations': {...}}}}, got a 2-element list [current/1.0.49795, trailing/1.0.49608].

Cause 3 — redshift-catalog-describe-event-categories#task#1, DescribeEventCategories, 200 -> 200:
  $...EventCategoriesMapList.EventCategoriesMap[0]..[5]: all six missing. Expected [0] = {'SourceType': 'cluster', 'Events': {'EventInfoMap': [{'EventId': 'REDSHIFT-EVENT-2000', 'EventDescription': 'Cluster <cluster name> created at <time in UTC>.', 'EventCategories': {'EventCategory': 'management'}, 'Severity': 'INFO'}, {'EventId': 'REDSHIFT-EVENT-2001', ... 'management'}, {'EventId': 'REDSHIFT-EVENT-3015', ... 'pending'}, {'EventId': 'REDSHIFT-EVENT-3016', ... 'monitoring'}, {'EventId': 'REDSHIFT-EVENT-3000', ...}, {'EventId': 'REDSHIFT-EVENT-3014', ...}, {'EventId': 'REDSHIFT-EVENT-3001', ...}, {'EventId': 'REDSHIFT-EVENT-4000', ... 'security'}, {'EventId': 'REDSHIFT-EVENT-4502', 'Severity': 'ERROR', ...}, ...]}}.

Cause 4 — redshift-catalog-describe-orderable-cluster-options#task#1, DescribeOrderableClusterOptions, 200 -> 200:
  OrderableClusterOption[0]: expected {'ClusterVersion': '1.0', 'ClusterType': 'multi-node', 'NodeType': 'ra3.16xlarge', 'AvailabilityZones': {'AvailabilityZone': [us-west-2a..d each with SupportedPlatforms VPC]}}, got the same AZ block with NodeType 'dc2.large'.
  OrderableClusterOption[1]: expected multi-node/ra3.4xlarge, got single-node/dc2.large. Divergent positions: [0],[1],[2],[3],[7],[9],[11],[17],[19].

Cause 5 — redshift-catalog-describe-reserved-node-offerings#task#1, DescribeReservedNodeOfferings, 200 -> 200:
  ReservedNodeOffering[0]: expected {'NodeType': 'rg.large', 'Duration': '94608000', 'OfferingType': 'All Upfront', 'FixedPrice': '3743.6', 'UsagePrice': '0.0', 'CurrencyCode': 'USD', 'RecurringCharges': {'RecurringCharge': {'RecurringChargeFrequency': 'Hourly', 'RecurringChargeAmount': '0.0'}}, 'ReservedNodeOfferingType': 'Regular'}, got {'NodeType': 'dc2.large', 'Duration': '31536000', 'OfferingType': 'No Upfront', 'FixedPrice': '0', 'UsagePrice': '0', 'RecurringCharges': {'RecurringCharge': {'RecurringChargeAmount': '0.175', 'RecurringChargeFrequency': 'Hourly'}}, ...}.
  ReservedNodeOffering[1]: expected rg.xlarge / 94608000 / 'No Upfront' / FixedPrice 0.0 / RecurringChargeAmount 0.33075, got dc2.large / 31536000 / 'Partial Upfront' / FixedPrice 766.5 / RecurringChargeAmount 0.088. All 60 positions [0]..[59] divergent.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) against it.
3. Inspect these five steps, all redshift, all 200 -> 200 mismatches:
   - redshift-catalog-describe-cluster-tracks#task#1 (DescribeClusterTracks: MaintenanceTrack DatabaseVersions are a synthetic 1.0.49xxx series)
   - redshift-static-catalog-metadata-reads#task#2 (DescribeClusterTracks: 2-element list where one MaintenanceTrack member is expected)
   - redshift-catalog-describe-event-categories#task#1 (DescribeEventCategories: all 6 EventCategoriesMap entries diverge)
   - redshift-catalog-describe-orderable-cluster-options#task#1 (DescribeOrderableClusterOptions: only dc2.large offered, no ra3.* shapes)
   - redshift-catalog-describe-reserved-node-offerings#task#1 (DescribeReservedNodeOfferings: only 1-year dc2.large offerings, price fields inconsistent with OfferingType)
4. Equivalently, from a client: redshift describe-cluster-tracks, redshift describe-event-categories, redshift describe-orderable-cluster-options and redshift describe-reserved-node-offerings against the emulator, and compare the catalogs with the recorded AWS responses in those fixtures.
