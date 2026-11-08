Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.alias-weight-ladder-observed-distribution
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.alias-weight-ladder-observed-distribution|_assert_sample_healthy#16|AssertionError|assert not s["bad"], (
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# lambda: a weighted alias's invocations are almost all killed — Runtime.ExitError 'Runtime exited with error: signal: killed' instead of a clean 200 per version

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: repeated Invokes through a weighted alias kill the execution environment, so a weighted alias's traffic split can never be observed.
AWS: an alias with `RoutingConfig.AdditionalVersionWeights` routes each Invoke to one of its two versions; every invocation returns `200` with `x-amz-executed-version` (`ExecutedVersion`) naming the version that ran and with that version's own payload, and over 200 invocations the observed split tracks the configured weight. A `Runtime.ExitError` with `errorMessage: "RequestId: <id> Error: Runtime exited with error: signal: killed"` is what AWS reports only when the sandbox is killed (e.g. out of memory or an exec failure) — it is not a normal outcome of a weighted-alias invoke. Smithy: lambda `Invoke` returns `StatusCode`, `FunctionError`, `LogResult`, `Payload`, `ExecutedVersion`.
Emulator: 198 of 200 invokes of the 0.5-weight alias are not a clean 200 whose returned marker matches `ExecutedVersion`; the failures are `FunctionError: 'Unhandled'` with payload `{"errorMessage":"RequestId: <id> Error: Runtime exited with error: signal: killed","errorType":"Runtime.ExitError",...}` and `ExecutedVersion` reported as version `2` with a `None` marker.
Why: alias-based traffic shifting (canary and linear deploys, the whole CodeDeploy/SAM `AutoPublishAlias` story) is unusable: nearly every request to a weighted alias is a 'Unhandled' function error, and no client can measure or trust the split.
Fix: the Lambda invoke path / execution-environment lifecycle for alias-routed invocations — the environment must survive repeated invokes so each returns the routed version's payload with a matching `ExecutedVersion`, rather than being killed.
Traces: lambda.alias-weight-ladder-observed-distribution.

## Evidence
Expected: each of 200 invokes of a 0.5-weighted alias returns StatusCode 200, no FunctionError, and a payload marker matching the response's `ExecutedVersion`, with the observed split near the configured weight.
Actual: `AssertionError: sim2real-rung-50 (weight 0.5): 198 of 200 invokes were not a clean 200 whose returned marker matched its ExecutedVersion. First three: [('2', None, 'FunctionError:\'Unhandled\':b\'{"errorMessage":"RequestId: <id> Error: Runtime exited with error: signal: killed","errorType":"Runtime.ExitError",...', ...]`

## Steps to Reproduce
1. Create a function, publish two versions whose handlers return distinct markers, and create an alias with `RoutingConfig.AdditionalVersionWeights` giving the second version weight 0.5.
2. Invoke the alias 200 times synchronously.
3. Every response must be StatusCode 200 with no FunctionError and a marker matching `ExecutedVersion`; the emulator kills nearly every invocation with `Runtime.ExitError ... signal: killed`.
