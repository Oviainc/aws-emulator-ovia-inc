Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanrooms.protected-query-list-rule-round-trip
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms.protected-query-list-rule-round-trip|run#45|AssertionError|assert final["status"] == "SUCCESS", f"query ended in {final['status']}: {final.get('error')}"
Issue-Commit: e28dc1337060a874b1991c751dde47e1a17d0887

# StartProtectedQuery never executes: the query goes to FAILED with a non-AWS error code QUERY_EXECUTION_UNAVAILABLE instead of SUCCESS, so no result is written and no statistics are produced

> _Note: this issue was identified against the code base at commit `e28dc1337060a874b1991c751dde47e1a17d0887`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartProtectedQuery

## Description
One net-new root cause in the cleanrooms service.

Cause 1 of 1: a protected query submitted against a Glue-backed configured table with a LIST analysis rule is accepted, but the emulator immediately drives it to a terminal FAILED state with an emulator-invented error instead of executing it. Failing check: cleanrooms.protected-query-list-rule-round-trip.

AWS: StartProtectedQuery accepts an SQL protected query for a membership and returns a ProtectedQuery whose `status` begins at SUBMITTED. Smithy: ProtectedQueryStatus enum values are SUBMITTED, STARTED, CANCELLED, CANCELLING, FAILED, SUCCESS, TIMED_OUT; ProtectedQuery.status is @required; ProtectedQuery.statistics targets ProtectedQueryStatistics with member totalDurationInMillis ("The duration of the protected query, from creation until query completion, in milliseconds"); ProtectedQuery.result targets ProtectedQueryResult whose `output` is @required; ProtectedQuery.error targets ProtectedQueryError with @required `message` and @required `code`. On AWS a well-formed LIST-rule query that satisfies the rule's constraints (join to a collaborator table present, SELECT DISTINCT present, no ORDER BY) and whose membership carries a defaultResultConfiguration advances SUBMITTED -> STARTED -> SUCCESS, populates statistics.totalDurationInMillis with a positive value, and writes the result rows as a CSV object under the membership's configured S3 output bucket/keyPrefix. A FAILED terminus on AWS carries an error whose `code` is one of AWS's own protected-query failure codes (e.g. the analysis-rule violation classes the check's comments quote, such as a missing join to a collaborator table or a missing DISTINCT quantifier) — it is never emitted for a query that violates no rule.

Emulator: the query reaches terminal status FAILED with error {'code': 'QUERY_EXECUTION_UNAVAILABLE', 'message': 'this emulator serves the Clean Rooms control plane only: it has no analytics data plane, so the protected query was accepted and recorded but never executed'}. Neither that code nor that message exists in AWS's vocabulary — the fixed text is wholly emulator-authored and names the emulator itself, and the code QUERY_EXECUTION_UNAVAILABLE is not an AWS protected-query failure code. Because the query never runs, `statistics.totalDurationInMillis` is not a positive completion duration and no CSV result object is written under the membership's result-configuration prefix, so the subsequent S3 read finds nothing.

Why: a client that submits a protected query and polls GetProtectedQuery to a terminal state is the entire point of the Clean Rooms data plane. Against the emulator every such client sees an unconditional failure for a perfectly valid query, with an error code it cannot map to any documented AWS failure class, so retry/branch logic keyed on AWS codes falls through to its unknown-error path. Any client that then reads the collaboration's results from S3 gets an empty prefix rather than the joined rows, and any client that reports query duration reads no completion statistic. Analysis-rule enforcement is also unobservable: a query that genuinely violates a LIST rule and one that is perfectly valid are indistinguishable, both FAILED with the same emulator code.

Fix: the StartProtectedQuery / GetProtectedQuery state machine and the protected-query execution state behind it (services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/start_protected_query.rs, get_protected_query.rs, and the protected-query state module). The query must advance through SUBMITTED/STARTED to SUCCESS for a query that satisfies its configured-table analysis rules, resolve the Glue table references to their backing S3 locations, project/join only the columns the LIST analysis rule permits, write the result as a CSV object under the membership's defaultResultConfiguration outputConfiguration.s3 bucket + keyPrefix with resultFormat honoured, and populate statistics.totalDurationInMillis with a positive elapsed duration. A FAILED terminus must be reserved for real rule or SQL violations and must carry an AWS-worded ProtectedQueryError code and message for that violation class, never an emulator-capability notice. The variable parts of a correct failure message are the offending column/table names and rule type taken from the request and the configured analysis rule; the fixed text is AWS's wording for that class.

