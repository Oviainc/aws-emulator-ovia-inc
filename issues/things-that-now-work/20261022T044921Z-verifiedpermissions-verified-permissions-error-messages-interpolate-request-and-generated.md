Issue-Source: trace-sweep-divergence
Issue-Trace: verifiedpermissions-policy-store-deletion-protection
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: verifiedpermissions-policy-store-deletion-protection verifiedpermissions-policy-store-alias-workflow verifiedpermissions-policy-store-lifecycle-and-tags verifiedpermissions-token-authorization-error-paths
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions|DeletePolicyStore|4xx->4xx|$.message
Issue-Key: verifiedpermissions|GetPolicy|4xx->4xx|$.message
Issue-Key: verifiedpermissions|ListTagsForResource|4xx->4xx|$.message
Issue-Related: 20261022T044921Z-verifiedpermissions-verified-permissions-policy-store-alias-arns-drop-the.md
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# Verified Permissions error messages interpolate request and generated identifiers where AWS emits fixed text

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePolicyStore

## Description
Across several Verified Permissions error paths the emulator builds the exception message by splicing the looked-up identifier into prose, where the service emits a fixed, identifier-free sentence and carries the specifics in the exception's own modeled members. In every cause below the HTTP status and the error shape are already correct, and the requirement asserted is the RULE (a constant message that embeds no request or generated value), never the recorded wording. This bundle has 4 causes.

Cause 1 of 4: DeletePolicyStore's deletion-protection refusal embeds the policy-store id.
AWS: Smithy InvalidStateException, client error, httpError 406, single required message member; the model's own documentation for the refusal is the fixed sentence pair "The policy store can't be deleted because deletion protection is enabled. To delete this policy store, disable deletion protection." — no identifier appears in it, and DeletePolicyStoreInput carries only policyStoreId. Recorded example text: "The policy store cannot be deleted because deletion protection is enabled. Deletion protection must be disabled first."
Emulator: status 406 and code InvalidStateException are correct, but the message is templated from the request, example: "policy store PSJPfe8fgQnwl8FAu3TKk6j5 can't be deleted because deletion protection is enabled. To delete this policy store, disable deletion protection."
Why: the message is no longer a stable constant across calls and now leaks a resource identifier where AWS emits none, so log scrubbing, alert matching and tests that key on the constant never match.
Fix: the deletion-protection refusal path of DeletePolicyStore (services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/delete_policy_store.rs) must emit the constant InvalidStateException message rather than formatting policyStoreId into it. Open behavioral issue 20261022T031055Z-verifiedpermissions-verified-permissions-deletepolicystore-deletion-protection-refusal-interpolates-the.md describes this same defect from the behavioral suite but carries no wire replay key; this cause supplies the fixture/step binding.
Traces: verifiedpermissions-policy-store-deletion-protection#task#2.

Cause 2 of 4: GetPolicy's not-found message embeds the policy id.
AWS: ResourceNotFoundException (httpError 404) has required message, resourceId and resourceType; the identifier belongs in resourceId and the message is the fixed, identifier-free "Resource not found" — the same constant is recorded for three different operations in this corpus, which is what establishes the rule.
Emulator: 404 with the correct resourceId/resourceType but message "No policy exists with the input ID: SPJPfe8fgQnwl8FAu3TKk6j5" — the requested id is spliced in.
Why: the message duplicates, in unparseable prose, information the modeled members already carry, and stops being a constant a client can match on.
Fix: the not-found error construction on the GetPolicy path (src/ops/get_policy.rs) / the crate's shared ResourceNotFoundException builder must use the constant message and leave the identifier to resourceId.
Traces: verifiedpermissions-policy-store-alias-workflow#task#7.

