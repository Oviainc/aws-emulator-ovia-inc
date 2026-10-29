Issue-Source: trace-sweep-divergence
Issue-Trace: outposts-error-paths-not-found-and-validation
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: outposts-error-paths-not-found-and-validation outposts-assets-and-connection outposts-decommission-and-delete-conflicts outposts-quote-and-order-lifecycle
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts|GetOrder|4xx->4xx|$.Message
Issue-Key: outposts|StartConnection|4xx->4xx|$.Message
Issue-Key: outposts|DeleteOutpost|4xx->4xx|$.Message
Issue-Key: outposts|GetQuote|4xx->4xx|$.Message
Issue-Key: outposts|DeleteQuote|4xx->4xx|$.Message
Issue-Key: outposts|GetCapacityTask|4xx->4xx|$.Message
Issue-Key: outposts|CancelOrder|4xx->4xx|$.Message
Issue-Key: outposts|CancelCapacityTask|4xx->4xx|$.Message
Issue-Key: outposts|CreateOrder|4xx->4xx|$.Message
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# outposts not-found refusals echo the identifier instead of AWS's not-found wording

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOrder

## Description
9 net-new root causes, one per operation. Every NotFoundException this service raises is worded by one shared "identifier echo" helper that renders `<memberName>=<value as supplied>` (see services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/get_order.rs, which documents the refusal as "worded as the identifier echo of the value as supplied"). AWS Outposts words each not-found refusal as a sentence, and the wording differs per resource kind; the status (404) and the error class (NotFoundException, Smithy: NotFoundException httpError 404, member Message, ErrorMessage len 1..1000) are already correct in every case, so the defect is the Message text only. In each cause below, the quoted expected text is AWS's FIXED text, with the identifier (the order/quote/asset/Outpost/capacity-task id, or the full ARN) being the only variable part, taken from the request; a recorded id such as `oo-00000000000000000` is a filler for that slot, not part of the requirement.

Cause 1 of 9: GetOrder, unknown OrderId.
AWS: 404 NotFoundException, Message `No such order for order id <orderId>` — fixed text "No such order for order id " plus the OrderId from the request (Smithy: GetOrder.OrderId @httpLabel pattern ^oo-[a-f0-9]{17}$; recorded behavior supplies the wording, the model documents no message text).
Emulator: 404 NotFoundException with Message `orderId=oo-00000000000000000` — the bare member-name=value echo.
Why: a client that matches on or surfaces the service's message text (log scraping, user-facing error display, SDK error-message assertions in tests) sees a string AWS never emits, so error handling keyed on AWS's wording never fires.
Fix: the GetOrder handler's not-found arm in services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/get_order.rs, and the shared identifier-echo message helper it calls — the helper must stop being the message for not-found paths.
Traces: outposts-error-paths-not-found-and-validation#task#3.

