Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the Marketplace Agreement (AWSMPCommerceService) emulator

_Filed: 2026-10-21T22:08:17Z_
_Operation: service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301 | Class: stub_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). No client workflow can run.

Implement a FIRST, SMALL slice: the agreement-request -> agreement lifecycle, statefully, over the `awsJson1_0` wire protocol (`POST /`, `X-Amz-Target: AWSMPCommerceService_v20200301.<OperationName>`).

### Happy path (in order)

1. **CreateAgreementRequest** — input `intent` (Intent: AMEND|NEW|REPLACE, required), `requestedTerms` (list<RequestedTerm>, 1..30, required), optional `clientToken` (@idempotencyToken), `sourceAgreementIdentifier`, `agreementProposalIdentifier`, `taxConfiguration`. Must mint and PERSIST a new agreement request keyed by a generated `agreementRequestId` matching `^ar-[A-Za-z0-9]+$`, storing the submitted `requestedTerms`, `intent` and tax configuration. Output `agreementRequestId` plus a `chargeSummary` computed from the stored requested terms (not a canned value). The same `clientToken` replayed must return the SAME `agreementRequestId` rather than creating a second request.
2. **AcceptAgreementRequest** — input `agreementRequestId` (required, `^ar-[A-Za-z0-9]+$`), optional `purchaseOrders` (list<PurchaseOrder>, 1..86). Must look the stored request up; unknown id -> `ResourceNotFoundException` (404), already-accepted id -> `ConflictException` (409). On success mint and persist an Agreement whose accepted terms are derived from the request's `requestedTerms`, set `status = ACTIVE`, set `acceptanceTime`/`startTime` from the clock, set `initialAgreementId` to its own `agreementId` (for `intent = NEW`), attach the supplied `purchaseOrders`, and return the new `agreementId` (`^[A-Za-z0-9_/-]+$`).
3. **DescribeAgreement** — input `agreementId` (required). Must return the PERSISTED agreement: `agreementId`, `acceptor`, `proposer`, `startTime`, `endTime`, `acceptanceTime`, `agreementType`, `estimatedCharges`, `proposalSummary`, `status`, `initialAgreementId`, `endTimeBehavior`. Unknown id -> `ResourceNotFoundException` (404). Every field must come from what step 2 stored, never echoed from the request or hardcoded.
4. **GetAgreementTerms** — input `agreementId` (required), `maxResults` (1..50), `nextToken` (len ..8192, `^[a-zA-Z0-9+/=_-]+$`). Must return `acceptedTerms` (list<AcceptedTerm>) derived from the `requestedTerms` accepted in step 2, honouring `maxResults` and returning/consuming a real `nextToken` (omitted on the last page). Unknown agreement -> `ResourceNotFoundException`.
5. **SearchAgreements** — input `catalog`, `filters` (list<Filter>, 1..10), `sort`, `maxResults` (1..50), `nextToken`. Must return `agreementViewSummaries` built by scanning the stored agreements, applying the `filters` (at minimum `AgreementId` / `Status` / `PartyType`-style filters) and `maxResults`/`nextToken` paging. An agreement created in step 2 must be findable here; a cancelled one must reflect its new status.
6. **CancelAgreement** — input `agreementId` (required). Must flip the stored agreement's `status` to `CANCELLED` (empty output body). Unknown id -> `ResourceNotFoundException` (404); an agreement that is not `ACTIVE` -> `ConflictException` (409). A subsequent `DescribeAgreement` must report `CANCELLED`, proving the write persisted.

### Requirements

- NEVER STUBS: each of the six must compute its response from the persisted store. Hardcoded bodies, echoing the input back, or minting an id without storing the object are failures.
- Ids must match the modeled patterns: agreement requests `^ar-[A-Za-z0-9]+$`, agreements `^[A-Za-z0-9_/-]+$`.
- Modeled errors must use the right shape and status: `ValidationException` 400, `AccessDeniedException` 403, `ResourceNotFoundException` 404, `ConflictException` 409, `ThrottlingException` 429, `InternalServerException` 500 (`ServiceQuotaExceededException` 402 on CreateAgreementRequest).
- Pagination tokens must be opaque, pattern-valid and resumable, not a fixed string.
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line in each of the six handler files as they are implemented.

