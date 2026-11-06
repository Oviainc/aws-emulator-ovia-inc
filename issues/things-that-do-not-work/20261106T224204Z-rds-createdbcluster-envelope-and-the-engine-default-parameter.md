Issue-Source: trace-sweep-divergence
Issue-Trace: rds-global-cluster-delete-with-member
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-global-cluster-delete-with-member rds-db-parameter-group-lifecycle-and-engine-defaults
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.GlobalClusterResourceId
Issue-Key: rds|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].MinimumEngineVersion
Issue-Related: 20261029T214632Z-rds-dbcluster-response-envelope-omits-modeled-members-and.md
Issue-Related: 20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# RDS CreateDBCluster envelope and the engine-default parameter catalog serve emulator-authored data

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Two net-new causes, both in the rds service: a successful (200) read-back carries values the emulator authored itself rather than the values AWS derives from the request, from the cluster's global-cluster membership, from the region, or from the engine family's real reference data. Both are success-path payload-content defects, not status or error defects. Each is an independent projection in a different handler and must be fixed and closed separately.

=== Cause 1 of 2: CreateDBCluster's DBCluster envelope omits three members and reports a region-wrong HostedZoneId ===
Fixture rds-global-cluster-delete-with-member, step task#7.
AWS: CreateDBCluster returns a DBCluster (Smithy: CreateDBCluster output member DBCluster -> shape DBCluster). On the create path AWS populates, among others:
  - GlobalClusterResourceId — the immutable resource id of the global cluster the new cluster was created into (present whenever GlobalClusterIdentifier was supplied). Recorded behavior only: smithy_query reports no GlobalClusterResourceId member on DBCluster in this model (did_you_mean DbClusterResourceId / GlobalClusterIdentifier), while the recording shows AWS returning one for a cluster created as a global-cluster member. It is a generated id, so its literal value is not a requirement — its PRESENCE and its correspondence to the owning global cluster are.
  - DocdbAnalyticsEnabled and CustomDataGroupPresent — boolean flags (Smithy: DBCluster.DocdbAnalyticsEnabled and DBCluster.CustomDataGroupPresent, both documented as 'ADDED to this model from recorded AWS responses: the shipped model carries no such member on this shape, while the recorded cluster envelope of that control plane always reports the flag'). 'Always reports' is the requirement: they are emitted on every cluster envelope, defaulting to false for a plain Aurora cluster.
  - HostedZoneId — 'The ID that Amazon Route 53 assigns when you create a hosted zone' (Smithy: DBCluster.HostedZoneId). For RDS/Aurora this is the region-stable Route 53 hosted zone id of that region's RDS endpoint domain, derived from the cluster's region; the recording's region resolves to Z1PVIF0B656C1W.
Emulator: 200 (status correct) but the returned DBCluster omits GlobalClusterResourceId, DocdbAnalyticsEnabled and CustomDataGroupPresent entirely (all three absent, not null), and returns a single hard-coded HostedZoneId (ZTGGYY785VSNMZ) that is not the cluster region's RDS hosted zone id — the value is a constant in the emulator rather than a lookup on the cluster's region.
Why: a client that creates a cluster into a global cluster and reads GlobalClusterResourceId from the create response to correlate the member with its global parent gets nothing back and cannot build that association without a second DescribeGlobalClusters round trip; code that branches on DocdbAnalyticsEnabled or CustomDataGroupPresent sees an absent member where AWS guarantees a boolean and either crashes on the missing key or silently takes the wrong branch. The HostedZoneId defect is worse than cosmetic: DNS automation (and CloudFormation/Route 53 alias records pointed at an Aurora endpoint) uses this id as the alias target's hosted zone; a constant from the wrong region makes every such alias record invalid, so the cluster endpoint never resolves through it.
Fix: the CreateDBCluster handler's DBCluster response projection/serializer — populate the three omitted members from the created cluster's state (global-cluster membership and the two always-reported flags) and resolve HostedZoneId from the cluster's region reference table instead of a literal.
Note: open issue 20261029T214632Z-rds-dbcluster-response-envelope-omits-modeled-members-and.md owns the same envelope defect on the DescribeDBClusters read path and carries that step's replay key; it does not carry this CreateDBCluster step, so it does not close this cause.
Traces: rds-global-cluster-delete-with-member#task#7.