Traces: cleanrooms.protected-query-list-rule-round-trip — asserts final['status'] == 'SUCCESS', then statistics.totalDurationInMillis > 0, then that the joined rows 1,10,A / 2,20,B / 3,30,C appear in the CSV written under s3://<results bucket>/results/.

No open cleanrooms issue covers this: the existing open cleanrooms issues concern tagging status codes, UpdateConfiguredTable analysis-rule refusal wording, GetConfiguredTableAnalysisRule not-found wording, DeleteAnalysisTemplate and CreateAnalysisTemplate message wording, the DeleteConfiguredTable stub, and the configured-table-association analysis-rule round trip — all control-plane; none of them makes a protected query execute.

## Evidence
Check cleanrooms.protected-query-list-rule-round-trip, assertion at line 217 of check_cleanrooms_protected_query_list_rule_round_trip.py:

  assert final["status"] == "SUCCESS", f"query ended in {final['status']}: {final.get('error')}"
  AssertionError: query ended in FAILED: {'message': 'this emulator serves the Clean Rooms control plane only: it has no analytics data plane, so the protected query was accepted and recorded but never executed', 'code': 'QUERY_EXECUTION_UNAVAILABLE'}

Expected (AWS):
  GetProtectedQuery().protectedQuery.status == "SUCCESS" (reached via SUBMITTED -> STARTED)
  .statistics.totalDurationInMillis > 0
  a CSV object under s3://<results-bucket>/results/ containing the joined rows
  (for this check's inputs: 1,10,A / 2,20,B / 3,30,C)
  no `error` member on a successful query

Actual (emulator):
  status == "FAILED" on first terminal poll
  error.code == "QUERY_EXECUTION_UNAVAILABLE"  <- not an AWS ProtectedQueryError code
  error.message == "this emulator serves the Clean Rooms control plane only: it has no
                    analytics data plane, so the protected query was accepted and
                    recorded but never executed"  <- emulator-authored text, names the emulator
  no result object written under the membership's configured result prefix
  no positive completion duration in statistics

Model citations (cleanrooms Smithy):
  ProtectedQueryStatus = [SUBMITTED, STARTED, CANCELLED, CANCELLING, FAILED, SUCCESS, TIMED_OUT]
  ProtectedQuery.status @required; ProtectedQuery.statistics -> ProtectedQueryStatistics
  ProtectedQueryStatistics.totalDurationInMillis: "The duration of the protected query,
    from creation until query completion, in milliseconds"
  ProtectedQuery.result -> ProtectedQueryResult { output @required }
  ProtectedQuery.error -> ProtectedQueryError { message @required, code @required }

## Steps to Reproduce
1. Create two S3 buckets (one for source data, one for results — Clean Rooms refuses a result location in the same bucket as the queried data). Put a CSV of rows (id,value,joinkey) under data/ in the data bucket and a CSV of labels (joinkey,label) under labels/.
2. Create a Glue database and two EXTERNAL_TABLE CSV tables pointing at s3://<data-bucket>/data/ and s3://<data-bucket>/labels/ (classification=csv, skip.header.line.count=1).
3. Create an IAM role trusted by cleanrooms.amazonaws.com with glue:Get* on the database and s3 read on the data bucket / s3 write on the results bucket.
4. cleanrooms CreateCollaboration (creatorMemberAbilities CAN_QUERY, CAN_RECEIVE_RESULTS), then CreateMembership with defaultResultConfiguration.outputConfiguration.s3 = {bucket: <results-bucket>, keyPrefix: "results/", resultFormat: "CSV"} and roleArn = the role above.
5. For each Glue table: CreateConfiguredTable (analysisMethod DIRECT_QUERY), CreateConfiguredTableAnalysisRule with analysisRuleType LIST and policy v1.list {joinColumns: [joinkey], listColumns: [...]} (join column disjoint from list columns), CreateConfiguredTableAssociation, then CreateConfiguredTableAssociationAnalysisRule LIST with allowedResultReceivers = [caller account id].
6. StartProtectedQuery(type="SQL", membershipIdentifier=<membership>, sqlParameters.queryString="SELECT DISTINCT rows_assoc.id, rows_assoc.value, labels_assoc.label FROM rows_assoc JOIN labels_assoc ON rows_assoc.joinkey = labels_assoc.joinkey").
7. Poll GetProtectedQuery until status is terminal (SUCCESS/FAILED/CANCELLED/TIMED_OUT).

Observed: the first terminal read is FAILED with error.code "QUERY_EXECUTION_UNAVAILABLE" and the emulator-capability message above; nothing is written under s3://<results-bucket>/results/.
Expected: status SUCCESS, statistics.totalDurationInMillis > 0, and a CSV result object under that prefix containing the joined rows.
