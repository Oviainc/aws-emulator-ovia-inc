Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-automated-reasoning-policy-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-automated-reasoning-policy-lifecycle bedrock-automated-reasoning-policy-unset-vs-present-description-preserved
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetAutomatedReasoningPolicy|2xx->2xx|$.definitionHash
Issue-Key: bedrock|UpdateAutomatedReasoningPolicy|2xx->2xx|$.definitionHash
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.createdAt
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.definitionHash
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.description
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.name
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.policyArn
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.version
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|status
Issue-Commit: 52f91172eb2f0209f53a5585da2848c5cfe26576

# bedrock Automated Reasoning policy definitionHash is derived differently, blocking version creation (3 causes)

> _Note: this issue was identified against the code base at commit `52f91172eb2f0209f53a5585da2848c5cfe26576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAutomatedReasoningPolicy

## Description
THREE causes, one shared fault: the emulator's Automated Reasoning policy state derives the policy `definitionHash` by a different canonicalization/digest than AWS, so every read and write that carries the hash returns a token AWS would not have produced, and the one operation that consumes the token back as a pre-condition (CreateAutomatedReasoningPolicyVersion) then refuses the request.

NOTE ON PRIOR ART: these three replay keys were previously reported under 20261029T211357Z-bedrock-resource-state-defects-definitionhash-policy-version-creation.md, which is now in the RESOLVED folder (the tagging causes 4-6 of that issue no longer reproduce, but causes 1-3 do). A resolved issue does not cover a live failure, so the three causes are re-filed here against the current build. The replay reports no owner for any of them (owned_by is empty for all three clusters).

Cause 1 of 3: GetAutomatedReasoningPolicy returns a definitionHash that is not the digest AWS derives from the same policy definition.
AWS: GET /automated-reasoning-policies/{policyArn} answers 200 with a required `definitionHash` member. Smithy (com.amazonaws.bedrock, GetAutomatedReasoningPolicyResponse.definitionHash -> AutomatedReasoningPolicyHash): @required, @length min 128 max 128, @pattern ^[0-9a-z]{128}$ — a 128-character lowercase-hex digest. The requirement is not a particular literal but a FUNCTION: the same canonical policy definition must always yield the same digest AWS yields, because the value is the optimistic-concurrency token a client feeds back as `lastUpdatedDefinitionHash` to UpdateAutomatedReasoningPolicy and CreateAutomatedReasoningPolicyVersion (both model that member as @required AutomatedReasoningPolicyHash). The recorded digests in the fixtures are the values AWS produces for the definitions those fixtures create; they are inputs to the comparison, not literals to hard-code.
Emulator: answers 200 with a well-formed but different 128-hex digest for the same definition — the hashed canonical form, the field set included in it, or the digest function differs from AWS's. The divergence is stable and reproduces on 4 steps across 2 fixtures (bedrock-automated-reasoning-policy-lifecycle#task#2 and #task#5, bedrock-automated-reasoning-policy-unset-vs-present-description-preserved#task#3 and #task#4), and the same wrong digest is returned for the same definition each time, so it is a deterministic algorithm difference rather than a per-run value.
Why: a client that holds AWS's digest for a known definition (or that compares the digest across a read-modify-write to detect concurrent edits) cannot round-trip it. Optimistic concurrency silently changes meaning: the client's stored token never matches, so every conditional write looks like a lost update.
Fix: the Automated Reasoning policy state's definition-hash computation — the canonical serialization of AutomatedReasoningPolicyDefinition that is fed to the digest and the digest function itself — shared by the create, read, update, version and export paths so all five agree.
Traces: bedrock-automated-reasoning-policy-lifecycle#task#2 (representative; cluster also covers #task#5 and bedrock-automated-reasoning-policy-unset-vs-present-description-preserved#task#3, #task#4).

Cause 2 of 3: UpdateAutomatedReasoningPolicy returns the same wrongly-derived definitionHash for the post-update definition.
AWS: PATCH /automated-reasoning-policies/{policyArn} answers 200 with @required `definitionHash` (same AutomatedReasoningPolicyHash constraints) — the digest of the definition AS STORED BY THIS UPDATE, so that the caller can immediately reuse it as the next `lastUpdatedDefinitionHash` without re-reading the policy.
Emulator: answers 200 with a 128-hex digest that differs from AWS's digest for the same updated definition — the same algorithm difference as cause 1, surfacing in the update path.
Why: the token handed back by the update is the one a read-modify-write loop reuses, so the loop diverges from AWS on its very next conditional call. Combined with cause 1 there is no path by which a client ever observes AWS's digest for a policy.
Fix: same definition-hash computation, applied where UpdateAutomatedReasoningPolicy recomputes the hash after storing the new definition.
Traces: bedrock-automated-reasoning-policy-lifecycle#task#4.

Cause 3 of 3: CreateAutomatedReasoningPolicyVersion on a policy that exists is refused 400 instead of cutting a version.
AWS: POST /automated-reasoning-policies/{policyArn}/versions answers 200 with @required policyArn (the versioned '<policy-arn>:<version>' form), version (@pattern ^([1-9][0-9]{0,11})$ — the first version of a policy is "1"), name, definitionHash and createdAt, plus the optional description carried over from the policy. Smithy: CreateAutomatedReasoningPolicyVersionRequest.lastUpdatedDefinitionHash is @required AutomatedReasoningPolicyHash, i.e. the call is a conditional write gated on the caller's hash matching the policy's current definitionHash; a mismatch is modeled as ConflictException (httpError 400), and ValidationException (400) covers malformed input.
Emulator: answers 400 and emits NONE of the output members — $.policyArn, $.version, $.name, $.description, $.definitionHash and $.createdAt are all absent, so no version is created. Hypothesis: this is a consequence of causes 1 and 2 — the `lastUpdatedDefinitionHash` the fixture supplies is the hash AWS recorded for that definition, which can never equal the digest this emulator recomputes, so the pre-condition always fails. The replay shows only the 400; it does not expose the error code or message body, so the exact refusal shape is not asserted here. Whatever the trigger, the requirement is that a version is cut (200 with the versioned ARN and version "1" for a first version) whenever the supplied hash matches the policy's current definition hash, and that a genuine mismatch is refused as ConflictException 400 rather than this unconditional refusal.
Why: policy versioning is entirely unusable. Nothing downstream of a version exists — a client cannot publish or promote a policy, cannot pin an inference call or an export to a stable version, and cannot build the ARN ':<version>' suffix that the modeled AutomatedReasoningPolicyArn pattern admits. Because the refusal is unconditional, the whole publish flow is blocked rather than merely mis-reported.
Fix: CreateAutomatedReasoningPolicyVersion's handler and its lastUpdatedDefinitionHash pre-condition check (which must compare against the SAME canonical hash cause 1 fixes), plus the policy-version state that mints the version number and the versioned ARN.
Traces: bedrock-automated-reasoning-policy-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All three clusters report service=bedrock, verdict=mismatch, empty stub_path and empty owned_by (no open issue carries their replay keys).

Cause 1 — fixture bedrock-automated-reasoning-policy-lifecycle, step task#2, GetAutomatedReasoningPolicy, expected status 200 / actual status 200:
  $.definitionHash: value_mismatch (expected 'f16ba1ceca36e1d21adce559481add6a4998b79ae203d933fd0206a28d5c2896513dd62f57b293cba282441269a72063b1d9da02fcf2b421e9bf8495ff8c87af', got 'd58d7a1f9cff2509a86ab8d254014c6e2d46af12429f2b636ae2283f310e56be68309071afa224d456b4c9fab4f120f69ea14f921b2df3e2bd3db426ecfd26c0')
  affected_count 4: bedrock-automated-reasoning-policy-lifecycle#task#2, bedrock-automated-reasoning-policy-lifecycle#task#5, bedrock-automated-reasoning-policy-unset-vs-present-description-preserved#task#3, bedrock-automated-reasoning-policy-unset-vs-present-description-preserved#task#4. The SAME expected digest appears for all four steps, and the SAME actual digest is returned for all four, confirming a deterministic algorithm difference and not a per-run value. Both values satisfy the modeled shape (128 lowercase-hex chars), so this is a value defect, not a serialization defect.

Cause 2 — fixture bedrock-automated-reasoning-policy-lifecycle, step task#4, UpdateAutomatedReasoningPolicy, expected status 200 / actual status 200:
  $.definitionHash: value_mismatch (expected '7c5fd5b1d8d41af0369983a503aa948857a83b28a6182941b643e74e092ca7bae2d68a5a316ddd533ad7e0a8aed7a0fea7d1edb99eb8359a67793f77e14545f5', got 'c91470da1ff8d35484af88c194a6417e1463d865a46d5f9bda05588ec2ec08b532e26dfb25fc1063c2df20e519e91fdf4089f93ff6269839864cd4f827254e69')
  The expected digest here differs from cause 1's because the definition changed, i.e. the recording shows the digest tracking the definition content as it should.

Cause 3 — fixture bedrock-automated-reasoning-policy-lifecycle, step task#6, CreateAutomatedReasoningPolicyVersion, expected status 200 / actual status 400:
  status: expected 200, got 400
  $.policyArn: missing (recorded 'arn:aws:bedrock:us-west-2:000000000000:automated-reasoning-policy/00000146m6zi:1' — the versioned form; the policy id and account are per-environment values, the ':<version>' suffix is the requirement)
  $.version: missing (expected '1' — the first version of a policy; fixed requirement for a first CreateAutomatedReasoningPolicyVersion)
  $.name: missing (recorded 'sim2real-probe-bedrock-arp-lifecycle-5f307cc9' — a generated per-run name; the requirement is that it echoes the policy's name)
  $.description: missing (recorded 'Updated by probe lifecycle test' — the requirement is that it carries over the policy's description)
  $.definitionHash: missing (recorded '7c5fd5b1...45f5', i.e. the same digest cause 2 should have returned)
  $.createdAt: missing (shaped, not asserted — a clock value)
  The step supplies lastUpdatedDefinitionHash as the hash the preceding Update returned in the recording, which is exactly the value cause 2 shows the emulator does not produce.

Model evidence (Smithy, com.amazonaws.bedrock#AmazonBedrockControlPlaneService):
  AutomatedReasoningPolicyHash: @length 128..128, @pattern ^[0-9a-z]{128}$.
  GetAutomatedReasoningPolicy: GET /automated-reasoning-policies/{policyArn}, 200, @readonly; output definitionHash @required.
  UpdateAutomatedReasoningPolicy: PATCH /automated-reasoning-policies/{policyArn}, 200, @idempotent; output definitionHash @required, updatedAt @required.
  CreateAutomatedReasoningPolicyVersion: POST /automated-reasoning-policies/{policyArn}/versions, 200, @idempotent; input lastUpdatedDefinitionHash @required AutomatedReasoningPolicyHash, clientRequestToken @idempotencyToken; output policyArn/version/name/definitionHash/createdAt @required, description optional; errors include ConflictException (400), ValidationException (400), ResourceNotFoundException (404), ServiceQuotaExceededException (400).
  AutomatedReasoningPolicyVersion: @pattern ^([1-9][0-9]{0,11})$ (no zero/"latest" sentinel).
  AutomatedReasoningPolicyArn: @pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$ (the optional ':<version>' suffix cause 3 never produces).

Fixed text vs variable parts: for causes 1 and 2 nothing in the value is fixed text — the whole 128-hex digest is a function of the stored policy definition, so the requirement is the function, and the recorded digests are the values that function must produce for those fixtures' definitions. For cause 3 the fixed requirements are HTTP 200, version "1" for a first version and the ':<version>' ARN suffix; the policy id, account, region, name, description and createdAt are values taken from the request and the emulator's state.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay wire fixture bedrock-automated-reasoning-policy-lifecycle (trace set simparity-wire-aws-v1); steps task#2, task#4 and task#6 are the three causes. Replay bedrock-automated-reasoning-policy-unset-vs-present-description-preserved as well for the other two members of cause 1's cluster (task#3, task#4).
3. Cause 1: CreateAutomatedReasoningPolicy with the fixture's policy definition, then GetAutomatedReasoningPolicy on the returned policyArn and compare $.definitionHash with the digest recorded for that definition. Observed: a different, stable 128-hex digest.
4. Cause 2: UpdateAutomatedReasoningPolicy on the same policy with the fixture's updated definition and compare the returned $.definitionHash with the recorded digest for the updated definition. Observed: a different 128-hex digest.
5. Cause 3: take the definitionHash returned by step 4, pass it as lastUpdatedDefinitionHash to CreateAutomatedReasoningPolicyVersion (POST /automated-reasoning-policies/{policyArn}/versions). Observed: HTTP 400 with none of policyArn / version / name / description / definitionHash / createdAt. Expected: HTTP 200 with the versioned '<policy-arn>:1' ARN and version "1".
6. Confirm a fix by re-replaying all three listed fixture#step bindings: bedrock-automated-reasoning-policy-lifecycle#task#2, #task#4 and #task#6.