=== Cause 2 of 2: DescribeEngineDefaultParameters returns a different parameter set, in a different order, with emulator-authored attributes and a non-corresponding Marker ===
Fixture rds-db-parameter-group-lifecycle-and-engine-defaults, step task#10.
AWS: DescribeEngineDefaultParameters takes the required DBParameterGroupFamily and returns EngineDefaults (Smithy: DescribeEngineDefaultParametersMessage.DBParameterGroupFamily required; output member EngineDefaults; paginated with inputToken Marker, outputToken EngineDefaults.Marker, items EngineDefaults.Parameters, pageSize MaxRecords, errors []). For a given family AWS serves that engine family's REAL default parameter set, ordered by ParameterName ascending, each Parameter carrying that parameter's own ParameterName, Description, ApplyType, DataType, AllowedValues, IsModifiable, MinimumEngineVersion, and (where defaulted) ParameterValue/Source. EngineDefaults.Marker is the opaque cursor identifying the position after the last row on the page; the next page must continue that same ordered sequence.
Emulator: 200 (status correct) but the page is a different catalog: the first row is 'autocommit' (ApplyType dynamic, DataType integer-ish attributes, IsModifiable true, Description 'Sets the autocommit mode.') where the family's first default is 'allow-suspicious-udfs' (ApplyType static, IsModifiable false, Description 'Controls whether user-defined functions that have only an xxx symbol for the main function can be loaded'). Because the sequence is shifted, EVERY indexed row diverges — the replay reports mismatches on ParameterName/Description/ApplyType/DataType/AllowedValues/IsModifiable/ParameterValue/Source/MinimumEngineVersion across rows [0] through [19] — and EngineDefaults.Marker encodes a position in the emulator's own sequence, not AWS's.
Why: this operation is the authoritative source a client uses to discover what is tunable on an engine family before it creates or modifies a parameter group. A caller reads parameters that do not exist for the family and never sees ones that do; it reads ApplyType/IsModifiable wrongly, so it believes a static parameter can be applied without a reboot (or that a modifiable parameter cannot be changed at all) and issues ModifyDBParameterGroup calls AWS rejects. Because the Marker belongs to a different ordering, paginating from it walks a different sequence than AWS would, so even a full enumeration cannot be reconciled.
Fix: the DescribeEngineDefaultParameters handler's engine-default reference data (the per-DBParameterGroupFamily default parameter catalog) plus its ordering and Marker/pagination derivation, so the rows, their attributes and the cursor all come from the family's real default set in ParameterName order.
Note: open issues 20261102T104537Z-rds-cluster-parameter-group-and-catalog-reads-answer.md and 20261104T053605Z-rds-5-net-new-causes-db-subnet-groups.md touch parameter-group and engine-default catalog reads, but neither carries this fixture#step, so neither closes this cause.
Traces: rds-db-parameter-group-lifecycle-and-engine-defaults#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Expected = recorded AWS response, Actual = emulator response.

Cause 1 — rds-global-cluster-delete-with-member#task#7, CreateDBCluster, expected_status 200 / actual_status 200:
  $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.GlobalClusterResourceId: missing (expected '<shaped-not-asserted>', got None)
  $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.DocdbAnalyticsEnabled: missing (expected 'false', got None)
  $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.CustomDataGroupPresent: missing (expected 'false', got None)
  $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.HostedZoneId: value_mismatch (expected 'Z1PVIF0B656C1W', got 'ZTGGYY785VSNMZ')
  GlobalClusterResourceId is recorded as <shaped-not-asserted> — a generated id, so presence and correspondence to the owning global cluster are the requirement, not the literal. HostedZoneId is region-stable (not per-run): the emulator returns one constant regardless of region.

