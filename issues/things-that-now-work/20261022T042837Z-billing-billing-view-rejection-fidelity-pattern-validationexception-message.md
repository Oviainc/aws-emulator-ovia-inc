Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billing.get-billing-view-invalid-arn-pattern-rejected billing.delete-billing-view-primary-rejected billing.get-enterprise-support-charge-summary-malformed-month-rejected billing.get-billing-view-cross-account-arn-access-denied-rejected billing.create-billing-view-non-management-account-deterministic-rejection billing.list-billing-views-filters-by-arn billing.update-billing-view-nonexistent-arn-rejected
Issue-Shape-Id: com.amazonaws.billing#AWSBilling
Issue-Key: billing.get-billing-view-invalid-arn-pattern-rejected|run#7|AssertionError|assert message == _ARN_PATTERN_MESSAGE, message
Issue-Key: billing.delete-billing-view-primary-rejected|run#5|AssertionError|assert fmt_message == _ARN_PATTERN_MESSAGE, fmt_message
Issue-Key: billing.get-enterprise-support-charge-summary-malformed-month-rejected|run#4|AssertionError|assert message == _MONTH_PATTERN_MESSAGE, message
Issue-Key: billing.get-billing-view-cross-account-arn-access-denied-rejected|run#13|AssertionError|assert code == "AccessDeniedException", (
Issue-Key: billing.create-billing-view-non-management-account-deterministic-rejection|run#8|AssertionError|assert len(successes) == 0, f"expected zero successes from a non-management account, got: {results}"
Issue-Key: billing.list-billing-views-filters-by-arn|_error#9|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: billing.update-billing-view-nonexistent-arn-rejected|run#9|AssertionError|assert code == "ValidationException", code
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# Billing: billing-view rejection fidelity — pattern-ValidationException message rule, cross-account ARN returns 404 instead of AccessDenied, CreateBillingView not gated to the management account, ListBillingViews arns filter silently drops unknown ARNs, UpdateBillingView uses the wrong rejection class

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBillingView

## Description
Five distinct divergences in the AWS Billing (awsbilling) emulator's rejection behaviour for billing views. All five are in the `billing` service. This issue bundles FIVE root causes; each must be closed before submitting.

Cause 1 of 5: the shared pattern-constraint ValidationException message is rendered by the wrong rule — it echoes the offending value and prints the regex with its ^/$ anchors.
  AWS: Smithy model declares the @pattern constraints the message is built from: GetBillingViewRequest.arn / DeleteBillingViewRequest.arn / UpdateBillingViewRequest.arn target BillingViewArn with pattern ^arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/[a-zA-Z0-9/:_\+=\.\-@]{0,75}[a-zA-Z0-9]$, and GetEnterpriseSupportChargeSummaryRequest.billingMonth targets EnterpriseSupportBillingMonth with pattern ^\d{4}-(0[1-9]|1[0-2])$. The rejection is HTTP 400 `ValidationException` (modeled, httpError 400) and its message is BUILT FROM THE REQUEST, so the rule is a requirement, not the recorded text. The rule AWS applies: "<n> validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern WITHOUT the leading ^ and trailing $>" — the offending VALUE is not echoed, and the anchors are stripped.
  Emulator: the boundary validator applies a different rule — it interpolates the rejected value and keeps the anchors, producing e.g. "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/[a-zA-Z0-9/:_\+=\.\-@]{0,75}[a-zA-Z0-9]$" and "Value '2024-13' at 'billingMonth' ... pattern: ^\d{4}-(0[1-9]|1[0-2])$". Code and status are correct; only the message-construction rule diverges.
  Why: two concrete breakages. (a) Echoing the request value into a 400 body leaks caller-supplied input back out for every constrained member of every billing operation, including members the model marks @sensitive (BillingViewName, BillingViewDescription) once those are validated the same way. (b) Clients and tests that parse the pattern out of the message to re-validate locally receive an anchored regex AWS never sends, so the extracted pattern does not match what AWS documents.
  Fix: the shared request-boundary constraint validator that renders pattern-constraint ValidationException messages for this service (the deserialize→validate stage ahead of every operation's handler, which services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_view.rs line 147 documents as "refused by the boundary validator before this runs"). Apply the AWS rule for every @pattern member, not per operation.
  Traces: billing.get-billing-view-invalid-arn-pattern-rejected, billing.delete-billing-view-primary-rejected, billing.get-enterprise-support-charge-summary-malformed-month-rejected.

Cause 2 of 5: GetBillingView leaks existence information for a cross-account ARN — it answers ResourceNotFoundException (404) instead of AccessDeniedException (403).
  AWS: Smithy model binds GetBillingView with modeled errors AccessDeniedException (httpError 403, "You don't have sufficient access to perform this action") and ResourceNotFoundException (httpError 404). Recorded behavior: a well-formed BillingViewArn whose embedded account id is a FOREIGN account is refused with HTTP 403 `AccessDeniedException` — the account boundary is enforced before any existence lookup, so the service never reveals whether a view exists in another account. Recorded message example: "User: <caller arn> is not authorized to perform: billing:GetBillingView on resource: <arn> because no resource-based policy allows the billing:GetBillingView action". The requirement is the 403 + AccessDeniedException classification for any ARN not scoped to the caller's own account; the message text is an example (it is built from the caller's identity and the requested ARN).
  Emulator: the handler does a bare store lookup keyed on the ARN and maps a miss to ResourceNotFoundException, with no account-boundary check at all (services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_view.rs lines 157-163).
  Why: a caller probing foreign-account ARNs can distinguish "exists" from "does not exist" by the status code, which is exactly the enumeration AWS's 403 prevents; and a client branching on AccessDeniedException to prompt for cross-account permissions instead takes its not-found path.
  Fix: the operation's handler — check the ARN's embedded account id against the caller's account and return AccessDeniedException before the existence lookup. The same gate belongs on every operation that accepts a BillingViewArn/ResourceArn.
  Traces: billing.get-billing-view-cross-account-arn-access-denied-rejected.

