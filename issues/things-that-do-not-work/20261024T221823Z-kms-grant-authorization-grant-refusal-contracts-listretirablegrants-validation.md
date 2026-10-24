Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.create-grant-exceeding-key-policy-permissions-rejected kms.grant-constraint-mismatched-context-rejected kms.sep-kms-key-scope-grant-context-equals-matches-reordered-context-but-absent-context-denied kms.sep-kms-key-scope-grant-on-one-key-does-not-authorize-another-key-denied kms.e2e-grant-based-access-then-retire-blocks-further-use kms.retired-grant-cannot-be-used kms.op-unknown-grant-id-rejected kms.invalid-arn-rejected-across-read-ops kms.list-retirable-grants-invalid-marker-rejected kms.e2e-key-rotation-and-decrypt-old-ciphertext kms.update-primary-region-on-non-multi-region-key-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.create-grant-exceeding-key-policy-permissions-rejected|_err#18|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: kms.grant-constraint-mismatched-context-rejected|run#17|AssertionError|assert message == denied, f"unexpected message: {message!r}"
Issue-Key: kms.sep-kms-key-scope-grant-context-equals-matches-reordered-context-but-absent-context-denied|run#27|AssertionError|assert code == denied, (
Issue-Key: kms.sep-kms-key-scope-grant-on-one-key-does-not-authorize-another-key-denied|run#23|AssertionError|assert with_token == denied_b, (
Issue-Key: kms.e2e-grant-based-access-then-retire-blocks-further-use|_poll_until_refused#16|Timeout|time.sleep(interval_s)
Issue-Key: kms.retired-grant-cannot-be-used|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: kms.op-unknown-grant-id-rejected|run#24|AssertionError|assert message2 == (
Issue-Key: kms.invalid-arn-rejected-across-read-ops|_error#6|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: kms.list-retirable-grants-invalid-marker-rejected|run#6|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Key: kms.e2e-key-rotation-and-decrypt-old-ciphertext|_poll_until#11|AssertionError|raise AssertionError(f"{what} not met within {timeout_s}s; last saw {last!r}")
Issue-Key: kms.update-primary-region-on-non-multi-region-key-rejected|run#19|AssertionError|assert msg_mr_no_replica == (
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# KMS grant authorization, grant-refusal contracts, ListRetirableGrants validation, on-demand rotation state and UpdatePrimaryRegion refusal all diverge (8 causes)

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGrant

## Description
EIGHT distinct net-new root causes in the KMS emulator, every one grounded in a failing behavioral check of this suite that the report marked neither ALREADY FILED nor OWNED. A fixer must close all eight before submitting.

Related open issues that do NOT cover these causes (named for context only): 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md (its Cause 1 is the data-plane authorization gate and its Cause 8 asked for ListRetirableGrants to be implemented at all — ListRetirableGrants now answers, so the two checks below fail on NEW assertions inside the new handler), and 20261024T140708Z-kms-error-semantics-checks-are-blocked-by-five.md.

Cause 1 of 8 — CreateGrant performs no authorization check, so a principal the key policy does not allow to issue grants can still issue one.
AWS: CreateGrant is authorized against the key policy and the caller's identity policy; a principal whose key policy grants only kms:DescribeKey and omits kms:CreateGrant is refused AccessDeniedException (HTTP 400) and NO grant is created. Smithy: CreateGrant's input is KeyId+Operations (required) and it models DisabledException/InvalidArnException/NotFoundException/KMSInvalidStateException — the authorization refusal is the service-common AccessDeniedException, and the DryRun member exists precisely because the decision is server-side.
Emulator: CreateGrant returns HTTP 200 with a GrantId and GrantToken for that same principal.
Why: grant issuance is the most privilege-amplifying operation in KMS — a client cannot demonstrate or regression-test a least-privilege key policy that withholds kms:CreateGrant, because the emulator mints the grant anyway and every downstream deny-path test then passes vacuously.
Fix: an authorization gate on the CreateGrant handler (key-policy + identity-policy evaluation for kms:CreateGrant) ahead of grant minting.
Traces: kms.create-grant-exceeding-key-policy-permissions-rejected.

