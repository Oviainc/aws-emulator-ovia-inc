Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-api-key-duplicate-value-conflict
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-api-key-duplicate-value-conflict apigateway-api-key-enable-disable-cycle apigateway-api-key-import-update-lifecycle apigateway-api-key-lifecycle apigateway-api-key-lifecycle-and-bulk-import apigateway-api-key-lifecycle-with-tags apigateway-api-key-list-lifecycle apigateway-api-key-tag-lifecycle apigateway-api-key-update-lifecycle apigateway-api-key-usage-plan-workflow apigateway-apikey-and-tag-error-paths apigateway-apikey-create-patch-update-and-lifecycle apigateway-apikey-update-lifecycle apigateway-tag-api-key-lifecycle apigateway-usage-plan-key-association-and-usage-tracking apigateway-usage-plan-tagging-with-apikey-association apigateway-apikey-stage-association-via-patch-add-remove apigateway-tag-isolation-across-resource-types apigateway-stage-api-key-toggle apigateway-stage-restricted-api-key-toggle
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateApiKey|2xx->2xx|$.stageKeys
Issue-Related: 20261022T142802Z-apigateway-patch-update-operations-are-accepted-but-never-applied.md
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Empty stageKeys and tags members omitted from ApiKey responses

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApiKey

## Description
Four causes, one shared fault: the ApiKey serializer drops collection members when they are empty, where AWS emits them as an empty list / empty map. This is a wire-shape divergence (the status matches everywhere), and it affects both the create and the read paths.

Cause 1 of 4: CreateApiKey response omits `stageKeys: []`.
AWS: CreateApiKey is POST /apikeys with success 201 and output shape ApiKey, whose members include stageKeys (ListOfString) and tags (MapOfStringToString) — Smithy: CreateApiKey output {id, value, name, customerId, description, enabled, createdDate, lastUpdatedDate, stageKeys, tags}. For a key created with no stage associations the recording shows `stageKeys: []` present in the body.
Emulator: `$.stageKeys` is absent from the 201 body (`missing (expected [], got None)`).
Why: a client that iterates apiKey.stageKeys without a null guard throws rather than iterating zero elements; typed SDKs report the list as unset instead of empty, and code that distinguishes "no stages" from "not returned" takes the wrong branch.
Fix: the ApiKey output serializer — emit empty collections rather than skipping them.
Traces: fixture `apigateway-api-key-duplicate-value-conflict`, step `setup#1`; 17 affected steps across the api-key lifecycle, import, tag and usage-plan fixtures.

Cause 2 of 4: GetApiKey omits both `stageKeys: []` and `tags: {}`.
AWS: GetApiKey (GET /apikeys/{apiKey}, 200) returns the same ApiKey shape, so both members are present and empty for an untagged key with no stage associations.
Emulator: both `$.stageKeys` and `$.tags` are missing from the 200 body.
Why: as Cause 1, and the missing `tags` map makes it impossible for a client to tell an untagged key from one whose tags were not serialized.
Fix: the shared ApiKey serializer used by the read path.
Traces: fixture `apigateway-api-key-duplicate-value-conflict`, step `task#3`; 12 affected steps.

Cause 3 of 4: GetApiKey omits `stageKeys: []` on the plain lifecycle read.
Emulator: `$.stageKeys: missing (expected [], got None)` — the same omission reached through a fixture that asserts only that member, so it is its own recorded unit to re-replay.
Why/Fix: as Cause 2.
Traces: fixture `apigateway-api-key-lifecycle`, step `task#2`.

Cause 4 of 4: GetApiKey omits `tags: {}` on a key that does have stage associations.
Emulator: `$.tags: missing (expected {}, got None)` while stageKeys is correctly populated — showing the omission is per-member and empty-valued, not a wholesale loss of collections.
Why/Fix: as Cause 2.
Traces: fixture `apigateway-stage-api-key-toggle`, step `task#10`; 4 affected steps (also apigateway-stage-restricted-api-key-toggle#task#10 and the #task#14 pair).

## Evidence
Representative (Cause 1): fixture apigateway-api-key-duplicate-value-conflict step setup#1, operation CreateApiKey, expected HTTP 201 / actual HTTP 201, divergence_key `service=apigateway|CreateApiKey|status=201->201|keys=$.stageKeys`, detail `$.stageKeys: missing (expected [], got None)`; affected_count=17.
Cause 2: apigateway-api-key-duplicate-value-conflict#task#3 — GetApiKey 200/200, `$.stageKeys: missing (expected [], got None)` and `$.tags: missing (expected {}, got None)`; affected_count=12.
Cause 3: apigateway-api-key-lifecycle#task#2 — `$.stageKeys: missing (expected [], got None)`.
Cause 4: apigateway-stage-api-key-toggle#task#10 — `$.tags: missing (expected {}, got None)`; affected_count=4.
Smithy evidence: CreateApiKey POST /apikeys code 201 and GetApiKey GET /apikeys/{apiKey} code 200 both return output shape ApiKey, whose members include stageKeys (ListOfString) and tags (MapOfStringToString).
Run summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, state_reset_failures=0.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay wire fixture `apigateway-api-key-duplicate-value-conflict` and inspect step `setup#1`: the CreateApiKey 201 body has no `stageKeys` key at all where the recording has `stageKeys: []`.
3. Inspect step `task#3` of the same fixture: the GetApiKey 200 body omits both `stageKeys: []` and `tags: {}`.
4. Replay `apigateway-api-key-lifecycle` step `task#2` (stageKeys) and `apigateway-stage-api-key-toggle` step `task#10` (tags).
5. All four units must emit the empty collection members before this issue is closed.
