Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-storage-lens-configuration-validation-boundary
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-storage-lens-configuration-validation-boundary s3control-storage-lens-configuration-absent-envelopes s3control-storage-lens-configuration-lifecycle-and-tagging s3control-storage-lens-configuration-put-replaces-whole-document
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|PutStorageLensConfiguration|4xx->2xx|$
Issue-Key: s3control|PutStorageLensConfiguration|4xx->2xx|status
Issue-Key: s3control|PutStorageLensConfiguration|2xx->2xx|status
Issue-Key: s3control|PutStorageLensConfiguration|4xx->4xx|$.Error
Issue-Key: s3control|DeleteStorageLensConfiguration|2xx->2xx|status
Issue-Key: s3control|GetStorageLensConfiguration|4xx->4xx|$.Error
Issue-Key: s3control|GetStorageLensConfigurationTagging|4xx->4xx|$.Error
Issue-Key: s3control|DeleteStorageLensConfigurationTagging|4xx->4xx|$.Error
Issue-Key: s3control|DeleteStorageLensConfiguration|4xx->4xx|$.Error
Issue-Key: s3control|PutStorageLensConfigurationTagging|2xx->2xx|status
Issue-Key: s3control|DeleteStorageLensConfigurationTagging|2xx->2xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control Storage Lens configuration errors omit the flat Error envelope and 204 statuses

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStorageLensConfiguration

## Description
10 net-new root causes on the s3control Storage Lens surface. They share one fault family: the Storage Lens operations neither validate the config id nor the document, and their refusals omit the flat top-level `<Error>` envelope this sub-surface uses (unlike the rest of s3control, which wraps errors in `<ErrorResponse>`), while their empty-body successes answer 200 rather than 204.

--- Cause 1 of 10: PutStorageLensConfiguration accepts a config id the wire rejects ---
AWS: recorded behavior only. Smithy: PutStorageLensConfiguration.ConfigId @pattern ^[a-zA-Z0-9\-\_\.]+$ — the model permits `.`, but the recorded wire refuses a dotted id at URI-parse time with HTTP 400 and the flat envelope `<Error><Code>InvalidURI</Code><Message>Couldn't parse the specified URI.</Message><URI>...</URI><RequestId>..</RequestId><HostId>..</HostId></Error>`. Fixed text: the code `InvalidURI` and the message `Couldn't parse the specified URI.`; the variable part is `URI`, the request path taken from the request (`storagelens/<ConfigId>`), plus the per-run RequestId/HostId.
Emulator: answers 200 with an empty body — the configuration is accepted and stored.
Why: a client probing id syntax believes a dotted config id is valid and later cannot address the configuration the way AWS would.
Fix: PutStorageLensConfiguration request validation in the s3control crate — reject a config id the wire cannot parse with 400 InvalidURI before touching state.
Traces: s3c-storage-lens-configuration-validation-boundary#task#1 (3 steps in cluster).