Cause 3 of 5: CreateBillingView is not gated to the AWS Organizations management account — it succeeds from a non-management account and leaves real billing views behind.
  AWS: Smithy model binds CreateBillingView with the service-common ValidationException (httpError 400) alongside ConflictException/ResourceNotFoundException/ServiceQuotaExceededException/BillingViewHealthStatusException. Recorded behavior: from a standalone/member (non-management) account, CreateBillingView is ALWAYS refused with the identical, deterministic HTTP 400 `ValidationException` — never a success, and never a mix of error codes across concurrent attempts. The requirement is that every such call is refused with that one code and that no billing view is created.
  Emulator: five concurrent CreateBillingView calls with distinct valid names ALL returned HTTP 200 and minted ARNs (arn:aws:billing::000000000000:billingview/custom-1 … custom-5). The management-account gate is absent entirely.
  Why: this is an over-permissive accept — the emulator performs an operation real AWS refuses. A client or IaC tool validated against the emulator will believe member-account billing-view creation works, then fail in production; and downstream reads see views that cannot exist in a non-management account, so any test asserting "exactly one PRIMARY view / no custom views" is corrupted by the orphans left behind.
  Fix: the operation's handler and its validation — add the caller-is-management-account gate ahead of any state mutation.
  Traces: billing.create-billing-view-non-management-account-deterministic-rejection.

Cause 4 of 5: ListBillingViews silently drops an unknown ARN from the `arns` filter instead of rejecting the request.
  AWS: Smithy model binds ListBillingViews.arns to BillingViewArnList (list of BillingViewArn, length 0..10) and declares the service-common ValidationException (httpError 400). Recorded behavior: when `arns` names a well-formed billingview ARN that does not exist, the whole request is refused with HTTP 400 `ValidationException` (recorded message example: "[<arn>] is an invalid billing view arn") rather than returning a 200 whose result set simply omits it — the filter validates each requested ARN. The requirement is the 400 + ValidationException classification; the message text is an example built from the request.
  Emulator: the call SUCCEEDS with HTTP 200 and body {"billingViews": []}.
  Why: another over-permissive accept. A client cannot distinguish "that view does not exist" from "that view exists but matched no other filter", and — because a 200 with an empty list is indistinguishable from the filter never having been applied — a caller that mistypes an ARN gets silence instead of an error.
  Fix: the operation's handler and its validation — resolve and validate each entry of `arns` and refuse the request on the first unknown one.
  Traces: billing.list-billing-views-filters-by-arn.

Cause 5 of 5: UpdateBillingView uses the wrong rejection class for an ARN that is not an existing CUSTOM billing view — ResourceNotFoundException (404) instead of ValidationException (400).
  AWS: Smithy model binds UpdateBillingView with both ResourceNotFoundException (httpError 404) and the service-common ValidationException (httpError 400). Recorded behavior: AWS COLLAPSES "well-formed but nonexistent" and "exists but is the account's PRIMARY view" into one HTTP 400 `ValidationException` with one message (recorded example: "The Billing View ARN does not correspond to a Custom Billing View. Only Custom Billing Views may be edited."), i.e. eligibility is decided before existence. The requirement is the 400 + ValidationException classification for both inputs; the message text is an example.
  Emulator: a well-formed but nonexistent billingview ARN is refused with `ResourceNotFoundException`.
  Why: a client distinguishing "bad request, do not retry" (400) from "resource missing" (404) branches the wrong way; and because AWS deliberately does not reveal whether a non-CUSTOM ARN exists, the emulator's 404/400 split discloses existence that AWS hides.
  Fix: the operation's handler — classify any ARN that does not name an existing CUSTOM view as ValidationException, ahead of the store lookup.
  Traces: billing.update-billing-view-nonexistent-arn-rejected.

## Evidence
Replayed behavioral suite (simparity-behavioral-aws-v1), billing 3/19 passed (16%), 16 failed.