Cause 2 of 8 — a request refused on the grant path carries an emulator-authored Message instead of the IAM denial message AWS builds from the request.
AWS: when a grant (or grant token) does not authorize the request, the refusal is AccessDeniedException whose message is built from the request as "User: <caller principal/session ARN> is not authorized to perform: kms:<Action> on resource: <key ARN> because no identity-based policy allows the kms:<Action> action". This is a request-derived rule (principal ARN + action + resource ARN), not free text, and it is what the recordings measure on three separate grant-constraint paths.
Emulator: the CODE and status are right (AccessDeniedException) but the message is the emulator's own, e.g. "The grant tokens in the request do not authorize kms:Encrypt on arn:aws:kms:<region>:<account>:key/<id>." and "The grant tokens in the request do not authorize kms:Decrypt on arn:aws:kms:<region>:<account>:key/<id>."
Why: clients and SDK-based test suites parse this message to distinguish "no identity-based policy allows the action" from the other AccessDenied variants AWS emits (resource-policy deny, resource does not exist), and the emulator's text also discloses the resolved key ARN on a denied request. The message is additionally the only observable distinguishing a grant-constraint miss from an unrelated deny, so every one of these checks reads as a divergence.
Fix: the KMS error serializer / the authorization decision's refusal construction — build the IAM denial message from the request's principal, action and resource rather than describing the grant-token lookup.
Traces: kms.grant-constraint-mismatched-context-rejected (EncryptionContextSubset miss), kms.sep-kms-key-scope-grant-context-equals-matches-reordered-context-but-absent-context-denied (EncryptionContextEquals vs a context-free blob), kms.sep-kms-key-scope-grant-on-one-key-does-not-authorize-another-key-denied (key A's token presented for key B's blob).

Cause 3 of 8 — the TOKENLESS authorization path is never evaluated, so retiring a grant has no observable effect and the post-retirement denial never appears.
AWS: a grant is the only authority a bare IAM role (no identity policy, no key-policy statement) has on a key. After RetireGrant succeeds, that role's ordinary request — no GrantTokens member — is refused AccessDeniedException (HTTP 400). Smithy: RetireGrant takes GrantToken / KeyId+GrantId and returns Unit; Decrypt and Encrypt carry GrantTokens as an OPTIONAL list, so the propagated-grant lookup is a separate, required path from the token path.
Emulator: after RetireGrant the same role's tokenless Encrypt/Decrypt keeps returning HTTP 200 with a CiphertextBlob/Plaintext. Both checks therefore spin in their bounded poll waiting for the refusal and are killed at the suite's 60 s cap ("Check timed out after 60.0s"); neither reaches its later assertions. The token path IS evaluated (see Cause 2), so the gap is specifically the propagated-grant/identity lookup used when no token is presented.
Why: revocation and retirement are the only ways to withdraw KMS access. An emulator in which they are no-ops makes every "access removed" rehearsal pass while production would deny, and makes the entire grant lifecycle (create → use → retire → re-grant) untestable.
Fix: the KMS data-plane authorization gate's tokenless branch — resolve the caller's stored grants for the key the ciphertext names (and the identity/key policy) on every Encrypt/Decrypt, not only when GrantTokens is supplied, and make RetireGrant/RevokeGrant remove that authority.
Traces: kms.e2e-grant-based-access-then-retire-blocks-further-use (60 s timeout), kms.retired-grant-cannot-be-used (60 s timeout).

