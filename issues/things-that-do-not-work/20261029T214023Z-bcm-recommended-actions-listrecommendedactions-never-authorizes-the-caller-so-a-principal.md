Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-recommended-actions.access-denied-without-permission-rejected bcm-recommended-actions.explicit-deny-overrides-broad-allow-rejected
Issue-Shape-Id: com.amazonaws.bcmrecommendedactions#AWSBillingAndCostManagementRecommendedActions
Issue-Key: bcm-recommended-actions.access-denied-without-permission-rejected|run#18|AssertionError|assert error is not None, "expected the permission-less role to be denied"
Issue-Key: bcm-recommended-actions.explicit-deny-overrides-broad-allow-rejected|_poll_for_denial#23|AssertionError|raise AssertionError(
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# ListRecommendedActions never authorizes the caller, so a principal with no permissions — and one carrying an explicit IAM Deny — is served the full inventory instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendedActions

## Description
The only operation this service binds is served to every signed caller regardless of the caller's IAM permissions. The modelled AccessDeniedException is never reached, so both the default-deny case and the explicit-Deny-beats-Allow case answer 200 with the account's recommended actions. Two causes, both in this operation's authorization step, bundled because they share one service and one operation.

Cause 1 of 2: a principal with zero permissions is not denied.
  AWS: Smithy: ListRecommendedActions declares error AccessDeniedException (errorKind client, httpError 403, awsQueryError code BCMRecommendedActionsAccessDenied); its doc is "You don't have sufficient access to perform this action." IAM is default-deny, so a role with no identity-based policy attached gets AccessDeniedException for bcm-recommended-actions:ListRecommendedActions. The message text is recorded behavior only; the recording shows IAM's standard no-identity-based-policy sentence: `User: <caller ARN> is not authorized to perform: bcm-recommended-actions:ListRecommendedActions because no identity-based policy allows the bcm-recommended-actions:ListRecommendedActions action`. Fixed text: the `User: ` prefix, ` is not authorized to perform: `, and ` because no identity-based policy allows the ` / ` action` frame. Variable parts taken from the request and state: the caller's ARN (for an assumed role, `arn:<partition>:sts::<account-id>:assumed-role/<role-name>/<session-name>`) and the service-qualified action name.
  Emulator: the call SUCCEEDS with HTTP 200 and the full `recommendedActions` list; no ClientError is raised at all, so the check's denial assertion never sees an error. The handler states this explicitly: "Authorization is deliberately unmodelled. The emulator verifies every request signature at its boundary and holds no per-principal policy, so this handler never evaluates permissions and the modelled AccessDeniedException (403) stays constructible but unreached."
  Why: a client or test that exercises least-privilege — verifying that a role it did NOT grant the action to cannot read Billing and Cost Management recommendations — gets a 200 and a populated list from the emulator and a 403 from AWS. Permission-boundary regressions pass against the emulator, and billing data that AWS treats as access-controlled is readable by any signed principal.
  Fix: the ListRecommendedActions handler's authorization step — evaluate the caller's identity-based policy (the IAM principal the signature resolves to, as the sibling bcm services' authorization does) before reading the store, and return the modelled AccessDeniedException (403) worded as IAM words a no-identity-based-policy denial.
  Traces: bcm-recommended-actions.access-denied-without-permission-rejected. Code: services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/ops/list_recommended_actions.rs:28-32 (the "Authorization is deliberately unmodelled" contract).

Cause 2 of 2: an explicit IAM Deny does not override a broad Allow.
  AWS: Smithy: same AccessDeniedException (403). IAM policy evaluation is explicit-Deny-wins: a role carrying a broad Allow on `bcm-recommended-actions:*` plus an inline explicit Deny on `bcm-recommended-actions:ListRecommendedActions` is refused with AccessDeniedException for that one action, while unrelated actions on the same credentials (sts:GetCallerIdentity) still succeed; deleting the Deny restores the call. Message text is recorded behavior only — IAM's explicit-deny wording, with the caller ARN and the service-qualified action name taken from the request.
  Emulator: the same role keeps getting HTTP 200 with the inventory for the whole 30s enforcement window the check polls; the explicit Deny is never enforced because no policy is consulted on any code path.
  Why: explicit Deny is the mechanism operators rely on to carve an action out of a broad grant. Against the emulator that carve-out is a no-op, so a guardrail that is verified in staging and relied on in production silently does nothing — and the failure is in the *permissive* direction, which no client-side check catches.
  Fix: same component as cause 1 — the handler's authorization step must evaluate Deny statements with precedence over Allow, not merely check for the presence of an Allow.
  Traces: bcm-recommended-actions.explicit-deny-overrides-broad-allow-rejected.

Note: the sibling bcm services carry the same class of finding as separate open issues (e.g. 20261024T164554Z-bcm-dashboards-createdashboard-error-response, 20261024T164554Z-bcm-dashboards-getdashboard-error-response); none of them covers this service or this operation.

## Evidence
Behavioral replay simparity-behavioral-aws-v1, service bcm-recommended-actions, operation ListRecommendedActions.

Cause 1 — bcm-recommended-actions.access-denied-without-permission-rejected [mismatch]:
  The check creates an IAM role with a trust policy only and NO permission policy, assumes it via sts:AssumeRole, re-signs a bcm-recommended-actions client with those temporary credentials against the same emulator endpoint, and calls list_recommended_actions in a retry loop (tolerating only UnrecognizedClientException while the credentials propagate).
  AssertionError: expected the permission-less role to be denied — i.e. after 15 attempts the call never raised a ClientError; `error is None`.
  Expected: Error.Code == "AccessDeniedException" with Message "User: arn:<partition>:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: bcm-recommended-actions:ListRecommendedActions because no identity-based policy allows the bcm-recommended-actions:ListRecommendedActions action" (the ARN and session name are this run's request values; the sentence frame and the action name are the requirement).
  Actual: HTTP 200, a successful ListRecommendedActions response.
  Positive control in the same check: the fully-permissioned admin client's call also succeeds — so the two principals are indistinguishable.

Cause 2 — bcm-recommended-actions.explicit-deny-overrides-broad-allow-rejected [mismatch]:
  The check uses ONE role throughout: ReadOnlyAccess plus an inline Allow on bcm-recommended-actions:*, captures a successful ListRecommendedActions as a pre-deny positive control, then adds an inline explicit Deny on ListRecommendedActions and polls for the denial.
  AssertionError: an explicit Deny on ListRecommendedActions was never enforced within 30.0s; last observation: HTTPStatusCode 200 with the full inventory —
  {'recommendedActions': [{'id': 'ra-savings-opportunities', 'type': 'REVIEW_SAVINGS_OPPORTUNITY_RECOMMENDATIONS', 'severity': 'INFO', 'feature': 'COST_OPTIMIZATION_HUB', ...}, {'id': 'ra-expiring-reserved-instances', ...}, {'id': 'ra-expiring-savings-plans', ...}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected: AccessDeniedException (403) for that one action on that role, with sts:GetCallerIdentity on the same credentials still succeeding.
  (The ids, account id 000000000000, timestamps and the 30s window are this run's values; the requirement is that an explicit Deny is enforced and refused with the modelled AccessDeniedException.)

Source corroboration, not inference: src/ops/list_recommended_actions.rs:28-32 documents that this handler never evaluates permissions and that AccessDeniedException is "constructible but unreached".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Cause 1: create an IAM role whose only policy is a trust policy allowing the account root to assume it (no permission policy at all). sts:AssumeRole into it, build a bcm-recommended-actions client signed with the returned temporary credentials against the emulator endpoint, and call ListRecommendedActions. The emulator answers 200 with the inventory; AWS answers 403 AccessDeniedException with the no-identity-based-policy message.
3. Cause 2: create a role with ReadOnlyAccess plus an inline Allow on bcm-recommended-actions:*, confirm ListRecommendedActions succeeds on its credentials, then attach an inline policy with an explicit Deny on bcm-recommended-actions:ListRecommendedActions and poll the same call for up to 30s. The emulator keeps answering 200; AWS refuses with AccessDeniedException while sts:GetCallerIdentity on the same credentials still succeeds.
4. Delete the Deny and re-call to confirm the pre/post-deny difference the check relies on: against the emulator there is no difference at any point.