Cause 2 of 9: CancelOrder, unknown OrderId.
AWS: 404 NotFoundException, Message `No such order for order id <orderId>` (same fixed text as GetOrder; variable part is the request's OrderId).
Emulator: Message `orderId=oo-00000000000000000`.
Why: same as cause 1 — the refusal is unrecognizable to a client keyed on AWS's text.
Fix: CancelOrder's not-found arm (src/ops/cancel_order.rs) plus the shared message helper.
Traces: outposts-error-paths-not-found-and-validation#task#8.

Cause 3 of 9: GetQuote, unknown QuoteIdentifier.
AWS: 404 NotFoundException, Message `Could not find Quote with identifier <quoteIdentifier>` — fixed text "Could not find Quote with identifier " plus the identifier as supplied.
Emulator: Message `quoteIdentifier=oq-00000000000000000`.
Why: as above; the quote-not-found case is also the one a caller distinguishes from a validation failure by message, and the echo form is identical for both kinds here.
Fix: GetQuote's not-found arm (src/ops/get_quote.rs) plus the shared message helper.
Traces: outposts-error-paths-not-found-and-validation#task#4.

Cause 4 of 9: DeleteQuote, unknown QuoteIdentifier.
AWS: 404 NotFoundException, Message `Could not find Quote with identifier <quoteIdentifier>` (same fixed text as GetQuote).
Emulator: Message `quoteIdentifier=oq-00000000000000000`.
Why: as above.
Fix: DeleteQuote's not-found arm (src/ops/delete_quote.rs) plus the shared message helper.
Traces: outposts-error-paths-not-found-and-validation#task#5.

Cause 5 of 9: GetCapacityTask, Outpost that does not exist.
AWS: 404 NotFoundException, Message `No Outpost with outpostId=<outpostId>` — fixed text "No Outpost with outpostId=" plus the resolved Outpost id; note AWS names the member `outpostId` in the message even though the request member is `OutpostIdentifier`.
Emulator: Message `outpostIdentifier=op-00000000000000000` — the request member name, not AWS's.
Why: a client cannot tell from the message which resource AWS considered missing in the wording it expects, and the member name it keys on differs.
Fix: GetCapacityTask's not-found arm (src/ops/get_capacity_task.rs) plus the shared message helper.
Traces: outposts-error-paths-not-found-and-validation#task#7.

Cause 6 of 9: CancelCapacityTask, Outpost that does not exist.
AWS: 404 NotFoundException, Message `No Outpost with outpostId=<outpostId>` (same fixed text as cause 5).
Emulator: Message `outpostIdentifier=op-00000000000000000`.
Why: as above.
Fix: CancelCapacityTask's not-found arm (src/ops/cancel_capacity_task.rs) plus the shared message helper.
Traces: outposts-error-paths-not-found-and-validation#task#9.

Cause 7 of 9: CreateOrder against an Outpost that does not exist.
AWS: 404 NotFoundException whose Message names the Outpost by its FULL ARN — `outpostId=arn:aws:outposts:<region>:<accountId>:outpost/<outpostId>` — i.e. the fixed text "outpostId=" plus the ARN AWS resolves the supplied identifier to (partition, region and account id come from the request's signing context, the Outpost id from the request; the recorded `arn:aws:outposts:us-west-2:000000000000:outpost/op-00000000000000000` is one filling of that template).
Emulator: Message `outpostIdentifier=op-00000000000000000` — wrong member name AND the bare id rather than the resolved ARN.
Why: callers that parse the ARN out of the refusal to report which account/region the Outpost was looked up in get nothing, and the message does not match AWS's text.
Fix: CreateOrder's Outpost-resolution not-found arm (src/ops/create_order.rs) plus the shared message helper; the ARN rendering must come from the same ARN builder the success path uses.
Traces: outposts-quote-and-order-lifecycle#task#5.

Cause 8 of 9: DeleteOutpost for an Outpost that no longer exists (after decommission/delete).
AWS: 404 NotFoundException, Message `outpostId=arn:aws:outposts:<region>:<accountId>:outpost/<outpostId>` — the resolved ARN, as in cause 7.
Emulator: Message `outpostId=op-17ff9bd06b1cfcc4d` — correct member name, but the bare id instead of the ARN.
Why: same as cause 7; a client cannot distinguish "wrong account/region" from "really gone".
Fix: DeleteOutpost's not-found arm (src/ops/delete_outpost.rs) plus the shared message helper.
Traces: outposts-decommission-and-delete-conflicts#teardown#6.

Cause 9 of 9: StartConnection with an AssetId no asset has.
AWS: 404 NotFoundException, Message `No such asset id <assetId> found.` — fixed text "No such asset id " + the 10-digit AssetId from the request + " found." (trailing period is part of the text).
Emulator: Message `assetId=1234567890`.
Why: as above; this is the one asset-level refusal in the connection flow, so install tooling keyed on AWS's sentence cannot recognize it.
Fix: StartConnection's asset-resolution not-found arm (src/ops/start_connection.rs) plus the shared message helper.
Traces: outposts-assets-and-connection#task#5.

Related open issues that do NOT cover these: 20261029T061148Z-outposts-getconnection-error-response (the echo's LENGTH bound, GetConnection only) and 20261028T160628Z-outposts-startconnection-error-response (store-ceiling 500 classification). Neither changes the not-found message TEXT for the nine operations above.

## Evidence
Replay of simparity-wire-aws-v1, all nine clusters status-correct (404) and Message-divergent:
- GetOrder (outposts-error-paths-not-found-and-validation#task#3): expected '$.Message' = 'No such order for order id oo-00000000000000000', got 'orderId=oo-00000000000000000'.
- CancelOrder (#task#8): expected 'No such order for order id oo-00000000000000000', got 'orderId=oo-00000000000000000'.
- GetQuote (#task#4): expected 'Could not find Quote with identifier oq-00000000000000000', got 'quoteIdentifier=oq-00000000000000000'.
- DeleteQuote (#task#5): expected 'Could not find Quote with identifier oq-00000000000000000', got 'quoteIdentifier=oq-00000000000000000'.
- GetCapacityTask (#task#7): expected 'No Outpost with outpostId=op-00000000000000000', got 'outpostIdentifier=op-00000000000000000'.
- CancelCapacityTask (#task#9): expected 'No Outpost with outpostId=op-00000000000000000', got 'outpostIdentifier=op-00000000000000000'.
- CreateOrder (outposts-quote-and-order-lifecycle#task#5): expected 'outpostId=arn:aws:outposts:us-west-2:000000000000:outpost/op-00000000000000000', got 'outpostIdentifier=op-00000000000000000'.
- DeleteOutpost (outposts-decommission-and-delete-conflicts#teardown#6): expected 'outpostId=arn:aws:outposts:us-west-2:000000000000:outpost/op-17ff9bd06b1cfcc4d', got 'outpostId=op-17ff9bd06b1cfcc4d'.
- StartConnection (outposts-assets-and-connection#task#5): expected 'No such asset id 1234567890 found.', got 'assetId=1234567890'.
In every cluster the HTTP status (404) and error class (NotFoundException) matched; only $.Message diverged.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Representative: fixture outposts-error-paths-not-found-and-validation, step task#3 — GET /orders/oo-00000000000000000 for an order that was never created. Observe 404 with body {"Message":"orderId=oo-00000000000000000"} where AWS answers {"Message":"No such order for order id oo-00000000000000000"}.
3. Repeat for each of the other eight bindings in trace_ids (GetQuote/DeleteQuote with an unknown oq- id, GetCapacityTask/CancelCapacityTask and CreateOrder/DeleteOutpost with an unknown op- id, CancelOrder with an unknown oo- id, StartConnection with an unknown 10-digit AssetId) and compare $.Message against the recorded response.