Cause 4 of 8 — RetireGrant with an unknown GrantId refuses with the right code but the wrong refusal message.
AWS: RetireGrant(KeyId=<full key ARN>, GrantId=<unknown>) is refused AccessDeniedException — KMS will not confirm whether the grant exists — with the request-derived message "User: <caller ARN> is not authorized to perform: kms:RetireGrant on this resource because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access". Smithy: RetireGrant models InvalidGrantIdException/NotFoundException/InvalidArnException; the resource-nonexistence refusal is the service-common AccessDeniedException. The recording measures this exact variant, and the sibling assertions in the same check pass (RevokeGrant on an unknown id -> NotFoundException "Grant ID <id> not found", and RetireGrant with a non-ARN KeyId -> NotFoundException "Invalid arn <key id>"), so the handler is otherwise aligned.
Emulator: AccessDeniedException with "No grant with the id 0000000000000000000000000000000000000000000000000000000000000000 authorizes kms:RetireGrant."
Why: the emulator's text confirms that the grant does not exist — the precise disclosure AWS's wording is designed to avoid — and clients matching on the "resource does not exist in this Region, no resource-based policies allow access" variant take the wrong branch.
Fix: the RetireGrant handler's refusal construction / the KMS error serializer.
Traces: kms.op-unknown-grant-id-rejected.

Cause 5 of 8 — ListRetirableGrants does not validate the RetiringPrincipal ARN's grammar and answers 200 with an empty page.
AWS: a structurally malformed principal ARN (a real partition/account but no IAM resource-type segment, e.g. arn:aws:iam::<account>:this-is-not-a-valid-iam-resource-segment) is refused InvalidArnException (HTTP 400). Smithy: ListRetirableGrants models InvalidArnException (httpError 400) and RetiringPrincipal is PrincipalIdType with length 1..256 and pattern ^[\w+=,.@:/-]+$ — the pattern admits the string, so the ARN-grammar check is a handler responsibility, which is exactly why the operation models InvalidArnException.
Emulator: HTTP 200 with {"Grants": [], "Truncated": false}.
Why: an empty page is indistinguishable from "this principal legitimately holds no retirable grants", so a client's malformed-input bug is silently swallowed and an inventory sweep reports a false negative instead of failing.
Fix: ListRetirableGrants input validation (principal-ARN grammar) in services/service-com-amazonaws-kms-trentservice/src/ops/list_retirable_grants.rs.
Traces: kms.invalid-arn-rejected-across-read-ops.

Cause 6 of 8 — ListRetirableGrants' InvalidMarkerException carries an emulator-authored Message where AWS sends none.
AWS: ListRetirableGrants with a Marker the service never issued is refused InvalidMarkerException (Smithy: modeled, httpError 400; Marker is MarkerType length 1..1024, pattern ^[\u0020-\u00FF]*$) and the recording measures an EMPTY Error.Message — the presence or absence of the Message member is a structural property of the refusal envelope, not prose.
Emulator: InvalidMarkerException with "The marker that specifies where pagination should next begin is not valid." The code and status are correct.
Why: this is the same empty-Message contract already filed as Cause 5 of 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md for other KMS refusals, now reproduced by the newly implemented ListRetirableGrants handler; a client that renders Error.Message shows invented text that AWS never sends.
Fix: the KMS error serializer for this operation — do not synthesize a Message where the measured contract is an empty one.
Traces: kms.list-retirable-grants-invalid-marker-rejected.

Cause 7 of 8 — RotateKeyOnDemand succeeds but records no on-demand rotation, so GetKeyRotationStatus never reports OnDemandRotationStartDate.
AWS: RotateKeyOnDemand mints a new backing key-material version and the rotation is observable: GetKeyRotationStatus then reports OnDemandRotationStartDate, and Decrypt of a pre-rotation ciphertext still succeeds while reporting the OLD KeyMaterialId. Smithy: GetKeyRotationStatusResponse$OnDemandRotationStartDate is a modeled DateType member, and DecryptResponse$KeyMaterialId is a modeled BackingKeyIdType (length 64, pattern ^[a-f0-9]+$) — they are the only observables that identify which material served a ciphertext.
Emulator: RotateKeyOnDemand returns 200, but GetKeyRotationStatus keeps omitting OnDemandRotationStartDate; the check polls 30 s and reports "OnDemandRotationStartDate to be recorded not met within 30.0s; last saw False". (The earlier Decrypt KeyMaterialId assertion passed, so the member is served — only the rotation event is not recorded.)
Why: rotation is unobservable, so a target that treats RotateKeyOnDemand as a no-op is indistinguishable from one that rotates; any client that waits on OnDemandRotationStartDate before proceeding hangs forever, and material-identity auditing after rotation is impossible.
Fix: the RotateKeyOnDemand handler and the key's rotation state (record the on-demand rotation start and mint a new material id), surfaced by the GetKeyRotationStatus handler.
Traces: kms.e2e-key-rotation-and-decrypt-old-ciphertext.

