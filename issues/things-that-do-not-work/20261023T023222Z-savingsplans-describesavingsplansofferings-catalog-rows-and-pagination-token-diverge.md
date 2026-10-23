Issue-Source: trace-sweep-divergence
Issue-Trace: savingsplans-create-plan-error-paths
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: savingsplans-create-plan-error-paths savingsplans-queued-plan-delete-lifecycle savingsplans-discover-offerings-and-rates savingsplans-filtered-offering-rates-and-plans
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans|DescribeSavingsPlansOfferings|2xx->2xx|$.nextToken
Issue-Key: savingsplans|DescribeSavingsPlansOfferings|2xx->2xx|$.searchResults[*]
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# DescribeSavingsPlansOfferings catalog rows and pagination token diverge

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSavingsPlansOfferings

## Description
DescribeSavingsPlansOfferings answers 200 with a catalog that does not match the recorded AWS catalog, and never emits a continuation token. Two causes, both in the offerings search path.

Cause 1 of 2: offering rows carry a single generic usageType and differently-worded descriptions, in a different payment-option order.
  AWS: Smithy: SavingsPlanOffering.usageType (SavingsPlanUsageType, length 0..255, pattern ^[a-zA-Z0-9_ \/.:-]+$) and SavingsPlanOffering.description (SavingsPlanDescription, pattern ^[a-zA-Z0-9_\- ]+$); DescribeSavingsPlansOfferings input exposes a usageTypes filter list over exactly that member. The concrete values are recorded behavior only: the recording shows one Compute offering per (term, payment option) whose usageType encodes both — e.g. 'ComputeSP:1yrAllUpfront' for the 1 year All Upfront offering and 'ComputeSP:1yrNoUpfront' for the 1 year No Upfront one — with descriptions of the form '1 year All Upfront Compute Savings Plan', and searchResults ordered All Upfront, No Upfront, Partial Upfront.
  Emulator: every seeded Compute offering carries the same usageType 'ComputeSP:Hrs', descriptions of the form 'Compute Savings Plans 1 year All Upfront', and the rows come back in a different payment-option order (index 1 is the Partial Upfront offering where the recording has No Upfront).
  Why: usageType is a filter input and the only field that distinguishes one term/payment-option offering from another in a billing report. A client that filters offerings by the real usage type ('ComputeSP:1yrNoUpfront') gets an empty catalog from the emulator, so it can never reach CreateSavingsPlan; a client that reads usageType to decide which offering it just found cannot tell the offerings apart, since all of them report the same value.
  Fix: the seeded offering catalog in the service state (the Default-built catalog that DescribeSavingsPlansOfferings searches), plus the ordering the search walks it in.

Cause 2 of 2: nextToken is never returned, so a truncated offerings page is indistinguishable from the last page.
  AWS: Smithy: DescribeSavingsPlansOfferings output nextToken (PaginationToken, length 0..1024, pattern ^[A-Za-z0-9/=\+]+$), documented as 'The token to use to retrieve the next page of results. This value is null when there are no more results to return.' Recorded behavior: for the recorded requests AWS truncated the page and returned a non-empty nextToken alongside searchResults (the token value itself is an opaque, environment-specific string and is not a requirement).
  Emulator: omits nextToken entirely on every offerings response, returning None where the recording has a token.
  Why: a client paginating the catalog — including every AWS SDK paginator — stops at the first page and treats a truncated catalog as the complete one, so offerings that exist are invisible and a subsequent CreateSavingsPlan for one of them fails.
  Fix: the DescribeSavingsPlansOfferings handler's paging (maxResults/nextToken) and its response serializer.

Traces: savingsplans-create-plan-error-paths#setup#1 (causes 1 and 2; also savingsplans-queued-plan-delete-lifecycle#setup#1), savingsplans-discover-offerings-and-rates#task#1 (cause 2; also savingsplans-filtered-offering-rates-and-plans#task#2).

## Evidence
Cause 1 — fixture savingsplans-create-plan-error-paths step setup#1, status 200 vs 200:
  $.searchResults[0] expected {description: '1 year All Upfront Compute Savings Plan', paymentOption: 'All Upfront', usageType: 'ComputeSP:1yrAllUpfront', serviceCode: 'ComputeSavingsPlans', durationSeconds: 31536000, currency: 'USD', productTypes: ['Fargate','EC2','Lambda']}
  got {description: 'Compute Savings Plans 1 year All Upfront', paymentOption: 'All Upfront', usageType: 'ComputeSP:Hrs', ...}
  $.searchResults[1] expected paymentOption 'No Upfront' / usageType 'ComputeSP:1yrNoUpfront'; got paymentOption 'Partial Upfront' / usageType 'ComputeSP:Hrs'
  (offeringId was '<shaped-not-asserted>' in the recording and is not asserted)

Cause 2 — same step: $.nextToken missing (expected '<shaped-not-asserted>', got None).
  fixture savingsplans-discover-offerings-and-rates step task#1, status 200 vs 200: $.nextToken missing (expected an opaque 110-char token, got None).

Emulator source backing cause 1: the seeded catalog sets usage_type 'ComputeSP:Hrs' for the Compute offerings (services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs:500).

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1 + 2: fixture savingsplans-create-plan-error-paths, step setup#1 — POST /DescribeSavingsPlansOfferings for Compute plans; compare searchResults[*].usageType/description/order and the presence of nextToken against the recorded response.
3. Cause 2 alone: fixture savingsplans-discover-offerings-and-rates, step task#1 — POST /DescribeSavingsPlansOfferings; the recorded response carries nextToken, the emulator's does not.
4. Also reproducible at savingsplans-queued-plan-delete-lifecycle#setup#1 and savingsplans-filtered-offering-rates-and-plans#task#2.
