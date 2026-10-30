Issue-Source: trace-sweep-divergence
Issue-Trace: keyspaces-keyspace-lifecycle-tags-and-rejections
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: keyspaces-keyspace-lifecycle-tags-and-rejections keyspaces-table-create-active-transition-and-tags keyspaces-type-lifecycle-and-rejections
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces|CreateKeyspace|4xx->4xx|$.message
Issue-Key: keyspaces|CreateKeyspace|4xx->4xx|status
Issue-Key: keyspaces|GetKeyspace|4xx->4xx|$.message
Issue-Key: keyspaces|GetKeyspace|4xx->4xx|status
Issue-Key: keyspaces|GetTable|4xx->4xx|status
Issue-Key: keyspaces|CreateType|4xx->4xx|$.message
Issue-Key: keyspaces|CreateType|4xx->4xx|status
Issue-Key: keyspaces|GetType|4xx->4xx|status
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# keyspaces serves modeled client exceptions with 409/404 instead of the wire's HTTP 400, and words two refusals itself

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeyspace

## Description
This issue bundles 5 net-new root causes in the keyspaces crate, all from the same fault family: the HTTP status line the emulator puts on a modeled client exception. Amazon Keyspaces is an awsJson1_0 service and answers EVERY client-side exception on the recorded wire with HTTP 400, carrying the exception type in the error payload / X-Amzn-Errortype; the emulator instead derives the status from the Smithy @httpError trait (ConflictException 409, ResourceNotFoundException 404) and so answers 409 or 404. Two of the five additionally word the refusal sentence themselves. Five causes, all 5 must be closed before submitting.

Cause 1 of 5: CreateKeyspace on a keyspace name already held — status AND message.
AWS: Smithy (services/service-com-amazonaws-keyspaces-keyspacesservice/smithy-model/com-amazonaws-keyspaces-keyspacesservice.json) declares CreateKeyspace error ConflictException (errorKind client, httpError 409). The RECORDED wire response for this refusal is HTTP 400 with error code ConflictException and message `Cannot add existing keyspace <keyspaceName>` — recorded behavior is authoritative for the status, and the httpError trait is inert for this protocol's error status on the wire. Fixed text: `Cannot add existing keyspace ` — the only variable part is the keyspaceName exactly as supplied in the request: unquoted, no trailing period, no Region clause.
Emulator: answers HTTP 409 with message `Keyspace '<keyspaceName>' already exists in Region '<region>'.` — wrong status, name single-quoted, an extra Region clause taken from the credential scope, trailing period.
Why: SDK/client retry and error-classification layers branch on the HTTP status before the payload; a 409 is treated as a different class of failure (and by some HTTP intermediaries as retryable/redirectable) than the 400 AWS returns, so 'name already taken' handling written against AWS never runs. Clients matching the refusal sentence also miss.
Fix: the keyspaces error-to-HTTP mapping in the crate's protocol error serializer (it must emit 400 for client-kind exceptions rather than the modeled httpError), plus the CreateKeyspace handler's duplicate-name ConflictException message template.
Traces: keyspaces-keyspace-lifecycle-tags-and-rejections#task#4.

Cause 2 of 5: GetKeyspace for a keyspace that does not exist — status AND message.
AWS: Smithy declares GetKeyspace error ResourceNotFoundException (errorKind client, httpError 404), doc 'The operation tried to access a keyspace, table, or type that doesn't exist.' The RECORDED response is HTTP 400 with code ResourceNotFoundException and message `Keyspace <keyspaceName> does not exist`. Fixed text: `Keyspace ` + ` does not exist`; variable part is the keyspaceName from the request, unquoted, no trailing period.
Emulator: answers HTTP 404 with `Keyspace '<keyspaceName>' was not found.`
Why: a caller distinguishing 'absent' from 'transport/routing failure' sees 404, which many HTTP stacks and proxies surface as a routing error rather than a modeled API error, so the absent-keyspace branch is never taken; the sentence also does not match.
Fix: same shared client-error status mapping as Cause 1, plus the shared keyspaces not-found payload builder used by the GetKeyspace handler.
Traces: keyspaces-keyspace-lifecycle-tags-and-rejections#task#7.

