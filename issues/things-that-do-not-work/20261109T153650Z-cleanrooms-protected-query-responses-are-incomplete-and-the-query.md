Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-protected-query-returns-joined-row
Issue-Step: task#20
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-protected-query-returns-joined-row
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|StartProtectedQuery|2xx->2xx|$.protectedQuery.computeConfiguration
Issue-Key: cleanrooms|StartProtectedQuery|2xx->2xx|$.protectedQuery.createTime
Issue-Key: cleanrooms|StartProtectedQuery|2xx->2xx|$.protectedQuery.differentialPrivacy
Issue-Key: cleanrooms|StartProtectedQuery|2xx->2xx|$.protectedQuery.resultConfiguration.outputConfiguration.s3.keyPrefix
Issue-Key: cleanrooms|GetProtectedQuery|2xx->2xx|$.protectedQuery.computeConfiguration
Issue-Key: cleanrooms|GetProtectedQuery|2xx->2xx|$.protectedQuery.createTime
Issue-Key: cleanrooms|GetProtectedQuery|2xx->2xx|$.protectedQuery.result
Issue-Key: cleanrooms|GetProtectedQuery|2xx->2xx|$.protectedQuery.resultConfiguration.outputConfiguration.s3.keyPrefix
Issue-Key: cleanrooms|GetProtectedQuery|2xx->2xx|$.protectedQuery.statistics
Issue-Key: cleanrooms|GetProtectedQuery|2xx->2xx|$.protectedQuery.status
Issue-Commit: c204ffd13545bba78014bc80d4ce821b8ce22cdd

# Protected-query responses are incomplete and the query never succeeds: StartProtectedQuery/GetProtectedQuery drop computeConfiguration, differentialPrivacy and the per-call result keyPrefix, and the query settles FAILED with no result or statistics

