Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.get-account-settings-enforced-limits lambda.invalid-parameter-rejection-on-writes lambda.provisioned-concurrency-is-capped-by-reserved-concurrency lambda.runtime-management-config-lifecycle
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.get-account-settings-enforced-limits|run#19|AssertionError|assert message == expected_message, (
Issue-Key: lambda.invalid-parameter-rejection-on-writes|run#14|AssertionError|assert provisioned_message == (
Issue-Key: lambda.provisioned-concurrency-is-capped-by-reserved-concurrency|run#26|AssertionError|assert err.get("Message") == "Requested Provisioned Concurrency should not be greater than the reservedConcurrentExecution for function", f"expected the exact o
Issue-Key: lambda.runtime-management-config-lifecycle|run#14|AssertionError|assert manual_msg == expected, (
Issue-Commit: bb4602e7ae983d0b86fd9b19deccaf384a5ade46

# lambda: four concurrency / runtime-management refusals diagnose the wrong rule — provisioned-concurrency over-allocation reports the reserved-floor message, and the account-floor and Manual-runtime sentences are wrong (4 causes)

> _Note: this issue was identified against the code base at commit `bb4602e7ae983d0b86fd9b19deccaf384a5ade46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutProvisionedConcurrencyConfig

## Description
Four net-new root causes on the Lambda concurrency and runtime-management write paths. All four refuse with the correct HTTP status and error code, but the refusal diagnoses the WRONG rule or states AWS's sentence incorrectly — in cause 2 the emulator answers a message about a completely different constraint than the one the request violated. No existing open issue covers these; 20261025T022656Z-lambda-ten-control-plane-writes-are-accepted-that.md is about writes that are wrongly ACCEPTED, and none of the replay keys below appear in it.

Cause 1 of 4: PutFunctionConcurrency's unreserved-floor message is missing its terminal period.
  AWS: Smithy: `PutFunctionConcurrency` models `InvalidParameterValueException` at httpError 400 for a reservation that would push the account's unreserved concurrency below its floor. AWS's message rule, built from the request/account, is `Specified ReservedConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [<floor>].` — a complete sentence ending in a period.
  Emulator: the same sentence with the terminal period dropped: `... below its minimum value of [100]`.
  Why: the message is produced by a known rule (AWS's fixed sentence with the floor interpolated), so the truncation is a rule defect, not a cosmetic variance; exact-match clients and the suite's account-settings contract both fail on it.
  Fix: the PutFunctionConcurrency handler's unreserved-floor validation message.
  Traces: lambda.get-account-settings-enforced-limits

Cause 2 of 4: PutProvisionedConcurrencyConfig above the ACCOUNT's unreserved concurrency reports the reserved-FLOOR rule instead.
  AWS: Smithy: `PutProvisionedConcurrencyConfig` models `InvalidParameterValueException` at httpError 400. When the requested provisioned concurrency exceeds the account's unreserved concurrency, AWS's message rule is `Specified ConcurrentExecutions for function is greater than account's unreserved concurrency [<unreserved>].`
  Emulator: refuses with `Specified ConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [100]` — the sentence for cause 1's DIFFERENT constraint (the reserved-concurrency floor), so the response names a rule the request did not violate.
  Why: the caller is told the wrong thing to change. The two rules have different remedies (raise the account quota / lower the requested value vs. leave headroom unreserved), so a client acting on the message takes the wrong corrective action.
  Fix: the PutProvisionedConcurrencyConfig handler's validation — it is reusing the PutFunctionConcurrency floor check instead of comparing against the account's unreserved concurrency.
  Traces: lambda.invalid-parameter-rejection-on-writes

Cause 3 of 4: PutProvisionedConcurrencyConfig above the FUNCTION's reserved concurrency uses the wrong sentence.
  AWS: Smithy: `PutProvisionedConcurrencyConfig` models `InvalidParameterValueException` at httpError 400. AWS documents that provisioned concurrency may not exceed the function's reserved concurrency minus provisioned concurrency on other versions, and its refusal message is the constant `Requested Provisioned Concurrency should not be greater than the reservedConcurrentExecution for function`.
  Emulator: `Specified ConcurrentExecutions for function is greater than the function's reserved concurrency of [4]` — the right constraint, but under a sentence AWS does not use, with the reserved value interpolated into a message AWS keeps constant.
  Why: a client cannot distinguish the account-level over-allocation (cause 2) from the function-level one by message, which is the only signal the API gives for which limit to raise.
  Fix: the PutProvisionedConcurrencyConfig handler's per-function reserved-concurrency check message.
  Traces: lambda.provisioned-concurrency-is-capped-by-reserved-concurrency

