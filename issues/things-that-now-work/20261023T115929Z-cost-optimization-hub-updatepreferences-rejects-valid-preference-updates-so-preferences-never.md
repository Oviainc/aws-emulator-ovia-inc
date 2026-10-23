Issue-Source: trace-sweep-divergence
Issue-Trace: cost-optimization-hub-preferences-configure-and-verify
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-optimization-hub-preferences-configure-and-verify
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub|UpdatePreferences|2xx->4xx|$.preferredCommitment
Issue-Key: cost-optimization-hub|UpdatePreferences|2xx->4xx|$.savingsEstimationMode
Issue-Key: cost-optimization-hub|UpdatePreferences|2xx->4xx|status
Issue-Key: cost-optimization-hub|GetPreferences|2xx->2xx|$.preferredCommitment.paymentOption
Issue-Key: cost-optimization-hub|GetPreferences|2xx->2xx|$.preferredCommitment.term
Issue-Commit: 982a07ada62dfbeb289a85399a86f8ade68b0ada

# UpdatePreferences rejects valid preference updates so preferences never persist

> _Note: this issue was identified against the code base at commit `982a07ada62dfbeb289a85399a86f8ade68b0ada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePreferences

## Description
Cost Optimization Hub's preference write path refuses requests AWS accepts, and because nothing is stored the following read reports the service defaults instead of what was set. Three causes, all in the same preference handler/state pair.

Cause 1 of 3 — UpdatePreferences refuses a well-formed preference update with ValidationException (fixture cost-optimization-hub-preferences-configure-and-verify, step task#3).
AWS: Smithy UpdatePreferences takes savingsEstimationMode (SavingsEstimationMode: BeforeDiscounts|AfterDiscounts), memberAccountDiscountVisibility and preferredCommitment (PreferredCommitment{term, paymentOption}); it models AccessDeniedException(403), InternalServerException(500), ThrottlingException(429) and ValidationException(400), and the model states NO precondition tying savingsEstimationMode=AfterDiscounts to any other preference. Recorded behavior: the request returns HTTP 200 with the effective record echoed back — savingsEstimationMode 'AfterDiscounts' and preferredCommitment {paymentOption: PartialUpfront, term: OneYear}.
Emulator: answers HTTP 400 (ValidationException) and emits no preference members at all (both $.preferredCommitment and $.savingsEstimationMode are absent from the body). The handler gates the write on invented preconditions before storing anything: an AfterDiscounts request is refused unless member-account discount visibility already resolves to All, and any request that carries memberAccountDiscountVisibility is refused unless the caller is the management account.
Why: a client that sets its savings-estimation mode and commitment preference gets a hard 400 on a request AWS accepts, so no preference can ever be configured; worse, the error is the modeled ValidationException, so retry logic treats a valid request as permanently malformed.
Fix: UpdatePreferences' validation/handler — remove or correct the pre-store preconditions so an accepted update is merged and the effective record returned with 200.
Traces: cost-optimization-hub-preferences-configure-and-verify#task#3; handler gates at services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/update_preferences.rs:183-209 (helpers at :239 and :249).

Cause 2 of 3 — the same refusal on the restore/teardown write (fixture cost-optimization-hub-preferences-configure-and-verify, step teardown#5).
AWS: recorded behavior only — a second UpdatePreferences call that restores the account's preferences returns HTTP 200.
Emulator: HTTP 400. Only the status diverges, so the operation is rejected outright again rather than producing a wrong body.
Why: preferences cannot be put back, so a workflow that configures and then restores leaves the account wedged and any test or client teardown fails.
Fix: same component as Cause 1 — UpdatePreferences' pre-store validation. Confirm BOTH writes of the fixture return 200 before closing.
Traces: cost-optimization-hub-preferences-configure-and-verify#teardown#5.

Cause 3 of 3 — GetPreferences reports the service defaults rather than the preferences that were set (fixture cost-optimization-hub-preferences-configure-and-verify, step task#4).
AWS: Smithy GetPreferences output carries savingsEstimationMode, memberAccountDiscountVisibility and preferredCommitment; PreferredCommitment documents term defaulting to ThreeYears and paymentOption defaulting to AllUpfront ONLY when the value is null, i.e. when the account has never set it. Recorded behavior: after the update, the read returns preferredCommitment {paymentOption: 'PartialUpfront', term: 'OneYear'}.
Emulator: HTTP 200 but $.preferredCommitment.paymentOption = 'AllUpfront' and $.preferredCommitment.term = 'ThreeYears' — the documented null-defaults, i.e. a store that was never written.
Why: a client cannot read back its own configuration; it is silently told it prioritises AllUpfront/ThreeYears, which changes which commitment recommendations and savings figures it acts on.
Fix: GetPreferences' state read / the preference store's merge — the effective record must report stored values and fall back to the defaults only for members never set. Hypothesis: this is downstream of Causes 1-2 (the write was refused, so there was nothing to read), so re-replay task#4 after fixing the write and keep this cause open if the read still returns defaults.
Traces: cost-optimization-hub-preferences-configure-and-verify#task#4.

## Evidence
Fixture cost-optimization-hub-preferences-configure-and-verify, trace set simparity-wire-aws-v1:

- task#3 UpdatePreferences — expected status 200 with $.preferredCommitment = {'paymentOption': 'PartialUpfront', 'term': 'OneYear'} and $.savingsEstimationMode = 'AfterDiscounts'; actual status 400, both members missing (None).
- teardown#5 UpdatePreferences — expected status 200; actual status 400.
- task#4 GetPreferences — expected/actual status 200; $.preferredCommitment.paymentOption expected 'PartialUpfront', got 'AllUpfront'; $.preferredCommitment.term expected 'OneYear', got 'ThreeYears'.

Model references (smithy_query on services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/smithy-model/com-amazonaws-costoptimizationhub-costoptimizationhubservice.json): UpdatePreferences input/output {savingsEstimationMode, memberAccountDiscountVisibility, preferredCommitment}, errors ValidationException(400)/AccessDeniedException(403)/ThrottlingException(429)/InternalServerException(500); PreferredCommitment.term doc 'If the value is null, it will default to ThreeYears'; PreferredCommitment.paymentOption doc 'If the value is null, it will default to AllUpfront'. No modeled dependency between savingsEstimationMode and memberAccountDiscountVisibility.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture cost-optimization-hub-preferences-configure-and-verify.
3. Step task#3 calls UpdatePreferences with savingsEstimationMode=AfterDiscounts and preferredCommitment={paymentOption: PartialUpfront, term: OneYear}: the emulator answers 400 ValidationException where the recording has 200 echoing those members.
4. Step task#4 calls GetPreferences: the emulator answers 200 with preferredCommitment {AllUpfront, ThreeYears} instead of {PartialUpfront, OneYear}.
5. Step teardown#5 calls UpdatePreferences again to restore the account: the emulator answers 400 where the recording has 200.
