Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-agreement-charges-invoices-and-purchase-orders
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-agreement-charges-invoices-and-purchase-orders
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|ListAgreementCharges|2xx->4xx|$.items
Issue-Key: marketplace-agreement|ListAgreementCharges|2xx->4xx|status
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# ListAgreementCharges rejects a valid request with 400 instead of an empty page

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAgreementCharges

## Description
Cause 1 of 1: ListAgreementCharges answers a well-formed request with a client error instead of an empty result page.
AWS: every member of ListAgreementChargesInput is optional — catalog, agreementId (ResourceId @pattern ^[A-Za-z0-9_/-]+$), agreementType, maxResults (@range 1..50) and nextToken (Smithy: ListAgreementCharges, input, no @required members); the operation is @readonly and @paginated with items=items, pageSize=maxResults, inputToken/outputToken=nextToken. A request whose members all satisfy their constraints is accepted, and when no charge matches the filter the service answers HTTP 200 with `items` present and empty and no `nextToken`. An empty match set is not an error: ValidationException (Smithy: @error client, @httpError 400) is reserved for input that fails a constraint.
Emulator: answers HTTP 400 and serves no `items` member at all, so the caller gets a client error where AWS gives it an empty first page.
Why: an empty result is the normal state of a newly created agreement that has not been charged yet, and of any filter that matches nothing. A client that paginates charges sees a 400, treats its own request as malformed, and aborts the sweep instead of concluding there are zero charges; a paginating loop that branches on items/nextToken never runs at all. Because the operation is @readonly the caller has no way to tell this apart from a genuine input rejection.
Fix: ListAgreementCharges' handler must validate only the modeled input constraints and otherwise scan state, returning 200 with `items` serialized as an empty list (and `nextToken` omitted) when the filter matches nothing, rather than refusing the request. The empty-list serialization must emit the member, not drop it.
Traces: marketplace-agreement-charges-invoices-and-purchase-orders#task#1.

No open issue carries this replay key. 20261029T024420Z-marketplace-agreement-updatepurchaseorders-wrong-behavior.md and 20261029T071242Z-marketplace-agreement-listagreementinvoicelineitems-wrong-behavior.md touch the same fixture's domain but concern UpdatePurchaseOrders timestamps and ListAgreementInvoiceLineItems store settling, not this operation's status.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture marketplace-agreement-charges-invoices-and-purchase-orders, step task#1, operation ListAgreementCharges, verdict mismatch:
  status: expected 200, got 400
  $.items: expected [] (member present, empty list), got None (member absent)

The divergence is the status class and the absent `items` member; no recorded value, id or timestamp is asserted. Model support: ListAgreementChargesInput has no @required members; the operation is @readonly and @paginated (items=items, pageSize=maxResults @range 1..50, nextToken @length max 8192 @pattern ^[a-zA-Z0-9+/=_-]+$); ValidationException is @httpError 400 and applies only to input that fails a constraint.

## Steps to Reproduce
1. Boot the emulator from the mainline checkout.
2. Replay fixture marketplace-agreement-charges-invoices-and-purchase-orders, step task#1: call ListAgreementCharges with input whose members all satisfy their modeled constraints, against state in which no charge matches the filter.
3. Observe HTTP 400 with no `items` member in the body, instead of HTTP 200 with `items` present and empty and `nextToken` omitted.
