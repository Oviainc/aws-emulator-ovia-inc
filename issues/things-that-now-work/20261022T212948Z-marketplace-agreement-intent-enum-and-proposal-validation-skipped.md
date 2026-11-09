Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-agreement.create-agreement-request-invalid-intent-rejected marketplace-agreement.describe-agreement-missing-permission-denied marketplace-agreement.update-purchase-orders-exceeds-max-entries-rejected
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement.create-agreement-request-invalid-intent-rejected|_error_code#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: marketplace-agreement.describe-agreement-missing-permission-denied|_poll_until_code#23|Timeout|time.sleep(interval_s)
Issue-Key: marketplace-agreement.update-purchase-orders-exceeds-max-entries-rejected|run#9|AssertionError|assert control_code == "ValidationException", f"expected ValidationException, got {control_code!r}"
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# marketplace-agreement: intent enum and proposal validation skipped on CreateAgreementRequest, no IAM evaluation on DescribeAgreement, UpdatePurchaseOrders answers NotImplemented (4 causes)

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgreementRequest

## Description
Four distinct root causes in the marketplace-agreement (AWSMPCommerceService_v20200301, awsJson1_0) emulator, all grounded in failing behavioral checks. Model consulted: services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/smithy-model/com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301.json.

=== Cause 1 of 4: CreateAgreementRequest accepts an `intent` value outside the modeled enum and mints a quote ===
AWS: Smithy: CreateAgreementRequest.intent targets enum `Intent` (`com.amazonaws.marketplaceagreement#Intent`) with exactly the values NEW | AMEND | REPLACE, and is @required. A value outside that set does not satisfy the modeled constraint, so the request is refused before any quote is produced; the service's common errors include ValidationException (HTTP 400, "The input fails to satisfy the constraints specified by the service."). Recorded behavior: the rejection is ValidationException with message "Provided intent is invalid." (message shown as an example; the requirement is the 400/ValidationException classification for an unmodeled enum value).
Emulator: a CreateAgreementRequest carrying intent="SUPERSEDE_EVERYTHING" (and no agreementProposalIdentifier) SUCCEEDS with HTTP 200 and returns a minted quote, e.g. {'agreementRequestId': 'ar-0000000000000001', 'chargeSummary': {'newAgreementValue': '0'}}. So the enum membership of `intent` is not enforced at the boundary, and the handler's own cross-member rule (agreementProposalIdentifier is required when intent is not AMEND, src/ops/create_agreement_request.rs) is also not reached/applied for such a value — an unknown intent is treated as a legal intent and persisted into state.
Why: a client that mistypes or sends a newer/unsupported intent gets a 200 and a persisted agreement request whose intent is meaningless, instead of the 400 ValidationException real AWS returns. Negative-path tests pass against the emulator and fail against AWS, and downstream AcceptAgreementRequest operates on a record AWS would never have created.
Fix: input validation for CreateAgreementRequest — the modeled enum constraint on `intent` must be enforced before the handler (boundary validator/generated INPUT_SCHEMA validation), and the handler's presence rules must apply to every non-AMEND intent.
Traces: marketplace-agreement.create-agreement-request-invalid-intent-rejected (first leg).

=== Cause 2 of 4: CreateAgreementRequest never checks that agreementProposalIdentifier names an active proposal ===
AWS: Smithy: CreateAgreementRequest.agreementProposalIdentifier targets AgreementProposalId (len 1..64, pattern ^(at-|ap-)[A-Za-z0-9]+$) and documents "The agreement proposal signed by the proposer ... required if the intent is not AMEND"; CreateAgreementRequest models ValidationException (400) and ResourceNotFoundException. Recorded behavior: a well-formed but non-existent/inactive proposal id with intent=NEW is refused with ValidationException, message example "Provided agreement proposal is inactive." — i.e. the proposal must resolve to a real, active proposal before a quote is minted. The requirement is that a request naming a proposal the service cannot resolve is refused with 400/ValidationException rather than succeeding.
Emulator: src/ops/create_agreement_request.rs validates only presence/absence rules for agreementProposalIdentifier and sourceAgreementIdentifier plus idempotency and quota; there is no proposal resolution at all, so any pattern-conforming `ap-`/`at-` string is accepted and a quote is minted. The check's positive-control leg (intent=NEW with agreementProposalIdentifier="ap-sim2realnonexistentproposal") is the leg that exercises this; it is currently unreachable because Cause 1 fails first.
Why: the whole point of the quote flow is that a request references a signed proposal. A client gets back an agreementRequestId derived from nothing, and the error class that tells it the proposal is gone/inactive never appears.
Fix: CreateAgreementRequest handler — refuse an agreementProposalIdentifier the service holds no active proposal for with the modeled ValidationException (HTTP 400), consistent with crate::state::validation_exception.
Traces: marketplace-agreement.create-agreement-request-invalid-intent-rejected (positive-control leg).