--- Cause 2 of 10: PutStorageLensConfiguration success status ---
AWS: recorded behavior only — a successful put answers 204 No Content (the model's http code 200 is stale for this empty-body member).
Emulator: answers 200.
Why: a client distinguishing 204 from 200 expects no body only on 204.
Fix: the PutStorageLensConfiguration handler's success path.
Traces: s3c-storage-lens-configuration-validation-boundary#task#2 (6 steps in cluster).

--- Cause 3 of 10: PutStorageLensConfiguration malformed document refusal has no body ---
AWS: recorded behavior only — a malformed/invalid Storage Lens document answers 400 with the flat envelope `<Error><Code>MalformedXML</Code><Message>The XML you provided was not well-formed or did not validate against our published schema</Message>...</Error>`. Fixed text: code `MalformedXML` and that exact message; variable parts: RequestId/HostId.
Emulator: answers 400 but with no `$.Error` element at all.
Why: a client cannot read the error code or message of a refusal it did get, so it cannot distinguish a schema problem from any other 400.
Fix: the PutStorageLensConfiguration error serializer — emit the flat Error envelope for this sub-surface.
Traces: s3c-storage-lens-configuration-validation-boundary#task#6.

--- Cause 4 of 10: DeleteStorageLensConfiguration success status ---
AWS: recorded behavior only — 204 No Content.
Emulator: answers 200.
Why: as cause 2.
Fix: the DeleteStorageLensConfiguration handler's success path.
Traces: s3c-storage-lens-configuration-validation-boundary#teardown#9 (4 steps in cluster).

--- Cause 5 of 10: GetStorageLensConfiguration absent-config refusal has no body ---
AWS: recorded behavior only — 404 with the flat envelope `<Error><Code>NoSuchConfiguration</Code><Message>The specified configuration does not exist.</Message><RequestId>..</RequestId><HostId>..</HostId></Error>`. Fixed text: code `NoSuchConfiguration` and that exact message; only RequestId/HostId vary per run.
Emulator: answers 404 with no `$.Error` element.
Why: the client gets a bare 404 and cannot tell NoSuchConfiguration from any other not-found condition; error-code-driven retry/branching breaks.
Fix: the GetStorageLensConfiguration error path and the Storage Lens error serializer.
Traces: s3control-storage-lens-configuration-absent-envelopes#task#4 (4 steps in cluster).

--- Cause 6 of 10: GetStorageLensConfigurationTagging absent-config refusal has no body ---
AWS: recorded behavior only — 404 NoSuchConfiguration with the same flat envelope and the message `The specified configuration does not exist.`
Emulator: answers 404 with no `$.Error` element.
Why: as cause 5.
Fix: the GetStorageLensConfigurationTagging error path.
Traces: s3control-storage-lens-configuration-absent-envelopes#task#5.

--- Cause 7 of 10: DeleteStorageLensConfigurationTagging absent-config refusal has no body ---
AWS: recorded behavior only — 404 NoSuchConfiguration, flat envelope, message `The specified configuration does not exist.`
Emulator: answers 404 with no `$.Error` element.
Why: as cause 5.
Fix: the DeleteStorageLensConfigurationTagging error path.
Traces: s3control-storage-lens-configuration-absent-envelopes#task#6.

--- Cause 8 of 10: DeleteStorageLensConfiguration absent-config refusal has no body ---
AWS: recorded behavior only — 404 NoSuchConfiguration, flat envelope, message `The specified configuration does not exist.`
Emulator: answers 404 with no `$.Error` element.
Why: as cause 5.
Fix: the DeleteStorageLensConfiguration error path.
Traces: s3control-storage-lens-configuration-absent-envelopes#task#7 (2 steps in cluster).

--- Cause 9 of 10: PutStorageLensConfigurationTagging success status ---
AWS: recorded behavior only — 204 No Content.
Emulator: answers 200.
Why: as cause 2.
Fix: the PutStorageLensConfigurationTagging handler's success path.
Traces: s3control-storage-lens-configuration-lifecycle-and-tagging#task#6.

--- Cause 10 of 10: DeleteStorageLensConfigurationTagging success status ---
AWS: recorded behavior only — 204 No Content.
Emulator: answers 200.
Why: as cause 2.
Fix: the DeleteStorageLensConfigurationTagging handler's success path.
Traces: s3control-storage-lens-configuration-lifecycle-and-tagging#task#8.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator:
- PutStorageLensConfiguration (dotted id): `status: expected 400, got 200` and `$: empty_body (expected {'ErrorResponse': ... 'Code': 'InvalidURI', 'Message': "Couldn't parse the specified URI.", 'URI': 'storagelens/<configId>' ...}, got {})` — s3c-storage-lens-configuration-validation-boundary#task#1, 3 affected steps.
- PutStorageLensConfiguration success: `status: expected 204, got 200` — same fixture task#2, 6 affected steps.
- PutStorageLensConfiguration malformed document: `$.Error: missing (expected {'Code': 'MalformedXML', 'Message': 'The XML you provided was not well-formed or did not validate against our published schema', ...}, got None)` — same fixture task#6.
- DeleteStorageLensConfiguration success: `status: expected 204, got 200` — same fixture teardown#9, 4 affected steps.
- GetStorageLensConfiguration / GetStorageLensConfigurationTagging / DeleteStorageLensConfigurationTagging / DeleteStorageLensConfiguration on an absent config: all four report `$.Error: missing (expected {'Code': 'NoSuchConfiguration', 'Message': 'The specified configuration does not exist.', ...}, got None)` at HTTP 404 — s3control-storage-lens-configuration-absent-envelopes#task#4/5/6/7.
- PutStorageLensConfigurationTagging and DeleteStorageLensConfigurationTagging successes: `status: expected 204, got 200` — s3control-storage-lens-configuration-lifecycle-and-tagging#task#6 and #task#8.

## Steps to Reproduce
1. Replay s3c-storage-lens-configuration-validation-boundary#task#1: PUT /v20180820/storagelens/<id-with-dots> with a valid Storage Lens document. The emulator stores it and answers 200; the recording answers 400 InvalidURI with the flat Error envelope naming the URI.
2. Same fixture task#2 / teardown#9 and s3control-storage-lens-configuration-lifecycle-and-tagging#task#6/#task#8: successful put/delete of a configuration or its tagging answers 200 instead of 204.
3. Same fixture task#6: PUT a malformed Storage Lens document — the emulator answers 400 with no Error element; the recording carries Code MalformedXML.
4. Replay s3control-storage-lens-configuration-absent-envelopes#task#4..#task#7: GET/DELETE configuration and its tagging for an id that was never created — the emulator answers a bodyless 404; the recording carries Code NoSuchConfiguration and Message 'The specified configuration does not exist.'
