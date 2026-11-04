Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.import-key-material-incorrect-key-material-rejected kms.import-key-material-incorrect-material-rejected kms.import-key-material-token-single-use-rejected kms.grant-name-idempotency-and-revoke-semantics kms.update-custom-key-store-reconfigures-cluster kms.create-custom-key-store-requires-active-cluster-rejected kms.generate-data-key-spec-and-bytes-exclusivity kms.kms-cross-region-decrypt-of-a-single-region-ciphertext-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.import-key-material-incorrect-key-material-rejected|_error#19|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: kms.import-key-material-incorrect-material-rejected|_error#12|AssertionError|raise AssertionError(
Issue-Key: kms.import-key-material-token-single-use-rejected|run#30|AssertionError|assert failures[0]["Message"] == expected, (
Issue-Key: kms.grant-name-idempotency-and-revoke-semantics|run#43|AssertionError|assert reused["GrantId"] == other, (
Issue-Key: kms.update-custom-key-store-reconfigures-cluster|_error#13|AssertionError|raise AssertionError("expected the call to be refused, it succeeded")
Issue-Key: kms.create-custom-key-store-requires-active-cluster-rejected|run#18|AssertionError|assert refused == expected, f"expected {expected!r}, got {refused!r}"
Issue-Key: kms.generate-data-key-spec-and-bytes-exclusivity|run#11|AssertionError|assert both["Message"] == EXCLUSIVE_MESSAGE, (
Issue-Key: kms.kms-cross-region-decrypt-of-a-single-region-ciphertext-rejected|run#28|AssertionError|assert foreign_arn["Message"] == denied_message, (
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# kms: 7 net-new root causes — ImportKeyMaterial adopts foreign material, named-grant GrantId is not parameter-derived, an XKS CreateCustomKeyStore succeeds instead of probing the proxy, and four refusal messages diverge from AWS

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportKeyMaterial

## Description
SEVEN distinct net-new root causes in the KMS emulator, each grounded in a failing behavioral check of this suite that the replay report marked neither ALREADY FILED nor OWNED. A fixer must close all seven before submitting. Smithy model consulted at services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json.

Overlap disclosure (read before starting): three of the causes below also appear in the Issue-Trace-Ids of the OPEN issue 20261029T175759Z-kms-5-net-new-over-permissive-state-root.md (grant-name idempotency, and the two ImportKeyMaterial mismatched-material checks), but that issue's recorded Issue-Key offsets no longer match this run's replay keys, so the report did not mark those checks OWNED and they are still failing today. Cause 6 restates the same defect as the OPEN issue 20261030T233647Z-kms-createcustomkeystore-refuses-an-inactive-cloudhsm-cluster-with.md, and Cause 2 is the successor observable of the OPEN issue 20261026T090118Z-kms-reopens-createcustomkeystore-external-key-store-still-answers-notimplemented.md (the call no longer answers NotImplemented — it now answers 200 OK). If any of those issues is closed by the same change, close these causes with it; otherwise each is independently reproducible from the check ids listed.

