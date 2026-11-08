Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-api-key-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-api-key-lifecycle wafv2-delete-apikey-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|CreateAPIKey|2xx->2xx|$.APIKey
Issue-Commit: 885965a44b7ceb28a47f9ba92cf63bd950a83073

# wafv2 CreateAPIKey mints an API-key value in the wrong form

> _Note: this issue was identified against the code base at commit `885965a44b7ceb28a47f9ba92cf63bd950a83073`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAPIKey

## Description
Cause 1 of 1: CreateAPIKey answers 200 with an $.APIKey value whose FORM is not the form WAF mints an API key in, so the fixture's shape assertion on the key value fails even though the call succeeds.

AWS: recorded behavior only for the value's syntax. Smithy (services/service-com-amazonaws-wafv2-awswaf-20190729 model): CreateAPIKey -> CreateAPIKeyResponse$APIKey, target APIKey, @length(min 1, max 2048), @pattern("\\S"), documented as "The generated, encrypted API key. You can copy this for use in your JavaScript CAPTCHA integration." The model fixes no further syntax, so the requirement here is the recorded one: the replayed step asserts the key by SHAPE (the recording's literal secret is not compared - it is reported as '<shaped-not-asserted>'), and the shape WAF's recorded key has is the padded standard-base64 envelope WAF emits, of a form/width that does not vary with the token domains the caller named. Hypothesis: the dimension that fails is the alphabet/padding and the resulting width - WAF's recorded key is standard base64 over A-Za-z0-9+/ with '=' padding, while the emulator's value visibly carries '_' and '-' and no padding.

Emulator: the replayed step returns 200 and an APIKey rendered as unpadded URL-safe base64 - a ~184-character token containing '-' and '_' and no '=' - which the replay reports as $.APIKey value_mismatch against the recorded shape. The value is built in the wafv2 state's API-key module: a 12-byte nonce plus the AES-GCM ciphertext and 16-byte tag of a NUL-separated payload (layout marker, scope, creation millis, a 64-hex-character key id, then the token domains), encoded with base64 URL_SAFE_NO_PAD. Because the sealed payload embeds the token domains themselves, the key's LENGTH also grows with the number and length of the domains the request named, which a real WAF key's width does not.

Why: a client that treats the API key as an opaque token WAF minted still sees a value of the wrong syntactic class. Anything that validates or transports the key by its recorded form - a fixture or SDK-level assertion on the key's character class or width, a consumer that round-trips it through a decoder expecting padded standard base64, or a test that asserts the key's width is independent of the token domains requested - succeeds against AWS and fails against the emulator. It also leaks the key's internal structure: the key's length discloses how many token domains it was defined over, which WAF's fixed-width key does not.

Fix: the APIKey minting in the wafv2 service state's API-key module (the value `new_api_key` renders and the coder it renders with), not the CreateAPIKey handler's validation - the handler already answers whatever the minter produced, and GetDecryptedAPIKey/DeleteAPIKey/ListAPIKeys read the same value back, so the encoder and the decoder/`is_api_key_shaped` form check must move together. Render the envelope in the form WAF mints (standard base64 alphabet with padding) and keep the sealed envelope's width independent of the requested token domains.

Traces: wafv2-api-key-lifecycle#task#1 (representative) and wafv2-delete-apikey-nonexistent-rejected#task#4, both CreateAPIKey, both expected 200 / got 200 with $.APIKey diverging in shape.

## Evidence
Replay cluster service=wafv2|CreateAPIKey|status=200->200|keys=$.APIKey, verdict mismatch, affected_count 2.
Expected: status 200, $.APIKey matching the recorded key's shape (reported as '<shaped-not-asserted>' because the literal secret is not compared).
Actual: status 200, $.APIKey = 'TY1wka2LID98IsEl23ONSawLRFfOvXpHxzxKlzXsO85A15swuenuwnY7e2qL1EZCZrF7nCgi_bDvLEclS9oAUBODMRjGXXn1gch-WBgBb7NN1Ov6vnB00JFhgZmdiLomciqapWbk6_XPLUVfVcRSXLHnbY61lk9tdSUZSHZVJiqJ' - an unpadded URL-safe base64 string (contains '_' and '-', no '='), ~184 characters. That one value is a per-run secret and is quoted only to show the FORM; it is not itself a requirement.
Source of the emulator's form: services/service-com-amazonaws-wafv2-awswaf-20190729/src/state/api_keys.rs lines 73-79 (`api_key_coder` -> base64 URL_SAFE_NO_PAD), lines 84-101 (`api_key_payload` embeds the token domains, so the envelope width tracks the request), lines 125-145 (`new_api_key`); the value is returned unchanged by services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/create_api_key.rs lines 266-300.
Smithy evidence: CreateAPIKeyResponse$APIKey -> APIKey, @length(1..2048), @pattern(\S); no further syntax modeled, hence "recorded behavior only" for the alphabet/padding/width.
No open issue owns this cluster (replay reports owned_by: [] and stub_path: ""); the nearest open wafv2 issues (20261029T192556Z-wafv2-request-validation-never-refuses-ten-operations-answer.md, 20261029T192556Z-wafv2-managed-rule-catalog-contents-and-wcu-capacity.md) cover refusal statuses and the managed-rule catalog, not the minted API-key value's form.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture wafv2-api-key-lifecycle, step task#1: POST the wafv2 JSON-RPC target AWSWAF_20190729.CreateAPIKey with a REGIONAL scope and a single-element TokenDomains list.
3. Read $.APIKey from the 200 response: it is unpadded URL-safe base64 ('-'/'_' present, no '=' padding) and its length varies with the TokenDomains supplied, so it does not match the shape of the recorded WAF key.
4. The same divergence reproduces at wafv2-delete-apikey-nonexistent-rejected#task#4, the other member of the cluster.
