Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-agent-runtime-control-plane-errors
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-agent-runtime-control-plane-errors acc-browser-profile-name-scoped-listing bedrock-agentcore-control-payment-error-paths acc-memory-update-and-strategy-round-trip bedrock-agentcore-control-memory-validation-errors bedrock-agentcore-control-configuration-bundle-errors bedrock-agentcore-control-consent-portal-errors bedrock-agentcore-control-workload-identity-error-paths acc-workload-identity-update-round-trip
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateAgentRuntime|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|ListBrowserProfiles|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreatePaymentManager|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreateMemory|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreateConfigurationBundle|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreateConsentPortal|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreateWorkloadIdentity|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|UpdateWorkloadIdentity|4xx->4xx|$.message
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Validation-error messages quote the anchored regex pattern instead of the pattern AWS reports

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentRuntime

## Description
Status (400) and error code (ValidationException) are correct everywhere below; the rendered message is built from the request and the model, and the emulator's renderer gets that construction wrong in three consistent ways: it prints the @pattern with its outer ^...$ anchors (AWS prints the pattern with the anchors stripped), it prints the offending value for members where AWS omits it, and for list members it reports an indexed member path with a single constraint instead of the list name with the member's whole constraint set. 8 causes, one message renderer.

Cause 1 of 8: CreateAgentRuntime agentRuntimeName.
AWS: Smithy CreateAgentRuntime.agentRuntimeName @pattern ^[a-zA-Z][a-zA-Z0-9_]{0,47}$, error ValidationException (400). The recorded message renders the constraint as 'Member must satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9_]{0,47}' - i.e. the modelled pattern with the leading ^ and trailing $ removed.
Emulator: renders '... pattern: ^[a-zA-Z][a-zA-Z0-9_]{0,47}$'.
Why: tests and tools that match the AWS validation text (or echo it to users) see a pattern AWS never emits; the message is otherwise identical, so this is a rendering rule, not a classification bug.
Fix: the shared validation-message renderer (strip the anchors when quoting a @pattern).
Traces: bedrock-agentcore-control-agent-runtime-control-plane-errors#task#1 (representative).

Cause 2 of 8: ListBrowserProfiles name - same anchor rendering (@pattern ^[a-zA-Z][a-zA-Z0-9_]{0,47}$ on BrowserProfileName).
Emulator: anchored pattern in the message. Fix/Why as cause 1.
Traces: acc-browser-profile-name-scoped-listing#task#4.

Cause 3 of 8: CreatePaymentManager name - two validation errors; the length clause matches, the pattern clause is rendered anchored ('^[a-zA-Z][a-zA-Z0-9]{0,47}$' vs AWS '[a-zA-Z][a-zA-Z0-9]{0,47}').
Fix/Why as cause 1.
Traces: bedrock-agentcore-control-payment-error-paths#task#1.

Cause 4 of 8: CreateMemory name - anchors plus an un-elided value.
AWS: Smithy CreateMemory.name -> Name @pattern ^[a-zA-Z][a-zA-Z0-9_]{0,47}$, ValidationException (400). The recording renders "Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][a-zA-Z0-9_]{0,47}" - no value echoed (recorded behavior only; the member is not marked @sensitive in the model).
Emulator: "Value 'sim2real-probe-acc-mem-f4cd2ee7' at 'name' ... pattern: ^[a-zA-Z][a-zA-Z0-9_]{0,47}$" - echoes the value and anchors the pattern. Cluster covers acc-memory-update-and-strategy-round-trip#task#3 and bedrock-agentcore-control-memory-validation-errors#task#1.
Fix: same renderer; suppress the value for the members AWS renders value-less.
Traces: acc-memory-update-and-strategy-round-trip#task#3.

Cause 5 of 8: CreateConfigurationBundle bundleName - anchors plus un-elided value (@pattern ^[a-zA-Z][a-zA-Z0-9_]{0,99}$; AWS renders "Value at 'bundleName' ... pattern: [a-zA-Z][a-zA-Z0-9_]{0,99}").
Fix/Why as cause 4.
Traces: bedrock-agentcore-control-configuration-bundle-errors#task#1.

Cause 6 of 8: CreateConsentPortal name - anchors plus un-elided value (@pattern ^[a-zA-Z0-9_-]{1,50}$; AWS renders "Value at 'name' ... pattern: [a-zA-Z0-9_-]{1,50}").
Fix/Why as cause 4.
Traces: bedrock-agentcore-control-consent-portal-errors#task#3.

Cause 7 of 8: CreateWorkloadIdentity name - anchors plus un-elided value (@pattern ^[A-Za-z0-9_.-]+$; AWS renders "Value at 'name' ... pattern: [A-Za-z0-9_.-]+").
Fix/Why as cause 4.
Traces: bedrock-agentcore-control-workload-identity-error-paths#task#2.