----------------------------------------------------------------
Cause 1 of 7 — ImportKeyMaterial does not verify that the material being imported is the key's OWN material: correctly sized but DIFFERENT material is adopted with HTTP 200.
Failing checks: kms.import-key-material-incorrect-key-material-rejected, kms.import-key-material-incorrect-material-rejected
AWS: Smithy models ImportKeyMaterial with IncorrectKeyMaterialException (errorKind client, httpError 400) and returns KeyMaterialId (BackingKeyIdType, @length 64, @pattern ^[a-f0-9]+$) precisely so material identity is observable. Once material A has been imported into an EXTERNAL-origin key, only A may ever be re-associated with that key — whether after DeleteImportedKeyMaterial returned the key to PendingImport, or as an ImportType=EXISTING_KEY_MATERIAL reimport on an Enabled key. A mismatched import is refused HTTP 400 IncorrectKeyMaterialException; the message's fixed text is "Imported key material does not have the expected identifier (<KeyMaterialId>)." where <KeyMaterialId> is the 64-hex identifier the ORIGINAL import returned, taken from the emulator's own state. The rejected import must leave the key's state untouched (PendingImport stays PendingImport, Enabled stays Enabled), and re-importing the original bytes must succeed.
Emulator: HTTP 200 with a freshly minted KeyMaterialId for the foreign material — `AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'KeyId': 'arn:aws:kms:<region>:<account>:key/<id>', 'KeyMaterialId': '<64-hex>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. The same happens for the ImportType=EXISTING_KEY_MATERIAL reimport path.
Why: this is the one guarantee that a BYOK/HYOK key can never be re-pointed at material the customer did not escrow. A client rehearsing key-material identity passes in the emulator and is refused in production, and ciphertexts under the original material become silently undecryptable in a way the emulator never reproduces.
Fix: the ImportKeyMaterial handler plus the key's material state in the kms crate — record the imported material's identifier and compare it on every (re)import before the material is adopted.

----------------------------------------------------------------
Cause 2 of 7 — CreateCustomKeyStore for an EXTERNAL_KEY_STORE never contacts the XKS proxy: a store over an unreachable proxy URI is created with HTTP 200.
Failing check: kms.update-custom-key-store-reconfigures-cluster
AWS: Smithy models XksProxyUriUnreachableException on CreateCustomKeyStore (errorKind client, httpError 400), alongside XksProxyInvalidConfigurationException / XksProxyIncorrectAuthenticationCredentialException. The input shapes are validated first (XksProxyUriEndpoint @pattern ^https://[a-zA-Z0-9.-]+$, XksProxyUriPath @pattern ^(/[a-zA-Z0-9\/_-]+/kms/xks/v\d{1,2})$|^(/kms/xks/v\d{1,2})$, XksProxyAuthenticationCredential's AccessKeyId base32 and RawSecretAccessKey base64), then KMS reaches out to the proxy SYNCHRONOUSLY during the call and, when it cannot be reached, refuses HTTP 400 XksProxyUriUnreachableException. The message is fixed text with no variable parts: "AWS KMS cannot communicate with the external key store proxy. This might be a transient network issue. If you see this error repeatedly, verify that your external key store proxy is active and is connected to the network, and that its endpoint URI is correct in your external key store."
Emulator: the call SUCCEEDS — `AssertionError: expected the call to be refused, it succeeded` at the _error() helper — so a custom key store is created over a proxy that answers nothing.
Why: the whole point of the synchronous probe is that an XKS deployment cannot be half-built; a client that treats CreateCustomKeyStore success as "the proxy is wired up" gets a store it can never connect, and the single pre-flight check AWS offers cannot be rehearsed.
Fix: the CreateCustomKeyStore handler's EXTERNAL_KEY_STORE branch — after shape validation, probe XksProxyUriEndpoint+XksProxyUriPath and refuse XksProxyUriUnreachableException (400) when it is unreachable, creating no store.

----------------------------------------------------------------
Cause 3 of 7 — a named grant's GrantId is not derived from its parameters, so recreating a revoked identical named grant mints a new id.
Failing check: kms.grant-name-idempotency-and-revoke-semantics
AWS: Smithy models CreateGrant$Name as GrantNameType (@length 1..256, @pattern ^[a-zA-Z0-9:/_-]+$) and the response's GrantId as GrantIdType (@length 1..128); the idempotency rule is the handler's. KMS derives a named grant's id from its parameters (key, grantee principal, the Operations SET, name), so: a byte-identical replay returns the same GrantId (this leg passes), reordering Operations returns the same GrantId (passes), the same Name with DIFFERENT Operations yields a NEW grant and leaves the first untouched (passes), and after RevokeGrant, recreating the identical named grant returns THAT grant's original GrantId.
Emulator: the final leg fails — `AssertionError: recreating the revoked Encrypt grant must return its GrantId '<first id>', got '<different id>'`. The id is random per-call state rather than a function of the grant's parameters. (The two ids in that message are this run's values; the requirement is that the second equals the first.)
Why: clients and CloudFormation-style reconcilers treat a named grant as idempotent and key their bookkeeping on the returned GrantId; an id that changes across a revoke/recreate cycle makes a re-applied desired state look like a different grant and silently breaks duplicate-grant detection.
Fix: the CreateGrant handler's named-grant identity — derive the GrantId from the grant's parameters instead of minting it.

----------------------------------------------------------------
Cause 4 of 7 — ImportKeyMaterial's "this material is already associated with this key" refusal carries an emulator-authored message.
Failing check: kms.import-key-material-token-single-use-rejected
AWS: when ImportType=NEW_KEY_MATERIAL names material that is already associated with the KMS key, the refusal message's fixed text is "Key material (id: <KeyMaterialId>) being imported as NEW_KEY_MATERIAL is already associated with KMS key <key ARN>." — the parenthesised id is the 64-hex KeyMaterialId (BackingKeyIdType) of the material from the request/state, the trailing value is the key's own ARN, and the sentence ends in a period. Everything outside those two slots is fixed text.
Emulator: the message is "expected new key material but the key material being imported is already associated with the KMS key" — emulator-authored prose, naming neither the material id nor the key ARN and carrying no terminal period.
Why: operators and runbooks match on these messages, and the two identifiers AWS embeds are the only way to tell WHICH material and WHICH key collided when a BYOK re-import pipeline fails; a client log of the emulator's text is not actionable and any assertion on AWS's wording fails.
Fix: the ImportKeyMaterial handler's refusal construction — emit AWS's template with the material id and key ARN filled in from the request and the key's state.

----------------------------------------------------------------
Cause 5 of 7 — GenerateDataKey's KeySpec/NumberOfBytes mutual-exclusion refusal carries an emulator-authored message.
Failing check: kms.generate-data-key-spec-and-bytes-exclusivity
AWS: Smithy models GenerateDataKey with both KeySpec (DataKeySpec enum) and NumberOfBytes (NumberOfBytesType, @range 1..1024) optional, so the "exactly one of them" rule is the handler's. Supplying both is refused ValidationException with the fixed message "Please specify either number of bytes or key spec." — fixed text, no variable parts. (The sibling leg of the same check, the @range violation message "1 validation error detected: Value '<n>' at 'numberOfBytes' failed to satisfy constraint: Member must have value less than or equal to 1024", passed.)
Emulator: ValidationException with "KeySpec and NumberOfBytes both specify the length of the data key; specify either one of them, but not both." — the error code is right, the message is emulator prose.
Why: a client that asserts on or branches off AWS's message text for this very common SDK misuse mismatches, and the divergence shows the refusal is produced by a hand-written string rather than the service's validation vocabulary.
Fix: the GenerateDataKey handler's input validation — emit AWS's exact sentence for the both-specified case.

----------------------------------------------------------------
Cause 6 of 7 — CreateCustomKeyStore's CloudHsmClusterNotActiveException is returned with an EMPTY Error.Message.
Failing check: kms.create-custom-key-store-requires-active-cluster-rejected
AWS: Smithy models CloudHsmClusterNotActiveException on CreateCustomKeyStore (errorKind client, httpError 400), distinct from CloudHsmClusterNotFoundException. An existing but uninitialized CloudHSM cluster is refused HTTP 400 CloudHsmClusterNotActiveException whose message is the fixed text "Cluster is not active." — no variable parts — and no key store is created.
Emulator: the code is right but the message is the empty string — `AssertionError: expected ('CloudHsmClusterNotActiveException', 'Cluster is not active.'), got ('CloudHsmClusterNotActiveException', '')`.
Why: an empty Error.Message gives a client nothing to log or branch on, and botocore renders the refusal as "An error occurred (CloudHsmClusterNotActiveException) ... : " — the operator cannot tell a not-active cluster from any other 400 without reading the code programmatically.
Fix: the CreateCustomKeyStore handler's cluster-state refusal — populate Error.Message with AWS's fixed sentence. (Same defect as the open issue 20261030T233647Z-kms-createcustomkeystore-refuses-an-inactive-cloudhsm-cluster-with.md, refiled because this check's replay key is not carried by it.)

----------------------------------------------------------------
Cause 7 of 7 — Decrypt's cross-Region refusal uses the legacy "ciphertext refers to a CMK" sentence instead of AWS's authorization-shaped AccessDeniedException message.
Failing check: kms.kms-cross-region-decrypt-of-a-single-region-ciphertext-rejected
AWS: decrypting a single-Region ciphertext through a key ARN that names another Region is refused AccessDeniedException (Decrypt's modeled error set covers NotFoundException/InvalidCiphertextException/IncorrectKeyException; the cross-Region case collapses onto the service-common AccessDeniedException, which the check confirms — the CODE matched). The message's fixed template is "User: <caller principal ARN> is not authorized to perform: kms:Decrypt on the resource associated with this ciphertext because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access" — the only variable part is the caller's principal ARN, taken from the signing identity; the rest, including the action name kms:Decrypt, is fixed text with no terminal period.
Emulator: AccessDeniedException with "The ciphertext refers to a customer master key that does not exist, does not exist in this region, or you are not allowed to access." — the obsolete pre-2021 KMS sentence, which names no principal and no action.
Why: a cross-Region key reference is one of the most common real deployment bugs, and AWS's message is what tells an operator WHICH principal and WHICH action were refused; tooling that parses the "User: ... is not authorized to perform: ..." shape (the universal AWS authorization-failure form) extracts nothing from the emulator's text, and anything asserting on AWS's wording fails.
Fix: the Decrypt handler's cross-Region/unresolvable-key refusal path — build the AccessDeniedException message from AWS's template with the caller's principal ARN substituted.

Traces: kms.import-key-material-incorrect-key-material-rejected, kms.import-key-material-incorrect-material-rejected, kms.import-key-material-token-single-use-rejected, kms.grant-name-idempotency-and-revoke-semantics, kms.update-custom-key-store-reconfigures-cluster, kms.create-custom-key-store-requires-active-cluster-rejected, kms.generate-data-key-spec-and-bytes-exclusivity, kms.kms-cross-region-decrypt-of-a-single-region-ciphertext-rejected

## Evidence
Replay report (suite simparity-behavioral-aws-v1, 242/378 passed), service kms 221/239 (92%), 18 failed. The eight failures below were marked neither ALREADY FILED nor OWNED.

Cause 1 — [mismatch] kms.import-key-material-incorrect-key-material-rejected
  check line 78 -> _error() line 123:
  "AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'KeyId': 'arn:aws:kms:us-west-2:000000000000:key/4392668d-4c84-408e-98b1-8c682de300f7', 'KeyMaterialId': 'd6b9a468c2f256bb8a67a59d11090c20c26126aa1139d1af0e552c376e812a3f', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '170', ...}}}"
  [mismatch] kms.import-key-material-incorrect-material-rejected, check line 53 -> _error() line 79:
  "AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'KeyId': 'arn:aws:kms:us-west-2:000000000000:key/05fb95f9-fddd-4394-93b4-5199e495328b', 'KeyMaterialId': '6a3d27aa5a6c6670375b1b1a43f009453606835d94b911622a161c3d7366ad26', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"
  Expected in both: HTTP 400 IncorrectKeyMaterialException, message "Imported key material does not have the expected identifier (<the first import's KeyMaterialId>).", key state unchanged. (ARNs/ids above are this run's values.)

Cause 2 — [mismatch] kms.update-custom-key-store-reconfigures-cluster
  check line 69 -> _error() line 98: "AssertionError: expected the call to be refused, it succeeded"
  The refused call is CreateCustomKeyStore(CustomKeyStoreType='EXTERNAL_KEY_STORE', XksProxyUriEndpoint='https://xks.sim2real.example.com', XksProxyUriPath='/kms/xks/v1', XksProxyConnectivity='PUBLIC_ENDPOINT', XksProxyAuthenticationCredential={AccessKeyId: <base32>, RawSecretAccessKey: <base64>}).
  Expected: HTTP 400 XksProxyUriUnreachableException with the fixed message "AWS KMS cannot communicate with the external key store proxy. This might be a transient network issue. If you see this error repeatedly, verify that your external key store proxy is active and is connected to the network, and that its endpoint URI is correct in your external key store." and no store created.
  Actual: HTTP 200, a store is created. (The three preceding legs of the same check — UpdateCustomKeyStore on an absent id, on an unparseable id, and DescribeCustomKeyStores on an absent id, all CustomKeyStoreNotFoundException with an empty message — passed.)

Cause 3 — [mismatch] kms.grant-name-idempotency-and-revoke-semantics
  check line 162: assert reused["GrantId"] == other
  "AssertionError: recreating the revoked Encrypt grant must return its GrantId '3b02f5724a55fccd1491e573d07e7ddf12014181f0a53e64caf109b6b7e449b1', got '4751cbc31b86238aba7636611a8992034ad90eb27faada462790de6461a28a3e'"
  Everything before it passed: identical replay -> same GrantId, reversed Operations -> same GrantId, same Name + different Operations -> new grant with the first unchanged, ListGrants showing both, RevokeGrant removing them, and the second RevokeGrant answering NotFoundException with "Grant ID <id> not found".

Cause 4 — [mismatch] kms.import-key-material-token-single-use-rejected
  check line 121: "AssertionError: expected message 'Key material (id: 0520db4c0ef30b6f35a4615f0868c4663be2d1854ebda5a7e74c737caff70da7) being imported as NEW_KEY_MATERIAL is already associated with KMS key arn:aws:kms:us-west-2:000000000000:key/6e3d4eeb-267d-4079-b361-f7eee36c5e48.', got 'expected new key material but the key material being imported is already associated with the KMS key'"
  Fixed text: "Key material (id: " ... ") being imported as NEW_KEY_MATERIAL is already associated with KMS key " ... "."; variable: the 64-hex material id and the key ARN.

Cause 5 — [mismatch] kms.generate-data-key-spec-and-bytes-exclusivity
  check line 71: "AssertionError: expected message 'Please specify either number of bytes or key spec.', got 'KeySpec and NumberOfBytes both specify the length of the data key; specify either one of them, but not both.'"
  Code ValidationException matched; the message is wholly fixed text with no variable parts.

Cause 6 — [mismatch] kms.create-custom-key-store-requires-active-cluster-rejected
  check line 65: "AssertionError: expected ('CloudHsmClusterNotActiveException', 'Cluster is not active.'), got ('CloudHsmClusterNotActiveException', '')"

Cause 7 — [mismatch] kms.kms-cross-region-decrypt-of-a-single-region-ciphertext-rejected
  check line 143: "AssertionError: expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: kms:Decrypt on the resource associated with this ciphertext because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access', got 'The ciphertext refers to a customer master key that does not exist, does not exist in this region, or you are not allowed to access.'"
  Code AccessDeniedException matched. Fixed text is everything but the leading principal ARN, which is the caller's own signing identity (shown here as the emulator's default user).

## Steps to Reproduce
Boot the emulator from this checkout and replay the eight check ids in trace_ids. Per cause:

1. (Cause 1) kms.CreateKey(Origin='EXTERNAL'); GetParametersForImport; ImportKeyMaterial(wrapped material A) -> note KeyMaterialId; DeleteImportedKeyMaterial; GetParametersForImport again; ImportKeyMaterial(wrapped material B, 32 bytes, B != A) -> observe HTTP 200 and a NEW KeyMaterialId; expect HTTP 400 IncorrectKeyMaterialException with "Imported key material does not have the expected identifier (<A's KeyMaterialId>)." and the key still PendingImport. Repeat with ImportType='EXISTING_KEY_MATERIAL' on the Enabled key for the second check.
2. (Cause 2) kms.CreateCustomKeyStore(CustomKeyStoreName=<new>, CustomKeyStoreType='EXTERNAL_KEY_STORE', XksProxyUriEndpoint='https://<a host that answers nothing>', XksProxyUriPath='/kms/xks/v1', XksProxyConnectivity='PUBLIC_ENDPOINT', XksProxyAuthenticationCredential={AccessKeyId:<20-30 base32 chars>, RawSecretAccessKey:<43-64 base64 chars>}) -> observe HTTP 200 and a CustomKeyStoreId; expect HTTP 400 XksProxyUriUnreachableException and DescribeCustomKeyStores showing no such store.
3. (Cause 3) kms.CreateKey; CreateGrant(KeyId, GranteePrincipal=<account root>, Operations=['Encrypt'], Name='n') -> id X; RevokeGrant(KeyId, GrantId=X); CreateGrant with the identical parameters -> observe a GrantId != X; expect X.
4. (Cause 4) kms.CreateKey(Origin='EXTERNAL'); GetParametersForImport; ImportKeyMaterial(material A); GetParametersForImport again; ImportKeyMaterial(the SAME material A, ImportType='NEW_KEY_MATERIAL') -> compare Error.Message against "Key material (id: <A's KeyMaterialId>) being imported as NEW_KEY_MATERIAL is already associated with KMS key <key ARN>."
5. (Cause 5) kms.GenerateDataKey(KeyId=<key>, KeySpec='AES_256', NumberOfBytes=32) -> expect ValidationException "Please specify either number of bytes or key spec."
6. (Cause 6) kms.CreateCustomKeyStore(CustomKeyStoreName=<new>, CloudHsmClusterId=<an existing but UNINITIALIZED cluster>, TrustAnchorCertificate=<cert>, KeyStorePassword=<pw>) -> expect ('CloudHsmClusterNotActiveException', 'Cluster is not active.'); observe an empty Error.Message.
7. (Cause 7) kms.CreateKey in region A (single-Region); Encrypt under it; call Decrypt in region B passing KeyId=<the region-A key ARN> and that ciphertext -> expect AccessDeniedException whose message is "User: <caller ARN> is not authorized to perform: kms:Decrypt on the resource associated with this ciphertext because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access"; observe the legacy "The ciphertext refers to a customer master key that does not exist..." sentence.
