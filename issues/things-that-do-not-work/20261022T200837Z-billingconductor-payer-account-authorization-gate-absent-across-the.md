Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billingconductor.create-billing-group-requires-payer-account billingconductor.create-billing-group-invalid-input-still-payer-gated billingconductor.create-pricing-plan-nonexistent-pricing-rule-arns-still-payer-gated billingconductor.list-billing-groups-rules-and-account-associations-require-payer-account billingconductor.associate-and-disassociate-pricing-rules-require-payer-account billingconductor.list-and-associated-resource-read-ops-require-payer-account billingconductor.disassociate-accounts-requires-payer-account
Issue-Shape-Id: com.amazonaws.billingconductor#AWSBillingConductor
Issue-Key: billingconductor.create-billing-group-requires-payer-account|run#9|AssertionError|assert code == "AccessDeniedException", f"unexpected error code: {code} ({message})"
Issue-Key: billingconductor.create-billing-group-invalid-input-still-payer-gated|_error_info#4|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: billingconductor.create-pricing-plan-nonexistent-pricing-rule-arns-still-payer-gated|_error_info#8|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: billingconductor.list-billing-groups-rules-and-account-associations-require-payer-account|_error_info#4|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: billingconductor.associate-and-disassociate-pricing-rules-require-payer-account|run#12|AssertionError|assert all(code == "AccessDeniedException" for _, code in results_a), (
Issue-Key: billingconductor.list-and-associated-resource-read-ops-require-payer-account|run#5|AssertionError|assert code_a == "AccessDeniedException", f"ListCustomLineItems: expected AccessDeniedException, got {code_a}"
Issue-Key: billingconductor.disassociate-accounts-requires-payer-account|run#5|AssertionError|assert control_code == "NotFoundException", (
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# billingconductor payer-account authorization gate absent across the control plane (6 causes: creates/lists succeed or return NotImplemented instead of 403 AccessDeniedException; TagResource ARN pattern over-enforced)

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBillingGroup

## Description
This issue carries 6 causes. AWS Billing Conductor is a payer-account-only API: every operation in the service is gated on the caller being the AWS Organizations payer (management) account, and a non-payer caller is rejected with HTTP 403 AccessDeniedException whose Message is built from the request as "Account <caller-account-id> is not authorized to call <OperationName>. Only payer account is authorized" (that construction is the requirement; the recorded account id is environment-specific and an example only). The emulator implements no such gate anywhere, so every operation either succeeds for any caller or falls through to its scaffold stub. The pre-existing open issue 20261022T134058Z-billingconductor-listbillinggroups-returns-200-instead-of-403-accessdeniedexception-for.md covers ONLY the wire replay key for ListBillingGroups; the causes below are the behavioral surface it does not carry.

Cause 1 of 6: CreateBillingGroup is not payer-gated, and the gate must precede field-level validation.
AWS: Smithy — CreateBillingGroup (POST /create-billing-group, success code 200) models AccessDeniedException (errorKind client, httpError 403, members {Message: String @required}, doc "You do not have sufficient access to perform this action.") alongside ValidationException (400). Recorded behavior: from a non-payer account the call is rejected 403 AccessDeniedException, and it is rejected the same way when the body is semantically invalid (AccountGrouping.LinkedAccountIds empty and PrimaryAccountId absent) — the account-eligibility check runs BEFORE field/semantic validation, so ValidationException is never reached.
Emulator: answers HTTP 200 with a success body (e.g. {"Arn": "arn:aws:billingconductor::<acct>:billinggroup/<id>"}) for a well-formed request, and ALSO 200 for the semantically invalid request — no error code, no Message.
Why: a client that tests the payer precondition by catching AccessDeniedException sees success, creates phantom billing groups that cannot exist on AWS, and never exercises its authorization-failure path; validation-ordering assertions also invert.
Fix: the CreateBillingGroup handler's authorization gate, placed ahead of its input validation — preferably a service-wide caller-eligibility check in the billingconductor request dispatch path so all operations share it — plus the error serializer rendering the modeled AccessDeniedException as 403 with x-amzn-errortype: AccessDeniedException.
Traces: billingconductor.create-billing-group-requires-payer-account, billingconductor.create-billing-group-invalid-input-still-payer-gated.

Cause 2 of 6: CreatePricingPlan is not payer-gated, and the gate must precede referenced-resource-existence checking.
AWS: Smithy — CreatePricingPlan (POST /create-pricing-plan) models AccessDeniedException (403) and ResourceNotFoundException (404). Recorded behavior: from a non-payer account, a CreatePricingPlan naming a PricingRuleArns entry that does not exist is rejected 403 AccessDeniedException, not 404 ResourceNotFoundException, and no plan is created.
Emulator: the call SUCCEEDS (200) and a pricing plan is created, so neither the gate nor the reference check is applied.
Why: the caller cannot tell "not authorized" from "created"; state is mutated on a request real AWS refuses, leaving emulator state unreachable on AWS.
Fix: CreatePricingPlan handler authorization gate, ordered before the PricingRuleArns existence lookup.
Traces: billingconductor.create-pricing-plan-nonexistent-pricing-rule-arns-still-payer-gated.

