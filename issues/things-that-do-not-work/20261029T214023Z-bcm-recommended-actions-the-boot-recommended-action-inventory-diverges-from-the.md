Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-recommended-actions.recommended-action-type-matches-documented-feature-severity bcm-recommended-actions.filter-type-partition-invariant
Issue-Shape-Id: com.amazonaws.bcmrecommendedactions#AWSBillingAndCostManagementRecommendedActions
Issue-Key: bcm-recommended-actions.recommended-action-type-matches-documented-feature-severity|run#5|AssertionError|assert action["severity"] == expected_severity, (action["type"], action["severity"])
Issue-Key: bcm-recommended-actions.filter-type-partition-invariant|run#13|AssertionError|assert equals_ids, f"TYPE EQUALS {_TARGET_TYPE} returned no items; unfiltered set had {len(unfiltered_ids)}"
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# The boot recommended-action inventory diverges from the documented action-type catalog: REVIEW_EXPIRING_SP carries severity CRITICAL instead of WARNING, and only three of the twenty modelled action types are ever present

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendedActions

## Description
This service has no create/update/delete operation — AWS computes the recommendations itself — so the emulator's boot inventory IS the data contract a client reads. That inventory diverges from the fixed (type -> feature, severity) catalog the service publishes, and it is too narrow for an exclusion/inclusion filter over TYPE to behave as the API's filtering contract requires. Two causes, both in this service's boot-inventory state, bundled because they share one service and one operation.

Cause 1 of 2: a stored action's severity contradicts the documented severity for its type.
  AWS: Smithy models RecommendedAction$type (ActionType), $feature (Feature) and $severity (Severity) as enums but does not state their pairing; the pairing is published by the service in the "Understanding recommended action types" catalog, which fixes it per type (it is a property of the service, not of account data). For REVIEW_EXPIRING_SP the documented pair is feature SAVINGS_PLANS, severity WARNING — the same WARNING tier as its sibling REVIEW_EXPIRING_RI (RESERVATIONS, WARNING); CRITICAL is reserved for the payments types (PAYMENTS_PAST_DUE, UPDATE_INVALID_PAYMENT_METHOD, UPDATE_EXPIRED_PAYMENT_METHOD).
  Emulator: the boot inventory stores REVIEW_EXPIRING_SP with Severity::Critical, so ListRecommendedActions reports {"type": "REVIEW_EXPIRING_SP", "feature": "SAVINGS_PLANS", "severity": "CRITICAL"}. The feature is right; only the severity tier is wrong. The other two seeded records (REVIEW_SAVINGS_OPPORTUNITY_RECOMMENDATIONS -> COST_OPTIMIZATION_HUB/INFO, REVIEW_EXPIRING_RI -> RESERVATIONS/WARNING) match the catalog.
  Why: severity is what a client routes and prioritises on — a dashboard, alerting rule or SEVERITY filter keyed to CRITICAL picks up an expiring Savings Plan from the emulator that AWS classifies as WARNING, and a SEVERITY EQUALS WARNING filter misses it. Because there is no write API, a client cannot correct or work around the value.
  Fix: the boot inventory in this service's state — the `computed_recommendations` record for "ra-expiring-savings-plans" must carry the documented severity for its type, and the catalog pairing should be expressed once (type -> feature, severity) so a seeded record cannot name a pair the service does not publish.
  Traces: bcm-recommended-actions.recommended-action-type-matches-documented-feature-severity. Code: services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/state.rs:228-256 (computed_recommendations), specifically the Severity::Critical argument on the ra-expiring-savings-plans record at :247-254.

