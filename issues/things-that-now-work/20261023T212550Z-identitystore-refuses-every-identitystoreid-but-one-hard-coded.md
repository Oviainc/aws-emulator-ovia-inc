Issue-Source: trace-sweep-divergence
Issue-Trace: identitystore-create-group-describe-fields
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: identitystore-create-group-describe-fields identitystore-create-group-describe-reflects-fields identitystore-group-without-description-field-omitted identitystore-update-group-description-persist identitystore-update-group-description-persists identitystore-create-user-describe-reflects-fields identitystore-get-user-id-by-unique-attribute identitystore-update-user-display-name-operations identitystore-user-without-optional-fields-omitted
Issue-Shape-Id: com.amazonaws.identitystore#AWSIdentityStore
Issue-Key: identitystore|CreateGroup|2xx->4xx|$.GroupArn
Issue-Key: identitystore|CreateGroup|2xx->4xx|$.GroupId
Issue-Key: identitystore|CreateGroup|2xx->4xx|$.IdentityStoreId
Issue-Key: identitystore|CreateGroup|2xx->4xx|$.Revision
Issue-Key: identitystore|CreateGroup|2xx->4xx|status
Issue-Key: identitystore|CreateUser|2xx->4xx|status
Issue-Key: identitystore|GetUserId|2xx->4xx|$.IdentityStoreId
Issue-Key: identitystore|GetUserId|2xx->4xx|$.UserArn
Issue-Key: identitystore|GetUserId|2xx->4xx|$.UserId
Issue-Key: identitystore|GetUserId|2xx->4xx|status
Issue-Key: identitystore|ListGroups|2xx->4xx|$.Groups
Issue-Key: identitystore|ListGroups|2xx->4xx|status
Issue-Commit: 84e4c8a49718449b744f7486d6db0c693effe22b

# identitystore refuses every IdentityStoreId but one hard-coded seed with 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `84e4c8a49718449b744f7486d6db0c693effe22b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
SHARED FAULT (5 causes, one root cause): the identitystore emulator treats the set of existing identity stores as a single hard-coded constant. services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs declares `SEED_IDENTITY_STORE_ID = "d-9067f3a1c2"`, `ComAmazonawsIdentitystoreAwsidentitystoreState::default()` inserts that one key into `stores`, and `resolve_store` / `resolve_store_mut` answer `identity_store_not_found(..)` (HTTP 404 ResourceNotFoundException, ResourceType IDENTITY_STORE) for anything else. Every operation of this API is addressed by `IdentityStoreId`, so an identity store id that is NOT that single literal makes the whole service unusable: all 5 clusters below are that one refusal surfacing on 4 different operations, and it also blocks the 26 dependent steps of these fixtures. Each cause must be confirmed by re-replaying the fixture/step named under it.

Cause 1 of 5: CreateGroup against a caller-named identity store.
AWS: CreateGroup takes a required `IdentityStoreId` constrained only by @length 1..93 and pattern ^(arn:aws[a-z-]*:identitystore::\d{12}:identitystore/)?(d-[0-9a-f]{10}|<uuid>)$ (Smithy: CreateGroup.IdentityStoreId), and answers 200 with required IdentityStoreId/GroupId/GroupArn/Revision. ResourceNotFoundException (httpError 404) is modelled, but nothing in the model ties store existence to one particular id value; the recording shows AWS answering 200 for the store id `d-a2ede64c82`, echoing it back in the response.
Emulator: 404 with no body members at all — $.IdentityStoreId, $.GroupId, $.GroupArn, $.Revision all absent — because `d-a2ede64c82` != the seeded `d-9067f3a1c2`.
Why: a client that reads its identity store id from its own environment (ListInstances, config, a recorded account) can never create a group; every call is a not-found even though the id is well formed and is the store the client actually has.
Fix: the identity-store resolution seam in the service's state (`resolve_store`/`resolve_store_mut` and the seeding of `stores`) must stop keying existence off one literal and accept any `IdentityStoreId` that satisfies the modelled pattern, vivifying/registering the store on first use, so 404 is reserved for genuinely malformed or out-of-model identifiers.
Traces: identitystore-create-group-describe-fields#task#3 (expected 200, got 404).