Cause 3 of 6: read/list operations serve successful empty pages instead of the 403 gate.
AWS: Smithy — ListBillingGroups, ListPricingRules and ListAccountAssociations each model AccessDeniedException (errorKind client, httpError 403). Recorded behavior: even filter-less LIST calls are not exempt from the payer restriction; each returns 403 AccessDeniedException with the request-derived "Account <id> is not authorized to call <Op>. Only payer account is authorized" message.
Emulator: ListBillingGroups answers HTTP 200 with {"BillingGroups": []}; the sibling list reads are likewise ungated.
Why: "not authorized" is indistinguishable from "authorized, zero resources", so proof-of-absence and entitlement-probing logic silently takes the wrong branch, and no SDK auth-failure handling is triggered.
Fix: the same service-wide authorization gate applied to the read handlers (ListBillingGroups / ListPricingRules / ListAccountAssociations), ahead of the state read. Partly overlaps the open ListBillingGroups wire issue named above, which does not carry ListPricingRules or ListAccountAssociations.
Traces: billingconductor.list-billing-groups-rules-and-account-associations-require-payer-account.

Cause 4 of 6: AssociatePricingRules / DisassociatePricingRules answer NotImplemented instead of the 403 gate, and the gate must be a deterministic pre-check under concurrency.
AWS: Smithy — AssociatePricingRules (PUT /associate-pricing-rules) and DisassociatePricingRules model AccessDeniedException (403), ResourceNotFoundException (404) and ValidationException (400). Recorded behavior: from a non-payer account both are rejected 403 AccessDeniedException even when pointed at syntactically well-formed but nonexistent plan/rule ARNs, and the rejection is deterministic — many concurrent identical calls are ALL denied identically, none ever slipping through to a later validation stage.
Emulator: every one of 6 concurrent racers against each operation returns error code "NotImplemented" (the request reaches the unimplemented scaffold instead of an authorization pre-check), so the modeled error set is never produced.
Why: callers receive an unmodeled error code their error handling cannot classify (it is neither retryable-throttling nor an access denial), and no client can validate the payer precondition on the association surface.
Fix: the authorization gate must run in the dispatch path ahead of handler dispatch so these operations deny before reaching their scaffold; the association handlers then need the gate plus their modeled error mapping.
Traces: billingconductor.associate-and-disassociate-pricing-rules-require-payer-account.

Cause 5 of 6: custom-line-item read operations answer NotImplemented instead of the 403 gate.
AWS: Smithy — ListCustomLineItems, ListCustomLineItemVersions and ListResourcesAssociatedToCustomLineItem each model AccessDeniedException (errorKind client, httpError 403). Recorded behavior: from a non-payer account each is rejected 403 AccessDeniedException with the payer-account-specific message "Account <id> is not authorized to call <Op>. Only payer account is authorized" — notably NOT the generic IAM "no identity-based policy allows" wording, which is how a client tells a per-account entitlement gate apart from a missing permission.
Emulator: ListCustomLineItems returns error code "NotImplemented" (scaffold reached), so the check never gets to the sibling reads.
Why: the unmodeled NotImplemented code is unclassifiable by SDK error handling, and the entitlement signal AWS gives is absent.
Fix: dispatch-level authorization gate ahead of handler dispatch for the custom-line-item read operations. The separately filed scaffold issue for ListCustomLineItems covers only its missing implementation, not the gate ordering required here.
Traces: billingconductor.list-and-associated-resource-read-ops-require-payer-account.

Cause 6 of 6: TagResource rejects a well-formed-but-nonexistent billinggroup ARN with 400 ValidationException (modeled @pattern enforced) where AWS reports 404 NotFoundException, and DisassociateAccounts is not payer-gated.
AWS: Smithy — TagResource (POST /tags/{ResourceArn}) models ResourceNotFoundException (errorKind client, httpError 404) and ValidationException (400); its ResourceArn member carries @pattern "^arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/?[0-9]{12}$|...". Recorded behavior: TagResource against arn:aws:billingconductor::<acct>:billinggroup/<non-numeric-suffix> reaches resource-existence checking and is answered with error code NotFoundException, HTTP 404, message "Resource was not found" — i.e. AWS does not enforce that ARN @pattern as a request-rejecting constraint for this member. DisassociateAccounts (POST /disassociate-accounts) models AccessDeniedException (403) and, from a non-payer account, is rejected 403 AccessDeniedException even for a nonexistent billing-group ARN.
Emulator: TagResource returns HTTP 400 ValidationException whose message is the pattern-constraint text ("1 validation error detected: Value '...' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ..."), so the call is refused before existence checking; the DisassociateAccounts assertion is therefore never reached and that operation remains ungated.
Why: a client cannot distinguish "no such resource" (404, retriable after create) from "malformed request" (400, never retriable); tagging flows that tolerate 404 abort instead.
Fix: relax pre-handler @pattern enforcement on TagResource.ResourceArn to AWS's observed behavior (resolve the ARN and raise the 404 not-found error for an unknown resource) and add the payer-account gate to the DisassociateAccounts handler. Separate from the already-filed TagResource scaffold issue, which covers only the missing handler body.
Traces: billingconductor.disassociate-accounts-requires-payer-account.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: billingconductor 0/18 passed; 11 failures are ALREADY FILED scaffold blocks, the 7 below are net-new.

