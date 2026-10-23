Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: savingsplans.describe-savings-plans-too-many-arns-rejected savingsplans.create-savings-plan-negative-commitment-rejected
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans.describe-savings-plans-too-many-arns-rejected|run#5|AssertionError|assert error["Message"] == (
Issue-Key: savingsplans.create-savings-plan-negative-commitment-rejected|run#7|AssertionError|assert err["Message"] == "'commitment' cannot be less than 0.001", f"unexpected create_neg message: {err['Message']!r}"
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# savingsplans: ValidationException validation rules diverge — constraint-violation message omits the rejected value, and CreateSavingsPlan commitment floor is 0 instead of 0.001

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSavingsPlans

## Description
Two distinct root causes in the savingsplans emulator's request-validation layer. Both operations return the correct HTTP status (400) and the correct error code (ValidationException), so the divergence is in the validation RULE / rendered constraint detail, not in error classification. 2 causes in this issue; both must be closed.

=== Cause 1 of 2: DescribeSavingsPlans list-length ValidationException does not render the rejected value (check: savingsplans.describe-savings-plans-too-many-arns-rejected) ===
AWS: Smithy: DescribeSavingsPlans.savingsPlanArns targets SavingsPlanArnList with @length {min:0, max:100} (member SavingsPlanArn @pattern '^arn:aws([-a-z]*)?:[a-z]+:...:savingsplan/<uuid>$'); modeled error ValidationException (httpError 400). Exceeding the modeled max is rejected by the standard AWS input-validation framework, whose message is built FROM THE REQUEST in the fixed form: "<n> validation error detected: Value '<the rejected value, rendered>' at '<member path>' failed to satisfy constraint: <constraint text>". For this member the recorded AWS response renders the value as the bracketed list of the ARNs actually sent, e.g. "1 validation error detected: Value '[arn:aws:savingsplans::<acct>:savingsplan/<uuid>, ...]' at 'originalRequest.savingsPlanArns' failed to satisfy constraint: Member must have length less than or equal to 100". The requirement is that rule: the framed message, the member path 'originalRequest.savingsPlanArns', the constraint text from the model, and a rendering of the submitted value — not the one recorded literal.
Emulator: returns ValidationException/400 with the correct frame, member path and constraint text, but substitutes a COUNT SUMMARY for the value: "1 validation error detected: Value '101 items' at 'originalRequest.savingsPlanArns' failed to satisfy constraint: Member must have length less than or equal to 100". The rejected value itself is never rendered.
Why: clients and test suites that parse or diff the standard validation-error payload to recover which input was rejected (a very common pattern for AWS SDK error handling and for golden-output tests) see a synthesized placeholder where AWS puts the request data. Nothing in the response identifies the offending input, so a caller cannot reconstruct what it sent from the error.
Fix: the DescribeSavingsPlans input-validation path (the shared constraint-violation message formatter for list @length violations) must render the submitted member value in the AWS framework form rather than an item count.
Traces: savingsplans.describe-savings-plans-too-many-arns-rejected. NOTE: this check asserts the message before reaching its positive control (a 2-ARN well-formed-but-nonexistent DescribeSavingsPlans returning an empty savingsPlans list), so that control is still unverified and should be confirmed when the fix is replayed.

