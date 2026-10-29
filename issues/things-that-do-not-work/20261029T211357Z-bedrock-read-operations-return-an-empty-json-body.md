Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-delete-resource-policy-missing-not-notfound
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-delete-resource-policy-missing-not-notfound bedrock-model-invocation-logging-config
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetResourcePolicy|2xx->2xx|$
Issue-Key: bedrock|GetModelInvocationLoggingConfiguration|2xx->2xx|$
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock read operations return an empty JSON body instead of the modeled output members

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
TWO causes, one shared fault: these read handlers serialize an empty JSON object '{}' where AWS serializes the operation's output member — in one case a present value, in the other an explicit null. The 200 status is right; the body is missing the member the client reads.

Cause 1 of 2: GetResourcePolicy for a resource with no resource policy attached (including after DeleteResourcePolicy).
AWS: 200 with body {"resourcePolicy": "{}"} — the member IS present and carries the empty-policy document as a JSON string. Smithy: GetResourcePolicy output resourcePolicy targets ResourcePolicyDocument (@length 1..20480), and ResourceNotFoundException (404) exists but is NOT what AWS uses for a missing policy on an existing resource — the recording shows 200 with the empty document.
Emulator: 200 with body {} — resourcePolicy absent entirely.
Why: an SDK caller sees resourcePolicy as unset and cannot distinguish 'no policy' from 'field not served'; code that parses the returned document as JSON gets nothing to parse and typically throws instead of reading an empty policy.
Fix: GetResourcePolicy's serializer/handler — always emit resourcePolicy, with the empty document when no statement is attached.
Traces: bedrock-delete-resource-policy-missing-not-notfound#task#2 (also task#6 in the same cluster).

Cause 2 of 2: GetModelInvocationLoggingConfiguration when no logging configuration is set.
AWS: 200 with body {"loggingConfig": null} — the member is present and explicitly null. Smithy: GetModelInvocationLoggingConfiguration takes no input and its output has the single member loggingConfig (LoggingConfig), with no not-found error modelled at all (errors are AccessDeniedException, InternalServerException, ThrottlingException only).
Emulator: 200 with body {} — loggingConfig absent entirely.
Why: the operation's only documented way to say 'logging is not configured' is an explicit null loggingConfig; an empty object leaves clients that check for the key's presence unable to tell 'unset' from a truncated response.
Fix: GetModelInvocationLoggingConfiguration's serializer — emit the loggingConfig key with a null value when the configuration is unset.
Traces: bedrock-model-invocation-logging-config#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline. Both causes matched on status (200) and diverged only in the body.

Cause 1 — bedrock-delete-resource-policy-missing-not-notfound#task#2 GetResourcePolicy (200/200): $ reported 'empty_body' — expected {'resourcePolicy': '{}'}, got {}. Cluster affected_count 2 (task#2 and task#6).

Cause 2 — bedrock-model-invocation-logging-config#task#3 GetModelInvocationLoggingConfiguration (200/200): $ reported 'empty_body' — expected {'loggingConfig': None}, got {}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay bedrock-delete-resource-policy-missing-not-notfound#task#2 (and #task#6) and bedrock-model-invocation-logging-config#task#3.
3. Inspect the raw 200 bodies: both are '{}'.
4. Equivalently: call GetResourcePolicy on a resource with no policy (or after DeleteResourcePolicy) — AWS answers {"resourcePolicy": "{}"}; call GetModelInvocationLoggingConfiguration with no logging configured — AWS answers {"loggingConfig": null}.