Cause 1 — billingconductor.create-billing-group-requires-payer-account: expected code AccessDeniedException, got code None, i.e. the call succeeded. billingconductor.create-billing-group-invalid-input-still-payer-gated: "expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Arn': 'arn:aws:billingconductor::000000000000:billinggroup/000000000001'}". Expected: HTTP 403, error code AccessDeniedException, Message of the form "Account <id> is not authorized to call CreateBillingGroup. Only payer account is authorized" (example text; the rule is request-derived). Actual: HTTP 200 success payload.

Cause 2 — billingconductor.create-pricing-plan-nonexistent-pricing-rule-arns-still-payer-gated: "expected a rejection, but the call SUCCEEDED". Expected HTTP 403 AccessDeniedException (NOT 404 ResourceNotFoundException); actual: 200 and a plan created.

Cause 3 — billingconductor.list-billing-groups-rules-and-account-associations-require-payer-account: "expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'BillingGroups': []}". Expected HTTP 403 AccessDeniedException on ListBillingGroups, ListPricingRules, ListAccountAssociations.

Cause 4 — billingconductor.associate-and-disassociate-pricing-rules-require-payer-account: "AssociatePricingRules: expected every racer denied with AccessDeniedException, got [('error','NotImplemented') x6]". Expected: all 6 concurrent racers denied identically with HTTP 403 AccessDeniedException, zero winners; actual: unmodeled error code NotImplemented from every racer.

Cause 5 — billingconductor.list-and-associated-resource-read-ops-require-payer-account: "ListCustomLineItems: expected AccessDeniedException, got NotImplemented". Expected HTTP 403 AccessDeniedException with the payer-account-specific message for ListCustomLineItems, ListCustomLineItemVersions, ListResourcesAssociatedToCustomLineItem.

Cause 6 — billingconductor.disassociate-accounts-requires-payer-account: "positive control TagResource: expected NotFoundException, got ValidationException (1 validation error detected: Value 'arn:aws:billingconductor::000000000000:billinggroup/BOGUS00006' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/?[0-9]{12}$|...)". Expected: HTTP 404, error code NotFoundException, message "Resource was not found"; then HTTP 403 AccessDeniedException from DisassociateAccounts. Actual: HTTP 400 ValidationException from pre-handler pattern enforcement, DisassociateAccounts never gated.

Model citations used: CreateBillingGroup/ListBillingGroups/CreatePricingPlan/AssociatePricingRules/DisassociateAccounts/TagResource all list AccessDeniedException (errorKind client, httpError 403); AccessDeniedException{Message: String @required}; ResourceNotFoundException httpError 404; ValidationException httpError 400; TagResource.ResourceArn @pattern as quoted above.

## Steps to Reproduce
1. Clone this checkout, boot the emulator and run the behavioral suite (simparity-behavioral-aws-v1), service billingconductor.
2. Using credentials for an account that is NOT the organization payer account:
   a. CreateBillingGroup with a valid body, then again with AccountGrouping={'LinkedAccountIds': []} and no PrimaryAccountId — both return HTTP 200 with an Arn instead of HTTP 403 AccessDeniedException.
   b. CreatePricingPlan with PricingRuleArns=['arn:aws:billingconductor::<acct>:pricingrule/BOGUS00001'] — returns 200 instead of 403 AccessDeniedException.
   c. ListBillingGroups(MaxResults=5) — returns 200 {'BillingGroups': []} instead of 403 AccessDeniedException; same for ListPricingRules and ListAccountAssociations.
   d. AssociatePricingRules / DisassociatePricingRules against well-formed nonexistent plan/rule ARNs, 6 concurrent callers each — every call returns error code NotImplemented instead of 403 AccessDeniedException for all 6.
   e. ListCustomLineItems(MaxResults=5) — returns error code NotImplemented instead of 403 AccessDeniedException; same for ListCustomLineItemVersions and ListResourcesAssociatedToCustomLineItem.
   f. TagResource(ResourceArn='arn:aws:billingconductor::<acct>:billinggroup/BOGUS00006', Tags={'env':'x'}) — returns HTTP 400 ValidationException about the resourceArn pattern instead of HTTP 404 NotFoundException 'Resource was not found'; then DisassociateAccounts(Arn=<same ARN>, AccountIds=['111111111111']) must return 403 AccessDeniedException.
3. Re-replay the 7 check ids in trace_ids after the fix; each must observe the 403/404 statuses and error codes above.