Cause 4 of 4: PutRuntimeManagementConfig(Manual) without RuntimeVersionArn uses the wrong sentence.
  AWS: Smithy: `PutRuntimeManagementConfig` takes `UpdateRuntimeOn` and `RuntimeVersionArn` and models `InvalidParameterValueException` at httpError 400. When `UpdateRuntimeOn=Manual` is sent without `RuntimeVersionArn` AWS answers the constant `Missing runtime version ARN. A runtime version ARN is required when UpdateRuntimeVersionOn=Manual. Provide a valid runtime version ARN and retry the request.`
  Emulator: `RuntimeVersionArn must be specified when UpdateRuntimeOn is Manual` — a terser message of the emulator's own, missing the remediation clause.
  Why: this is the required-member diagnosis for a conditional requirement the model cannot express; AWS's sentence is the contract for what the caller must supply, and the runtime-management lifecycle check grades it.
  Fix: the PutRuntimeManagementConfig handler's Manual-mode validation message.
  Traces: lambda.runtime-management-config-lifecycle

## Evidence
Replayed against the booted emulator. In all four the code is `InvalidParameterValueException` and the status 400 (correct); the Message diverges.

Cause 1 — lambda.get-account-settings-enforced-limits, PutFunctionConcurrency reserving 2000:
  expected `Specified ReservedConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [100].`
  got      `Specified ReservedConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [100]`  (no terminal period)

Cause 2 — lambda.invalid-parameter-rejection-on-writes, PutProvisionedConcurrencyConfig(ProvisionedConcurrentExecutions = AccountLimit.ConcurrentExecutions + 10,000,000) on a published version:
  expected `Specified ConcurrentExecutions for function is greater than account's unreserved concurrency [<AccountLimit.UnreservedConcurrentExecutions>].`
  got      `Specified ConcurrentExecutions for function decreases account's UnreservedConcurrentExecution below its minimum value of [100]`  (the reserved-floor rule, not the unreserved-exceeded rule)

Cause 3 — lambda.provisioned-concurrency-is-capped-by-reserved-concurrency, PutProvisionedConcurrencyConfig above the function's reserved concurrency:
  expected `Requested Provisioned Concurrency should not be greater than the reservedConcurrentExecution for function`
  got      `Specified ConcurrentExecutions for function is greater than the function's reserved concurrency of [4]`

Cause 4 — lambda.runtime-management-config-lifecycle, PutRuntimeManagementConfig(UpdateRuntimeOn='Manual') with no RuntimeVersionArn:
  expected `Missing runtime version ARN. A runtime version ARN is required when UpdateRuntimeVersionOn=Manual. Provide a valid runtime version ARN and retry the request.`
  got      `RuntimeVersionArn must be specified when UpdateRuntimeOn is Manual`

The bracketed numbers (100, 4) are per-run account/function values; the requirement is the sentence rule and, for cause 2, that the rule reported is the one the request actually violated.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the four checks in trace_ids.
3. Cause 1/2: CreateFunction, then GetAccountSettings to read AccountLimit.ConcurrentExecutions / UnreservedConcurrentExecutions. Call PutFunctionConcurrency with a reservation above the account's headroom (cause 1); then PublishVersion and call PutProvisionedConcurrencyConfig with AccountLimit.ConcurrentExecutions + 10,000,000 on that version (cause 2). Assert each Message is the sentence for the rule actually violated.
4. Cause 3: PutFunctionConcurrency(ReservedConcurrentExecutions=4) on a function, PublishVersion, then PutProvisionedConcurrencyConfig with more than 4 on that version; assert the exact over-allocation sentence.
5. Cause 4: PutRuntimeManagementConfig(FunctionName=fn, UpdateRuntimeOn='Manual') with RuntimeVersionArn omitted; assert the exact missing-ARN sentence.