> _Note: this issue was identified against the code base at commit `c204ffd13545bba78014bc80d4ce821b8ce22cdd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartProtectedQuery

## Description
Two net-new root causes in the cleanrooms service, both on the ProtectedQuery projection served by the protected-query pair. Both are wire divergences replayed from fixture cleanrooms-protected-query-returns-joined-row; neither carries an owning open issue in the replay report.

Cause 1 of 2: StartProtectedQuery returns a ProtectedQuery that is missing computeConfiguration and differentialPrivacy and that reports the membership's default result keyPrefix instead of the result configuration in effect for the call. Fixture cleanrooms-protected-query-returns-joined-row, step task#20 (HTTP 200 both sides, body members diverge).

AWS: StartProtectedQuery (POST /memberships/{membershipIdentifier}/protectedQueries, success 200) returns @required protectedQuery: ProtectedQuery. Smithy: ProtectedQuery.computeConfiguration targets the ComputeConfiguration union ('The compute configuration for the protected query'), whose worker member is WorkerComputeConfiguration with type: WorkerComputeType @default 'CR.1X' (addedDefault, clientOptional), number: Integer @range 2..1024 @default 16 (addedDefault, clientOptional) and properties: WorkerComputeConfigurationProperties; StartProtectedQueryInput also accepts computeConfiguration. Because the member defaults are modelled, the compute configuration in effect for a query is always determinate, and AWS reports it back on the ProtectedQuery: the recording shows worker {type: 'CR.1X', number: 16, properties.spark['spark.cleanrooms.athena.optimizationsEnabled']: 'true'} — i.e. the request's properties with the modelled type/number defaults applied. ProtectedQuery.differentialPrivacy targets DifferentialPrivacyParameters whose sensitivityParameters list is @required; the recording shows it present with an empty sensitivityParameters list for a query with no differential-privacy rule, so the member is emitted (with an empty list) rather than omitted. ProtectedQuery.resultConfiguration targets ProtectedQueryResultConfiguration ('Contains any details needed to write the query results'), whose outputConfiguration.s3 is ProtectedQueryS3OutputConfiguration with @required resultFormat, @required bucket and keyPrefix (KeyPrefix, @length 0..512, pattern ^[\w!.=*/-]*$). StartProtectedQueryInput.resultConfiguration ('The details needed to write the query results') is the per-call override of the membership's defaultResultConfiguration, so the echoed keyPrefix is the one in effect for THIS query — the call's resultConfiguration when the request supplies one, the membership default only when it does not. Recorded behavior: the response's keyPrefix carries a collaboration-partition segment ('results/collaboration=<collaborationId>'), not the bare membership-default prefix.

Emulator: the response omits $.protectedQuery.computeConfiguration entirely (got None) and omits $.protectedQuery.differentialPrivacy entirely (got None), and $.protectedQuery.resultConfiguration.outputConfiguration.s3.keyPrefix is the membership default's bare prefix ('results/') rather than the prefix in effect for the query ('results/collaboration=<collaborationId>' in the recording). In the handler the ProtectedQuery is built with differential_privacy hard-coded to None, compute_configuration from a helper that yields nothing when the request omits/partially fills the union instead of applying the modelled CR.1X/16 defaults, and result_configuration resolved through a membership-default fallback that does not preserve the call's own resultConfiguration. Hypothesis: the same helper collapses a supplied per-call resultConfiguration onto the stored membership default, which is why the collaboration-partition prefix disappears.

Why: a client that submits a protected query and reads the returned ProtectedQuery to learn where results will land computes the wrong S3 key and then reads an empty prefix — the prefix it was told ('results/') is not the prefix the service used. A client that echoes or re-submits the returned computeConfiguration (for example to re-run a query with the same worker sizing, or to display/bill worker count) sees an absent union where AWS always reports the effective worker type and count, and a client reading differentialPrivacy.sensitivityParameters dereferences an absent member instead of an empty list, which is a null-pointer path in generated SDKs that treat the member as present-with-required-list.

Fix: the StartProtectedQuery handler and the ProtectedQuery projection it shares with GetProtectedQuery — the compute-configuration defaulting helper must materialize the modelled WorkerComputeConfiguration defaults (type CR.1X, number 16) and carry through the request's worker properties; the result-configuration resolution must prefer the call's own resultConfiguration over the membership default and preserve its keyPrefix verbatim; and differentialPrivacy must be emitted with an empty sensitivityParameters list rather than omitted. Belongs in the operation's handler/state projection, not the serializer.

Traces: cleanrooms-protected-query-returns-joined-row#task#20 — $.protectedQuery.computeConfiguration missing, $.protectedQuery.differentialPrivacy missing, $.protectedQuery.resultConfiguration.outputConfiguration.s3.keyPrefix 'results/' instead of the collaboration-partitioned prefix.

Cause 2 of 2: GetProtectedQuery polls to terminal status FAILED instead of SUCCESS, so result and statistics are never populated and the same computeConfiguration/differentialPrivacy/keyPrefix omissions persist on the read path. Fixture cleanrooms-protected-query-returns-joined-row, step task#21 (HTTP 200 both sides; the poll aborts because FAILED is a fail_on terminus, after 2 attempts).

AWS: GetProtectedQuery (GET /memberships/{membershipIdentifier}/protectedQueries/{protectedQueryIdentifier}, readonly, success 200) returns @required protectedQuery. Smithy: ProtectedQueryStatus values are SUBMITTED, STARTED, CANCELLED, CANCELLING, FAILED, SUCCESS, TIMED_OUT, and ProtectedQuery.status is @required; ProtectedQuery.result targets ProtectedQueryResult whose output is @required (ProtectedQueryOutput union with s3: ProtectedQueryS3Output { location @required } — 'The S3 location of the result'); ProtectedQuery.statistics targets ProtectedQueryStatistics.totalDurationInMillis ('The duration of the protected query, from creation until query completion, in milliseconds'); ProtectedQuery.error targets ProtectedQueryError and is reserved for a query that actually failed. On AWS a well-formed SQL protected query that satisfies its configured tables' analysis rules advances SUBMITTED -> STARTED -> SUCCESS, and at SUCCESS the ProtectedQuery carries result.output.s3.location (the s3:// URI of the written result object under the effective bucket + keyPrefix), statistics.totalDurationInMillis, the effective computeConfiguration and differentialPrivacy. FAILED is only a terminus for a real rule or SQL violation.

Emulator: the first/second terminal poll reports $.protectedQuery.status 'FAILED' where the recording has 'SUCCESS', so the harness' poll aborts on a fail_on status. Consequently $.protectedQuery.result is absent (AWS: output.s3.location pointing at the written result object) and $.protectedQuery.statistics is absent (AWS: totalDurationInMillis). The handler's own doc states the query is never executed and the clock drives the accepted query to a modelled failure state (crate::state::PROTECTED_QUERY_FAILURE_CODE) after a settle delay. The same projection defects as Cause 1 are visible here too: computeConfiguration missing, differentialPrivacy missing, keyPrefix 'results/' instead of the collaboration-partitioned prefix. The downstream S3 read in this fixture then finds an empty prefix (that cluster, s3 ListObjectsV2 at task#22, is already owned by 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md and is not filed here).

Why: polling to a terminal state is the whole client contract of the Clean Rooms data plane. Against the emulator every valid query terminates FAILED, so a client's success branch is never taken: it never receives result.output.s3.location, never reports a completion duration, and cannot distinguish a query that genuinely violates a LIST analysis rule from one that is perfectly valid. Retry/branch logic keyed on AWS terminal statuses falls through to its failure path for queries AWS would have succeeded.

Fix: the GetProtectedQuery handler and the protected-query execution state behind it (the settle/terminal-status machinery in the service's protected-query state module). A query that satisfies its configured-table analysis rules must advance SUBMITTED/STARTED -> SUCCESS, populate statistics.totalDurationInMillis with a positive elapsed duration and result.output.s3.location with the s3:// URI of the object written under the effective result bucket + keyPrefix, and reserve the FAILED terminus (with an AWS-worded ProtectedQueryError code/message for the violated rule class, its variable parts being the offending table/column names and rule type taken from the request and the configured analysis rule) for real violations.

Traces: cleanrooms-protected-query-returns-joined-row#task#21 — $.protectedQuery.status 'FAILED' vs 'SUCCESS' (poll fail_on after 2 attempts), $.protectedQuery.result missing, $.protectedQuery.statistics missing, plus the Cause-1 projection members.

Not covered by any open issue: 20261108T183934Z-cleanrooms-startprotectedquery-never-executes-the-query-goes-to-failed.md describes the never-executed query from the BEHAVIORAL corpus (check cleanrooms.protected-query-list-rule-round-trip) and carries no wire replay key, so this fixture's steps are not gated by it — the replay report leaves owned_by empty for both clusters. The other open cleanrooms issues concern tagging status codes (20261029T175308Z), GetConfiguredTableAnalysisRule not-found wording (20261031T001457Z), the configured-table-association analysis-rule round trip (20261108T171055Z) and the UpdateProtectedQuery stub — none of them covers the ProtectedQuery projection members or the SUCCESS terminus.

Excluded as not emulator defects: $.protectedQuery.createTime differs only as a per-run clock value (the fixture does not assert it: expected '<shaped-not-asserted>'), so it is not part of either cause; likewise the collaboration/membership/query UUIDs, bucket names and timestamps in the recording fill the templates above and are not requirements.

## Evidence
Fixture cleanrooms-protected-query-returns-joined-row (trace set simparity-wire-aws-v1).

Cause 1 — step task#20, StartProtectedQuery, status 200 expected and 200 received; body diverges:
  $.protectedQuery.computeConfiguration: missing
    expected {'worker': {'number': 16, 'properties': {'spark': {'spark.cleanrooms.athena.optimizationsEnabled': 'true'}}, 'type': 'CR.1X'}}
    actual   None
  $.protectedQuery.differentialPrivacy: missing
    expected {'sensitivityParameters': []}
    actual   None
  $.protectedQuery.resultConfiguration.outputConfiguration.s3.keyPrefix: value_mismatch
    expected 'results/collaboration=<collaborationId>'   (recording: 'results/collaboration=bdd5730f-3357-470e-8468-0576b4de3f17')
    actual   'results/'                                  (the membership default's bare prefix)
  (also reported: $.protectedQuery.createTime — a per-run clock value the fixture does not assert; NOT claimed as a defect)

Cause 2 — step task#21, GetProtectedQuery, status 200 expected and 200 received; body diverges:
  $.protectedQuery.status: value_mismatch — expected 'SUCCESS', actual 'FAILED'
    and: poll: 'FAILED' is in fail_on after 2 attempt(s)
  $.protectedQuery.result: missing
    expected {'output': {'s3': {'location': 's3://<results-bucket>/<keyPrefix>/<queryId>/'}}}   (recording: 's3://sim2real-probe-cr-res-3f8dd27c/results/collaboration=bdd5730f-3357-470e-8468-0576b4de3f17/b0ee20f1-557d-4016-b233-53af7f7fca43/')
    actual   None
  $.protectedQuery.statistics: missing — expected {'totalDurationInMillis': <positive duration>}, actual None
  $.protectedQuery.computeConfiguration: missing (as Cause 1)
  $.protectedQuery.differentialPrivacy: missing (as Cause 1)
  $.protectedQuery.resultConfiguration.outputConfiguration.s3.keyPrefix: 'results/' instead of the collaboration-partitioned prefix

Model citations (cleanrooms Smithy, services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/smithy-model/com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda.json):
  StartProtectedQuery: POST /memberships/{membershipIdentifier}/protectedQueries, 200; input accepts resultConfiguration + computeConfiguration; output protectedQuery @required
  GetProtectedQuery: GET /memberships/{membershipIdentifier}/protectedQueries/{protectedQueryIdentifier}, 200, readonly; output protectedQuery @required
  ProtectedQueryStatus = [SUBMITTED, STARTED, CANCELLED, CANCELLING, FAILED, SUCCESS, TIMED_OUT]; ProtectedQuery.status @required
  ComputeConfiguration = union{worker: WorkerComputeConfiguration}; WorkerComputeConfiguration.type @default 'CR.1X', .number @range 2..1024 @default 16, .properties -> WorkerComputeConfigurationProperties
  DifferentialPrivacyParameters.sensitivityParameters @required (list)
  ProtectedQueryResult.output @required; ProtectedQueryOutput union{s3: ProtectedQueryS3Output{location @required}, memberList, distribute}
  ProtectedQueryStatistics.totalDurationInMillis: 'The duration of the protected query, from creation until query completion, in milliseconds'
  ProtectedQueryS3OutputConfiguration.keyPrefix: KeyPrefix @length 0..512 pattern ^[\w!.=*/-]*$ ('The S3 prefix to unload the protected query results')

Code pointers (for the fixer, not requirements): services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/start_protected_query.rs lines 329-354 build the ProtectedQuery with differential_privacy: None, compute_configuration: st::effective_compute_configuration(input.compute_configuration), result_configuration: st::protected_query_result_configuration(input.result_configuration, &membership), statistics: None, result: None, and lines 356-362 record terminal_due = started_at + st::PROTECTED_QUERY_SETTLE_DELAY driving the query to st::PROTECTED_QUERY_FAILURE_CODE (module doc, lines 15-20).

## Steps to Reproduce
1. Replay the bound wire corpus (trace set simparity-wire-aws-v1) against the emulator built from this checkout: fixture cleanrooms-protected-query-returns-joined-row.
2. The fixture stands up the collaboration/membership, the Glue-backed configured tables with LIST analysis rules and associations, and a separate results bucket, then:
   a. StartProtectedQuery(type='SQL', membershipIdentifier=<membership>, sqlParameters.queryString=<DISTINCT join over the two associated tables>, resultConfiguration.outputConfiguration.s3 = {bucket: <results-bucket>, keyPrefix: 'results/collaboration=<collaborationId>', resultFormat: 'CSV'}, computeConfiguration.worker.properties.spark = {'spark.cleanrooms.athena.optimizationsEnabled': 'true'}) — step task#20.
   b. Poll GetProtectedQuery(membershipIdentifier=<membership>, protectedQueryIdentifier=<id from step a>) until a terminal status — step task#21.
3. Observe at step task#20: 200, but the returned protectedQuery has no computeConfiguration and no differentialPrivacy, and resultConfiguration.outputConfiguration.s3.keyPrefix is 'results/' (the membership default) rather than the keyPrefix supplied on the call.
   Expected: computeConfiguration.worker = {type: 'CR.1X', number: 16, properties.spark: {'spark.cleanrooms.athena.optimizationsEnabled': 'true'}}; differentialPrivacy = {sensitivityParameters: []}; keyPrefix echoed verbatim from the call's resultConfiguration.
4. Observe at step task#21: the second poll returns status 'FAILED' (a fail_on terminus) with no result and no statistics.
   Expected: status advances SUBMITTED/STARTED -> 'SUCCESS', result.output.s3.location is the s3:// URI under <results-bucket>/<keyPrefix>/<queryId>/, statistics.totalDurationInMillis is a positive duration, and the computeConfiguration/differentialPrivacy/keyPrefix members above are present and correct on the read path too.
