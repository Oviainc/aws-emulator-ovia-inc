Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-list-invoice-summaries-and-pdf
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-list-invoice-summaries-and-pdf
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|ListInvoiceSummaries|2xx->4xx|$.InvoiceSummaries
Issue-Key: invoicing|ListInvoiceSummaries|2xx->4xx|status
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# ListInvoiceSummaries refuses a filtered query with ValidationException instead of answering 200 with an empty InvoiceSummaries list

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInvoiceSummaries

## Description
Cause 1 of 1: ListInvoiceSummaries rejects a filtered summary query that AWS accepts, so the operation's success path is never reached and the registry is never read.

AWS: ListInvoiceSummaries is the service's read-only reporting operation (Smithy: ListInvoiceSummaries, input ListInvoiceSummariesRequest {Selector @required (InvoiceSummariesSelector {ResourceType enum INVOICE_ID|ACCOUNT_ID @required, Value @required}), Filter (InvoiceSummariesFilter {TimeInterval, BillingPeriod, InvoicingEntity, ReceiverRole}), NextToken, MaxResults @range(1..100)}; output ListInvoiceSummariesResponse {InvoiceSummaries @required list, NextToken}). A query whose Selector and Filter are inside the modelled constraints is ANSWERED, not refused: an account that has no invoices matching the filter is reported as success with the @required output member InvoiceSummaries present and EMPTY ([]), because a selector describing a set (ACCOUNT_ID) is answered with the empty set rather than an error. The recorded response for this step is HTTP 200 with body {"InvoiceSummaries": []}. The only 400 this operation models is ValidationException (Smithy: ValidationException, httpError 400, members message/reason (ValidationExceptionReason)/fieldList/resourceName), and it is owed ONLY when the input fails a constraint the service states — the documented period rules live on InvoiceSummariesFilter.ReceiverRole ("When ReceiverRole is specified: Data is available starting 2025-06-01. Queries for periods before 2025-06-01 return a validation error. TimeInterval supports a time interval of up to 5 years. Without ReceiverRole, TimeInterval is limited to one month."). The recording shows AWS did NOT consider this request to break any of those rules.

Emulator: the step is answered HTTP 400 instead of 200, and the response body carries no InvoiceSummaries member at all ($.InvoiceSummaries: expected [], got None) — i.e. a ValidationException error document in place of the success shape. The request is therefore refused BEFORE the invoice registry is read, so no page is ever computed.

The refusal is produced by the pre-registry period checks that run before state is touched (services/service-com-amazonaws-invoicing-invoicing/src/ops/list_invoice_summaries.rs, SummaryFilter::new lines 370-403), which are the only sources of a 400 inside this handler. Hypothesis: one of its two refusals is over-broad for the recorded filter — either (a) the data-start floor, which is applied not only to Filter.TimeInterval.StartDate but also to Filter.BillingPeriod (states_period_before_data_start, lines 282-301) and refuses with "Queries for periods before 2025-06-01 are not supported when ReceiverRole is specified" (reason invalidInput), or (b) the calendar-month interval bound (months_after / IntervalBound, lines 326-337, 397-403), which refuses with "TimeInterval supports a time interval of up to 5 years when ReceiverRole is specified" / "... up to one month when ReceiverRole is not specified" (reason invalidInput) whenever EndDate lands past StartDate + N calendar months. In both of those messages the quoted sentence is FIXED text and only the date (2025-06-01, from RECEIVER_ROLE_DATA_START) and the bound wording ("5 years"/"one month") vary; nothing from the request fills them, so neither message identifies the member it is blaming. The emulator must stop refusing the request shape this fixture replays and answer it from the registry.

Why: a client that lists invoice summaries for an account with no matching invoices gets a 400 ValidationException where AWS gives it a 200 with an empty list. Because ValidationException is a non-retryable client error, every SDK raises it instead of returning a page, so the normal "no invoices for this period" outcome becomes a hard failure; paginators abort on the first page, and any workflow that lists summaries before fetching an invoice document (as this fixture does) never reaches its later steps. Code written against AWS can never observe the empty-list result at all.

Fix: the pre-registry filter validation in the ListInvoiceSummaries handler — SummaryFilter::new and the helpers it calls (states_period_before_data_start, months_after, IntervalBound) in services/service-com-amazonaws-invoicing-invoicing/src/ops/list_invoice_summaries.rs. Narrow those refusals to exactly the inputs the model documents as invalid (the ReceiverRole-qualified period rules), and let every other legal Selector/Filter combination fall through to the registry walk, which already returns the empty list and the 200 success shape with InvoiceSummaries present.

Traces: invoicing-list-invoice-summaries-and-pdf#task#2 (expected 200 with InvoiceSummaries: [], got 400 with no InvoiceSummaries member).

Note: the other invoicing divergence in this replay (BatchGetInvoiceProfile answering the requester's own account) is already owned by open issue 20261108T050356Z-invoicing-batchgetinvoiceprofile-answers-the-requester-s-own-account-instead.md, and the GetInvoicePDF scaffold stub by 20261024T004801Z-invoicing-getinvoicepdf-stub.md; neither covers this ListInvoiceSummaries refusal.

## Evidence
Fixture invoicing-list-invoice-summaries-and-pdf, step task#2, operation ListInvoiceSummaries (service invoicing).

Expected (recorded AWS response): HTTP 200, body {"InvoiceSummaries": []} — the @required output member present and empty.
Actual (emulator): HTTP 400; $.InvoiceSummaries missing (expected [], got None) — an error document in place of the success shape.

Replay verdict: mismatch; divergence_key service=invoicing|ListInvoiceSummaries|status=200->400|keys=$.InvoiceSummaries,status. Affected steps: invoicing-list-invoice-summaries-and-pdf#task#2 (1 step in cluster). Summary for this run: 46 steps total, 43 passed, 3 mismatched, score 93.5%.

Model evidence: ListInvoiceSummariesResponse.InvoiceSummaries is @required, so a successful call always carries the list (empty when nothing matches); ValidationException (httpError 400) is the only modelled 400 and the only documented period restrictions are on InvoiceSummariesFilter.ReceiverRole (data from 2025-06-01; TimeInterval up to 5 years with ReceiverRole, one month without).

Code evidence: the only 400s this handler can emit come from SummaryFilter::new in services/service-com-amazonaws-invoicing-invoicing/src/ops/list_invoice_summaries.rs lines 370-403 — period_before_data_start() (lines 305-321) and interval_longer_than_bound() (lines 326-337) — both raised before crate::state::service_state is read (line 498), which is why the response contains no InvoiceSummaries at all.

## Steps to Reproduce
1. git_clone() and boot the emulator built from the checkout.
2. Replay wire fixture invoicing-list-invoice-summaries-and-pdf and let it run to step task#2, the ListInvoiceSummaries call (an invoicing summary query carrying a Selector plus a Filter that states a period).
3. Observe the response: the emulator answers HTTP 400 with a ValidationException document and no InvoiceSummaries member.
4. Compare with the recorded AWS response for that step: HTTP 200 with body {"InvoiceSummaries": []}.
5. Equivalently: call ListInvoiceSummaries with Selector {ResourceType: ACCOUNT_ID, Value: <a 12-digit account id>} and Filter stating a TimeInterval/BillingPeriod against the empty invoice registry; AWS answers 200 with an empty list, the emulator refuses it with 400.
