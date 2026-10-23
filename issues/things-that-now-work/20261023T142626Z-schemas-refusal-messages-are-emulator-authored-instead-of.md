Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-schema-errors
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-schema-errors schemas-registry-resource-policy
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|CreateSchema|4xx->4xx|$.Message
Issue-Key: schemas|PutResourcePolicy|4xx->4xx|$.Message
Issue-Commit: 17c220c4f82dc0c00315f2111baf55aa4ec61156

# schemas refusal messages are emulator-authored instead of the service's text

> _Note: this issue was identified against the code base at commit `17c220c4f82dc0c00315f2111baf55aa4ec61156`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchema

## Description
Two refusals carry the right status and error class but a message this emulator wrote itself. This issue bundles TWO causes; both must be closed.

Cause 1 of 2: CreateSchema's invalid-JSON message is not built the way AWS builds it.
AWS: Smithy: CreateSchema POST /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}, Content is `@required` with @length(1, 100000); error BadRequestException (httpError 400, members Code and Message). The message is built FROM the request: AWS reports the offending character and its byte offset within the submitted Content — recorded as `Content is not valid JSON: invalid character 'o' in literal null (expecting 'u') at offset 2`. The rule (prefix `Content is not valid JSON: `, then the offending character, the literal being parsed, and a 0-based byte `offset` into Content) is therefore the requirement; the exact recorded sentence is one example of it.
Emulator: same status 400 and same prefix, but the detail is its own parser's line/column diagnostic — `Content is not valid JSON: expected ident at line 1 column 2`.
Why: the offset is the only part of the refusal that tells a client WHERE in its document the defect is. A line/column pair taken from a different parser points somewhere else for any multi-line document and uses a vocabulary (`expected ident`) no AWS client or test fixture matches, so an editor or CI step that highlights the bad byte highlights the wrong one.
Fix: the CreateSchema handler's Content validation — report the JSON parse failure as an offending-character/byte-offset message instead of forwarding serde_json's line/column text (the same text is produced wherever Content is parsed, e.g. alongside `canonical_json` in the service state).
Traces: schemas-schema-errors#task#2.

Cause 2 of 2: PutResourcePolicy's revision-mismatch message is rewritten and leaks the rejected revision id.
AWS: Smithy: PutResourcePolicy PUT /v1/policy with optional RevisionId; error PreconditionFailedException (httpError 412, members Code and Message). Recorded behavior only: on a stale RevisionId the service returns 412 with the fixed sentence `The RevisionId provided does not match the latest RevisionId for the registry policy.` — a constant containing no request-derived or environment-derived value, so the constant itself is the requirement.
Emulator: returns the correct 412 but with an emulator-authored sentence that embeds the rejected revision id and advisory prose: `RevisionId 00000001-0000-4000-8000-000000000001 is not the current revision of this resource-based policy; read the policy again and retry with the revision it reports.`
Why: besides not matching AWS's fixed text, the substituted message embeds a generated revision id, so the refusal is not even stable across runs for a client or test that compares messages.
Fix: the PutResourcePolicy handler's precondition check / the PreconditionFailedException payload construction in the schemas service state — emit the service's constant text.
Traces: schemas-registry-resource-policy#task#6.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, 13 fixtures, 289 steps).

Cause 1 — schemas-schema-errors#task#2, CreateSchema, expected 400 / actual 400 (status and error class agree):
  $.Message: value_mismatch (expected "Content is not valid JSON: invalid character 'o' in literal null (expecting 'u') at offset 2", got 'Content is not valid JSON: expected ident at line 1 column 2')
  divergence_key service=schemas|CreateSchema|status=400->400|keys=$.Message

Cause 2 — schemas-registry-resource-policy#task#6, PutResourcePolicy, expected 412 / actual 412 (status and error class agree):
  $.Message: value_mismatch (expected 'The RevisionId provided does not match the latest RevisionId for the registry policy.', got 'RevisionId 00000001-0000-4000-8000-000000000001 is not the current revision of this resource-based policy; read the policy again and retry with the revision it reports.')
  divergence_key service=schemas|PutResourcePolicy|status=412->412|keys=$.Message
  (The emulator's revision ids are minted by mint_policy_revision_id in services/service-com-amazonaws-schemas-schemas/src/state.rs:44-48, so the substituted message carries a generated value.)

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture schemas-schema-errors, step task#2 — POST /v1/registries/name/<registry>/schemas/name/<schema> with a Content body whose third byte breaks a `null` literal; the 400 BadRequestException Message reports a line/column instead of the offending character and byte offset.
3. Cause 2: fixture schemas-registry-resource-policy, step task#6 — PUT /v1/policy?registryName=<registry> twice, replaying the first call's RevisionId on the second; the 412 PreconditionFailedException Message is the emulator's own sentence with the revision id embedded.
