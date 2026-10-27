Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-dashboards.get-resource-policy-absent-returns-empty-document
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards.get-resource-policy-absent-returns-empty-document|run#12|AssertionError|assert message == "1 validation error detected: Value 'arn:aws:bcm-dashboards::123:dashboard/not-a-real-resource' at 'resourceArn' failed to satisfy constraint:
Issue-Commit: c5b96e9739dc539af7576810c0a886e2a487da76

# ValidationException @pattern message renders the regex with its ^/$ anchors, which AWS strips

> _Note: this issue was identified against the code base at commit `c5b96e9739dc539af7576810c0a886e2a487da76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
One root cause (1 of 1) in the bcm-dashboards constraint-violation message formatter: the regular-expression text embedded in a @pattern ValidationException message is emitted verbatim from the Smithy trait, including the leading `^` and trailing `$` anchors, whereas AWS renders the pattern with those two anchor characters removed.

=== Cause 1 of 1: @pattern constraint message includes the regex anchors ===

AWS: Smithy (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json) — GetResourcePolicy.resourceArn targets DashboardArn, @required, @length min 20 max 2048, @pattern `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$`; GetResourcePolicy models ValidationException at @httpError 400. A resourceArn that is well-formed in length but does not match the shape's pattern (e.g. an account-id field that is not 12 digits) is refused with code `ValidationException` at HTTP 400 and a request-derived message of the service's standard Coral constraint form:

  "1 validation error detected: Value '<the rejected value>' at '<member name>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the pattern, WITHOUT its ^ and $ anchors>"

Fixed text of the template: `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `. Variable parts taken from the request and the model: the rejected value (here the supplied resourceArn), the member name (`resourceArn`), and the pattern text from the shape's @pattern trait with the anchoring `^` and `$` stripped. The same anchor-stripped rendering is visible in the already-filed CreateDashboard evidence, where AWS's recorded message carries `(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]` for a DashboardName whose modeled @pattern is `^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$` — so this is the service-wide rendering rule, not a per-operation quirk.

Emulator: status 400 and code `ValidationException` are CORRECT, the value position and member position are CORRECT, but the pattern position is copied straight from the trait with the anchors intact: the message ends `...Member must satisfy regular expression pattern: ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$` instead of `...pattern: arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)`.

Why: the constraint message is deterministic and request-derived, so clients, test suites and error-mapping layers that compare or parse the pattern position out of a ValidationException see two extra characters that real AWS never sends. A client that round-trips the reported pattern back into its own regex engine to re-validate input, or that asserts on the message text, diverges from production. Because every @pattern-constrained member of this service shares one formatter, this affects all of them (DashboardArn, DashboardName, ClientToken, GenericString, …), not just GetResourcePolicy.resourceArn.

Fix: the constraint-violation message formatter in this service's request-validation layer — when rendering a @pattern violation, strip the anchoring `^` prefix and `$` suffix from the trait's pattern before interpolating it into the message, leaving the value and member positions as they are. Related but distinct open issues, which must NOT be treated as covering this: "bcm-dashboards: DashboardName pattern unenforced, constraint/not-found error messages diverge, …" (20261022T215638Z) — its cause 1 is that DashboardName's @pattern is not enforced at all and its cause 2 is the @length value-position placeholder, neither of which is the anchor rendering; and "bcm-dashboards state gaps: AWS_MANAGED catalog missing, …" (20261023T081740Z), whose cause 6 is the separate empty-policyDocument defect on this same operation.

Traces: bcm-dashboards.get-resource-policy-absent-returns-empty-document

Note: the rest of this check's contract (GetResourcePolicy on a policy-less dashboard succeeding with an empty-ish policyDocument, and the dashboard remaining readable) is asserted BEFORE the failing assertion and passed here; the policyDocument '' vs '{}' representation is already filed as cause 6 of 20261023T081740Z.

## Evidence
Suite simparity-behavioral-ga-prod-v1 — 11/12 passed (91.7%), the single failure is bcm-dashboards 11/12 (92%).

[mismatch] bcm-dashboards.get-resource-policy-absent-returns-empty-document, at check_bcm_dashboards_get_resource_policy_absent_returns_empty_document.py:42

Call: get_resource_policy(resourceArn="arn:aws:bcm-dashboards::123:dashboard/not-a-real-resource")  (length-valid, pattern-invalid: the account-id position is '123', not 12 digits)

Code: ValidationException — MATCHED.
HTTP status: 400 — MATCHED.

Expected message (AWS):
  1 validation error detected: Value 'arn:aws:bcm-dashboards::123:dashboard/not-a-real-resource' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)

Actual message (emulator):
  1 validation error detected: Value 'arn:aws:bcm-dashboards::123:dashboard/not-a-real-resource' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$

AssertionError: expected the exact ValidationException message AWS returns, got "... pattern: ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$"

Delta: exactly the two anchor characters `^` (immediately after "pattern: ") and `$` (at end of message). Everything else in the message — including the request-derived value position and the member name — is byte-identical.

Model corroboration (smithy_query, query=operation, GetResourcePolicy): input resourceArn -> DashboardArn, required, length 20..2048, pattern "^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$"; errors include ValidationException (client, httpError 400). The emulator's string is the trait value verbatim; AWS's is the trait value minus its anchors.

## Steps to Reproduce
Boot the emulator on fresh state and point a botocore client at the bcm-dashboards endpoint.

1. create_dashboard(name=<valid name>, widgets=[one cost-and-usage widget], resourceTags=[{"key":"env","value":"sim2real-behavioral"}]) and keep the returned arn.
2. get_resource_policy(resourceArn=arn) — succeeds (this part passes today).
3. get_resource_policy(resourceArn="arn:aws:bcm-dashboards::123:dashboard/not-a-real-resource") — a value that satisfies DashboardArn's @length but violates its @pattern (the account-id position has 3 digits, not 12).
4. Catch the ClientError and read err.response["Error"]["Message"].
   Expect the pattern position to be the shape's @pattern with its `^` and `$` removed: "...Member must satisfy regular expression pattern: arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)".
   Observe the anchors present: "...pattern: ^arn:aws[...]+)$".
5. The same rendering bug should be checked on any other @pattern-constrained member of the service once the formatter is fixed (e.g. CreateDashboard.name, CreateScheduledReport.clientToken).

Re-replay: bcm-dashboards.get-resource-policy-absent-returns-empty-document