Cause 3 of 5: CreateType for a UDT that already exists in the keyspace — status AND message.
AWS: Smithy declares CreateType error ConflictException (errorKind client, httpError 409). The RECORDED response is HTTP 400 with code ConflictException and message `Cannot add already existing UDT <typeName> to keyspace <keyspaceName>`. Fixed text: `Cannot add already existing UDT ` + ` to keyspace `; variable parts are the typeName and keyspaceName from the request, each unquoted and NOT keyspace-qualified, no trailing period.
Emulator: answers HTTP 409 with `Type '<keyspaceName>.<typeName>' already exists.` — wrong status, different fixed text, and it renders a dotted keyspace-qualified type name AWS does not use in this sentence.
Why: idempotent 'create type if missing' flows key off the 400/ConflictException pair and off the sentence; with a 409 and a different sentence they treat an already-created type as an unexpected failure.
Fix: same shared client-error status mapping as Cause 1, plus the CreateType handler's already-exists ConflictException message template.
Traces: keyspaces-type-lifecycle-and-rejections#task#5.

Cause 4 of 5: GetTable for a table that does not exist — status only.
AWS: Smithy declares GetTable error ResourceNotFoundException (errorKind client, httpError 404). The RECORDED response is HTTP 400 with code ResourceNotFoundException; the message the emulator produces already matches.
Emulator: answers HTTP 404 with the correct code and message.
Why: as Cause 2 — the payload is right but the status line is not, so status-first error handling misclassifies a modeled API error as a transport/routing failure.
Fix: the shared client-error status mapping (no message change needed here).
Traces: keyspaces-table-create-active-transition-and-tags#task#5.

Cause 5 of 5: GetType for a type that does not exist — status only.
AWS: Smithy declares GetType error ResourceNotFoundException (errorKind client, httpError 404). The RECORDED response is HTTP 400 with code ResourceNotFoundException; message matches.
Emulator: answers HTTP 404.
Why: as Cause 4.
Fix: the shared client-error status mapping.
Traces: keyspaces-type-lifecycle-and-rejections#task#7.

Related open issue: '20261029T194003Z-keyspaces-refusal-messages-are-emulator-authored-prose-createkeyspace' describes the CreateKeyspace-conflict and GetKeyspace-not-found MESSAGE wording from the behavioral corpus and asserts the 409/404 statuses as correct. It does not carry these wire replay keys and does not cover the status-line defect or CreateType's sentence; whoever fixes that issue must not conclude the statuses are right.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, /__admin/health 200):

Cause 1 — keyspaces-keyspace-lifecycle-tags-and-rejections#task#4, CreateKeyspace:
  status: expected 400, got 409
  $.message: expected 'Cannot add existing keyspace <keyspaceName>', got "Keyspace '<keyspaceName>' already exists in Region '<region>'."
  (recorded run used keyspaceName sim2real_wire_ks1_694a55df and region us-west-2 — per-run values that fill the template, not part of it)

Cause 2 — keyspaces-keyspace-lifecycle-tags-and-rejections#task#7, GetKeyspace:
  status: expected 400, got 404
  $.message: expected 'Keyspace <keyspaceName> does not exist', got "Keyspace '<keyspaceName>' was not found."

Cause 3 — keyspaces-type-lifecycle-and-rejections#task#5, CreateType:
  status: expected 400, got 409
  $.message: expected 'Cannot add already existing UDT <typeName> to keyspace <keyspaceName>', got "Type '<keyspaceName>.<typeName>' already exists."

Cause 4 — keyspaces-table-create-active-transition-and-tags#task#5, GetTable:
  status: expected 400, got 404 (no message divergence reported)

Cause 5 — keyspaces-type-lifecycle-and-rejections#task#7, GetType:
  status: expected 400, got 404 (no message divergence reported)

In all five the error CODE matched; the status line diverged in all five and the message in Causes 1-3.

## Steps to Reproduce
Against the emulator's keyspaces endpoint (awsJson1_0), inspecting the raw HTTP status, not just the SDK error type:

Cause 1: CreateKeyspace(keyspaceName=K); poll until K is ACTIVE; CreateKeyspace(keyspaceName=K) again. Expect HTTP 400, ConflictException, message `Cannot add existing keyspace K`.
Cause 2: GetKeyspace(keyspaceName=K2) for a name never created. Expect HTTP 400, ResourceNotFoundException, message `Keyspace K2 does not exist`.
Cause 3: CreateKeyspace(K3); CreateType(K3, T, fieldDefinitions=[{name:f, type:text}]); poll until T is ACTIVE; CreateType(K3, T, same fields) again. Expect HTTP 400, ConflictException, message `Cannot add already existing UDT T to keyspace K3`.
Cause 4: GetTable(keyspaceName=K3, tableName=TB) for a table never created. Expect HTTP 400, ResourceNotFoundException.
Cause 5: GetType(keyspaceName=K3, typeName=T2) for a type never created. Expect HTTP 400, ResourceNotFoundException.

Replaying the fixtures/steps in Issue-Trace/Issue-Step and Issue-Trace-Ids reproduces each cause end to end.
