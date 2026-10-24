Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# Analyse the account's real Lambda functions instead of a hardcoded in-crate function inventory

_Filed: 2026-10-24T08:27:11Z_
_Operation: GetLambdaFunctionRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: Compute Optimizer reports recommendations for the Lambda functions that exist in the account/Region ("Returns recommendations for Lambda functions"), so a client that creates a function in the Lambda emulator, enrolls, and calls GetLambdaFunctionRecommendations sees a recommendation for THAT function ARN, and sees no recommendation for a function that does not exist.

Actual: the handler reports five functions baked into this crate's own state module (`invoice-renderer` $LATEST, `thumbnail-resizer:3`, `order-dispatcher`, `log-forwarder`, `nightly-reconciler`) and nothing else. The Lambda emulator's own function inventory is never consulted, so:
  * a function the client actually created is never analysed (its ARN selects nothing and, because this operation models no per-resource error list, it is silently absent from the answer), and
  * the response names five function ARNs that do not exist in the account at all, which a client that cross-checks against Lambda ListFunctions cannot reconcile.

The same crate already shows the correct pattern: GetECSServiceRecommendations resolves `ecs::state::service_state` and `cloudwatch::state::service_state` and computes its recommendations over the real ECS service inventory and the real metric store, so the Lambda reader diverges from its sibling as well as from AWS.

## Evidence

Spec: `GetLambdaFunctionRecommendations` — "Returns Lambda function recommendations" for the functions of the account/Region; `LambdaFunctionRecommendation$functionArn`/`functionVersion` identify a real function version. Handler: `src/ops/get_lambda_function_recommendations.rs` lines ~274-316 selects from `guard.visible_functions()` only. State: `src/state.rs` — `ComAmazonawsComputeoptimizerComputeoptimizerserviceState::functions` is filled from `observed_functions()` (the five hardcoded `ObservedFunction` fixtures at ~line 3126) and no operation or sibling service ever writes it. Contrast: `src/ops/get_ecs_service_recommendations.rs` lines ~340-351 and ~441-472 read the ECS inventory and CloudWatch store for the equivalent analysis. Tests: `tests/all/lambda_function_recommendations.rs` asserts the fixture inventory over the wire; no test creates a function in the Lambda emulator and asks Compute Optimizer about it, so nothing pins the cross-service behaviour.

## Steps to Reproduce

1. Boot the emulator with both Lambda and Compute Optimizer mounted.
2. Create a Lambda function (e.g. CreateFunction with FunctionName=my-worker, MemorySize=1024).
3. Call Compute Optimizer UpdateEnrollmentStatus(status=Active) and wait for the analysis delay.
4. Call GetLambdaFunctionRecommendations with functionArns=[the ARN of my-worker]: observe an empty lambdaFunctionRecommendations list — the function the account really has is never analysed.
5. Call GetLambdaFunctionRecommendations with no functionArns: observe recommendations for arn:...:function:invoice-renderer, function:thumbnail-resizer:3, function:order-dispatcher, function:log-forwarder and function:nightly-reconciler — functions that do not exist in the Lambda emulator.
