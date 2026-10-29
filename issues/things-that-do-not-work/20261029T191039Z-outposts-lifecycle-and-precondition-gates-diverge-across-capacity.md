Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-capacity-task-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-capacity-task-lifecycle outposts-decommission-and-delete-conflicts outposts-private-connectivity-config outposts-quote-and-order-lifecycle outposts-renewal-and-billing
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|StartCapacityTask|4xx->2xx|$.Message
Issue-Key: outposts|StartCapacityTask|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: outposts|StartCapacityTask|4xx->2xx|status
Issue-Key: outposts|StartOutpostDecommission|4xx->2xx|$.Message
Issue-Key: outposts|StartOutpostDecommission|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: outposts|StartOutpostDecommission|4xx->2xx|status
Issue-Key: outposts|CreatePrivateConnectivityConfig|4xx->2xx|$.Message
Issue-Key: outposts|CreatePrivateConnectivityConfig|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: outposts|CreatePrivateConnectivityConfig|4xx->2xx|status
Issue-Key: outposts|CreateQuote|4xx->2xx|$.Message
Issue-Key: outposts|CreateQuote|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: outposts|CreateQuote|4xx->2xx|status
Issue-Key: outposts|DeleteOutpost|2xx->4xx|status
Issue-Key: outposts|DeleteSite|2xx->4xx|status
Issue-Key: outposts|CreateRenewal|4xx->4xx|$.Message
Issue-Key: outposts|GetRenewalPricing|2xx->2xx|$.PricingResult
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# outposts lifecycle and precondition gates diverge across capacity, decommission, quote, renewal and delete paths

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCapacityTask

## Description
8 net-new root causes. The emulator's preconditions for the capacity-task, decommission, private-connectivity, quote, renewal and delete paths diverge in BOTH directions: four requests AWS refuses are accepted with 200, two deletes AWS accepts are refused, one refusal carries the generic constraint envelope instead of AWS's sentence, and one priced read reports UNABLE_TO_PRICE for an Outpost AWS prices.

Cause 1 of 8: StartCapacityTask on an Outpost that is not ACTIVE.
AWS: 400 ValidationException (Smithy: StartCapacityTask errors include ValidationException httpError 400; x-amzn-errortype: ValidationException) with Message `Failed to create capacity task for <outpostId>. Outpost should have LifeCycleStatus being ACTIVE, but <outpostId> has LifeCycleStatus <status>` — fixed text as quoted, with the Outpost id (twice, as resolved from the request) and the Outpost's CURRENT LifeCycleStatus from emulator state as the variable parts.
Emulator: 200 with a capacity-task body, and no x-amzn-errortype header — the task is created against a PENDING Outpost.
Why: a client that stages "create Outpost, then configure capacity" never learns it must wait for ACTIVE; it proceeds against a task AWS would never have created, and any test asserting the refusal passes a call that should fail.
Fix: StartCapacityTask's precondition checks in services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/start_capacity_task.rs — gate on the Outpost's LifeCycleStatus before minting the task.
Traces: outposts-capacity-task-lifecycle#task#3.

Cause 2 of 8: StartOutpostDecommission on an Outpost whose lifecycle status does not allow it.
AWS: 400 (client error, error class shaped but not asserted by the recording) with Message `Outpost <outpostId> has invalid lifecycleStatus=<status>` — fixed text "Outpost " + id + " has invalid lifecycleStatus=" + the Outpost's current status.
Emulator: 200 with no error header — the decommission is started on a PENDING Outpost.
Why: the emulator lets a caller decommission hardware that was never installed, so the decommission state machine is reachable from states AWS forbids and downstream reads report a lifecycle AWS cannot produce.
Fix: StartOutpostDecommission's precondition check (src/ops/start_outpost_decommission.rs).
Traces: outposts-decommission-and-delete-conflicts#task#4.

Cause 3 of 8: CreatePrivateConnectivityConfig with a VPC that does not exist.
AWS: 404 NotFoundException (x-amzn-errortype: NotFoundException) with Message `VPC <vpcId> could not be found` — fixed text "VPC " + the VpcId from the request + " could not be found".
Emulator: 200 with a private-connectivity-config body — the unknown VPC is accepted.
Why: a caller gets a configuration bound to a VPC that does not exist, and the only signal AWS gives that the VPC id was wrong (a 404) never arrives.
Fix: CreatePrivateConnectivityConfig's VPC resolution (src/ops/create_private_connectivity_config.rs) — resolve the VpcId and raise the modeled NotFoundException when it names nothing.
Traces: outposts-private-connectivity-config#task#3.

Cause 4 of 8: CreateQuote with no EC2 instance capacity requested.
AWS: 400 ValidationException (x-amzn-errortype: ValidationException) with the fixed Message `Your quote must contain at least one EC2 instance capacity type.` — wholly fixed text, no variable part.
Emulator: 200 and a quote is minted.
Why: a quote with no capacity is orderable in the emulator but rejected by AWS, so the whole quote→order flow can be driven from an input AWS refuses, and nothing surfaces the requirement.
Fix: CreateQuote's request validation (src/ops/create_quote.rs) — require at least one EC2 capacity entry in the quote requirements before pricing.
Traces: outposts-quote-and-order-lifecycle#task#3.

Cause 5 of 8: DeleteOutpost after the Outpost's capacity task has settled.
AWS: 200 — the Outpost deletes once nothing in flight holds it.
Emulator: 409 (ConflictException) — the conflict check still considers the Outpost held, so the delete is refused forever.
Why: the Outpost (and, cause 6, its site) can never be torn down; any lifecycle that creates an Outpost, runs a capacity task and cleans up dead-ends, leaking state into every later test or client run.
Fix: DeleteOutpost's conflict check (src/ops/delete_outpost.rs) — a completed/cancelled capacity task must not count as in-flight.
Traces: outposts-capacity-task-lifecycle#teardown#4.