Cause 8 of 8 — UpdatePrimaryRegion's "no replica in the target Region" refusal message is not the message AWS builds.
AWS: for a genuine multi-Region PRIMARY key (MultiRegion=true) with no replica in the requested PrimaryRegion, the refusal message is built from the request as "<key id> does not have a replica key in the specified Region." Smithy: UpdatePrimaryRegion models UnsupportedOperationException/NotFoundException/InvalidArnException/DisabledException/KMSInvalidStateException and takes KeyId + PrimaryRegion (RegionType, pattern ^([a-z]+-){2,3}\d+$); the recording measures this message for the has-no-replica case. The sibling assertion in the same check passes (an ordinary key -> UnsupportedOperationException "<key ARN> is not a multi-region key."), so only this branch's message diverges.
Emulator: "PrimaryRegion us-east-1 has no replica key of arn:aws:kms:<region>:<account>:key/mrk-<id>. The primary Region must already have a replica key." — a different sentence, naming the region first and the key by ARN.
Why: the two refusals (not-multi-Region vs no-replica-there) are what prove the rejection is about MultiRegion status rather than a blanket deny; a client that distinguishes them on message text cannot, and the emulator's phrasing inverts which operand the message names.
Fix: the UpdatePrimaryRegion handler's refusal construction for the missing-replica branch.
Traces: kms.update-primary-region-on-non-multi-region-key-rejected.

## Evidence
Cause 1 — kms.create-grant-exceeding-key-policy-permissions-rejected: "AssertionError: expected a ClientError, but the call succeeded: {'GrantToken': '6772616e742d746f6b656e...', 'GrantId': '8719c13c6343b767d9e5540db3e31027928b781843cce2db2d80de34366143c9', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}". Expected: AccessDeniedException, HTTP 400, no grant created.

Cause 2 — kms.grant-constraint-mismatched-context-rejected: "AssertionError: unexpected message: 'The grant tokens in the request do not authorize kms:Encrypt on arn:aws:kms:us-west-2:000000000000:key/7209758b-b3a0-4082-a9ab-c0fe8d5b838c.'"; expected (example built from the request) "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: kms:Encrypt on resource: <key ARN> because no identity-based policy allows the kms:Encrypt action". kms.sep-kms-key-scope-grant-context-equals-...: "an EncryptionContextEquals grant must refuse a context-free decrypt with AccessDeniedException, got ('AccessDeniedException', 'The grant tokens in the request do not authorize kms:Decrypt on arn:aws:kms:us-west-2:000000000000:key/d938cc2d-cdd1-4ffd-8bff-b7698aa2a054.')". kms.sep-kms-key-scope-grant-on-one-key-...: "key A's grant token presented for key B's blob must be refused with AccessDeniedException, got ('AccessDeniedException', 'The grant tokens in the request do not authorize kms:Decrypt on arn:aws:kms:us-west-2:000000000000:key/d7851089-dee3-4009-b1e3-4e69eba3671d.')". In all three the CODE matched; only the Message diverged.

Cause 3 — kms.e2e-grant-based-access-then-retire-blocks-further-use: "Error: Check timed out after 60.0s". The check's own probe is _poll_until_refused(assumed_kms.encrypt(KeyId, Plaintext) with NO GrantTokens, timeout_s=60.0, interval_s=2.0) run immediately after kms.retire_grant(KeyId=<key ARN>, GrantId=<id>) succeeded; it raises only if the call KEEPS succeeding, i.e. the tokenless Encrypt returned 200 on every 2 s probe for the full window. kms.retired-grant-cannot-be-used: "Error: Check timed out after 60.0s" — same shape, polling role_kms.decrypt(CiphertextBlob) with no GrantTokens to AccessDeniedException after kms.retire_grant(GrantToken=...).