Cause 2 of 2: the inventory is too narrow for the TYPE filter's partition contract to hold.
  AWS: Smithy: ActionFilter with key TYPE and matchOption EQUALS keeps the records whose type is one of `values`; NOT_EQUALS excludes them. The two are complements, so for any type value the EQUALS result and the NOT_EQUALS result are disjoint and their union is exactly the unfiltered result set, with both sides non-empty for a type the account actually has. A real account carries a broad spread of the twenty modelled ActionTypes — CREATE_BUDGET (BUDGETS, INFO) in particular is the baseline recommendation present whenever no budget exists, which is the default state of a fresh account.
  Emulator: the boot inventory holds exactly three records, of exactly three types (REVIEW_SAVINGS_OPPORTUNITY_RECOMMENDATIONS, REVIEW_EXPIRING_RI, REVIEW_EXPIRING_SP), so TYPE EQUALS CREATE_BUDGET returns an empty list while the unfiltered listing returns 3 — the EQUALS side of the partition is empty for a type the account should have, and seventeen of the twenty modelled types can never appear in any response. The filtering logic itself is correct (EQUALS/NOT_EQUALS are complements over whatever is stored); the stored set is the divergence.
  Why: any client exercising TYPE-based inclusion/exclusion, or reading a specific recommendation class (create a budget, enable Cost Optimization Hub, fix a payment method, tax registration, anomaly monitors), gets nothing from the emulator and a result from AWS. With no write API there is no way for a client or test to introduce the missing types through the public surface, so the gap is not recoverable at the client.
  Fix: the boot inventory in this service's state — `computed_recommendations` should install a representative record for the documented action types (at minimum CREATE_BUDGET), each with the catalog's feature/severity pair from cause 1, so that both sides of a TYPE EQUALS/NOT_EQUALS partition are non-empty for the types a fresh account carries.
  Traces: bcm-recommended-actions.filter-type-partition-invariant. Code: src/state.rs:213-256.

Note: open issue 20261029T134218Z-bcm-recommended-actions-listrecommendedactions-wrong-behavior--r4 concerns the same boot inventory but a different member (lastUpdatedTimeStamp being stamped by the first reader's clock); it does not cover the severity pairing or the set of types installed.

## Evidence
Behavioral replay simparity-behavioral-aws-v1, service bcm-recommended-actions, operation ListRecommendedActions. Both checks call ListRecommendedActions successfully (HTTP 200) — the divergence is in the reported data, not the status or shape.

Cause 1 — bcm-recommended-actions.recommended-action-type-matches-documented-feature-severity [mismatch]:
  The check walks every returned action whose `type` appears in the published catalog and asserts its `feature` and `severity` equal the catalog's pair.
  AssertionError: ('REVIEW_EXPIRING_SP', 'CRITICAL') — expected severity WARNING for type REVIEW_EXPIRING_SP (catalog pair: SAVINGS_PLANS, WARNING). The feature assertion for the same record passed.
  The two other seeded records passed both assertions, so the catalog mapping is honoured for them and violated for exactly this one.
  Source corroboration: src/state.rs:247-254 seeds "ra-expiring-savings-plans" with ActionType::ReviewExpiringSp and Severity::Critical.

Cause 2 — bcm-recommended-actions.filter-type-partition-invariant [mismatch]:
  The check paginates the unfiltered listing, then the TYPE EQUALS CREATE_BUDGET listing and the TYPE NOT_EQUALS CREATE_BUDGET listing, and asserts both sides are non-empty, disjoint, and reconstruct the unfiltered set.
  AssertionError: TYPE EQUALS CREATE_BUDGET returned no items; unfiltered set had 3.
  The unfiltered listing returns exactly the three boot records (ids ra-savings-opportunities, ra-expiring-reserved-instances, ra-expiring-savings-plans — these ids are the emulator's own generated values, not a requirement); no record of type CREATE_BUDGET exists, so the EQUALS partition is empty and NOT_EQUALS returns all three.
  Source corroboration: src/state.rs:228-256 installs three records; ActionType at src/state.rs:65-86 declares twenty members, seventeen of which no response can ever carry.
  Note the complement logic itself is sound over the stored set (NOT_EQUALS returned all 3, disjoint from the empty EQUALS side) — what fails is that the stored set carries none of the type under test.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Cause 1: call ListRecommendedActions(maxResults=1000) and look at the record whose `type` is REVIEW_EXPIRING_SP. The emulator reports severity CRITICAL; the published catalog pairs that type with feature SAVINGS_PLANS and severity WARNING (its sibling REVIEW_EXPIRING_RI is correctly WARNING in the same response, so the two expiry recommendations are reported in different tiers).
3. Cause 2: paginate ListRecommendedActions with no filter and record the ids. Then call it with filter={"actions":[{"key":"TYPE","matchOption":"EQUALS","values":["CREATE_BUDGET"]}]} and again with matchOption NOT_EQUALS over the same value. The EQUALS listing is empty while the unfiltered listing has three records, so the EQUALS/NOT_EQUALS partition has an empty side for a type a fresh account carries.
4. Repeat step 3 for any other documented type (ENABLE_COST_OPTIMIZATION_HUB, PAYMENTS_DUE, ADD_ALTERNATE_BILLING_CONTACT, CREATE_ANOMALY_MONITOR, …): every one returns an empty EQUALS listing, because the boot inventory installs only three of the twenty modelled ActionTypes and the service exposes no write path to add one.
