Issue-Source: trace-sweep-divergence
Issue-Trace: rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster rds-describe-global-clusters-db-cluster-id-filter rds-refarch-aur-create-cluster-production-settings-refusals rds-refarch-aur-engine-version-and-serverless-class-lookups
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBClusters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DescribeDBClusters|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|DescribeDBClusters|4xx->4xx|status
Issue-Key: rds|DescribeDBInstances|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DescribeGlobalClusters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ModifyDBCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ModifyDBCluster|4xx->4xx|status
Issue-Key: rds|DescribeOrderableDBInstanceOptions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DescribeOrderableDBInstanceOptions|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Related: 20261029T214632Z-rds-not-found-refusals-for-instances-clusters-subnet.md
Issue-Related: 20261102T074545Z-rds-create-path-refusal-messages-use-emulator-wording.md
Issue-Commit: 276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2

# RDS error envelopes return wrong code, status and message wording on cluster lookup, filter and create paths

> _Note: this issue was identified against the code base at commit `276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
This bundle carries NINE distinct root causes in the rds crate's refusal paths. Every one of them is a divergence in the HTTP status, the query-protocol Error/Code, or the Error/Message text of a refusal, and all nine must be closed before this issue is submitted. None of them is owned by an existing open issue: the replay reported owned_by=[] for each. Open issues that look adjacent but do NOT cover these steps are '20261029T214632Z-rds-not-found-refusals-for-instances-clusters-subnet.md', '20261029T214632Z-rds-answers-the-wrong-error-class-because-existence.md' and '20261102T074545Z-rds-create-path-refusal-messages-use-emulator-wording.md'.

Cause 1 of 9 - DescribeDBClusters not-found message substitutes the cluster ARN for the requested identifier.
AWS: Smithy (rds): DescribeDBClusters error DBClusterNotFoundFault, errorKind client, httpError 404, awsQueryError {code DBClusterNotFoundFault, httpResponseCode 404}. Recorded response: HTTP 404, Error/Type Sender, Error/Code DBClusterNotFoundFault, Error/Message of the form 'DBCluster <DBClusterIdentifier> not found.' - the fixed text is 'DBCluster ' ... ' not found.' and the only variable part is the DBClusterIdentifier the request supplied, echoed verbatim (the probe name in the recording is just that request value).
Emulator: same 404 and same code, but the message reads 'DBCluster arn:aws:rds:<region>:<account>:cluster:<identifier> not found.' - it formats a synthesized cluster ARN into the slot where AWS echoes the request's identifier.
Why: a client that parses or displays the failing identifier out of the message gets an ARN for a resource that does not exist; log matching, idempotency retries keyed on the identifier, and user-facing error text all break.
Fix: the DescribeDBClusters handler's not-found path, in the message formatting of the DBClusterNotFoundFault it raises.
Traces: rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster#task#1.

Cause 2 of 9 - a syntactically invalid DBClusterIdentifier is treated as a lookup miss instead of a request refusal.
AWS: Smithy models only DBClusterNotFoundFault (404) on DescribeDBClusters; identifier syntax validation surfaces as the query-protocol service-common InvalidParameterValue (recorded behavior). Recorded response for an identifier containing uppercase letters, an underscore and '!': HTTP 400, Error/Type Sender, Error/Code InvalidParameterValue, Error/Message 'Invalid database cluster identifier:  <value>' - the fixed text is 'Invalid database cluster identifier: ' (the recording carries two spaces after the colon) and the variable part is the rejected identifier taken from the request.
Emulator: HTTP 404 with Error/Code DBClusterNotFoundFault and message 'DBCluster <value> not found.' - it skips identifier validation and goes straight to the state lookup.
Why: a caller cannot tell 'the request is malformed, fix it' from 'the cluster does not exist, maybe create it'. Provisioning code that creates on 404 will try to create a cluster under a name RDS will never accept, and retry loops never terminate.
Fix: DescribeDBClusters input validation, which must run before the cluster state lookup.
Traces: rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster#task#4.

Cause 3 of 9 - DescribeDBClusters unrecognized-filter refusal is re-worded and padded with an invented supported-filter list.
AWS: Smithy: DescribeDBClusters input Filters targets FilterList. An unknown Filters.Name is refused with HTTP 400, Error/Code InvalidParameterValue and Error/Message 'Unrecognized filter name: <name>' - fixed text 'Unrecognized filter name: ' with the rejected Filters.Name from the request as the only variable part, and nothing after it (recorded behavior).
Emulator: HTTP 400 and the same code, but the message is 'unrecognized filter name: <name>; supported filters are clone-group-id, db-cluster-id, db-cluster-resource-id, domain, engine' - the leading word is lower-cased and an enumeration AWS never emits is appended.
Why: message text is a requirement of the error class; clients and test suites that assert on AWS's sentence see a different string, and the appended list is emulator-invented content a caller may come to rely on.
Fix: the shared filter-validation helper used by the DescribeDBClusters handler, in the InvalidParameterValue message it formats.
Traces: rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster#task#3.

Cause 4 of 9 - DescribeDBInstances unrecognized-filter refusal is re-worded the same way.
AWS: HTTP 400, Error/Code InvalidParameterValue, Error/Message 'Unrecognized filter name: <name>' - same fixed text, variable part is the request's Filters.Name (recorded behavior; Smithy: DescribeDBInstances input Filters targets FilterList).
Emulator: 'unrecognized filter name: <name>; supported filters are db-cluster-id, db-instance-id, dbi-resource-id, domain, engine'.
Why: same as cause 3 - the refusal sentence is not AWS's, and it leaks an invented catalogue of filter names.
Fix: the DescribeDBInstances handler's filter validation (separate code path from cause 3 - both must be corrected).
Traces: rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster#task#7.

Cause 5 of 9 - DescribeGlobalClusters unrecognized-filter refusal is re-worded the same way.
AWS: HTTP 400, Error/Code InvalidParameterValue, Error/Message 'Unrecognized filter name: <name>' (recorded behavior; Smithy: DescribeGlobalClusters input Filters targets FilterList, modeled error GlobalClusterNotFoundFault 404 only).
Emulator: 'unrecognized filter name: <name>; the only supported filter is region'.
Why: same as cause 3, and the appended claim is additionally wrong - see cause 6 of the sibling bundle, where db-cluster-id is in fact a supported filter.
Fix: the DescribeGlobalClusters handler's filter validation.
Traces: rds-describe-global-clusters-db-cluster-id-filter#task#3.

Cause 6 of 9 - the Serverless v2 min/max capacity refusal on CreateDBCluster uses the wrong error code and its own sentence.
AWS: Smithy: CreateDBCluster input member ServerlessV2ScalingConfiguration (MinCapacity/MaxCapacity); the capacity-ordering refusal is a query-protocol service-common error. Recorded response when MinCapacity exceeds MaxCapacity: HTTP 400, Error/Type Sender, Error/Code InvalidParameterValue, Error/Message 'Serverless v2 minimum capacity must be less than or equal to maximum capacity' - the whole sentence is fixed text; AWS interpolates neither the capacities nor the member path.
Emulator: HTTP 400 but Error/Code InvalidParameterCombination and Error/Message 'ServerlessV2ScalingConfiguration.MinCapacity (<min>) must not exceed MaxCapacity (<max>)' - a different error class plus a message built from the request values.
Why: SDK error-code dispatch and retry classification key on the code, so a caller branching on InvalidParameterValue falls through to a generic handler; the sentence is also not AWS's.
Fix: CreateDBCluster's ServerlessV2ScalingConfiguration validation, in the error class and message it raises.
Traces: rds-refarch-aur-create-cluster-production-settings-refusals#task#1 (same cluster also covers #task#9).

Cause 7 of 9 - ModifyDBCluster on a cluster that does not exist answers a state fault instead of a not-found fault.
AWS: Smithy: ModifyDBCluster errors include DBClusterNotFoundFault, httpError 404, awsQueryError {code DBClusterNotFoundFault, httpResponseCode 404}, alongside InvalidDBClusterStateFault (400). The existence check precedes the state check, so an absent cluster is refused with HTTP 404, Error/Type Sender, Error/Code DBClusterNotFoundFault, Error/Message 'DBCluster not found' - a fixed sentence with no interpolated identifier for this class (recorded behavior).
Emulator: HTTP 400, Error/Code InvalidDBClusterStateFault, Error/Message 'This cluster could not be modified. Please try again' - the wrong status, the wrong error class, and an emulator-authored sentence that invites a retry.
Why: 400/InvalidDBClusterStateFault reads as transient, so clients retry forever against a cluster that will never appear, instead of taking the permanent 404 branch.
Fix: the ModifyDBCluster handler, in the ordering of its existence check against its state check and in the error it raises when the cluster is absent.
Traces: rds-refarch-aur-create-cluster-production-settings-refusals#task#10.

Cause 8 of 9 - DescribeOrderableDBInstanceOptions invalid-engine refusal echoes the request value instead of AWS's fixed sentence.
AWS: Smithy: DescribeOrderableDBInstanceOptions input Engine is required; the operation models no errors, so the refusal is the query-protocol service-common InvalidParameterValue. Recorded response for an engine name that is not an RDS engine: HTTP 400, Error/Code InvalidParameterValue, Error/Message 'Invalid DB engine' - the entire message is fixed text and the rejected value is deliberately NOT echoed.
Emulator: HTTP 400 with the same code but Error/Message 'Unrecognized engine name: <engine>' - an emulator sentence that additionally reflects the request value back.
Fix: DescribeOrderableDBInstanceOptions engine validation, in the message it formats.
Why: callers asserting on AWS's sentence see a different string; the operation's refusal text is part of its contract.
Traces: rds-refarch-aur-engine-version-and-serverless-class-lookups#task#6.

Cause 9 of 9 - a missing required Engine is refused as a generic constraint violation instead of RDS's missing-parameter refusal.
AWS: Smithy: DescribeOrderableDBInstanceOptions input Engine has required (clientOptional) set. Recorded response when Engine is absent: HTTP 400, Error/Code InvalidParameterValue, Error/Message "Please specify a value for parameter 'engine'." - fixed text "Please specify a value for parameter '" + the member name in RDS's lower-case wire spelling + "'.", terminated by a period.
Emulator: HTTP 400 but Error/Code MalformedInput and Error/Message "1 validation error detected: Value null at 'engine' failed to satisfy constraint: Member must not be null" - a generic smithy-validation sentence under an error code RDS does not use for this class.
Why: the code is what SDKs and retry policies dispatch on; MalformedInput is not in RDS's query-protocol error vocabulary for a missing parameter, so callers that handle InvalidParameterValue see an unmodeled code.
Fix: the required-member validation layer for the rds query protocol, so an absent required member yields InvalidParameterValue with RDS's sentence rather than the generic constraint message.
Traces: rds-refarch-aur-engine-version-and-serverless-class-lookups#task#7.

## Evidence
All nine causes come from the simparity-wire-aws-v1 replay (service=rds, verdict=mismatch, owned_by=[], stub_path="").

Cause 1 - rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster#task#1, DescribeDBClusters, status 404 both sides: $.ErrorResponse.Error.Message value_mismatch - expected 'DBCluster sim2real-probe-aur-absent-fcdc2972 not found.', got 'DBCluster arn:aws:rds:us-west-2:000000000000:cluster:sim2real-probe-aur-absent-fcdc2972 not found.'

Cause 2 - rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster#task#4, DescribeDBClusters: status expected 400, got 404; Error.Code expected 'InvalidParameterValue', got 'DBClusterNotFoundFault'; Error.Message expected 'Invalid database cluster identifier:  Sim2Real_Probe_Bad!', got 'DBCluster Sim2Real_Probe_Bad! not found.'

Cause 3 - rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster#task#3, DescribeDBClusters, status 400 both sides: Error.Message expected 'Unrecognized filter name: cluster-id', got 'unrecognized filter name: cluster-id; supported filters are clone-group-id, db-cluster-id, db-cluster-resource-id, domain, engine'

Cause 4 - rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster#task#7, DescribeDBInstances, status 400 both sides: Error.Message expected 'Unrecognized filter name: db-cluster-identifier', got 'unrecognized filter name: db-cluster-identifier; supported filters are db-cluster-id, db-instance-id, dbi-resource-id, domain, engine'

Cause 5 - rds-describe-global-clusters-db-cluster-id-filter#task#3, DescribeGlobalClusters, status 400 both sides: Error.Message expected 'Unrecognized filter name: db-cluster-identifier', got 'unrecognized filter name: db-cluster-identifier; the only supported filter is region'

Cause 6 - rds-refarch-aur-create-cluster-production-settings-refusals#task#1 (cluster also covers #task#9), CreateDBCluster, status 400 both sides: Error.Code expected 'InvalidParameterValue', got 'InvalidParameterCombination'; Error.Message expected 'Serverless v2 minimum capacity must be less than or equal to maximum capacity', got 'ServerlessV2ScalingConfiguration.MinCapacity (4) must not exceed MaxCapacity (1)'

Cause 7 - rds-refarch-aur-create-cluster-production-settings-refusals#task#10, ModifyDBCluster: status expected 404, got 400; Error.Code expected 'DBClusterNotFoundFault', got 'InvalidDBClusterStateFault'; Error.Message expected 'DBCluster not found', got 'This cluster could not be modified. Please try again'

Cause 8 - rds-refarch-aur-engine-version-and-serverless-class-lookups#task#6, DescribeOrderableDBInstanceOptions, status 400 both sides: Error.Message expected 'Invalid DB engine', got 'Unrecognized engine name: aurora-postgres'

Cause 9 - rds-refarch-aur-engine-version-and-serverless-class-lookups#task#7, DescribeOrderableDBInstanceOptions, status 400 both sides: Error.Code expected 'InvalidParameterValue', got 'MalformedInput'; Error.Message expected "Please specify a value for parameter 'engine'.", got "1 validation error detected: Value null at 'engine' failed to satisfy constraint: Member must not be null"

Smithy grounding used (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): DescribeDBClusters errors=[DBClusterNotFoundFault httpError 404, awsQueryError DBClusterNotFoundFault/404]; ModifyDBCluster errors include DBClusterNotFoundFault (404) and InvalidDBClusterStateFault (400); DescribeOrderableDBInstanceOptions input Engine required=true, errors=[]; DescribeGlobalClusters input Filters->FilterList, errors=[GlobalClusterNotFoundFault 404]; CreateDBCluster input ServerlessV2ScalingConfiguration.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) with no arguments.
3. Inspect these steps, each of which is reported as a mismatch cluster with service=rds and owned_by=[]:
   - rds-refarch-aur-cluster-lookup-routes-on-an-absent-cluster, task#1 (DescribeDBClusters on an absent cluster), task#3 (DescribeDBClusters with Filters.Name=cluster-id), task#4 (DescribeDBClusters with DBClusterIdentifier=Sim2Real_Probe_Bad!), task#7 (DescribeDBInstances with Filters.Name=db-cluster-identifier)
   - rds-describe-global-clusters-db-cluster-id-filter, task#3 (DescribeGlobalClusters with Filters.Name=db-cluster-identifier)
   - rds-refarch-aur-create-cluster-production-settings-refusals, task#1 (CreateDBCluster with ServerlessV2ScalingConfiguration MinCapacity > MaxCapacity), task#10 (ModifyDBCluster against a cluster that was never created)
   - rds-refarch-aur-engine-version-and-serverless-class-lookups, task#6 (DescribeOrderableDBInstanceOptions with Engine=aurora-postgres), task#7 (DescribeOrderableDBInstanceOptions with Engine omitted)
4. For each, compare the HTTP status, $.ErrorResponse.Error.Code and $.ErrorResponse.Error.Message against the recorded AWS response quoted in the evidence.
