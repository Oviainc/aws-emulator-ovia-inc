Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.grant-name-idempotency-and-revoke-semantics kms.import-key-material-incorrect-key-material-rejected kms.import-key-material-incorrect-material-rejected kms.put-key-policy-lockout-fails kms.update-primary-region-on-replica-rejected kms.update-primary-region-disabled-replica-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.grant-name-idempotency-and-revoke-semantics|run#41|AssertionError|assert reused["GrantId"] == other, (
Issue-Key: kms.import-key-material-incorrect-key-material-rejected|_error#18|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: kms.import-key-material-incorrect-material-rejected|_error#11|AssertionError|raise AssertionError(
Issue-Key: kms.put-key-policy-lockout-fails|_error#11|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: kms.update-primary-region-on-replica-rejected|_error#11|AssertionError|raise AssertionError(
Issue-Key: kms.update-primary-region-disabled-replica-rejected|_error#15|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded: {result!r}")
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# kms: 5 net-new over-permissive/state root causes — a revoked named grant does not keep its parameter-derived GrantId, ImportKeyMaterial accepts material that is not the key's own, PutKeyPolicy has no lockout check, and UpdatePrimaryRegion validates neither the key's role nor the replica's state

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportKeyMaterial

## Description
FIVE distinct net-new root causes in the KMS emulator, all of them calls real AWS refuses (or identities AWS derives) that the emulator accepts with HTTP 200. Every one is grounded in a failing behavioral check of this suite that the report marked neither ALREADY FILED nor OWNED. A fixer must close all five before submitting.

Related OPEN issues that do NOT carry these checks' replay keys (named for context only): 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md — its Cause 2 describes the PutKeyPolicy lockout check for a DIFFERENT check id (kms.default-key-policy-grants-root-account-access); the check below (kms.put-key-policy-lockout-fails) is a separate replay key that also measures the refusal MESSAGE and the effect-absent read. Also 20261024T221823Z-kms-grant-authorization-grant-refusal-contracts-listretirablegrants-validation.md (grant authorization and the UpdatePrimaryRegion no-replica message).

Cause 1 of 5 — a named grant's GrantId is not derived from its parameters, so recreating a revoked identical grant mints a new id.
AWS: CreateGrant's Name makes the call idempotent for identical parameters — KMS derives a named grant's id from those parameters. Byte-identical replays return the same GrantId (that leg passed), reordering Operations returns the same GrantId (passed), the same Name with DIFFERENT Operations yields a new grant (passed), and after RevokeGrant, recreating the identical named grant returns THAT grant's original GrantId. Smithy: CreateGrant$Name is GrantNameType (length 1..256, pattern ^[a-zA-Z0-9:/_-]+$) and the response carries GrantId/GrantToken; the idempotency rule is the handler's.
Emulator: every other leg of the check passes, but recreating the revoked grant returns a freshly minted GrantId instead of the one the revoked grant had, i.e. the id is random state rather than a function of (key, grantee, operations, name).
Why: clients and CloudFormation-style reconcilers treat a named grant as idempotent and key their own bookkeeping on the returned GrantId; an id that changes across a revoke/recreate cycle makes a re-applied desired state look like a different grant, and duplicate-grant detection silently breaks.
Fix: the CreateGrant handler's named-grant identity (derive the GrantId from the grant's parameters rather than minting it).
Traces: kms.grant-name-idempotency-and-revoke-semantics.