Cause 2 of 5: the same CreateGroup refusal in fixture setup, status-only.
AWS: as Cause 1 — 200 for a well-formed caller-supplied IdentityStoreId.
Emulator: 404 on the setup CreateGroup, so the fixture's later describe/update/list steps never run (these are among the blocked steps).
Why: setup of any multi-step client workflow cannot complete, so no downstream behavior of the service is reachable.
Fix: same seam as Cause 1.
Traces: identitystore-create-group-describe-reflects-fields#setup#3, plus the cluster members identitystore-group-without-description-field-omitted#setup#3 and #setup#6, identitystore-update-group-description-persist#setup#3, identitystore-update-group-description-persists#setup#3 (expected 200, got 404).

Cause 3 of 5: CreateUser against a caller-named identity store.
AWS: CreateUser takes the same required `IdentityStoreId` (same @length/pattern constraints) and answers 200 with required IdentityStoreId/UserId/UserArn/Revision; ResourceNotFoundException 404 is modelled but not pinned to any single store id (Smithy: CreateUser).
Emulator: 404 ResourceNotFoundException for the recorded store id.
Why: no user can be created in the store the client has, so the entire user half of the API (describe, update, get-id, list) is unreachable.
Fix: same seam as Cause 1 — resolution is shared by reads and writes, so one change covers both.
Traces: identitystore-create-user-describe-reflects-fields#setup#3, plus cluster members identitystore-get-user-id-by-unique-attribute#setup#3, identitystore-update-user-display-name-operations#setup#3, identitystore-user-without-optional-fields-omitted#setup#3 and #setup#5 (expected 200, got 404).

Cause 4 of 5: GetUserId against a caller-named identity store.
AWS: GetUserId is @readonly, takes required IdentityStoreId + AlternateIdentifier, and answers 200 with required IdentityStoreId, UserId and UserArn (ResourceId / ResourceArn shapes) (Smithy: GetUserId). The recording shows 200 echoing the requested store id and the created user's id and arn.
Emulator: 404 with $.IdentityStoreId, $.UserId and $.UserArn all missing.
Why: unique-attribute lookup — the documented replacement for the deprecated ListUsers filters — always reports the user as absent, so clients resolving a user by UserName get a not-found for a user that exists.
Fix: same resolution seam; the read path must resolve the store identically to the write path.
Traces: identitystore-get-user-id-by-unique-attribute#task#4 (expected 200, got 404).

Cause 5 of 5: ListGroups against a caller-named identity store.
AWS: ListGroups is @readonly/@paginated and answers 200 with a required `Groups` list (Smithy: ListGroups; output Groups required). The recording shows 200 with one group whose DisplayName/GroupId/GroupArn/IdentityStoreId match the group just created and whose Description member is absent because none was supplied.
Emulator: 404 ResourceNotFoundException; $.Groups missing entirely.
Why: enumeration of groups in the client's own store is impossible, and because `Groups` is a required output member a client deserializing the 404 also loses the shape contract it depends on.
Fix: same resolution seam; list operations must enumerate the store the request names.
Traces: identitystore-group-without-description-field-omitted#task#5 (expected 200, got 404).

Note: the previously filed and resolved issue "identitystore accepts writes to a nonexistent identity store and drops UserStatus / CreatedBy / UpdatedBy from describe responses" added the store-existence gate; this issue is the opposite failure mode of that same gate — existence is now decided by one hard-coded literal rather than by the modelled identifier form, so every store but the seed is refused. The 404 status and the ResourceNotFoundException code are requirements here only in the negative sense (they must NOT be returned for a well-formed store id); the recorded "Identity store not found" message text is not at issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, score 40.9%, 13 of 66 steps mismatched, 26 blocked, 0 state_reset_failures, 0 not_implemented, no stub_path and no owned_by on any cluster).

Cluster 1 — service=identitystore|CreateGroup|status=200->404: fixture identitystore-create-group-describe-fields step task#3. Expected 200; got 404. detail: "$.GroupArn: missing (expected '<shaped-not-asserted>', got None); $.GroupId: missing; $.IdentityStoreId: missing (expected 'd-a2ede64c82', got None); $.Revision: missing".

Cluster 2 — service=identitystore|CreateGroup|status=200->404|keys=status: representative identitystore-create-group-describe-reflects-fields#setup#3, affected_count 5 (identitystore-group-without-description-field-omitted#setup#3, #setup#6, identitystore-update-group-description-persist#setup#3, identitystore-update-group-description-persists#setup#3). Expected 200; got 404.

