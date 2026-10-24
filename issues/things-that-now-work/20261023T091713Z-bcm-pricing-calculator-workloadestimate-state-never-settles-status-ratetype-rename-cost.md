Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-pricing-calculator-list-workload-estimates-pagination-includes-all-created
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-pricing-calculator-list-workload-estimates-pagination-includes-all-created bcm-pricing-calculator-workload-estimate-tagging bcm-pricing-calculator-workload-estimate-usage-lifecycle
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator|ListWorkloadEstimates|2xx->2xx|$.items[*].status
Issue-Key: bcm-pricing-calculator|ListWorkloadEstimates|2xx->2xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|ListWorkloadEstimates|2xx->2xx|$.nextToken
Issue-Key: bcm-pricing-calculator|CreateWorkloadEstimate|2xx->2xx|$.rateType
Issue-Key: bcm-pricing-calculator|CreateWorkloadEstimate|2xx->2xx|$.status
Issue-Key: bcm-pricing-calculator|CreateWorkloadEstimate|2xx->2xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|2xx->2xx|$.items[*].cost
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|2xx->2xx|$.items[*].currency
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|2xx->2xx|$.items[*].location
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|2xx->2xx|$.items[*].quantity.unit
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|2xx->2xx|$.items[*].status
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|2xx->2xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|GetWorkloadEstimate|2xx->2xx|$.status
Issue-Key: bcm-pricing-calculator|GetWorkloadEstimate|2xx->2xx|$.totalCost
Issue-Key: bcm-pricing-calculator|GetWorkloadEstimate|2xx->2xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|ListWorkloadEstimateUsage|2xx->2xx|$.items[*]
Issue-Key: bcm-pricing-calculator|ListWorkloadEstimateUsage|2xx->2xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|ListWorkloadEstimates|2xx->2xx|$.items[name="sim2real-probe-bcm-pricing-calculator-wlest-ab37f8d7"].status
Issue-Key: bcm-pricing-calculator|GetWorkloadEstimate|2xx->2xx|$.name
Issue-Commit: 8da75f4f8e37c6bec2b6e28a02ca64766de7a47e

# WorkloadEstimate state never settles: status, rateType, rename, cost and pagination token