Cause 6 of 8: DeleteSite after its last Outpost has been deleted.
AWS: 200 — the site deletes once no Outpost belongs to it.
Emulator: 400 — refused as if an Outpost still belonged to the site (a consequence of cause 5's Outpost that cannot be deleted, but the refusal arm is its own gate and must be verified independently).
Why: the site is undeletable, so site-scoped quotas and listings never return to a clean state.
Fix: DeleteSite's "site still has Outposts" check (src/ops/delete_site.rs) — recompute membership from live Outposts.
Traces: outposts-capacity-task-lifecycle#teardown#5.

Cause 7 of 8: CreateRenewal for an Outpost with no completed order.
AWS: 400 with Message `This Outpost does not have any completed orders. You cannot renew an Outpost without a completed order.` — wholly fixed text.
Emulator: 400 (status correct) with the generic constraint envelope `1 validation error detected: Value at 'outpostIdentifier' failed to satisfy constraint: the Outpost must have a subscription that can be renewed` — the framework's "N validation error(s) detected" template with an emulator-authored constraint clause, which AWS does not use for this precondition.
Why: the refusal reads as an input-constraint violation on outpostIdentifier rather than the business precondition it is, so a client cannot tell a malformed id from an unrenewable Outpost.
Fix: CreateRenewal's precondition refusal message (src/ops/create_renewal.rs) — raise the modeled ValidationException with AWS's sentence instead of routing through the generic constraint reporter.
Traces: outposts-renewal-and-billing#task#4.

Cause 8 of 8: GetRenewalPricing for an Outpost that has a completed order.
AWS: 200 with PricingResult `PRICED` (and the corresponding pricing options) for an Outpost whose subscription can be priced.
Emulator: 200 with PricingResult `UNABLE_TO_PRICE`.
Why: the renewal/billing flow can never be exercised — every Outpost looks unpriceable, so a client cannot read a renewal price or branch on PRICED.
Fix: GetRenewalPricing's pricing evaluation (src/ops/get_renewal_pricing.rs) — derive the result from the Outpost's uncancelled/completed orders and their catalog capacity.
Traces: outposts-renewal-and-billing#task#3. NOTE: issue 20261024T071625Z-outposts-getrenewalpricing-wrong-behavior is marked FIXED yet the corpus still shows UNABLE_TO_PRICE, and the open maintainability issue 20261029T110401Z-outposts-getrenewalpricing-maintainability does not change behavior; neither covers this.

## Evidence
Replay of simparity-wire-aws-v1:
- StartCapacityTask (outposts-capacity-task-lifecycle#task#3): status expected 400, got 200; '$.Message' missing (expected 'Failed to create capacity task for op-26992c62830635d43. Outpost should have LifeCycleStatus being ACTIVE, but op-26992c62830635d43 has LifeCycleStatus PENDING'); '$headers.x-amzn-errortype' missing (expected 'ValidationException').
- StartOutpostDecommission (outposts-decommission-and-delete-conflicts#task#4): status expected 400, got 200; '$.Message' missing (expected 'Outpost op-17ff9bd06b1cfcc4d has invalid lifecycleStatus=PENDING'); x-amzn-errortype missing.
- CreatePrivateConnectivityConfig (outposts-private-connectivity-config#task#3): status expected 404, got 200; '$.Message' missing (expected 'VPC vpc-0123456789abcdef0 could not be found'); x-amzn-errortype missing (expected 'NotFoundException').
- CreateQuote (outposts-quote-and-order-lifecycle#task#3): status expected 400, got 200; '$.Message' missing (expected 'Your quote must contain at least one EC2 instance capacity type.'); x-amzn-errortype missing (expected 'ValidationException').
- DeleteOutpost (outposts-capacity-task-lifecycle#teardown#4): status expected 200, got 409.
- DeleteSite (outposts-capacity-task-lifecycle#teardown#5): status expected 200, got 400.
- CreateRenewal (outposts-renewal-and-billing#task#4): '$.Message' expected 'This Outpost does not have any completed orders. You cannot renew an Outpost without a completed order.', got "1 validation error detected: Value at 'outpostIdentifier' failed to satisfy constraint: the Outpost must have a subscription that can be renewed".
- GetRenewalPricing (outposts-renewal-and-billing#task#3): '$.PricingResult' expected 'PRICED', got 'UNABLE_TO_PRICE'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Representative: fixture outposts-capacity-task-lifecycle, step task#3 — CreateSite, CreateOutpost, then POST /outposts/{id}/capacity immediately (Outpost still PENDING). The emulator answers 200 with a capacity task; AWS answers 400 ValidationException with the LifeCycleStatus sentence.
3. Same fixture, teardown#4 and teardown#5: after the capacity task settles, DELETE the Outpost (409 instead of 200) and then the site (400 instead of 200).
4. outposts-decommission-and-delete-conflicts#task#4: StartOutpostDecommission on a PENDING Outpost — 200 instead of 400.
5. outposts-private-connectivity-config#task#3: CreatePrivateConnectivityConfig with a VpcId that exists nowhere — 200 instead of 404 NotFoundException.
6. outposts-quote-and-order-lifecycle#task#3: CreateQuote with no EC2 capacity in the requirements — 200 instead of 400 ValidationException.
7. outposts-renewal-and-billing#task#3 and #task#4: GetRenewalPricing on an Outpost with a completed order (PricingResult UNABLE_TO_PRICE instead of PRICED) and CreateRenewal on an Outpost without one (generic constraint envelope instead of AWS's sentence).