Cause 2 — rds-db-parameter-group-lifecycle-and-engine-defaults#task#10, DescribeEngineDefaultParameters, expected_status 200 / actual_status 200. The page is shifted to a different parameter sequence, so all of rows [0]..[19] diverge. Head of the divergence list:
  EngineDefaults.Marker: value_mismatch (expected 'YmlubG9nX3Jvd3NfcXVlcnlfbG9nX2V2ZW50cw==', got 'bWF4X2hlYXBfdGFibGVfc2l6ZQ==') — base64 of a different parameter position
  Parameters.Parameter[0].ParameterName: value_mismatch (expected 'allow-suspicious-udfs', got 'autocommit')
  Parameters.Parameter[0].ApplyType:     value_mismatch (expected 'static', got 'dynamic')
  Parameters.Parameter[0].IsModifiable:  value_mismatch (expected 'false', got 'true')
  Parameters.Parameter[0].Description:   value_mismatch (expected 'Controls whether user-defined functions that have only an xxx symbol for the main function can be loaded', got 'Sets the autocommit mode.')
  Parameters.Parameter[1].ApplyType:     value_mismatch (expected 'static', got 'dynamic')
  Parameters.Parameter[1].DataType:      value_mismatch (expected 'boolean', got 'integer')
  Parameters.Parameter[1].AllowedValues: value_mismatch (expected '0,1', got '2-31536000')
  Parameters.Parameter[1].Description:   value_mismatch (expected 'Controls whether the server autogenerates SSL key and certificate files in the data directory, if they do not already exist.', got 'The time in seconds the server waits for a connect packet before responding with a bad handshake.')
  The full divergence_key additionally lists mismatched ParameterName/Description/ApplyType/DataType/AllowedValues/IsModifiable/MinimumEngineVersion/ParameterValue/Source on rows [2]..[19].

Model evidence cited above comes from the on-disk Smithy model at services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json: CreateDBCluster output DBCluster; DBCluster.HostedZoneId doc 'The ID that Amazon Route 53 assigns when you create a hosted zone'; DBCluster.DocdbAnalyticsEnabled and DBCluster.CustomDataGroupPresent docs 'ADDED to this model from recorded AWS responses ... the recorded cluster envelope of that control plane always reports the flag'; no GlobalClusterResourceId member exists on DBCluster (recorded behavior only); DescribeEngineDefaultParameters required DBParameterGroupFamily, output EngineDefaults, paginated {inputToken: Marker, outputToken: EngineDefaults.Marker, items: EngineDefaults.Parameters, pageSize: MaxRecords}.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).

Cause 1: replay fixture rds-global-cluster-delete-with-member step task#7 — CreateGlobalCluster, then CreateDBCluster supplying that GlobalClusterIdentifier. Inspect the 200 CreateDBClusterResponse's DBCluster: GlobalClusterResourceId, DocdbAnalyticsEnabled and CustomDataGroupPresent are absent, and HostedZoneId is the emulator's hard-coded constant rather than the cluster region's RDS Route 53 hosted zone id.

Cause 2: replay fixture rds-db-parameter-group-lifecycle-and-engine-defaults step task#10 — DescribeEngineDefaultParameters for the fixture's DBParameterGroupFamily. Compare EngineDefaults.Parameters row by row against the recording: row [0] is 'autocommit' instead of the family's first default 'allow-suspicious-udfs', every later index is correspondingly shifted, per-row ApplyType/DataType/AllowedValues/IsModifiable/Description are the emulator's values, and EngineDefaults.Marker decodes to a different parameter position.

After fixing, re-replay both fixture#step bindings (the representative rds-global-cluster-delete-with-member#task#7 plus rds-db-parameter-group-lifecycle-and-engine-defaults#task#10); both 200 payloads must match the recorded envelopes, with generated ids (GlobalClusterResourceId) checked for presence and correct global-cluster correspondence rather than literal equality.