=== Cause 2 of 2: CreateSavingsPlan enforces commitment >= 0 instead of the AWS minimum of 0.001 (check: savingsplans.create-savings-plan-negative-commitment-rejected) ===
AWS: Smithy: CreateSavingsPlan.commitment is @required, targets Amount (a plain string shape — the model states NO numeric range), modeled error ValidationException (httpError 400). Recorded behavior only: real AWS rejects commitment='-1' against a valid Compute / No Upfront / USD offering with ValidationException/400 and the message "'commitment' cannot be less than 0.001", and accepts commitment='0.001' on the same offering (the check's positive control purchases it into state 'queued'). The rule the recording states is therefore a MINIMUM of 0.001, i.e. the open interval [0, 0.001) is also rejected — not merely negative amounts.
Emulator: rejects commitment='-1' with the right code/status but with the message "commitment must be a non-negative decimal amount, got '-1'", i.e. its validator implements a non-negativity test rather than the 0.001 floor. Hypothesis (from the emulator's own stated rule): commitment values such as '0' or '0.0005' are accepted by the emulator while AWS rejects them with ValidationException.
Why: a client exercising the lower boundary of commitment gets a plan created (and, on real AWS, a billing commitment) for an amount AWS would refuse; conversely code that catches and reports the AWS floor sees a different rule. The lower-bound boundary of CreateSavingsPlan cannot be tested against this emulator.
Fix: CreateSavingsPlan's commitment validation (the operation's input validation, not its serializer) must enforce a minimum of 0.001 and emit the service-specific message for the floor, and the state/plan-creation path must create nothing on rejection (the check's no-side-effect assertion was not reached).
Traces: savingsplans.create-savings-plan-negative-commitment-rejected.

## Evidence
Cause 1 — savingsplans.describe-savings-plans-too-many-arns-rejected [mismatch]
  Request: DescribeSavingsPlans(savingsPlanArns=[101 well-formed savingsplan ARNs])
  Expected (AWS): 400 ValidationException, message framed from the request:
    "1 validation error detected: Value '[<arn1>, <arn2>, ... <arn101>]' at 'originalRequest.savingsPlanArns' failed to satisfy constraint: Member must have length less than or equal to 100"
  Actual (emulator): 400 ValidationException, message:
    "1 validation error detected: Value '101 items' at 'originalRequest.savingsPlanArns' failed to satisfy constraint: Member must have length less than or equal to 100"
  Status and error code match; the rendered rejected value does not.

Cause 2 — savingsplans.create-savings-plan-negative-commitment-rejected [mismatch]
  Request: CreateSavingsPlan(savingsPlanOfferingId=<Compute/No Upfront/USD, 31536000s offering>, commitment="-1", purchaseTime=now+200d)
  Expected (AWS): 400 ValidationException, message "'commitment' cannot be less than 0.001" (the rule: minimum commitment 0.001)
  Actual (emulator): 400 ValidationException, message "commitment must be a non-negative decimal amount, got '-1'" (the rule: minimum 0)
  Status and error code match; the enforced lower bound does not — the emulator's own message declares a non-negativity rule where AWS declares a 0.001 floor, so commitments in [0, 0.001) diverge.

## Steps to Reproduce
Cause 1:
  1. Point a botocore savingsplans client at the emulator.
  2. Build 101 well-formed, nonexistent ARNs: f"arn:aws:savingsplans::{account}:savingsplan/{uuid4()}".
  3. Call DescribeSavingsPlans(savingsPlanArns=<those 101 ARNs>).
  4. Observe 400 ValidationException whose message renders the value as "101 items" instead of the bracketed list of submitted ARNs.
  5. Then call DescribeSavingsPlans(savingsPlanArns=<first 2 of those ARNs>) and confirm it succeeds with savingsPlans == [] (positive control, not reached today).

Cause 2:
  1. Call DescribeSavingsPlansOfferings(planTypes=['Compute'], paymentOptions=['No Upfront'], currencies=['USD'], durations=[31536000], maxResults=1) and take searchResults[0].offeringId.
  2. Call CreateSavingsPlan(savingsPlanOfferingId=<that id>, commitment='-1', purchaseTime=now+200 days) and observe ValidationException whose message states a non-negativity rule rather than the 0.001 minimum.
  3. Call CreateSavingsPlan with commitment='0' (and '0.0005') on the same offering: AWS rejects these (below the 0.001 floor); confirm the emulator also rejects them after the fix.
  4. Positive control: CreateSavingsPlan with commitment='0.001' must succeed and DescribeSavingsPlans(savingsPlanIds=[id]) must report state 'queued'; clean up with DeleteQueuedSavingsPlan.