Cause 2 of 5 — ImportKeyMaterial does not verify that the imported material is the key's OWN material: mismatched material is accepted with HTTP 200.
AWS: once key material has been imported into an EXTERNAL-origin key, only that exact material may ever be (re)associated with it. Importing correctly-sized but DIFFERENT material — whether after DeleteImportedKeyMaterial returned the key to PendingImport, or as an ImportType=EXISTING_KEY_MATERIAL reimport — is refused IncorrectKeyMaterialException, HTTP 400, with the message template 'Imported key material does not have the expected identifier (<KeyMaterialId>).' where <KeyMaterialId> is the id the ORIGINAL import returned; the rejected import leaves the key's state untouched, and reimporting the original material succeeds. Smithy: ImportKeyMaterial models IncorrectKeyMaterialException (httpError 400) and InvalidCiphertextException, and returns KeyMaterialId (BackingKeyIdType, length 64, pattern ^[a-f0-9]+$) precisely so material identity is observable.
Emulator: HTTP 200 with a NEW KeyMaterialId for the mismatched material — the key silently adopts foreign material.
Why: this is the guarantee that an imported-material (BYOK/HYOK) key can never be re-pointed at material the customer did not originally escrow. An emulator that accepts any material of the right length makes every key-material-identity rehearsal pass while production refuses, and makes ciphertexts encrypted under the original material silently undecryptable in a way the emulator will never reproduce.
Fix: the ImportKeyMaterial handler plus the key's material state — record the imported material's identifier and compare it on every (re)import, ahead of adopting the material.
Traces: kms.import-key-material-incorrect-key-material-rejected, kms.import-key-material-incorrect-material-rejected.

Cause 3 of 5 — PutKeyPolicy applies no policy-lockout safety check, so a policy that locks the key out is stored.
AWS: a replacement key policy that leaves no principal a path to kms:PutKeyPolicy is refused MalformedPolicyDocumentException, HTTP 400, with the fixed message 'The new key policy will not allow you to update the key policy in the future.', and the stored document is left untouched (an independent GetKeyPolicy still returns the previous policy). Smithy: PutKeyPolicy models MalformedPolicyDocumentException (httpError 400) and its input has BypassPolicyLockoutSafetyCheck with default=false — the safety check IS the modeled default.
Emulator: HTTP 200 (content-length 2) and the lockout document is stored. The positive control in the same check (an ordinary admin policy written to the same key) succeeded, so the write path works; only the safety check is missing.
Why: a stack can irrecoverably lock itself out of a key in the emulator, and the single guardrail AWS users rely on before shipping a key policy cannot be rehearsed.
Fix: the PutKeyPolicy handler's validation path — lockout evaluation honouring BypassPolicyLockoutSafetyCheck, before the document is persisted.
Traces: kms.put-key-policy-lockout-fails.

Cause 4 of 5 — UpdatePrimaryRegion is accepted when called against a REPLICA key.
AWS: only a multi-Region PRIMARY key can designate a new primary. UpdatePrimaryRegion issued with a REPLICA's KeyId is refused KMSInvalidStateException with the message template '<replica key ARN> is not a multi-Region primary key.', and the key set's roles are unchanged (the replica is still REPLICA, PrimaryKey.Region still names the original primary's region). Smithy: UpdatePrimaryRegion models KMSInvalidStateException/UnsupportedOperationException/DisabledException/NotFoundException/InvalidArnException and takes KeyId + PrimaryRegion (RegionType, pattern ^([a-z]+-){2,3}\\d+$).
Emulator: HTTP 200 (content-length 2) for the replica-issued call.
Why: the primary designation is what decides where the key set's policy and role changes may be made; an emulator in which any replica can promote itself hides a request production refuses, and a client's role-confusion bug surfaces only in production.
Fix: the UpdatePrimaryRegion handler's precondition check on the named key's MultiRegionKeyType.
Traces: kms.update-primary-region-on-replica-rejected.

Cause 5 of 5 — UpdatePrimaryRegion is accepted when the replica being promoted is DISABLED.
AWS: promoting a replica that is currently Disabled is refused DisabledException with the message template '<replica key ARN> is disabled.', and both sides' roles stay exactly as they were; once the replica is re-enabled the identical call succeeds and performs the swap. Smithy: DisabledException is modeled on UpdatePrimaryRegion.
Emulator: HTTP 200 (content-length 2) with the replica Disabled — the key state is never consulted. The preceding legs passed: ReplicateKey produced an Enabled REPLICA and DisableKey moved it to Disabled.
Why: promoting a disabled key leaves a key set whose primary cannot perform cryptographic operations, which is exactly why AWS refuses it; the emulator lets a client build that state and reports success.
Fix: the UpdatePrimaryRegion handler's key-state precondition (the target replica must be Enabled).
Traces: kms.update-primary-region-disabled-replica-rejected.

## Evidence
Cause 1 — kms.grant-name-idempotency-and-revoke-semantics: "AssertionError: recreating the revoked Encrypt grant must return its GrantId '373d2af2...5db', got '6ac24865...4dd'". The identical-replay, reordered-Operations, different-Operations, ListGrants and non-idempotent-RevokeGrant legs all passed first; the ids shown are this run's values, the requirement is that the second id equals the first.

Cause 2 — kms.import-key-material-incorrect-key-material-rejected: "AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'KeyId': 'arn:aws:kms:<region>:<account>:key/<id>', 'KeyMaterialId': '8309af3a...881', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" for ImportKeyMaterial of freshly generated 32-byte material B into a key previously imported with material A and returned to PendingImport. kms.import-key-material-incorrect-material-rejected: "AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'KeyId': ..., 'KeyMaterialId': '6a3d27aa...d26', 'HTTPStatusCode': 200}" for an ImportType=EXISTING_KEY_MATERIAL reimport of correctly-sized but different bytes. Both expect IncorrectKeyMaterialException / HTTP 400 and the message 'Imported key material does not have the expected identifier (<first import's KeyMaterialId>).'