Cause 4 — kms.op-unknown-grant-id-rejected: "AssertionError: No grant with the id 0000000000000000000000000000000000000000000000000000000000000000 authorizes kms:RetireGrant." (the assertion prints the received message). Expected: "User: <caller ARN> is not authorized to perform: kms:RetireGrant on this resource because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access". Code AccessDeniedException matched.

Cause 5 — kms.invalid-arn-rejected-across-read-ops: "AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'Grants': [], 'Truncated': False, 'ResponseMetadata': {'HTTPStatusCode': 200, 'content-length': '31', ...}}" for kms.list_retirable_grants(RetiringPrincipal='arn:aws:iam::<account>:this-is-not-a-valid-iam-resource-segment'). Expected InvalidArnException, HTTP 400.

Cause 6 — kms.list-retirable-grants-invalid-marker-rejected: code was InvalidMarkerException as required, then "AssertionError: expected an empty message, got 'The marker that specifies where pagination should next begin is not valid.'" for Marker='not-a-real-pagination-token-xyz'.

Cause 7 — kms.e2e-key-rotation-and-decrypt-old-ciphertext: "AssertionError: OnDemandRotationStartDate to be recorded not met within 30.0s; last saw False" — i.e. kms.rotate_key_on_demand(KeyId) returned 200 and every subsequent kms.get_key_rotation_status(KeyId) omitted OnDemandRotationStartDate.

Cause 8 — kms.update-primary-region-on-non-multi-region-key-rejected: "AssertionError: expected the ValidationException message to name the missing replica, got 'PrimaryRegion us-east-1 has no replica key of arn:aws:kms:us-west-2:000000000000:key/mrk-02af9b2595a24720813384579a67bcbd. The primary Region must already have a replica key.'"; expected "<key id> does not have a replica key in the specified Region." The preceding ordinary-key leg (UnsupportedOperationException, "<key ARN> is not a multi-region key.") passed.

## Steps to Reproduce
Replay the behavioral suite and re-run the eleven check ids in trace_ids. Per cause:
1. CreateKey; CreateRole with no policies; PutKeyPolicy granting that role only kms:DescribeKey on the key; assume the role and call CreateGrant(KeyId, GranteePrincipal=<role ARN>, Operations=['Decrypt']) — observe HTTP 200 with GrantId/GrantToken instead of AccessDeniedException (400).
2. CreateKey; CreateGrant(Operations=['Encrypt'], Constraints={'EncryptionContextSubset': {'Department':'Finance'}}) for a bare role; as that role Encrypt with the matching context + GrantTokens (succeeds), then Encrypt with a mismatched/absent context + the same token — inspect Error.Message. Repeat with Constraints={'EncryptionContextEquals': ...} against a context-free blob, and with key A's token presented for key B's ciphertext.
3. CreateKey; CreateGrant(['Encrypt']) for a bare role; as the role Encrypt with the GrantToken (succeeds); RetireGrant(KeyId=<key ARN>, GrantId); then as the role Encrypt WITHOUT GrantTokens, repeatedly for 60 s — observe HTTP 200 on every attempt instead of AccessDeniedException.
4. CreateKey; RetireGrant(KeyId=<full key ARN>, GrantId='0'*64) — inspect Error.Message.
5. ListRetirableGrants(RetiringPrincipal='arn:aws:iam::<your account>:this-is-not-a-valid-iam-resource-segment') — observe 200 with an empty Grants list instead of InvalidArnException (400).
6. ListRetirableGrants(RetiringPrincipal=<your caller ARN>, Marker='not-a-real-pagination-token-xyz') — observe InvalidMarkerException carrying a Message, which must be empty.
7. CreateKey(KeyUsage='ENCRYPT_DECRYPT', KeySpec='SYMMETRIC_DEFAULT'); RotateKeyOnDemand(KeyId); poll GetKeyRotationStatus(KeyId) — observe OnDemandRotationStartDate never appears.
8. CreateKey(MultiRegion=True); UpdatePrimaryRegion(KeyId, PrimaryRegion=<a region with no replica>) — inspect the refusal message.