=== Cause 3 of 4: DescribeAgreement performs no IAM authorization evaluation — an explicit Deny is ignored ===
AWS: Smithy: DescribeAgreement models AccessDeniedException (HTTP 403, "User does not have sufficient access to perform this action"), and AccessDeniedException is also a service-common error; authSchemes: sigv4. AWS evaluates identity-based policy BEFORE resource existence: with an explicit Deny on aws-marketplace:DescribeAgreement, the identical call on a NON-EXISTENT agreementId returns AccessDeniedException (403), not ResourceNotFoundException; removing the Deny returns the identical call to the not-found outcome. Recorded message is built from the request (the assumed-role ARN and the denied action) and is an example of that template, not a literal requirement; the requirement is the 403/AccessDeniedException classification and its precedence over the not-found lookup.
Emulator: src/ops/describe_agreement.rs does no authorization whatsoever (`let _ = ctx;`, then a direct store lookup), so after an explicit IAM Deny is attached to the calling role the identical call keeps answering ResourceNotFoundException ("Provided agreement does not exist."). The check polls for the AccessDeniedException that never arrives and the check times out after 60s. Note the pre-Deny and post-restore legs do match, so the divergence is isolated to the Deny leg.
Why: any client or test that asserts least-privilege behavior — that a role without (or explicitly denied) aws-marketplace:DescribeAgreement cannot read agreements — sees the call succeed/return not-found on the emulator. Authorization regressions are invisible, and the 403-before-404 ordering AWS guarantees is inverted.
Fix: authorization for the marketplace-agreement operations (DescribeAgreement first) — evaluate the caller's identity-based policy for aws-marketplace:<action> before the state lookup and return the modeled AccessDeniedException at HTTP 403.
Traces: marketplace-agreement.describe-agreement-missing-permission-denied.

=== Cause 4 of 4: UpdatePurchaseOrders is an unimplemented stub answering NotImplemented ===
AWS: Smithy: UpdatePurchaseOrders(purchaseOrders: PurchaseOrders @required, @length min 1 max 86) -> UpdatePurchaseOrdersOutput, errors ConflictException(409), ResourceNotFoundException(404), ValidationException(400), AccessDeniedException(403), ThrottlingException(429), InternalServerException(500). NotImplemented is not a modeled error for this operation and no AWS status maps to it. Recorded behavior: a request at the 86-entry maximum clears the size check and is then refused with ValidationException (message example "This action is not supported." for an account not provisioned for purchase-order association) — i.e. a size-compliant request is answered with a MODELED outcome.
Emulator: the boundary correctly refuses 87 entries with ValidationException, but a size-compliant 86-entry request reaches the handler, which is still the generated scaffold stub: src/ops/update_purchase_orders.rs:160 returns `crate::scaffolded::Scaffolded::not_implemented("UpdatePurchaseOrders")`, so the client receives error code 'NotImplemented' (501) instead of any modeled response or error.
Why: a client cannot distinguish "refused" from "unsupported"; botocore surfaces an unmodeled error code so error handling keyed on ValidationException/ConflictException/ResourceNotFoundException never fires, and purchase-order association is unusable end to end.
Fix: implement the UpdatePurchaseOrders handler (its own op file) over the agreement/charge records in crate::state, returning the modeled output or a modeled refusal; remove the scaffold NotImplemented path.
Traces: marketplace-agreement.update-purchase-orders-exceeds-max-entries-rejected (control leg at the 86-entry maximum).

## Evidence
Suite: 13/17 passed; marketplace-agreement 13/17 (76%), 4 failed.

