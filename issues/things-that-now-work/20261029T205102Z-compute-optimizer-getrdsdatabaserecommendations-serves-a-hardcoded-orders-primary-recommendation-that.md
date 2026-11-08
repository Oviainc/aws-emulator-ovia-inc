Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-rds-recommendations-and-projected-metrics
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-rds-recommendations-and-projected-metrics
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer|GetRDSDatabaseRecommendations|2xx->2xx|$.rdsDBRecommendations[*]
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# GetRDSDatabaseRecommendations serves a hardcoded orders-primary recommendation that is not in the account

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRDSDatabaseRecommendations

## Description
ONE cause. An unfiltered GetRDSDatabaseRecommendations read returns a fully-populated RDS recommendation the account never asked about and does not own, served from an inventory baked into the crate rather than derived from the account's state.

=== Cause 1 of 1: an unfiltered recommendation read returns a crate-resident RDS DB recommendation ===
AWS: Smithy (services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/smithy-model/com-amazonaws-computeoptimizer-computeoptimizerservice.json): GetRDSDatabaseRecommendations input {resourceArns: ResourceArns, nextToken, maxResults (@range 0..1000), filters: RDSDBRecommendationFilters, accountIds, recommendationPreferences}; output {nextToken, rdsDBRecommendations: RDSDBRecommendations, errors: GetRecommendationErrors}. Both output lists are optional, so an empty body is a well-formed success. Compute Optimizer only emits an RDSDBRecommendation after it has accumulated enough CloudWatch metric history for a DB instance the enrolled account actually owns; a freshly enrolled account with no analyzed RDS inventory therefore has nothing to return. Recorded behavior: the read is answered HTTP 200 with `rdsDBRecommendations` EMPTY (no element at index 0) and no `errors` entry.
Emulator: HTTP 200 (the status matches) but the body carries an extra `rdsDBRecommendations[0]`: a complete recommendation object for an RDS DB instance ARN that appears nowhere in the request or in any prior step of the fixture — a `postgres` instance of class `db.r6g.2xlarge`, instanceFinding `Overprovisioned`, one instanceRecommendationOptions entry with a savingsOpportunity, utilizationMetrics and effectiveRecommendationPreferences. Every one of those values is fixed in the crate, including the resource ARN itself and its region token, which is a DIFFERENT region from the one the fixture's other RDS steps address. The emulator is not analysing account state here: it is replaying a canned finding.
Why: a client cannot distinguish "Compute Optimizer has produced a finding for this DB instance" from "the emulator has a demo row". Anything that consumes the recommendation list acts on a resource that does not exist in the account — a rightsizing workflow issues a ModifyDBInstance against a phantom DB identifier, a savings report attributes estimated monthly savings to an instance nobody is paying for, and a test that asserts "a clean account has no RDS recommendations" fails. The inverse is also broken: a caller that does own an analysable DB instance has no way to tell its finding apart from the canned one.
Fix: the compute-optimizer crate's GetRDSDatabaseRecommendations handler and the RDS findings catalog it reads from in the service state. The reader must project the account's own recorded/registered RDS inventory (and only instances with accumulated analysis) into `rdsDBRecommendations`, returning an empty list when the account has none, instead of emitting a catalog entry that is resident in the crate independently of any account state. The same shape of defect is already filed for two sibling readers — 20261024T082711Z-compute-optimizer-getebsvolumerecommendations-wrong-behavior.md and 20261024T082711Z-compute-optimizer-getlambdafunctionrecommendations-wrong-behavior.md ("analyse the account's real inventory instead of a hardcoded in-crate inventory") — but neither covers the RDS reader, and the open wire issue 20261029T183426Z-compute-optimizer-resources-with-no-recommendation-are-reported-as-absent.md covers the OPPOSITE state on this same operation (an explicitly-named ARN with no finding, fixture step task#4), not this unsolicited extra element at task#3.
Traces: compute-optimizer-rds-recommendations-and-projected-metrics#task#3

## Evidence
Replayed against a running emulator (emulator_up=true, bootstrap_signal=false, 52/70 steps passed, score 74.3%), so the service answers and this is a fidelity divergence rather than a bootstrap gap.

Divergence cluster service=compute-optimizer|GetRDSDatabaseRecommendations|status=200->200|keys=$.rdsDBRecommendations[0]
  fixture compute-optimizer-rds-recommendations-and-projected-metrics step task#3
  expected_status 200, actual_status 200 (status matches; the body diverges)
  $.rdsDBRecommendations[0]: extra
    expected: None (the member has no element at index 0 — the list is empty)
    actual:   {'resourceArn': 'arn:aws:rds:us-west-2:000000000000:db:orders-primary', 'accountId': '000000000000', 'engine': 'postgres', 'engineVersion': '15.4', 'currentDBInstanceClass': 'db.r6g.2xlarge', 'currentStorageConfiguration': {'storageType': 'gp3', 'allocatedStorage': 500, 'iops': 3000, 'maxAllocatedStorage': 1000, 'storageThroughput': 125}, 'idle': 'False', 'instanceFinding': 'Overprovisioned', 'storageFinding': 'Optimized', 'currentInstancePerformanceRisk': 'Medium', 'instanceRecommendationOptions': [{'dbInstanceClass': 'db.r6g.xlarge', 'projectedUtilizationMetrics': [{'name': 'CPU', 'statistic': 'Maximum', 'value': 35.0}, {'name': 'Memory', 'statistic': 'Maximum', 'value': 48.0}, {'name': 'DatabaseConnections', 'statistic': 'Maximum', 'value': 12.0}], 'performanceRisk': 0.92, 'rank': 1, 'savingsOpportunity': {'savingsOpportunityPercentage': 25.0, 'estimatedMonthlySavings': {'currency': 'USD', 'value': 75.0}}}], 'utilizationMetrics': [{'name': 'CPU', 'statistic': 'Maximum', 'value': 17.5}, {'name': 'Memory', 'statistic': 'Maximum', 'value': 24.0}, {'name': 'DatabaseConnections', 'statistic': 'Maximum', 'value': 12.0}], 'effectiveRecommendationPreferences': {'lookBackPeriod': 'DAYS_14', 'savingsEstimationMode': {'source': 'PublicPricing'}}, 'lookbackPeriodInDays': 14.0}

The requirement is structural, not value-wise: the list must contain an element only for a DB instance the account holds an analysis for. The particular db identifier, engine version, instance classes, metric values, performance risks and savings figures above are the crate's fixed row and are quoted only to show it is a canned, environment-independent payload (note the us-west-2 ARN, a region the rest of the fixture does not use, and the all-zero account id, which is the emulator's own placeholder rather than a value derived from the caller).