Cause 8 of 8: UpdateWorkloadIdentity allowedResourceOauth2ReturnUrls - wrong member path and a single constraint instead of the aggregated member constraints.
AWS: Smithy ResourceOauth2ReturnUrlListType member carries @length 1..2048 and @pattern ^\w+:(\/?\/?)[^\s]+$. The recording renders one error against the list member itself: "Value at 'allowedResourceOauth2ReturnUrls' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 2048, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: \w+:(\/?\/?)[^\s]+]" - the list name, no value, and the member's whole constraint set in brackets with anchors stripped.
Emulator: "Value 'not a return url' at 'allowedResourceOauth2ReturnUrls.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\w+:(\/?\/?)[^\s]+$" - indexed path, echoed value, single anchored constraint.
Why: the message no longer names the member AWS names, so clients that surface the failing field point at a path AWS never returns.
Fix: the same renderer's handling of constrained list members (report the list member, aggregate the member's constraints).
Traces: acc-workload-identity-update-round-trip#task#5.

## Evidence
All eight clusters agree on status 400 and on the ValidationException classification; only $.message diverges.
Cause 1 bedrock-agentcore-control-agent-runtime-control-plane-errors#task#1: expected '... pattern: [a-zA-Z][a-zA-Z0-9_]{0,47}', actual '... pattern: ^[a-zA-Z][a-zA-Z0-9_]{0,47}$'.
Cause 2 acc-browser-profile-name-scoped-listing#task#4: same anchored-pattern delta on 'name'.
Cause 3 bedrock-agentcore-control-payment-error-paths#task#1: '2 validation errors detected: ...' identical except '^[a-zA-Z][a-zA-Z0-9]{0,47}$' vs '[a-zA-Z][a-zA-Z0-9]{0,47}'.
Cause 4 acc-memory-update-and-strategy-round-trip#task#3 (+bedrock-agentcore-control-memory-validation-errors#task#1): expected "Value at 'name' ... [a-zA-Z][a-zA-Z0-9_]{0,47}", actual "Value 'sim2real-probe-acc-mem-f4cd2ee7' at 'name' ... ^[a-zA-Z][a-zA-Z0-9_]{0,47}$".
Cause 5 bedrock-agentcore-control-configuration-bundle-errors#task#1: expected "Value at 'bundleName' ... [a-zA-Z][a-zA-Z0-9_]{0,99}", actual value echoed + anchored pattern.
Cause 6 bedrock-agentcore-control-consent-portal-errors#task#3: expected "Value at 'name' ... [a-zA-Z0-9_-]{1,50}", actual "Value 'bad portal name!' at 'name' ... ^[a-zA-Z0-9_-]{1,50}$".
Cause 7 bedrock-agentcore-control-workload-identity-error-paths#task#2: expected "Value at 'name' ... [A-Za-z0-9_.-]+", actual "Value 'a!a' at 'name' ... ^[A-Za-z0-9_.-]+$".
Cause 8 acc-workload-identity-update-round-trip#task#5: expected "Value at 'allowedResourceOauth2ReturnUrls' ... Member must satisfy constraint: [Member must have length less than or equal to 2048, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: \\w+:(\\/?\\/?)[^\\s]+]", actual "Value 'not a return url' at 'allowedResourceOauth2ReturnUrls.1.member' ... pattern: ^\\w+:(\\/?\\/?)[^\\s]+$".
Model grounding: every pattern quoted above is the member's @pattern as modelled (anchored), confirming AWS strips the outer anchors when rendering and the emulator does not.

## Steps to Reproduce
1. PUT /runtimes/ with agentRuntimeName '1_starts_with_digit' (bedrock-agentcore-control-agent-runtime-control-plane-errors task#1): the 400 body quotes '^[a-zA-Z][a-zA-Z0-9_]{0,47}$' where AWS quotes '[a-zA-Z][a-zA-Z0-9_]{0,47}'.
2. Repeat with POST /browser-profiles (name filter), POST /payment-managers, POST /memories/create, POST /configuration-bundles/create, POST /identities/CreateConsentPortal and POST /identities/CreateWorkloadIdentity using the invalid names in the listed fixtures: same anchor rendering, and for memory/bundle/consent-portal/workload-identity names the emulator also echoes the offending value AWS omits.
3. POST /identities/UpdateWorkloadIdentity with allowedResourceOauth2ReturnUrls containing 'not a return url' (acc-workload-identity-update-round-trip task#5): the emulator reports 'allowedResourceOauth2ReturnUrls.1.member' with one anchored pattern instead of 'allowedResourceOauth2ReturnUrls' with the aggregated member constraints.