[mismatch] marketplace-agreement.create-agreement-request-invalid-intent-rejected
  Expected: ClientError ('ValidationException', 'Provided intent is invalid.') for intent='SUPERSEDE_EVERYTHING'.
  Actual: HTTP 200 — AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'agreementRequestId': 'ar-0000000000000001', 'chargeSummary': {'newAgreementValue': '0'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.
  Second leg (unreached): intent='NEW' + agreementProposalIdentifier='ap-sim2realnonexistentproposal' must give ('ValidationException', 'Provided agreement proposal is inactive.').
  Model: Intent enum values = [NEW, AMEND, REPLACE]; intent @required.

[mismatch] marketplace-agreement.describe-agreement-missing-permission-denied
  Expected after attaching an explicit IAM Deny on aws-marketplace:DescribeAgreement to the calling role: AccessDeniedException (HTTP 403) on the identical DescribeAgreement call for a nonexistent agreementId; pre-Deny and post-restore legs expect ResourceNotFoundException ('Provided agreement does not exist.').
  Actual: 'Check timed out after 60.0s' — the Deny leg polls for AccessDeniedException which never arrives; the emulator keeps returning the not-found outcome. Source confirms no authorization step exists: services/.../src/ops/describe_agreement.rs lines 176-197 (`let _ = ctx;` then a plain store lookup).

[mismatch] marketplace-agreement.update-purchase-orders-exceeds-max-entries-rejected
  87-entry purchaseOrders list: correctly refused with ValidationException (passes).
  86-entry (at-max) list: AssertionError: expected ValidationException, got 'NotImplemented'.
  Source: services/.../src/ops/update_purchase_orders.rs:160 -> Err(crate::scaffolded::Scaffolded::not_implemented("UpdatePurchaseOrders")).
  Model: PurchaseOrders @length min 1 max 86; UpdatePurchaseOrders models no NotImplemented error.

## Steps to Reproduce
All calls against the emulator's marketplace-agreement endpoint in us-east-1 (the service answers only there).

Cause 1 — enum not enforced:
  mpa = boto3.client('marketplace-agreement', region_name='us-east-1')
  mpa.create_agreement_request(intent='SUPERSEDE_EVERYTHING', requestedTerms=[{'id': 'term-1'}])
  Observed: HTTP 200 with an agreementRequestId. Expected: ClientError ValidationException (HTTP 400).

Cause 2 — proposal not resolved:
  mpa.create_agreement_request(intent='NEW', requestedTerms=[{'id': 'term-1'}], agreementProposalIdentifier='ap-sim2realnonexistentproposal')
  Expected: ClientError ValidationException (HTTP 400) because the proposal does not resolve to an active proposal. Observed on AWS: message 'Provided agreement proposal is inactive.'

Cause 3 — IAM Deny ignored:
  1. iam.create_role(...) with a trust policy allowing the account root; put_role_policy with Allow on aws-marketplace:DescribeAgreement.
  2. sts.assume_role into that role; build a marketplace-agreement client (us-east-1) from the session credentials.
  3. as_role.describe_agreement(agreementId='no-such-agreement') -> ResourceNotFoundException (matches).
  4. iam.put_role_policy with an explicit {'Effect':'Deny','Action':'aws-marketplace:DescribeAgreement','Resource':'*'}.
  5. Repeat step 3 on the SAME session: expected AccessDeniedException (HTTP 403); observed ResourceNotFoundException indefinitely (check times out at 60s).
  6. Delete the Deny policy: expected the call to return to ResourceNotFoundException (restore leg).

Cause 4 — UpdatePurchaseOrders stub:
  entries = [{'agreementId': f'agreement-{i}', 'chargeId': f'charge-{i}', 'chargeRevision': 1} for i in range(86)]
  mpa.update_purchase_orders(purchaseOrders=entries)
  Observed: error code 'NotImplemented' (HTTP 501). Expected: a modeled outcome — ValidationException/ConflictException/ResourceNotFoundException or a successful UpdatePurchaseOrdersOutput.
  With range(87): correctly refused with ValidationException today.

## Resolution

* **Cause 1 (intent enum) — already enforced when this was fixed.** A value outside
  `Intent` is refused 400 `ValidationException` (`INVALID_INTENT`, "Provided intent is
  invalid.", one `fields` entry naming `intent`) at the boundary, before the handler's
  cross-member rules. Pinned over the class by
  `tests/all/intent_enum_values.rs`: every declared value served, seven values outside
  the enum refused.
* **Cause 3 (no IAM evaluation) — fixed.** `src/service/authorization.rs` evaluates the
  calling principal's identity-based policies (capped by its permissions boundary) for
  `aws-marketplace:<OperationName>` through the tree's one evaluation
  (`service_common::iam_authorization`), and `src/service.rs` takes that decision in front
  of EVERY one of the 25 operations, before the request's input is read and before the
  store is touched — so the modelled `AccessDeniedException` (403) now precedes the
  not-found lookup. Pinned by `tests/all/identity_authorization.rs` (documents x operation
  classes, the 403-before-lookup precedence, a refused call changing nothing, and the
  account's own credential staying unbounded). The recorded check
  `describe-agreement-missing-permission-denied` still reports a divergence at its deny
  leg: the emulator answers it 403 `AccessDeniedException` with IAM's sentence over every
  character the check's report discloses, and the rest of the expected value is not
  disclosed, so it could not be reconciled from the replay.
* **Cause 4 (UpdatePurchaseOrders) — already implemented when this was fixed.** The
  handler answers the modelled `ValidationException` carrying the published
  `UNSUPPORTED_ACTION` reason; the scaffold path is gone.
* **Cause 2 (agreementProposalIdentifier is not resolved) — NOT fixed, declined here.**
  A well-formed identifier that names no proposal still mints a quote. No component of
  this emulator publishes an agreement proposal this API can resolve: the component that
  owns offers and mints the identifier is marketplace-discovery
  (`GetOffer$agreementProposalId`, "An encoded string to be passed by the acceptor of the
  terms when creating an agreement"), and the identifiers its published catalogue carries
  (`ap-0001-analytics-annual`) do not satisfy this API's `AgreementProposalId`
  `@pattern ^(at-|ap-)[A-Za-z0-9]+$`, so this service's boundary refuses them before any
  handler. Refusing every unresolvable identifier without first minting resolvable ones in
  the owning service would make the whole quote flow (create -> accept -> agreement)
  unreachable, so this cause needs that two-service change and is still open.