Model evidence (smithy_query, query=operation on com-amazonaws-computeoptimizer-computeoptimizerservice.json): GetRDSDatabaseRecommendationsResponse declares only the optional members {nextToken, rdsDBRecommendations: RDSDBRecommendations, errors: GetRecommendationErrors}; there is no trait or default making any recommendation element mandatory, so an empty rdsDBRecommendations is the modeled answer for an account with nothing analyzed.

## Steps to Reproduce
1. Boot the emulator and opt the account in: UpdateEnrollmentStatus(status='Active'), confirm with GetEnrollmentStatus.
2. Do NOT create or register any RDS DB instance for the account.
3. Call GetRDSDatabaseRecommendations with no resourceArns and no filters (an account-wide read), as fixture compute-optimizer-rds-recommendations-and-projected-metrics does at step task#3.
4. Observe HTTP 200 whose body carries one rdsDBRecommendations element for an RDS DB ARN that was never created in this environment (in the recorded run, a db.r6g.2xlarge postgres instance in us-west-2 with instanceFinding 'Overprovisioned' and a 25% savingsOpportunity). AWS answers HTTP 200 with rdsDBRecommendations empty and no errors entry.
5. Replay to confirm a fix: fixture compute-optimizer-rds-recommendations-and-projected-metrics, step task#3.
