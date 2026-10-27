Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.provisioned-concurrency-is-capped-by-reserved-concurrency
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.provisioned-concurrency-is-capped-by-reserved-concurrency|run#42|AssertionError|assert shrink_err.get("Message") == expected_shrink_msg, (
Issue-Commit: ef9848bd2cbc74c99a7db78d709be0930262fd73

# lambda: PutFunctionConcurrency refuses a reservation below allocated provisioned concurrency with PutProvisionedConcurrencyConfig's message instead of its own

> _Note: this issue was identified against the code base at commit `ef9848bd2cbc74c99a7db78d709be0930262fd73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionConcurrency

## Description
One net-new root cause.

Cause 1 of 1: PutFunctionConcurrency emits the wrong error message when the requested reservation is below the function's already-allocated provisioned concurrency.
Failing check ids: lambda.provisioned-concurrency-is-capped-by-reserved-concurrency

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json) models PutFunctionConcurrency as PUT /2017-10-31/functions/{FunctionName}/concurrency, input member ReservedConcurrentExecutions (@range min 0, required), errors InvalidParameterValueException (httpError 400), ResourceConflictException (409), ResourceNotFoundException (404), ServiceException (500), TooManyRequestsException (429). The model carries no message text for InvalidParameterValueException, so the message is recorded behavior only: lowering a function's reserved concurrency below the total provisioned concurrency currently allocated across its versions/aliases is refused 400 InvalidParameterValueException with a message whose FIXED text is " ReservedConcurrentExecutions  " + " should not be lower than function's total provisioned concurrency [" + "]." (note AWS's leading space and the doubled space after the member name), and whose VARIABLE parts are (a) the ReservedConcurrentExecutions value carried by the request and (b) the function's total allocated provisioned concurrency summed across versions, read from service state. The suite's expected value is exactly this template; the check file records it as an unreplayed hypothesis (H4) for the enforcing side, but the refusal itself is what the emulator already performs — it is only the wording that diverges.

Emulator: the status (400) and the error code (InvalidParameterValueException) are correct, and the reservation is correctly left untouched, but the message is PutProvisionedConcurrencyConfig's: 'Requested Provisioned Concurrency should not be greater than the reservedConcurrentExecution for function'. Replay output: "AssertionError: expected the exact reserved-below-provisioned message, got 'Requested Provisioned Concurrency should not be greater than the reservedConcurrentExecution for function'". The handler refuses this case by reusing the shared constant crate::state::PROVISIONED_ABOVE_RESERVED_MESSAGE rather than formatting its own sentence, so the message names neither the requested reservation nor the allocated total. Every earlier leg of the same check passed, including the over-allocation refusal from PutProvisionedConcurrencyConfig (same code, and there the shared constant IS the right text) and the ProvisionedConcurrencyConfigNotFoundException 'No Provisioned Concurrency Config found for this function' read-back — so the two sides of the one invariant are distinguishable on AWS and indistinguishable here.

Why: a client relying on AWS's behavior is told the inverse of what happened. It asked to LOWER a reservation and is told its PROVISIONED concurrency request was too large, which points at a call it did not make. Because the emulator's text is a constant, it carries neither the rejected ReservedConcurrentExecutions value nor the allocated provisioned total, so a caller (or an operator reading a log) cannot tell how far below the floor the request fell and cannot compute the smallest acceptable reservation without extra GetFunctionConcurrency / ListProvisionedConcurrencyConfigs calls. Tooling that branches on the message — deploy scripts that distinguish 'shrink blocked by allocation' from 'allocation exceeds reservation' to decide whether to release provisioned concurrency first or raise the reservation — takes the wrong remediation branch on this message.

Fix: the validation branch inside the PutFunctionConcurrency handler. It must format its own message for this error class from the requested reservation and the function's total provisioned concurrency instead of reusing the PutProvisionedConcurrencyConfig constant; the shared constant should stay the message for the over-allocation direction only.

Traces: failing check lambda.provisioned-concurrency-is-capped-by-reserved-concurrency. Divergence originates at services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/put_function_concurrency.rs:269-273 (the `if requested < provisioned` branch returning crate::state::PROVISIONED_ABOVE_RESERVED_MESSAGE); the shared constant lives in services/service-com-amazonaws-lambda-awsgirapiservice/src/state.rs. No open issue covers this: the nearest ones, 20261025T020208Z-cloudwatch-dynamodb-lambda-lambda-listeventsourcemappings-answers-resourcenotfoundexception-function-not-found-for.md and 20261025T192750Z-cloudwatch-2-net-new-root-causes-putmanagedinsightrules-accepts.md, concern ListEventSourceMappings/reserved-concurrency admission and CloudWatch AWS/Lambda concurrency metrics respectively, not this operation's refusal message.

## Evidence
Check: lambda.provisioned-concurrency-is-capped-by-reserved-concurrency — verdict [mismatch].

Setup the check reached before failing (all of it passed): function created, two versions published (v1, v2), PutFunctionConcurrency ReservedConcurrentExecutions=4 accepted and read back by GetFunctionConcurrency; PutProvisionedConcurrencyConfig(v1, 2) reached Status=READY with AllocatedProvisionedConcurrentExecutions=2; PutProvisionedConcurrencyConfig(v2, 3) refused 400 InvalidParameterValueException with the exact message 'Requested Provisioned Concurrency should not be greater than the reservedConcurrentExecution for function'; GetProvisionedConcurrencyConfig(v2) answered ProvisionedConcurrencyConfigNotFoundException 'No Provisioned Concurrency Config found for this function'; PutProvisionedConcurrencyConfig(v2, 2) accepted, bringing the total to exactly the reservation.

The diverging call: PutFunctionConcurrency(FunctionName=<fn>, ReservedConcurrentExecutions=reserved-1) while the function's total allocated provisioned concurrency equals reserved.

Expected (AWS): HTTP 400, Code InvalidParameterValueException, Message = " ReservedConcurrentExecutions  <requested> should not be lower than function's total provisioned concurrency [<total provisioned>]." — fixed text " ReservedConcurrentExecutions  ", " should not be lower than function's total provisioned concurrency [", "]."; variable parts <requested> (the request's ReservedConcurrentExecutions) and <total provisioned> (the function's allocated provisioned concurrency summed across versions, from state).

Actual (emulator): HTTP 400, Code InvalidParameterValueException (correct), Message 'Requested Provisioned Concurrency should not be greater than the reservedConcurrentExecution for function' — a constant, carrying no request or state values, and describing the opposite direction of the invariant.

Replay detail: AssertionError: expected the exact reserved-below-provisioned message, got 'Requested Provisioned Concurrency should not be greater than the reservedConcurrentExecution for function' (check_lambda_provisioned_concurrency_is_capped_by_reserved_concurrency.py line 175).

State after the refusal is correct: GetFunctionConcurrency still reports the original reservation, so only the message diverges.

## Steps to Reproduce
1. Create a Lambda function (python3.12, any handler) and wait for State=Active.
2. Publish version v1; update the configuration; wait for LastUpdateStatus=Successful; publish version v2.
3. PutFunctionConcurrency(FunctionName=fn, ReservedConcurrentExecutions=4).
4. PutProvisionedConcurrencyConfig(FunctionName=fn, Qualifier=v1, ProvisionedConcurrentExecutions=2); wait for Status=READY.
5. PutProvisionedConcurrencyConfig(FunctionName=fn, Qualifier=v2, ProvisionedConcurrentExecutions=2); wait for Status=READY. The function's total allocated provisioned concurrency is now 4, equal to its reservation.
6. Call PutFunctionConcurrency(FunctionName=fn, ReservedConcurrentExecutions=3).
7. Observe the refusal: the code is InvalidParameterValueException as AWS returns, but the Message is the constant 'Requested Provisioned Concurrency should not be greater than the reservedConcurrentExecution for function' instead of AWS's " ReservedConcurrentExecutions  3 should not be lower than function's total provisioned concurrency [4]." (3 and 4 being the request value and the state total).
8. GetFunctionConcurrency(FunctionName=fn) still reports 4, confirming only the message diverges.

Replay the single check to confirm a fix: lambda.provisioned-concurrency-is-capped-by-reserved-concurrency.