Cluster 3 — service=identitystore|CreateUser|status=200->404|keys=status: representative identitystore-create-user-describe-reflects-fields#setup#3, affected_count 5 (identitystore-get-user-id-by-unique-attribute#setup#3, identitystore-update-user-display-name-operations#setup#3, identitystore-user-without-optional-fields-omitted#setup#3, #setup#5). Expected 200; got 404.

Cluster 4 — service=identitystore|GetUserId|status=200->404: identitystore-get-user-id-by-unique-attribute#task#4. Expected 200; got 404. detail: "$.IdentityStoreId: missing (expected 'd-e00797f110', got None); $.UserArn: missing (expected 'arn:aws:identitystore:::user/{{user_id}}', got None); $.UserId: missing (expected '{{user_id}}', got None)".

Cluster 5 — service=identitystore|ListGroups|status=200->404: identitystore-group-without-description-field-omitted#task#5. Expected 200; got 404. detail: "$.Groups: missing (expected [{'DisplayName': 'sim2real-wire-idstore-nodesc-d3b3da1b169c', 'GroupArn': 'arn:aws:identitystore:::group/{{group_id_nodesc}}', 'GroupId': '{{group_id_nodesc}}', 'IdentityStoreId': 'd-486a49b0e4', 'Revision': '<shaped-not-asserted>', 'CreatedAt'/'CreatedBy'/'UpdatedAt'/'UpdatedBy': shaped}], got None)".

The three fixtures use three different, all well-formed, identity store ids (d-a2ede64c82, d-e00797f110, d-486a49b0e4); none equals the emulator's single seeded value.

Code in the checkout confirming the cause: services/service-com-amazonaws-identitystore-awsidentitystore/src/state.rs line 39 `pub const SEED_IDENTITY_STORE_ID: &str = "d-9067f3a1c2";`; lines 780-787 `Default::default()` inserts only that key into `stores`; lines 799-823 `resolve_store` / `resolve_store_mut` return `identity_store_not_found(identifier)` for any other key; lines 862-870 build the 404 ResourceNotFoundException with ResourceType IDENTITY_STORE and message "Identity store not found" (line 847). Smithy model consulted: services/service-com-amazonaws-identitystore-awsidentitystore/smithy-model/com-amazonaws-identitystore-awsidentitystore.json — CreateGroup, CreateUser, GetUserId, ListGroups (IdentityStoreId @required, @length 1..93, pattern allowing any d-[0-9a-f]{10} or UUID form, optionally ARN-qualified).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture identitystore-create-group-describe-fields, step task#3 — CreateGroup with IdentityStoreId 'd-a2ede64c82' and a DisplayName. Expect 200 with IdentityStoreId/GroupId/GroupArn/Revision; observe 404 ResourceNotFoundException with no body members.
4. Cause 2: replay identitystore-create-group-describe-reflects-fields, step setup#3 (same for identitystore-group-without-description-field-omitted setup#3/#6 and identitystore-update-group-description-persist[s] setup#3) — CreateGroup in setup. Expect 200; observe 404, which blocks the fixture's remaining steps.
5. Cause 3: replay identitystore-create-user-describe-reflects-fields, step setup#3 (and identitystore-get-user-id-by-unique-attribute setup#3, identitystore-update-user-display-name-operations setup#3, identitystore-user-without-optional-fields-omitted setup#3/#5) — CreateUser. Expect 200 with IdentityStoreId/UserId/UserArn/Revision; observe 404.
6. Cause 4: replay identitystore-get-user-id-by-unique-attribute, step task#4 — GetUserId with IdentityStoreId 'd-e00797f110' and an AlternateIdentifier naming the created user. Expect 200 with IdentityStoreId/UserId/UserArn; observe 404.
7. Cause 5: replay identitystore-group-without-description-field-omitted, step task#5 — ListGroups with IdentityStoreId 'd-486a49b0e4'. Expect 200 with the required Groups list containing the created group (Description absent); observe 404 with Groups missing.
8. Equivalently by hand: issue any identitystore operation with a well-formed IdentityStoreId other than 'd-9067f3a1c2' (e.g. AWSIdentityStore.ListGroups with {"IdentityStoreId":"d-a2ede64c82"}) and observe 404 ResourceNotFoundException; repeat with 'd-9067f3a1c2' and observe 200.