Cause 1 — pattern-ValidationException message rule (3 checks):
  billing.get-billing-view-invalid-arn-pattern-rejected
    Expected: "1 validation error detected: Value at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/[a-zA-Z0-9/:_\+=\.\-@]{0,75}[a-zA-Z0-9]"
    Actual:   "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/[a-zA-Z0-9/:_\+=\.\-@]{0,75}[a-zA-Z0-9]$"
  billing.delete-billing-view-primary-rejected
    Actual:   "1 validation error detected: Value 'not-an-arn-at-all' at 'arn' failed to satisfy constraint: ... pattern: ^arn:aws[a-z-]*:(billing)::...$"  (same two deltas: value echoed, anchors kept)
  billing.get-enterprise-support-charge-summary-malformed-month-rejected
    Expected: "1 validation error detected: Value at 'billingMonth' failed to satisfy constraint: Member must satisfy regular expression pattern: \d{4}-(0[1-9]|1[0-2])"
    Actual:   "1 validation error detected: Value '2024-13' at 'billingMonth' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{4}-(0[1-9]|1[0-2])$"

Cause 2 — cross-account ARN:
  billing.get-billing-view-cross-account-arn-access-denied-rejected
    Request: GetBillingView(arn="arn:aws:billing::111122223333:billingview/primary") (foreign account; caller's own PRIMARY resolves fine as positive control)
    Expected: AccessDeniedException (HTTP 403)
    Actual:   AssertionError: expected AccessDeniedException for a cross-account billing-view ARN, got 'ResourceNotFoundException' (HTTP 404)

Cause 3 — CreateBillingView management-account gate:
  billing.create-billing-view-non-management-account-deterministic-rejection
    Expected: all 5 concurrent CreateBillingView calls refused with the identical ValidationException; zero successes.
    Actual:   AssertionError: expected zero successes from a non-management account, got:
              [('ok','sim2real-behav-race-0-...','arn:aws:billing::000000000000:billingview/custom-1'),
               ('ok','sim2real-behav-race-1-...','arn:aws:billing::000000000000:billingview/custom-2'),
               ('ok','sim2real-behav-race-2-...','arn:aws:billing::000000000000:billingview/custom-5'),
               ('ok','sim2real-behav-race-3-...','arn:aws:billing::000000000000:billingview/custom-3'),
               ('ok','sim2real-behav-race-4-...','arn:aws:billing::000000000000:billingview/custom-4')]

Cause 4 — ListBillingViews arns filter:
  billing.list-billing-views-filters-by-arn
    Request: ListBillingViews(arns=["arn:aws:billing::<acct>:billingview/sim2real-behav-does-not-exist"])
    Expected: ValidationException (HTTP 400)
    Actual:   AssertionError: expected a rejection, but the call SUCCEEDED and returned {'billingViews': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 5 — UpdateBillingView rejection class:
  billing.update-billing-view-nonexistent-arn-rejected
    Request: UpdateBillingView(arn="arn:aws:billing::<acct>:billingview/sim2real-behav-does-not-exist", name=...)
    Expected: ValidationException (HTTP 400)
    Actual:   AssertionError: ResourceNotFoundException (HTTP 404)

Model citations all from services/service-com-amazonaws-billing-awsbilling/smithy-model/com-amazonaws-billing-awsbilling.json (sdkId Billing, protocol awsJson1_0). Handler citation: .../src/ops/get_billing_view.rs lines 147-163 (bare store lookup -> ResourceNotFoundException, no account-boundary check).

## Steps to Reproduce
Boot the emulator and point a botocore `billing` client at it.

Cause 1:
  client.get_billing_view(arn="not-an-arn") -> inspect Error.Message; it echoes 'not-an-arn' and prints the pattern with ^...$ anchors. Same for client.delete_billing_view(arn="not-an-arn-at-all") and client.get_enterprise_support_charge_summary(billingMonth="2024-13").
Cause 2:
  client.get_billing_view(arn="arn:aws:billing::111122223333:billingview/primary") -> observe ResourceNotFoundException/404; AWS answers AccessDeniedException/403. (Control: GetBillingView on the caller's own PRIMARY arn succeeds.)
Cause 3:
  primary = client.list_billing_views(billingViewTypes=["PRIMARY"])["billingViews"][0]["arn"]; then run 5 concurrent client.create_billing_view(name=<unique>, sourceViews=[primary]) -> observe 5 HTTP 200 responses minting arn:aws:billing::<acct>:billingview/custom-N; AWS refuses every one with ValidationException/400.
Cause 4:
  client.list_billing_views(arns=[f"arn:aws:billing::{account_id}:billingview/sim2real-behav-does-not-exist"]) -> observe HTTP 200 {'billingViews': []}; AWS answers ValidationException/400.
Cause 5:
  client.update_billing_view(arn=f"arn:aws:billing::{account_id}:billingview/sim2real-behav-does-not-exist", name="x") -> observe ResourceNotFoundException/404; AWS answers ValidationException/400 with the "does not correspond to a Custom Billing View" rule.

Re-replay billing.get-billing-view-invalid-arn-pattern-rejected, billing.delete-billing-view-primary-rejected, billing.get-enterprise-support-charge-summary-malformed-month-rejected, billing.get-billing-view-cross-account-arn-access-denied-rejected, billing.create-billing-view-non-management-account-deterministic-rejection, billing.list-billing-views-filters-by-arn and billing.update-billing-view-nonexistent-arn-rejected to confirm a fix.
