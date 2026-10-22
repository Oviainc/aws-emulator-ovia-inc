Issue-Source: trace-sweep-divergence
Issue-Trace: budgets-account-wide-listings
Issue-Step: teardown#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: budgets-account-wide-listings budgets-budget-error-paths
Issue-Shape-Id: com.amazonaws.budgets#AWSBudgetServiceGateway
Issue-Key: budgets|DescribeBudget|4xx->4xx|status
Issue-Key: budgets|CreateBudget|4xx->4xx|status
Issue-Key: budgets|CreateBudget|4xx->4xx|$.__type
Issue-Key: budgets|CreateBudget|4xx->4xx|$.message
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Budgets client errors use modeled status and wrong error code

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBudget

## Description
On every failure path exercised by the corpus the budgets emulator answers with a different HTTP status than AWS, and on input validation it also answers with a different error code and no message. Three causes, all in the budgets error-mapping/serialization path.

Cause 1 of 3 - DescribeBudget on a missing budget answers 404 instead of 400.
AWS: recorded behavior - DescribeBudget for a budget name that does not exist answers HTTP 400 carrying the NotFoundException error code (Smithy: DescribeBudget errors include NotFoundException, documented "We can't locate the resource that you specified"; its @httpError in the model is 404, but the recorded wire response from AWS for this awsJson1_1 operation is 400, and the recording is what clients are written against). Only the status diverged in replay, so the error code the emulator picks is already right.
Emulator: HTTP 404 with the same error code.
Why: a client or SDK retry/branch policy keyed on the HTTP status - 4xx bucketing, proxies and gateways that treat 404 as "route/endpoint missing" rather than "request rejected" - takes the wrong branch, and teardown/idempotent-delete flows that tolerate AWS's 400 fail against the emulator.
Fix: the budgets error serializer / status mapping for NotFoundException (do not take the status from the modeled @httpError for this service).
Traces: budgets-account-wide-listings#teardown#5 (also budgets-budget-error-paths#task#2).

Cause 2 of 3 - CreateBudget on a duplicate budget name answers 409 instead of 400.
AWS: recorded behavior - re-creating an existing budget name answers HTTP 400 carrying the DuplicateRecordException error code (Smithy: CreateBudget errors include DuplicateRecordException, documented "The budget name already exists. Budget names must be unique within an account"; modeled @httpError 409, recorded status 400). Only the status diverged, so the error code is already right.
Emulator: HTTP 409 with the same error code.
Why: same status-keyed branching problem as cause 1; a create-if-absent helper that recognises AWS's 400 duplicate response instead sees a 409 it does not handle and surfaces a hard failure.
Fix: the same budgets error serializer / status mapping, for DuplicateRecordException.
Traces: budgets-budget-error-paths#task#3.

Cause 3 of 3 - CreateBudget with a BudgetName that violates the member pattern returns InvalidParameterException with no message instead of ValidationException with a constraint message.
AWS: recorded behavior - a Budget.BudgetName that fails its constraint is rejected at request validation with HTTP 400 and error code ValidationException, and the message is built from the request: it names the failing member path and the constraint it violated (recorded example: "1 validation error detected: Value at 'budget.budgetName' failed to satisfy constraint: Member must satisfy regular expression pattern"). The constraint itself is modelled - Smithy: Budget.BudgetName (required) @length min 1 max 100, @pattern ^(?![^:\\]*/action/|(?i).*<script>.*</script>.*)[^:\\]+$ - so the member-naming message rule is known, while the exact recorded wording is an example. Note ValidationException is not a modelled member of CreateBudget's error list; it is the framework-level validation error AWS actually returns, which is why mapping this to a modelled error is wrong.
Emulator: HTTP 400 (status matches) but $.__type is InvalidParameterException and $.message is absent entirely.
Why: a client that branches on ValidationException to distinguish "my input is malformed" from a service-side InvalidParameterException takes the wrong path, and because there is no message at all the caller cannot tell which member was rejected - no error text to log, surface or parse.
Fix: CreateBudget input validation in the budgets service crate - validate Budget.BudgetName against its modelled length/pattern before the handler runs and emit a ValidationException whose message names the member path and the violated constraint.
Traces: budgets-budget-error-paths#task#4.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

1. budgets-account-wide-listings#teardown#5 DescribeBudget - status: expected 400, got 404 (divergence_key service=budgets|DescribeBudget|status=400->404|keys=status). Error code matched; only the status diverged. Cluster also covers budgets-budget-error-paths#task#2.

2. budgets-budget-error-paths#task#3 CreateBudget (duplicate budget name) - status: expected 400, got 409 (divergence_key service=budgets|CreateBudget|status=400->409|keys=status). Error code matched; only the status diverged.

3. budgets-budget-error-paths#task#4 CreateBudget (BudgetName violating the @pattern constraint) - status 400 as expected, but $.__type: expected 'ValidationException', got 'InvalidParameterException'; $.message: missing - expected a constraint message naming the member, recorded example "1 validation error detected: Value at 'budget.budgetName' failed to satisfy constraint: Member must satisfy regular expression pattern", got None.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture budgets-account-wide-listings, step teardown#5 (or budgets-budget-error-paths step task#2) - call DescribeBudget with a valid 12-digit AccountId and a BudgetName that does not exist; observe the HTTP status (404 vs AWS's 400).
3. Cause 2: replay fixture budgets-budget-error-paths, step task#3 - CreateBudget a budget, then CreateBudget again with the same BudgetName; observe the HTTP status (409 vs AWS's 400) on the second call.
4. Cause 3: replay fixture budgets-budget-error-paths, step task#4 - CreateBudget with a Budget.BudgetName that violates the modelled pattern (e.g. containing ':' or '\\' or the '/action/' substring); observe $.__type (InvalidParameterException vs ValidationException) and the absent $.message.
