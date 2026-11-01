Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.alias-weight-ladder-observed-distribution lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.alias-weight-ladder-observed-distribution|_poll#16|Timeout|return val
Issue-Key: lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers|_poll_until#8|Timeout|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# lambda: weighted-alias traffic is never observable across a weight ladder, and a NONE-auth function URL with a named-principal grant never answers

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two distinct root causes. Both end in `Error: Check timed out after 60.0s`.

Cause 1 of 2 — weighted alias distribution: AWS: recorded behavior only (the Lambda model carries AliasRoutingConfiguration.AdditionalVersionWeights but states no distribution guarantee). AWS behaviour exercised by the check: for an alias whose AdditionalVersionWeights is stepped through a ladder of weights, repeated Invokes are split between the primary version and the weighted version, each invocation reports the version that served it (ExecutedVersion / the handler's reported version), and the observed split tracks the configured weight at each rung of the ladder.
Emulator: the check never completes — no distribution can be observed within the 60s budget, so weighted routing cannot be graded at any rung.
Failing check: lambda.alias-weight-ladder-observed-distribution.

Cause 2 of 2 — function URL with AuthType NONE plus a principal-scoped grant: AWS: recorded behavior only. A function URL configured AuthType=NONE whose lambda:InvokeFunctionUrl grant names a specific principal does NOT admit anonymous callers: an unsigned request is refused (403 Forbidden) while the named principal's request is served.
Emulator: the request never reaches a terminal response — `Error: Check timed out after 60.0s` — so neither the anonymous refusal nor the named-principal success is observable.
Failing check: lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers.

Why: a client cannot do weighted (canary/linear) deployments — the only evidence that traffic shifted is the per-invocation executed version, and it never arrives; and a function URL's authorization posture cannot be verified, so an endpoint intended to be reachable only by a named principal cannot be shown to refuse anonymous callers. In both cases the call hangs rather than answering, so callers and CI block.

Fix: the Invoke path's alias resolution (selecting a version per the routing weights and reporting ExecutedVersion) and the function-URL request path (evaluating the URL's AuthType together with the lambda:InvokeFunctionUrl resource-policy principal, then answering 200/403). Related open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers Invoke's NotImplemented refusal and carries neither of these checks.

Traces: the two check ids listed above.

## Evidence
Both failures report `Error: Check timed out after 60.0s` (replay verdicts [mismatch]).
- alias-weight-ladder-observed-distribution: expected invocations split between the primary and weighted versions with each invocation reporting its executed version, tracking the configured weight at each rung; actual: no distribution observable within 60s.
- url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers: expected 403 for the unsigned/anonymous request and a served response for the named principal; actual: no terminal HTTP response within 60s.

## Steps to Reproduce
1. Publish two versions of a function whose handler reports its version; create an alias with AdditionalVersionWeights pointing at the second version.
2. Step the weight through a ladder (e.g. 0.1, 0.5, 0.9), invoking repeatedly at each rung and tallying ExecutedVersion.
3. Separately, CreateFunctionUrlConfig with AuthType=NONE and add a lambda:InvokeFunctionUrl permission naming one principal.
4. Issue an anonymous HTTPS request to the URL (expect 403) and one as the named principal (expect it to be served).
5. Against the emulator both scenarios hang; the checks are killed at the 60s harness budget.
