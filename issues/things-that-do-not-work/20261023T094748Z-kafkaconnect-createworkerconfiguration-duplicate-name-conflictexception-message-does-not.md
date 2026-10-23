Issue-Source: trace-sweep-divergence
Issue-Trace: kafkaconnect-worker-configuration-duplicate-name
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafkaconnect-worker-configuration-duplicate-name
Issue-Shape-Id: com.amazonaws.kafkaconnect#KafkaConnect
Issue-Key: kafkaconnect|CreateWorkerConfiguration|4xx->4xx|$.message
Issue-Commit: 3113486ab2df4ef97dea8b3047048aff3439051f

# kafkaconnect CreateWorkerConfiguration duplicate-name ConflictException message does not name the offending parameter

> _Note: this issue was identified against the code base at commit `3113486ab2df4ef97dea8b3047048aff3439051f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkerConfiguration

## Description
One cause, in the kafkaconnect CreateWorkerConfiguration conflict path: the 409 ConflictException is raised with a free-form, instance-specific sentence instead of the modeled conflict message naming the offending input member.

Cause 1 of 1: ConflictException on a duplicate worker-configuration name carries the wrong message shape.
AWS: POST /v1/worker-configurations models ConflictException with httpError 409 (Smithy: CreateWorkerConfiguration errors include ConflictException=409; ConflictException has a single optional `message` member of type __string). The model states the message text for this error: ConflictException is documented as "HTTP Status Code 409: Conflict. A resource with this name already exists. Retry your request with another name." The recording shows kafkaconnect emitting that documented sentence prefixed with the service's standard offending-parameter prefix, i.e. the rule is "Invalid parameter <inputMemberName>: <documented reason>" — here "Invalid parameter name: A resource with this name already exists." The same rule is visible elsewhere in this corpus for the same service ("Invalid parameter workerConfigurationArn: The requested resource doesn't exist.", "Invalid parameter s3Location: Could not access the specified custom plugin file. ..."), so the member-naming prefix is the rule rather than one recording's wording. The requirement is therefore: HTTP 409, error code ConflictException, and a message that names the offending input member (`name`) followed by the modeled conflict reason — not the recorded sentence byte-for-byte.
Emulator: HTTP 409 and the ConflictException error code are correct, but the message is "A worker configuration named <name> already exists (<arn>). Retry your request with another name." — it never names the offending input member, and it interpolates the resource name and a freshly generated worker-configuration ARN into the error text.
Why: a client cannot tell which input member conflicted from the message, so SDK/CLI error surfacing and any client-side mapping of kafkaconnect's "Invalid parameter <member>:" convention to a form field or retry decision gets nothing to key on. Interpolating a generated ARN also makes the message non-deterministic across environments, so log scraping and error-message assertions see a value that changes on every run even though the conflict is the same.
Fix: the CreateWorkerConfiguration handler's conflict/validation path — build the ConflictException message from the request's offending member in kafkaconnect's "Invalid parameter <member>: ..." form with the modeled conflict reason, and stop embedding the existing resource's generated ARN in the error text. Status and error code already match and need no change.
Traces: kafkaconnect-worker-configuration-duplicate-name#task#2.

Not covered by any open issue: the three open kafkaconnect issues (20261023T071142Z-kafkaconnect-createcustomplugin-accepts-an-unreachable-s3location-instead-of.md, ...-serializes-worker-configuration-creationtime-as-an-epoch.md, ...-worker-configuration-state-is-not-maintained-delete.md) concern s3Location validation, timestamp serialization, and worker-configuration delete/tag state respectively; none carries this replay key or touches the conflict message.

## Evidence
Cause 1 — kafkaconnect-worker-configuration-duplicate-name#task#2, CreateWorkerConfiguration (POST /v1/worker-configurations), creating a second worker configuration with a name already in use.

Status: expected 409, got 409 (match). Error code: ConflictException on both sides (x-amzn-errortype not flagged).

Only divergence is $.message:
  expected (recorded AWS example): 'Invalid parameter name: A resource with this name already exists.'
  actual   (emulator):             'A worker configuration named sim2real-probe-kafkaconnect-workercfg-dup-70ddde21 already exists (arn:aws:kafkaconnect:us-west-2:000000000000:worker-configuration/sim2real-probe-kafkaconnect-workercfg-dup-70ddde21/b57af15c-58aa-4de5-9253-9d8ef51043c5). Retry your request with another name.'

Smithy grounding: ConflictException = structure, errorKind client, httpError 409, members {message: __string}, doc "HTTP Status Code 409: Conflict. A resource with this name already exists. Retry your request with another name." CreateWorkerConfiguration.name is @required with @length min 1 max 128.

The specific name and the ARN above are the recording's/the emulator's own generated values and are not requirements; the requirement is the "Invalid parameter <member>: <modeled reason>" form and the absence of a generated ARN in the message.

## Steps to Reproduce
1. Boot the emulator and replay fixture kafkaconnect-worker-configuration-duplicate-name.
2. POST /v1/worker-configurations with a name N and valid propertiesFileContent; it succeeds (200).
3. POST /v1/worker-configurations again with the SAME name N (step task#2).
4. Observe HTTP 409 with error code ConflictException (correct) but a message that embeds the existing configuration's name and generated ARN, instead of a message naming the offending input member in kafkaconnect's documented form, e.g. 'Invalid parameter name: A resource with this name already exists.'
