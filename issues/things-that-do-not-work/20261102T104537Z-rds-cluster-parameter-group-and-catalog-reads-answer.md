Issue-Source: trace-sweep-divergence
Issue-Trace: rds-refarch-aur-cluster-parameter-hardening-route
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-refarch-aur-cluster-parameter-hardening-route rds-describe-global-clusters-db-cluster-id-filter rds-refarch-aur-create-cluster-production-settings-refusals rds-refarch-aur-engine-version-and-serverless-class-lookups
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyDBClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ModifyDBClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ModifyDBClusterParameterGroup|4xx->4xx|status
Issue-Key: rds|DescribeDBClusterParameters|2xx->4xx|$.DescribeDBClusterParametersResponse
Issue-Key: rds|DescribeDBClusterParameters|2xx->4xx|status
Issue-Key: rds|DescribeDBClusterParameters|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|DescribeDBClusterParameters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DescribeDBClusterParameters|4xx->4xx|status
Issue-Key: rds|CreateDBClusterParameterGroup|2xx->4xx|$.CreateDBClusterParameterGroupResponse
Issue-Key: rds|CreateDBClusterParameterGroup|2xx->4xx|status
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].ApplyMethod
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].ParameterName
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter[*].ParameterValue
Issue-Key: rds|DescribeGlobalClusters|2xx->4xx|$.DescribeGlobalClustersResponse
Issue-Key: rds|DescribeGlobalClusters|2xx->4xx|status
Issue-Key: rds|CreateDBCluster|4xx->2xx|$.ErrorResponse
Issue-Key: rds|CreateDBCluster|4xx->2xx|status
Issue-Key: rds|DescribeDBClusters|4xx->2xx|$.ErrorResponse
Issue-Key: rds|DescribeDBClusters|4xx->2xx|status
Issue-Key: rds|DescribeOrderableDBInstanceOptions|2xx->2xx|$.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions.OrderableDBInstanceOption
Issue-Key: rds|DescribeEngineDefaultClusterParameters|4xx->2xx|$.ErrorResponse
Issue-Key: rds|DescribeEngineDefaultClusterParameters|4xx->2xx|status
Issue-Related: 20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md
Issue-Commit: 276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2

# RDS cluster parameter group and catalog reads answer wrong outcomes and skip request refusals