Cause 3 — kms.put-key-policy-lockout-fails: "AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-length': '2', ...}}}" for PutKeyPolicy(PolicyName='default', Policy=<lockout document>, BypassPolicyLockoutSafetyCheck=False). Expected MalformedPolicyDocumentException / HTTP 400 / 'The new key policy will not allow you to update the key policy in the future.', with GetKeyPolicy still returning the admin policy.

Cause 4 — kms.update-primary-region-on-replica-rejected: "AssertionError: expected update_primary_region on a replica key to raise, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'content-length': '2', ...}}". Expected ('KMSInvalidStateException', '<replica ARN> is not a multi-Region primary key.').

Cause 5 — kms.update-primary-region-disabled-replica-rejected: "AssertionError: expected a ClientError but the call succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'content-length': '2', ...}}". Expected ('DisabledException', '<replica ARN> is disabled.') with both MultiRegionConfigurations unchanged.

## Steps to Reproduce
Replay the behavioral suite and re-run the six check ids in trace_ids. Per cause:
1. CreateKey; CreateGrant(KeyId, GranteePrincipal=<role>, Operations=['Encrypt'], Name='n') -> id X; RevokeGrant(KeyId, GrantId=X); CreateGrant with the identical parameters again — observe a GrantId != X.
2. CreateKey(Origin='EXTERNAL'); GetParametersForImport; ImportKeyMaterial(material A); DeleteImportedKeyMaterial; GetParametersForImport again; ImportKeyMaterial(material B != A, correctly sized) — observe HTTP 200 and a new KeyMaterialId instead of IncorrectKeyMaterialException (400). Repeat with ImportType='EXISTING_KEY_MATERIAL' on an Enabled imported key.
3. CreateKey; PutKeyPolicy with an admin policy (succeeds); PutKeyPolicy with a document that allows nobody kms:PutKeyPolicy and BypassPolicyLockoutSafetyCheck=False — observe HTTP 200; GetKeyPolicy shows the lockout document stored.
4. CreateKey(MultiRegion=True); ReplicateKey into another region; wait for the replica Enabled; call UpdatePrimaryRegion in the REPLICA's region with the replica's own KeyId — observe HTTP 200.
5. Same setup; DisableKey on the replica; call UpdatePrimaryRegion from the primary with PrimaryRegion=<replica region> — observe HTTP 200 instead of DisabledException (400).
