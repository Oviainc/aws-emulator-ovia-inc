Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-cfg-resolver-batch-and-caching-rules
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-cfg-resolver-batch-and-caching-rules appsync-cfg-resolver-sync-config-rules appsync-eval-code-error-detail
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateResolver|4xx->2xx|$.message
Issue-Key: appsync|CreateResolver|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateResolver|4xx->2xx|status
Issue-Key: appsync|CreateResolver|4xx->2xx|$.detail
Issue-Key: appsync|CreateResolver|4xx->2xx|$.reason
Issue-Key: appsync|ListResolvers|2xx->2xx|$.resolvers[*].code
Issue-Commit: e88affa0c81a58fb8423ccb328f6804dccd04335

# AppSync CreateResolver accepts resolvers AWS refuses: cachingKeys prefix check and APPSYNC_JS code compile check missing (3 causes)

> _Note: this issue was identified against the code base at commit `e88affa0c81a58fb8423ccb328f6804dccd04335`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolver

## Description
Three net-new causes with one shared fault: CreateResolver has no validation for two inputs AWS refuses with BadRequestException, so the emulator answers 200 and then stores and serves a resolver AWS never created.

Cause 1 of 3 — cachingConfig.cachingKeys with an unsupported prefix is accepted.
AWS: Smithy: CachingConfig.cachingKeys (CachingKeys) — "Valid values are entries from the $context.arguments, $context.source, and $context.identity maps"; CreateResolver models BadRequestException (httpError 400). A caching key that does not start with one of those roots is refused 400 with x-amzn-errortype BadRequestException and message 'Caching keys need to start with $context.args, $context.arguments, $context.identity, or $context.source.' — that message is entirely fixed text (no request-derived parts).
Emulator: answers 200 and creates the resolver; no $.message and no x-amzn-errortype header at all.
Why: a client that mis-specifies a caching key gets a resolver that looks healthy but whose per-key caching cannot work on AWS; the error-handling path for this misconfiguration is never exercised, and the deploy fails only in real AWS.
Fix: CreateResolver's input validation (shared with UpdateResolver) — reject cachingConfig.cachingKeys entries whose prefix is not $context.args / $context.arguments / $context.identity / $context.source with BadRequestException and the message above.
Traces: appsync-cfg-resolver-batch-and-caching-rules#task#8 (same cluster: appsync-cfg-resolver-sync-config-rules#task#6).

Cause 2 of 3 — APPSYNC_JS code that does not compile is accepted, and the CODE_ERROR refusal is never produced.
AWS: Smithy: Resolver.code (Code) with runtime AppSyncRuntime APPSYNC_JS; BadRequestException carries message, reason (BadRequestReason, only value CODE_ERROR) and detail (BadRequestDetail.codeErrors: list of CodeError {errorType, value, location{line,column,span}}). AWS compiles the submitted code and, when it fails to parse, refuses 400 with reason 'CODE_ERROR', message 'The code contains one or more errors.' (fixed text) and detail.codeErrors listing one CodeError per diagnostic whose errorType is the diagnostic class (recording: 'PARSE_ERROR'), whose value is the compiler text and whose location is the line/column/span of the offending token — the errorType, the compiler text and the location are all derived from the submitted code, not constants.
Emulator: answers 200 with a created resolver; $.reason, $.detail, $.message and x-amzn-errortype are all absent.
Why: a client relying on AWS's compile-time rejection ships broken resolver code that the emulator reports as deployed, and SDK/console error UIs that render reason+detail.codeErrors have nothing to render, so the whole code-diagnostics contract is untestable.
Fix: CreateResolver's validation (same check as CreateFunction) — compile the code for APPSYNC_JS runtimes and raise BadRequestException with reason CODE_ERROR and a populated detail.codeErrors instead of accepting it. The sibling CreateFunction occurrence in this fixture is owned by 20261029T200818Z-appsync-resolver-function-data-source-plane-dependency-refusals.md; the CreateResolver occurrence is not owned by it.
Traces: appsync-eval-code-error-detail#task#6.

Cause 3 of 3 — the wrongly accepted code is then served by ListResolvers.
AWS: because the create above is refused, the resolver keeps the last code AWS accepted, so ListResolvers reports that valid code body.
Emulator: ListResolvers answers 200 with $.resolvers[0].code set to the uncompilable body the emulator accepted (expected the valid handler body, got the body with the unterminated object literal).
Why: state diverges after the missing refusal — a client listing resolvers sees non-functional code recorded as the live revision, so the divergence is observable even for clients that never look at the create response.
Fix: once Cause 2 refuses the create, no state write happens; the resolver record (and ListResolvers' projection of it) must keep the previously accepted code.
Traces: appsync-eval-code-error-detail#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1:
- appsync-cfg-resolver-batch-and-caching-rules#task#8 CreateResolver: expected 400, got 200; $.message missing (expected 'Caching keys need to start with $context.args, $context.arguments, $context.identity, or $context.source.'); $headers.x-amzn-errortype missing. Same cluster: appsync-cfg-resolver-sync-config-rules#task#6.
- appsync-eval-code-error-detail#task#6 CreateResolver: expected 400, got 200; $.reason missing (expected 'CODE_ERROR'); $.message missing (expected 'The code contains one or more errors.'); $.detail missing (expected {'codeErrors': [{'errorType': 'PARSE_ERROR', 'value': "code.js(7,1): error TS1005: '}' expected.\n", 'location': {'line': 7, 'column': 1, 'span': 0}}]}); $headers.x-amzn-errortype missing.
- appsync-eval-code-error-detail#task#10 ListResolvers 200->200: $.resolvers[0].code value_mismatch — expected the compilable handler body, got the body the emulator accepted at task#6.

## Steps to Reproduce
1. CreateGraphqlApi and a NONE data source.
2. CreateResolver on Query/<field> with cachingConfig {ttl: 60, cachingKeys: ["$context.nonsense.id"]} (a key outside $context.args/$context.arguments/$context.identity/$context.source): the emulator answers 200 instead of 400 BadRequestException 'Caching keys need to start with $context.args, $context.arguments, $context.identity, or $context.source.' (cause 1).
3. CreateResolver with runtime {name: APPSYNC_JS, runtimeVersion: 1.0.0} and a code body containing an unterminated object literal: the emulator answers 200 instead of 400 BadRequestException reason CODE_ERROR / message 'The code contains one or more errors.' / detail.codeErrors with the parse diagnostic and its line/column (cause 2).
4. Call ListResolvers for that type: the uncompilable code body is reported as the resolver's code (cause 3).
Replay: appsync-cfg-resolver-batch-and-caching-rules#task#8, appsync-eval-code-error-detail#task#6 and #task#10.
