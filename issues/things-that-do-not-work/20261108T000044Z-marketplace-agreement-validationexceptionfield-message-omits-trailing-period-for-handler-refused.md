Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-agreement-validation-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-agreement-validation-error-paths
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|ListAgreementPaymentRequests|4xx->4xx|$.fields[name="partyType"].message
Issue-Key: marketplace-agreement|SearchAgreements|4xx->4xx|$.fields[*].message
Issue-Commit: 09ede02538826dd718336321255b799d2c318d86

# ValidationExceptionField message omits trailing period for handler-refused members

> _Note: this issue was identified against the code base at commit `09ede02538826dd718336321255b799d2c318d86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAgreementPaymentRequests

## Description
The crate deliberately keeps TWO different texts for the same ValidationException field class and picks between them by WHERE the refusal was raised: `FIELD_INVALID_CONSTRAINT = "Member is invalid."` for a value refused by the modelled constraint walk at the boundary, and `FIELD_INVALID = "Member is invalid"` (no trailing period) for a well-formed value refused by a rule inside a handler. AWS makes no such distinction: every `ValidationExceptionField$message` for an invalid member reads "Member is invalid." with the trailing period. So every refusal this service raises from a handler answers a message one character short of AWS's, on 2 replayed steps of this corpus.

This is ONE shared fault with 2 causes, both of which must be closed.

Cause 1 of 2 — ListAgreementPaymentRequests, an unacceptable `partyType` (fixture marketplace-agreement-validation-error-paths, step task#1).
AWS: Smithy: ListAgreementPaymentRequests.partyType targets `PartyType` (@required, @length min 1 max 32, @pattern ^[A-Za-z]+$) — a plain string, not an enum, so a well-formed string that names neither role passes the modelled constraints and is refused by the service's own rule. The modelled failure is `ValidationException` (HTTP 400), whose `fields` is a list of `ValidationExceptionField{name (@required), message (@required)}` and whose `reason` enum carries `INVALID_PARTY_TYPE`. Recorded response: HTTP 400, `ValidationException`, with `fields[name="partyType"].message` = "Member is invalid.". Fixed text of that message: the whole sentence "Member is invalid." — it has NO variable part; the member being complained about is carried by the sibling `fields[].name` (here the request's member path) and the rule by `reason`. The recorded requestId is a per-run value and is not part of the requirement.
Emulator: answers the right status (400), the right shape and the right `fields[].name`, but `fields[name="partyType"].message` = "Member is invalid" — the trailing period is missing, because `crate::state::validation_exception("INVALID_PARTY_TYPE", "partyType")` resolves `RefusalKind::Invalid` and `RefusalKind::field_message` maps that kind to `FIELD_INVALID` rather than to the punctuated text.
Why: a client that matches, logs, or surfaces the validation message compares against AWS's wording and sees a message AWS never emits; any exact-match assertion, localisation table or support-tooling lookup keyed on "Member is invalid." misses, and a caller cannot tell from the message alone that this is the same class of failure it gets from the real service. The divergence is silent — status, error code and reason all look right — so it is only ever caught by byte-comparing the body.
Fix: the service's shared ValidationException field-message table (the `RefusalKind::Invalid` arm of the field-message mapping used by this operation's validation path), not the operation handler: the handler already names the right reason and member. Collapse the invalid-member class onto the single punctuated text AWS uses, so a handler-raised refusal and a boundary-raised one state the same sentence.
Traces: marketplace-agreement-validation-error-paths#task#1.

Cause 2 of 2 — SearchAgreements, a member refused by a service rule rather than by a modelled constraint (fixture marketplace-agreement-validation-error-paths, step task#3).
AWS: Smithy: SearchAgreements input members `catalog` (@length 1..64, @pattern ^[a-zA-Z0-9.-]+$), `filters` (FilterList, @length 1..10), `sort`, `maxResults` (@range 1..50), `nextToken` (@length ..8192, @pattern ^[a-zA-Z0-9+/=_-]+$); modelled errors include `ValidationException` (HTTP 400) whose `reason` enum carries the service-rule reasons `INVALID_FILTERS`, `INVALID_FILTER_NAME`, `INVALID_FILTER_VALUES`, `UNSUPPORTED_FILTERS`, `INVALID_SORT_BY`, `INVALID_SORT_ORDER`, `INVALID_CATALOG`. Recorded response: HTTP 400, `ValidationException`, with `fields[0].message` = "Member is invalid." — again the whole sentence is fixed text with no variable part, the member path being carried by `fields[0].name`.
Emulator: `fields[0].message` = "Member is invalid", from the same unpunctuated `FIELD_INVALID` constant — reached both through `validation_exception`/`validation_exception_naming` for this operation's filter, sort and catalog rules and through the dedicated unsupported-filter-combination body, which sets `fields[].message` to that constant directly.
Why: same client-visible consequence as cause 1, on the operation whose refusals are almost entirely service-rule refusals (filters, sort, catalog), so SearchAgreements diverges on essentially every validation path a caller can hit rather than on an edge case.
Fix: the same shared field-message table; additionally the unsupported-filter-combination constructor, which hard-codes the unpunctuated constant into its `fields` entry instead of going through the class mapping.
Traces: marketplace-agreement-validation-error-paths#task#3.

Not covered by any open issue: the five open marketplace-agreement issues concern skipped intent/proposal validation on CreateAgreementRequest, entitlement propagation on AcceptAgreementRequest, UpdatePurchaseOrders timestamps, GetAgreementEntitlements nextToken acceptance, and DescribeAgreement IAM evaluation — none of them is about the wording of ValidationExceptionField$message.

## Evidence
Replay of trace set simparity-wire-aws-v1: 35 steps, 33 passed, 2 mismatched (score 94.3%), both mismatches in this service and both the same field-message text.

Cause 1 — fixture marketplace-agreement-validation-error-paths, step task#1, ListAgreementPaymentRequests:
  expected HTTP 400, actual HTTP 400 (status and error shape agree)
  divergent key: $.fields[name="partyType"].message
  expected: "Member is invalid."
  actual:   "Member is invalid"
  verdict: mismatch; divergence_key service=marketplace-agreement|ListAgreementPaymentRequests|status=400->400|keys=$.fields[name="partyType"].message

Cause 2 — fixture marketplace-agreement-validation-error-paths, step task#3, SearchAgreements:
  expected HTTP 400, actual HTTP 400
  divergent key: $.fields[0].message
  expected: "Member is invalid."
  actual:   "Member is invalid"
  verdict: mismatch; divergence_key service=marketplace-agreement|SearchAgreements|status=400->400|keys=$.fields[0].message

Source of the divergence in the checkout: services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state/failures.rs line 70 defines FIELD_INVALID = "Member is invalid" and line 75 defines FIELD_INVALID_CONSTRAINT = "Member is invalid."; RefusalKind::field_message (lines 114-121) maps RefusalKind::Invalid to the unpunctuated constant, and unsupported-filter refusals set fields[].message = FIELD_INVALID directly (line 523). The handler-side entry points are services/.../src/ops/list_agreement_payment_requests.rs line 177-182 (crate::state::validation_exception("INVALID_PARTY_TYPE", "partyType")) and the equivalent filter/sort/catalog refusals on SearchAgreements. Per-run values in the recorded bodies (requestId) were ignored by the comparison and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture corpus (trace set simparity-wire-aws-v1).
2. Cause 1: call ListAgreementPaymentRequests with a partyType that is a well-formed string matching ^[A-Za-z]+$ within 1..32 characters but that names neither of the two party roles (e.g. anything other than Proposer/Acceptor). Observe HTTP 400 ValidationException with reason INVALID_PARTY_TYPE and fields[0].name = partyType, and compare fields[0].message: the emulator returns "Member is invalid" where AWS returns "Member is invalid.". This is fixture marketplace-agreement-validation-error-paths step task#1.
3. Cause 2: call SearchAgreements with a request that passes the modelled constraints on catalog/filters/sort/maxResults/nextToken but is refused by one of the service's own rules (an unknown or unsupported filter name, an unsupported filter combination, an unacceptable sortBy, or a catalog this service does not expose). Observe HTTP 400 ValidationException and compare fields[0].message: again "Member is invalid" instead of "Member is invalid.". This is fixture marketplace-agreement-validation-error-paths step task#3.
4. For contrast, trigger a refusal from the modelled constraint walk instead (e.g. a partyType of length 0 or containing a digit, violating @length/@pattern): that path already answers the punctuated "Member is invalid.", which shows the two texts are chosen by where the refusal was raised rather than by what AWS states.
5. After fixing, re-replay marketplace-agreement-validation-error-paths steps task#1 AND task#3 — both causes must pass.