> _Note: this issue was identified against the code base at commit `8da75f4f8e37c6bec2b6e28a02ca64766de7a47e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkloadEstimate

## Description
The emulator's WorkloadEstimate state is inert: it stores the resource but never completes it. Five distinct causes, 9 divergence clusters. A fixer MUST close all five before submitting.

Cause 1 of 5: status is pinned to UPDATING and never reaches VALID.
  AWS: Smithy: WorkloadEstimateStatus = UPDATING | VALID | INVALID | ACTION_NEEDED; it is returned by CreateWorkloadEstimate, GetWorkloadEstimate and the ListWorkloadEstimates summaries. The recordings show the estimate settling to VALID — already VALID on the CreateWorkloadEstimate response, and VALID on every subsequent read and list, including after usage is added.
  Emulator: returns 'UPDATING' everywhere, forever — on create, on get, and in list items.
  Why: UPDATING is a non-terminal state, so a client that polls until the estimate is VALID before reading costs never terminates; SDK waiters and any "is my estimate ready" gate hang on a resource that is in fact complete.
  Fix: the workload-estimate state machine (the create/update path's status transition) in the bcm-pricing-calculator crate's state layer, not per-operation output mapping.
  Traces: list-workload-estimates-pagination-includes-all-created#task#4 (ListWorkloadEstimates; cluster also covers task#5, task#8, task#9, task#10) and #task#6; workload-estimate-tagging#setup#1 (CreateWorkloadEstimate); workload-estimate-usage-lifecycle#setup#1 (CreateWorkloadEstimate), #task#3 and #task#8 (GetWorkloadEstimate), #task#6 (ListWorkloadEstimates).

Cause 2 of 5: CreateWorkloadEstimate omits rateType from its response.
  AWS: Smithy: CreateWorkloadEstimateResponse.rateType targets WorkloadEstimateRateType (BEFORE_DISCOUNTS | AFTER_DISCOUNTS | AFTER_DISCOUNTS_AND_COMMITMENTS); the input member rateType is optional. The recording shows the response echoing the effective rate type, BEFORE_DISCOUNTS, when the request did not specify one.
  Emulator: the member is absent from the response body.
  Why: a client cannot tell which rate basis its estimate was created with, so it cannot know whether the costs it later reads are before or after discounts — and the value it would have to assume is the one the service already decided.
  Fix: CreateWorkloadEstimate's handler/output serializer must persist and echo the effective rateType (defaulting it) rather than leaving it unset.
  Traces: workload-estimate-tagging#setup#1.

Cause 3 of 5: ListWorkloadEstimates returns no nextToken when a further page exists.
  AWS: Smithy: ListWorkloadEstimates is @paginated (inputToken nextToken, outputToken nextToken, pageSize maxResults, items items); maxResults is range 1..25 and output.nextToken is NextPageToken (length 0..2048). The recording, at a page boundary with more estimates still to return, carries a nextToken (its literal value is environment-specific and is asserted only as present/shaped).
  Emulator: omits nextToken (got None) even though further items remain.
  Why: a paginating client stops at the first page and silently loses every later estimate — the fixture is named for exactly this, that the listing must include all created estimates.
  Fix: the ListWorkloadEstimates pagination/token emission in its handler (emit a continuation token whenever the truncated result leaves items unreturned).
  Traces: list-workload-estimates-pagination-includes-all-created#task#6.

Cause 4 of 5: usage entries are stored without their priced fields, so BatchCreateWorkloadEstimateUsage and ListWorkloadEstimateUsage return skeletal items.
  AWS: Smithy: BatchCreateWorkloadEstimateUsageItem and WorkloadEstimateUsageItem both carry location, quantity (WorkloadEstimateUsageQuantity with amount AND unit), cost (Double), currency (CurrencyCode enum) and status (WorkloadEstimateCostStatus enum) alongside the required serviceCode/usageType/operation. The recordings return all of them: a priced item with location, quantity.unit, cost, currency and status VALID.
  Emulator: returns only serviceCode, usageType, operation, id, usageAccountId and quantity.amount; location, quantity.unit, cost, currency and status are absent. Consequently GetWorkloadEstimate.totalCost stays 0.0 instead of summing the usage, including after a second batch is added.
  Why: the whole point of the API is the cost of the estimate. A client reads a usage line with no cost, no currency, no unit and no status, and an estimate whose totalCost is 0.0 — so it reports a free workload. The specific amounts depend on the recorded usage, but the members must be populated and totalCost must equal the sum of the priced usage.
  Fix: the usage-entry state and pricing roll-up in the bcm-pricing-calculator crate (BatchCreateWorkloadEstimateUsage must persist and return the priced members; the estimate's totalCost must aggregate them), plus the ListWorkloadEstimateUsage projection.
  Traces: workload-estimate-usage-lifecycle#task#2 (BatchCreateWorkloadEstimateUsage), #task#4 (ListWorkloadEstimateUsage; cluster also covers #task#10), #task#3 and #task#8 (GetWorkloadEstimate totalCost).

Cause 5 of 5: UpdateWorkloadEstimate does not persist a renamed estimate.
  AWS: Smithy: UpdateWorkloadEstimate takes identifier (ResourceId) and updates the estimate; GetWorkloadEstimateResponse.name targets WorkloadEstimateName (@length max 64, @pattern ^[a-zA-Z0-9-]+$). The recording shows the post-update read returning the NEW name.
  Emulator: the subsequent GetWorkloadEstimate still returns the pre-update name, so the rename was accepted but dropped.
  Why: a client that renames an estimate and re-reads it sees the old name, so it either retries forever or concludes the update silently failed; any lookup keyed on the new name misses.
  Fix: UpdateWorkloadEstimate's handler must write the mutated name into the estimate's state (the state layer's update path), which GetWorkloadEstimate then reads.
  Traces: workload-estimate-usage-lifecycle#task#8.

Note: all nine steps ALSO diverge on the response content-type header, which is a separate service-wide cause filed as the bcm-pricing-calculator content-type issue in this same sweep.

## Evidence
Cause 1 (status UPDATING vs VALID), verdict mismatch, status 200 both sides:
- list-workload-estimates-pagination-includes-all-created#task#4 (affected_count 5: task#4, task#5, task#8, task#9, task#10) ListWorkloadEstimates: "$.items[0].status: value_mismatch (expected 'VALID', got 'UPDATING')".
- list-workload-estimates-pagination-includes-all-created#task#6: same key, plus Cause 3's nextToken.
- workload-estimate-tagging#setup#1 CreateWorkloadEstimate: "$.status: value_mismatch (expected 'VALID', got 'UPDATING')".
- workload-estimate-usage-lifecycle#setup#1 CreateWorkloadEstimate: same.
- workload-estimate-usage-lifecycle#task#3 GetWorkloadEstimate: same.
- workload-estimate-usage-lifecycle#task#6 ListWorkloadEstimates: "$.items[name=\"sim2real-probe-bcm-pricing-calculator-wlest-ab37f8d7\"].status: value_mismatch (expected 'VALID', got 'UPDATING')".
- workload-estimate-usage-lifecycle#task#8 GetWorkloadEstimate: same.

Cause 2: workload-estimate-tagging#setup#1 CreateWorkloadEstimate — "$.rateType: missing (expected 'BEFORE_DISCOUNTS', got None)".

Cause 3: list-workload-estimates-pagination-includes-all-created#task#6 ListWorkloadEstimates — "$.nextToken: missing (expected '<shaped-not-asserted>', got None)" (the token value itself is not asserted, only its presence).

Cause 4:
- workload-estimate-usage-lifecycle#task#2 BatchCreateWorkloadEstimateUsage — "$.items[0].cost: missing (expected 0.2496, got None)"; "$.items[0].currency: missing (expected 'USD', got None)"; "$.items[0].location: missing (expected 'us-east-1', got None)"; "$.items[0].quantity.unit: missing (expected 'Hrs', got None)"; "$.items[0].status: missing (expected 'VALID', got None)".
- workload-estimate-usage-lifecycle#task#4 (affected_count 2: task#4, task#10) ListWorkloadEstimateUsage — expected item {'cost': 0.2496, 'currency': 'USD', 'location': 'us-east-1', 'quantity': {'amount': 24.0, 'unit': 'Hrs'}, 'status': 'VALID', 'serviceCode': 'AmazonEC2', 'usageType': 'BoxUsage:t3.micro', 'operation': 'RunInstances', ...}, got {'serviceCode': 'AmazonEC2', 'usageType': 'BoxUsage:t3.micro', 'operation': 'RunInstances', 'id': ..., 'usageAccountId': '000000000000', 'quantity': {'amount': 24.0}} — five members dropped.
- workload-estimate-usage-lifecycle#task#3 GetWorkloadEstimate — "$.totalCost: value_mismatch (expected 0.2496, got 0.0)"; #task#8 after a second batch — "$.totalCost: value_mismatch (expected 0.4992, got 0.0)".

Cause 5: workload-estimate-usage-lifecycle#task#8 GetWorkloadEstimate — "$.name: value_mismatch (expected 'sim2real-probe-bcm-pricing-calculator-wlest-renamed-ab37f8d7', got 'sim2real-probe-bcm-pricing-calculator-wlest-ab37f8d7')" (the '-renamed-' segment is the update the preceding UpdateWorkloadEstimate step applied; the probe-name prefix/suffix is environment-generated).

Model grounding: smithy_query on services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json — WorkloadEstimateStatus values; WorkloadEstimateRateType values; CreateWorkloadEstimate/GetWorkloadEstimate outputs (name, rateType, status, totalCost, costCurrency); ListWorkloadEstimates @paginated + maxResults range 1..25; BatchCreateWorkloadEstimateUsageItem and WorkloadEstimateUsageItem members (location/quantity/cost/currency/status).

state_reset_failures for this replay was 0, so none of these are cross-fixture state bleed.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture bcm-pricing-calculator-list-workload-estimates-pagination-includes-all-created step task#4 — create estimates, then ListWorkloadEstimates; every item's status must be VALID, not UPDATING. Same for CreateWorkloadEstimate (workload-estimate-tagging#setup#1) and GetWorkloadEstimate (workload-estimate-usage-lifecycle#task#3).
4. Cause 2: workload-estimate-tagging#setup#1 — call CreateWorkloadEstimate without rateType; the response must carry rateType BEFORE_DISCOUNTS.
5. Cause 3: list-workload-estimates-pagination-includes-all-created#task#6 — ListWorkloadEstimates with a maxResults smaller than the number of estimates; the response must carry a nextToken, and following it must yield the remaining estimates.
6. Cause 4: workload-estimate-usage-lifecycle#task#2 then #task#4 — BatchCreateWorkloadEstimateUsage, then ListWorkloadEstimateUsage; each item must carry location, quantity.unit, cost, currency and status. Then GetWorkloadEstimate (#task#3, #task#8): totalCost must equal the summed usage cost, not 0.0.
7. Cause 5: workload-estimate-usage-lifecycle#task#8 — UpdateWorkloadEstimate with a new name, then GetWorkloadEstimate; the returned name must be the new one.
8. All nine steps must report passed.