Cause 3 of 4: GetPolicyStore's not-found message embeds the policy-store id.
AWS: same ResourceNotFoundException contract and the same identifier-free constant message, with the id in resourceId.
Emulator: 404 with message "No policy store exists with the input ID: PSNONEXISTENT00000000"; clustered over 2 steps in the same fixture (task#2 and teardown#11).
Why: as cause 2.
Fix: the shared ResourceNotFoundException builder used by src/ops/get_policy_store.rs.
Traces: verifiedpermissions-policy-store-lifecycle-and-tags#task#2.

Cause 4 of 4: ListTagsForResource's not-found message embeds the full resource ARN.
AWS: same ResourceNotFoundException contract; message is the identifier-free constant and the resource identity is reported in resourceId/resourceType.
Emulator: 404 with message "No policy store exists with the input ARN: arn:aws:verifiedpermissions::000000000000:policy-store/PSFAKE0000000000000" — the whole ARN from the request is interpolated (and incidentally shows the region-less ARN form filed in the alias/ARN issue).
Why: as cause 2, and here the leaked value is a full ARN including the account id.
Fix: the shared ResourceNotFoundException builder used by src/ops/list_tags_for_resource.rs.
Traces: verifiedpermissions-token-authorization-error-paths#task#4.

Excluded on purpose: IsAuthorized's 404 in verifiedpermissions-authorization-decisions#task#6 also diverges in message only, but there the recording expects an id-interpolated message ("Policy store with id '<id>' not found"), which contradicts the constant-message rule the other operations establish. No message rule can be derived for that path from the model or the recording, so it is not filed.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator. In all four causes the expected and actual HTTP status are identical and the error shape is correct; only $.message diverges.

Cause 1 — fixture verifiedpermissions-policy-store-deletion-protection, step task#2, DeletePolicyStore:
  expected status 406, actual status 406
  $.message: value_mismatch (expected 'The policy store cannot be deleted because deletion protection is enabled. Deletion protection must be disabled first.', got "policy store PSJPfe8fgQnwl8FAu3TKk6j5 can't be deleted because deletion protection is enabled. To delete this policy store, disable deletion protection.")

Cause 2 — fixture verifiedpermissions-policy-store-alias-workflow, step task#7, GetPolicy:
  expected status 404, actual status 404
  $.message: value_mismatch (expected 'Resource not found', got 'No policy exists with the input ID: SPJPfe8fgQnwl8FAu3TKk6j5')

Cause 3 — fixture verifiedpermissions-policy-store-lifecycle-and-tags, step task#2, GetPolicyStore (cluster also covers teardown#11):
  expected status 404, actual status 404
  $.message: value_mismatch (expected 'Resource not found', got 'No policy store exists with the input ID: PSNONEXISTENT00000000')

Cause 4 — fixture verifiedpermissions-token-authorization-error-paths, step task#4, ListTagsForResource:
  expected status 404, actual status 404
  $.message: value_mismatch (expected 'Resource not found', got 'No policy store exists with the input ARN: arn:aws:verifiedpermissions::000000000000:policy-store/PSFAKE0000000000000')

Model evidence: InvalidStateException = client error, @httpError(406), single required message, documented refusal sentence "The policy store can't be deleted because deletion protection is enabled. To delete this policy store, disable deletion protection." ResourceNotFoundException = client error, @httpError(404), required message + resourceId ("the unique ID of the resource referenced in the failed request") + resourceType (enum ResourceType), i.e. the identifier has its own modeled home. The recorded constant 'Resource not found' appears for GetPolicy, GetPolicyStore and ListTagsForResource across three fixtures.

## Steps to Reproduce
1. Boot the emulator and point a Verified Permissions client at its data listener.
2. store = CreatePolicyStore(validationSettings={"mode":"OFF"}, deletionProtection="ENABLED"); DeletePolicyStore(policyStoreId=store) -> 406 InvalidStateException whose message contains the store id (cause 1).
3. GetPolicy(policyStoreId=store, policyId="SPDOESNOTEXIST000000") -> 404 whose message contains the policy id instead of the constant text (cause 2).
4. GetPolicyStore(policyStoreId="PSNONEXISTENT00000000") -> 404 whose message contains the store id (cause 3).
5. ListTagsForResource(resourceArn="arn:aws:verifiedpermissions:us-west-2:000000000000:policy-store/PSFAKE0000000000000") -> 404 whose message contains the whole ARN (cause 4).
6. Re-replay verifiedpermissions-policy-store-deletion-protection#task#2, verifiedpermissions-policy-store-alias-workflow#task#7, verifiedpermissions-policy-store-lifecycle-and-tags#task#2 and verifiedpermissions-token-authorization-error-paths#task#4.

## Resolution

Rule applied: an exception whose message this service fixes carries that message as a CONSTANT, with nothing of the request or of generated state interpolated, and reports the specifics in the members that model them. `src/state.rs` owns the two texts (`RESOURCE_NOT_FOUND_MESSAGE`, `DELETION_PROTECTION_MESSAGE`) and `src/ops/mod.rs` the two builders (`resource_not_found`, `deletion_protection_enabled`), which are the ONLY constructors of `ResourceNotFoundException` and `InvalidStateException` in the crate: every not-found path (GetPolicy, GetPolicyStore, GetPolicyStoreAlias, GetPolicyTemplate, GetSchema, GetIdentitySource, DeletePolicy, ListPolicies, CreatePolicy, CreatePolicyStoreAlias, CreatePolicyTemplate, CreateIdentitySource, PutSchema, UpdatePolicyStore, IsAuthorized, BatchIsAuthorized, ListTagsForResource, TagResource, UntagResource) answers the same text, and the deletion-protection refusal is the fixed sentence with no store id in it. `tests/all/error_messages.rs` asserts the rule over a table of operations, reference forms (id, 200-character id, punctuated id, alias reference, policy id, `name/` reference, template id, identity-source id, ARNs) and resource types.

Deliberate, so a later replay difference is not read as a regression: `IsAuthorized`'s 404 also answers the constant, although `verifiedpermissions-authorization-decisions#task#6` records an id-bearing message for it ("Policy store with id '<id>' not found"). No rule in the model or in the corpus produces that text, so under the message rule it is an example rather than a requirement, and keeping one operation interpolating the id would contradict the rule the other three operations establish. That step's `$.message` therefore differs from its recording by design.

Not changed, because the rule does not reach them: `ValidationException`/`ServiceQuotaExceededException`/`ConflictException`/`TooManyTagsException` messages, which are built from the request and name the offending member and its constraint; and `BatchGetPolicy`'s per-item `BatchGetPolicyErrorItem`, whose `message` the model documents as "A detailed error message" beside its own required `policyStoreId`/`policyId`.