> _Note: this issue was identified against the code base at commit `276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusterParameters

## Description
This bundle carries TEN distinct root causes in which the rds crate's parameter-group state, its catalog reads and its create-path preconditions make the emulator answer success where AWS refuses, refuse where AWS succeeds, or serve a parameter/option set that is not AWS's. All ten must be closed before this issue is submitted. The replay reported owned_by=[] for every one of them. Open issues that look adjacent but do NOT cover these steps: '20261029T214632Z-rds-parameter-listings-omit-engine-default-metadata-and.md', '20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md', '20261102T002746Z-rds-describeenginedefaultclusterparameters-and-createdbsubnetgroup-synthesize-data-the-request.md', '20261029T214632Z-rds-accepts-nine-requests-aws-refuses-missing-request.md' and '20261029T210641Z-rds-2-net-new-root-causes-a-freshly.md'.

Cause 1 of 10 - the account's engine-default DB cluster parameter groups do not exist, so modifying one answers not-found instead of refusing the modification.
AWS: every account is pre-populated with an engine-default cluster parameter group per family (default.<family>, e.g. default.aurora-postgresql16), and those groups are immutable. Smithy: ModifyDBClusterParameterGroup errors are DBParameterGroupNotFoundFault (httpError 404, awsQueryError DBParameterGroupNotFound/404) and InvalidDBParameterGroupStateFault (400); the immutability refusal is the query-protocol service-common InvalidParameterValue. Recorded response: HTTP 400, Error/Type Sender, Error/Code InvalidParameterValue, Error/Message 'Cannot modify a default parameter group.' - the entire sentence is fixed text, with no group name interpolated.
Emulator: HTTP 404, Error/Code DBParameterGroupNotFound, Error/Message 'DBClusterParameterGroup not found: <name>' - the default group is simply absent from state, so the handler never reaches the immutability guard.
Why: a client cannot tell 'default groups are immutable, make your own copy' from 'that group is missing'. Provisioning code that creates a group on 404 and modifies on 200 takes the wrong branch, and the whole engine-default parameter surface is unreachable.
Fix: the DB cluster parameter group state seeding (the default.<family> groups must exist for every supported family) plus the default-group guard in the ModifyDBClusterParameterGroup handler.
Traces: rds-refarch-aur-cluster-parameter-hardening-route#task#1.

Cause 2 of 10 - DescribeDBClusterParameters cannot read an engine-default cluster parameter group at all.
AWS: Smithy: DescribeDBClusterParameters input DBClusterParameterGroupName required, output Parameters -> ParametersList, errors=[DBParameterGroupNotFoundFault 404]. For a default.<family> group the recorded response is HTTP 200 with DescribeDBClusterParametersResult/Parameters present.
Emulator: HTTP 404 and no DescribeDBClusterParametersResponse envelope at all, because of the same missing default-group state as cause 1.
Why: reading engine defaults is the first step of every parameter-tuning workflow; a 404 here aborts it before anything can be compared or copied.
Fix: the same cluster parameter group state seeding, consumed by the DescribeDBClusterParameters handler.
Traces: rds-refarch-aur-cluster-parameter-hardening-route#task#2.

Cause 3 of 10 - DescribeDBClusterParameters never validates Source.
AWS: Smithy: DescribeDBClusterParameters input member Source (String). An unsupported Source value is refused with HTTP 400, Error/Code InvalidParameterValue and Error/Message 'Invalid Source: <value>. Allowed values are: engine-default, system, user' - fixed text 'Invalid Source: ' + the rejected Source from the request + '. Allowed values are: engine-default, system, user' (recorded behavior).
Emulator: HTTP 404, Error/Code DBParameterGroupNotFound, Error/Message 'DBClusterParameterGroup not found: <name>' - the group lookup fails first (cause 1/2) and the Source value is never examined, so even with the group present this validation is absent.
Why: a caller passing an unsupported Source gets a resource error rather than a parameter error, and once the group exists would silently get an unfiltered listing instead of a refusal.
Fix: DescribeDBClusterParameters input validation for Source, running independently of the group lookup.
Traces: rds-refarch-aur-cluster-parameter-hardening-route#task#3.

Cause 4 of 10 - CreateDBClusterParameterGroup refuses a create AWS accepts.
AWS: Smithy: CreateDBClusterParameterGroup input DBClusterParameterGroupName, DBParameterGroupFamily and Description all required; output DBClusterParameterGroup; modeled errors are only DBParameterGroupAlreadyExistsFault (400) and DBParameterGroupQuotaExceededFault (400). Recorded response for a fresh name on a supported Aurora PostgreSQL family: HTTP 200 with CreateDBClusterParameterGroupResult/DBClusterParameterGroup carrying DBClusterParameterGroupName and DBParameterGroupFamily echoed from the request, Description echoed from the request, and a DBClusterParameterGroupArn of the form arn:aws:rds:<region>:<account>:cluster-pg:<name>.
Emulator: HTTP 400 and no CreateDBClusterParameterGroupResponse envelope, for a name that does not exist yet and a family the emulator itself reports elsewhere as a cluster's parameter-group family. Hypothesis: the handler's family (or name) acceptance check rejects a family the rest of the crate treats as valid.
Why: no customer-owned cluster parameter group can be created, so every hardening/tuning flow stops at step one; neither of the two modeled 400 classes applies, so the refusal is also unmodeled.
Fix: the CreateDBClusterParameterGroup handler's request validation and its success path/serializer.
Traces: rds-refarch-aur-cluster-parameter-hardening-route#task#5.

Cause 5 of 10 - a customer cluster parameter group's parameter listing is not the set AWS reports after a modify.
AWS: Smithy: DescribeDBClusterParameters output Parameters -> ParametersList of Parameter. Recorded response: HTTP 200 with each Parameter carrying its engine-default metadata (ParameterName, ParameterValue, Description, Source, ApplyType, DataType, AllowedValues, IsModifiable, ApplyMethod), the entries ordered as AWS returns them, and only the parameters the group actually holds. In the recording the first entry carries AllowedValues '-1-2147483647', ApplyType 'dynamic', DataType 'integer', IsModifiable 'true' and a Description, and the second entry is the static parameter the request set, whose ApplyMethod is 'pending-reboot'.
Emulator: HTTP 200 but the first entry omits AllowedValues, ApplyType, DataType, Description and IsModifiable entirely; the second entry is a different parameter altogether (a force-SSL parameter with ParameterValue '2' and ApplyMethod 'immediate' where AWS records the logical-replication parameter with value '1' and ApplyMethod 'pending-reboot'); and a third entry is emitted that AWS does not return.
Why: clients read ApplyType/ApplyMethod to decide whether a reboot is required and AllowedValues/IsModifiable to validate edits; missing metadata plus an extra, differently-valued entry makes those decisions wrong and reports settings the group was never given.
Fix: the cluster parameter group state (what a modify records) and the DescribeDBClusterParameters projection that merges engine-default metadata onto stored parameters.
Traces: rds-refarch-aur-cluster-parameter-hardening-route#task#9.

Cause 6 of 10 - DescribeGlobalClusters rejects a filter AWS supports.
AWS: Smithy: DescribeGlobalClusters input Filters -> FilterList, errors=[GlobalClusterNotFoundFault 404]. AWS accepts the db-cluster-id filter on this operation and, when nothing matches, answers HTTP 200 with DescribeGlobalClustersResult/GlobalClusters present and empty plus ResponseMetadata/RequestId (recorded behavior).
Emulator: HTTP 400 with no DescribeGlobalClustersResponse envelope - its own refusal text (see the sibling bundle) states region is the only filter it accepts.
Why: the documented way to find the global cluster a DB cluster belongs to is unavailable; callers get a request error for a well-formed, supported query instead of an empty result set.
Fix: the DescribeGlobalClusters handler's filter support (accept db-cluster-id and match it against member clusters) and its empty-result success path.
Traces: rds-describe-global-clusters-db-cluster-id-filter#task#1 (cluster also covers #task#2).

Cause 7 of 10 - CreateDBCluster accepts a MasterUsername the engine reserves.
AWS: Smithy: CreateDBCluster input MasterUsername (String); the reserved-word refusal is the query-protocol service-common InvalidParameterValue. Recorded response when MasterUsername is an engine-reserved name: HTTP 400, Error/Type Sender, Error/Code InvalidParameterValue, Error/Message 'MasterUsername <username> cannot be used as it is a reserved word used by the engine' - fixed text 'MasterUsername ' + the requested username + ' cannot be used as it is a reserved word used by the engine'.
Emulator: HTTP 200 with no ErrorResponse at all - the cluster is created under the reserved master username.
Why: the emulator accepts a configuration AWS can never produce, so code paths that must handle the refusal are never exercised, and downstream state diverges permanently (see cause 8).
Fix: CreateDBCluster request validation, in its reserved-MasterUsername check.
Traces: rds-refarch-aur-create-cluster-production-settings-refusals#task#8.

Cause 8 of 10 - a cluster whose create AWS refused is afterwards readable, so DescribeDBClusters answers 200 where AWS answers 404.
AWS: Smithy: DescribeDBClusters errors=[DBClusterNotFoundFault httpError 404, awsQueryError DBClusterNotFoundFault/404]. Because the preceding create was refused, the identifier names nothing and the recorded response is HTTP 404, Error/Type Sender, Error/Code DBClusterNotFoundFault, Error/Message 'DBCluster <DBClusterIdentifier> not found.' - fixed text 'DBCluster ' ... ' not found.' with the request's identifier echoed verbatim.
Emulator: HTTP 200 with a full DBCluster envelope and no ErrorResponse.
Why: the emulator's state retains a resource AWS never created, so cleanup, drift detection and 'does it exist yet' polling all read a phantom cluster; any test that asserts the refusal took effect passes against the wrong state.
Fix: the CreateDBCluster handler must not commit cluster state on a validated refusal (the create-path validation of cause 7 has to run before the state write).
Traces: rds-refarch-aur-create-cluster-production-settings-refusals#task#11.

Cause 9 of 10 - DescribeOrderableDBInstanceOptions serves a different shape of option set than AWS for an Aurora PostgreSQL serverless lookup.
AWS: Smithy: DescribeOrderableDBInstanceOptions output OrderableDBInstanceOptions -> OrderableDBInstanceOptionsList, paginated (items OrderableDBInstanceOptions). Recorded response: HTTP 200 with the OrderableDBInstanceOption member REPEATED - one entry per orderable combination for the requested engine/class - each entry carrying the full option record (Engine, EngineVersion, DBInstanceClass, StorageType, AvailabilityZones with one AvailabilityZone entry per AZ in the region, SupportedEngineModes, SupportedNetworkTypes, LicenseModel and the Supports*/Capable boolean flags).
Emulator: HTTP 200 but the OrderableDBInstanceOption node is not a repeated list - the replay reports a type_mismatch where AWS returned a list of option entries - so the catalog projects a single, differently shaped option record instead of the full repeated set.
Why: clients enumerate this list to pick a valid instance class/engine-version combination; a node that is not a repeated member deserializes to one option (or fails), so selection logic sees a catalog that does not reflect what can actually be ordered.
Fix: the DescribeOrderableDBInstanceOptions catalog and its Query-protocol list serializer, which must emit one repeated OrderableDBInstanceOption member per orderable combination.
Traces: rds-refarch-aur-engine-version-and-serverless-class-lookups#task#4.

Cause 10 of 10 - DescribeEngineDefaultClusterParameters synthesizes defaults for a parameter-group family that does not exist.
AWS: Smithy: DescribeEngineDefaultClusterParameters input DBParameterGroupFamily required, output EngineDefaults, errors=[] - so an unknown family is refused with the query-protocol service-common error. Recorded response: HTTP 400, Error/Type Sender, Error/Code InvalidParameterValue, Error/Message 'ParameterGroupFamily <family> is not a valid parameter group family' - fixed text 'ParameterGroupFamily ' + the requested family + ' is not a valid parameter group family'.
Emulator: HTTP 200 with a synthesized EngineDefaults payload and no ErrorResponse - any family string is accepted.
Why: a typo in a family name silently yields a plausible-looking default parameter set, so callers tune against defaults for an engine family that does not exist and never see the refusal AWS gives.
Fix: DescribeEngineDefaultClusterParameters request validation, which must check DBParameterGroupFamily against the supported-family catalog before projecting defaults.
Traces: rds-refarch-aur-engine-version-and-serverless-class-lookups#task#9.

## Evidence
All ten causes come from the simparity-wire-aws-v1 replay (service=rds, verdict=mismatch, owned_by=[], stub_path="").

Cause 1 - rds-refarch-aur-cluster-parameter-hardening-route#task#1, ModifyDBClusterParameterGroup: status expected 400, got 404; Error.Code expected 'InvalidParameterValue', got 'DBParameterGroupNotFound'; Error.Message expected 'Cannot modify a default parameter group.', got 'DBClusterParameterGroup not found: default.aurora-postgresql16'

Cause 2 - rds-refarch-aur-cluster-parameter-hardening-route#task#2, DescribeDBClusterParameters: status expected 200, got 404; $.DescribeDBClusterParametersResponse missing (expected {'DescribeDBClusterParametersResult': {'Parameters': ''}, 'ResponseMetadata': {'RequestId': <shaped-not-asserted>}}, got None)

Cause 3 - rds-refarch-aur-cluster-parameter-hardening-route#task#3, DescribeDBClusterParameters: status expected 400, got 404; Error.Code expected 'InvalidParameterValue', got 'DBParameterGroupNotFound'; Error.Message expected 'Invalid Source: customer. Allowed values are: engine-default, system, user', got 'DBClusterParameterGroup not found: default.aurora-postgresql16'

Cause 4 - rds-refarch-aur-cluster-parameter-hardening-route#task#5, CreateDBClusterParameterGroup: status expected 200, got 400; $.CreateDBClusterParameterGroupResponse missing (expected CreateDBClusterParameterGroupResult/DBClusterParameterGroup with DBClusterParameterGroupArn 'arn:aws:rds:us-west-2:000000000000:cluster-pg:sim2real-probe-aur-cpg-9bbba349', DBParameterGroupFamily 'aurora-postgresql16', DBClusterParameterGroupName 'sim2real-probe-aur-cpg-9bbba349', Description 'sim2real refarch-aur hardening'; got None)

Cause 5 - rds-refarch-aur-cluster-parameter-hardening-route#task#9, DescribeDBClusterParameters, status 200 both sides: Parameter[0].AllowedValues missing (expected '-1-2147483647'), Parameter[0].ApplyType missing (expected 'dynamic'), Parameter[0].DataType missing (expected 'integer'), Parameter[0].Description missing (expected '(ms) Sets the minimum execution time above which statements will be logged.'), Parameter[0].IsModifiable missing (expected 'true'); Parameter[1].ParameterName expected 'rds.logical_replication', got 'rds.force_ssl'; Parameter[1].ParameterValue expected '1', got '2'; Parameter[1].ApplyMethod expected 'pending-reboot', got 'immediate'; Parameter[1].Description expected 'Enables logical decoding.', got 'Requires client connections to the cluster to use TLS.'; Parameter[2] extra (expected None, got a third rds.logical_replication entry)

Cause 6 - rds-describe-global-clusters-db-cluster-id-filter#task#1 (cluster also covers #task#2), DescribeGlobalClusters: status expected 200, got 400; $.DescribeGlobalClustersResponse missing (expected {'DescribeGlobalClustersResult': {'GlobalClusters': ''}, 'ResponseMetadata': {'RequestId': <shaped-not-asserted>}}, got None)

Cause 7 - rds-refarch-aur-create-cluster-production-settings-refusals#task#8, CreateDBCluster: status expected 400, got 200; $.ErrorResponse missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'MasterUsername rdsadmin cannot be used as it is a reserved word used by the engine'}, 'RequestId': <shaped-not-asserted>}, got None)

Cause 8 - rds-refarch-aur-create-cluster-production-settings-refusals#task#11, DescribeDBClusters: status expected 404, got 200; $.ErrorResponse missing (expected {'Error': {'Type': 'Sender', 'Code': 'DBClusterNotFoundFault', 'Message': 'DBCluster sim2real-probe-aur-new-fcdc2972 not found.'}, 'RequestId': <shaped-not-asserted>}, got None)

Cause 9 - rds-refarch-aur-engine-version-and-serverless-class-lookups#task#4, DescribeOrderableDBInstanceOptions, status 200 both sides: $.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions.OrderableDBInstanceOption type_mismatch - expected a list of option entries (Engine 'aurora-postgresql', EngineVersion '16.6', DBInstanceClass 'db.serverless', StorageType 'aurora', AvailabilityZones/AvailabilityZone for us-west-2a..d, SupportedEngineModes member 'provisioned', SupportedNetworkTypes members DUAL and IPV4, SupportsHttpEndpoint 'true', SupportsGlobalDatabases 'true', ...), got a non-list node

Cause 10 - rds-refarch-aur-engine-version-and-serverless-class-lookups#task#9, DescribeEngineDefaultClusterParameters: status expected 400, got 200; $.ErrorResponse missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'ParameterGroupFamily aurora-postgresql99 is not a valid parameter group family'}, 'RequestId': <shaped-not-asserted>}, got None)

Smithy grounding used (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): ModifyDBClusterParameterGroup errors=[DBParameterGroupNotFoundFault httpError 404 awsQueryError DBParameterGroupNotFound/404, InvalidDBParameterGroupStateFault 400]; DescribeDBClusterParameters input {DBClusterParameterGroupName required, Source, Filters, MaxRecords, Marker}, output Parameters->ParametersList, errors=[DBParameterGroupNotFoundFault 404]; CreateDBClusterParameterGroup input {DBClusterParameterGroupName, DBParameterGroupFamily, Description all required}, errors=[DBParameterGroupAlreadyExistsFault 400, DBParameterGroupQuotaExceededFault 400]; DescribeGlobalClusters input Filters->FilterList; CreateDBCluster input MasterUsername; DescribeDBClusters errors=[DBClusterNotFoundFault 404]; DescribeOrderableDBInstanceOptions output OrderableDBInstanceOptions->OrderableDBInstanceOptionsList, paginated; DescribeEngineDefaultClusterParameters input DBParameterGroupFamily required, errors=[].

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) with no arguments.
3. Inspect these steps, each reported as a mismatch cluster with service=rds and owned_by=[]:
   - rds-refarch-aur-cluster-parameter-hardening-route, task#1 (ModifyDBClusterParameterGroup on default.aurora-postgresql16), task#2 (DescribeDBClusterParameters on default.aurora-postgresql16), task#3 (DescribeDBClusterParameters with Source=customer), task#5 (CreateDBClusterParameterGroup on family aurora-postgresql16), task#9 (DescribeDBClusterParameters on the customer group after a modify)
   - rds-describe-global-clusters-db-cluster-id-filter, task#1 (DescribeGlobalClusters with the db-cluster-id filter)
   - rds-refarch-aur-create-cluster-production-settings-refusals, task#8 (CreateDBCluster with MasterUsername=rdsadmin), task#11 (DescribeDBClusters on the identifier whose create AWS refused)
   - rds-refarch-aur-engine-version-and-serverless-class-lookups, task#4 (DescribeOrderableDBInstanceOptions for aurora-postgresql / db.serverless), task#9 (DescribeEngineDefaultClusterParameters with DBParameterGroupFamily=aurora-postgresql99)
4. For each, compare the HTTP status, the presence/absence of the success envelope, and $.ErrorResponse.Error.Code / .Message (or the listed result members) against the recorded AWS response quoted in the evidence.