## State Model

In `ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State`:

- `agreement_requests: HashMap<String /* agreementRequestId "ar-..." */, AgreementRequest>` where `AgreementRequest { id, intent, requested_terms: Vec<RequestedTerm>, source_agreement_identifier: Option<String>, agreement_proposal_identifier: Option<String>, tax_configuration: Option<TaxConfiguration>, charge_summary: ChargeSummary, client_token: Option<String>, accepted_agreement_id: Option<String>, created_at }`
- `agreements: HashMap<String /* agreementId */, Agreement>` where `Agreement { id, initial_agreement_id, agreement_request_id, agreement_type: String, acceptor: Acceptor, proposer: Proposer, status: AgreementStatus, start_time, end_time: Option<_>, acceptance_time, end_time_behavior: Option<EndTimeBehavior>, estimated_charges: EstimatedCharges, proposal_summary: ProposalSummary, accepted_terms: Vec<AcceptedTerm>, purchase_orders: Vec<PurchaseOrder>, updated_at }`
- `idempotency: HashMap<String /* clientToken */, String /* agreementRequestId */>` so a replayed `CreateAgreementRequest` returns the original id.
- `agreement_order: Vec<String>` — creation-ordered agreement ids, giving `SearchAgreements` a stable scan order for pagination.
- `page_tokens: HashMap<String /* nextToken */, usize /* offset */>` (or an encoded offset) shared by `GetAgreementTerms` and `SearchAgreements`.

The remaining 19 operations (payment requests, cancellation requests, billing adjustments, invoice line items, charges, entitlements, UpdatePurchaseOrders) stay 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md (crate root) lists all 25 operations with their input/output members and errors; the six chosen ops and every field named above are quoted from it (CreateAgreementRequest lines 68-72, AcceptAgreementRequest 38-42, DescribeAgreement 74-78, GetAgreementTerms 99-104, SearchAgreements 159-164, CancelAgreement 50-54). README.md documents the `awsJson1_0` / `X-Amz-Target: AWSMPCommerceService_v20200301.<Op>` wire protocol and the `//! SCAFFOLDED, NOT IMPLEMENTED:` stub census marker. `src/ops/describe_agreement.rs:4` carries that marker and line 196 returns `Err(crate::scaffolded::Scaffolded::not_implemented("DescribeAgreement"))`; all 25 handler files are identical scaffolds. `src/state.rs:8` is `// TODO: add service-specific state fields`, so no state exists at all.

## Steps to Reproduce

1. Start the emulator and point an `aws-marketplace`-signing client (SDK id `Marketplace Agreement`) at it.
2. Call CreateAgreementRequest with intent=NEW, one requestedTerms entry and clientToken=tok-1 — expect 200 with an `agreementRequestId` matching `^ar-[A-Za-z0-9]+$` and a chargeSummary; today: HTTP 501 NotImplemented.
3. Repeat the identical CreateAgreementRequest with clientToken=tok-1 — expect the SAME agreementRequestId (idempotent replay), not a new one.
4. Call AcceptAgreementRequest with that agreementRequestId and one purchaseOrders entry — expect 200 with a new `agreementId`.
5. Call AcceptAgreementRequest again with the same agreementRequestId — expect ConflictException (409).
6. Call DescribeAgreement with the returned agreementId — expect status=ACTIVE, acceptanceTime/startTime set, initialAgreementId == agreementId, and the acceptor/proposer/estimatedCharges stored at acceptance time.
7. Call GetAgreementTerms with that agreementId and maxResults=1 — expect one acceptedTerm derived from the requested terms plus a pattern-valid nextToken; pass the token back and expect the next page and no token on the final page.
8. Call SearchAgreements with a filter selecting that agreementId — expect exactly one agreementViewSummary describing the created agreement.
9. Call CancelAgreement with that agreementId — expect 200 with an empty body, then DescribeAgreement must report status=CANCELLED and SearchAgreements must reflect it.
10. Call DescribeAgreement and CancelAgreement with an unknown agreementId — expect ResourceNotFoundException (404), not 501 and not 200.
