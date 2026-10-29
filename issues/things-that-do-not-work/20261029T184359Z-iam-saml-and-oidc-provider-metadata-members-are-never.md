Issue-Source: trace-sweep-divergence
Issue-Trace: iam-saml-provider-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-saml-provider-lifecycle iam-saml-provider-update iam-identity-providers-saml-and-oidc-batch6 iam-oidc-provider-lifecycle iam-add-client-id-to-openid-connect-provider
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetSAMLProvider|2xx->2xx|$.GetSAMLProviderResponse.GetSAMLProviderResult.AssertionEncryptionMode
Issue-Key: iam|GetSAMLProvider|2xx->2xx|$.GetSAMLProviderResponse.GetSAMLProviderResult.SAMLKeyHash
Issue-Key: iam|GetSAMLProvider|2xx->2xx|$.GetSAMLProviderResponse.GetSAMLProviderResult.Tags
Issue-Key: iam|GetSAMLProvider|2xx->2xx|$.GetSAMLProviderResponse.GetSAMLProviderResult.ValidUntil
Issue-Key: iam|GetSAMLProvider|2xx->2xx|$.GetSAMLProviderResponse.GetSAMLProviderResult.SAMLProviderUUID
Issue-Key: iam|ListSAMLProviders|2xx->2xx|$.ListSAMLProvidersResponse.ListSAMLProvidersResult.SAMLProviderList.member.IsTrustedIdentityPropagationEnabled
Issue-Key: iam|ListSAMLProviders|2xx->2xx|$.ListSAMLProvidersResponse.ListSAMLProvidersResult.SAMLProviderList.member.ValidUntil
Issue-Key: iam|ListOpenIDConnectProviders|2xx->2xx|$.ListOpenIDConnectProvidersResponse.ListOpenIDConnectProvidersResult.OpenIDConnectProviderList.member.IsTrustedIdentityPropagationEnabled
Issue-Key: iam|GetOpenIDConnectProvider|2xx->2xx|$.GetOpenIDConnectProviderResponse.GetOpenIDConnectProviderResult.ThumbprintList
Issue-Related: 20261029T184359Z-iam-modeled-empty-members-are-omitted-from-iam-success.md
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# SAML and OIDC provider metadata members are never returned

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSAMLProvider

## Description
Cause 1 of 3 - GetSAMLProvider omits the provider's derived and settable metadata. AWS: the response carries ValidUntil (timestamp parsed from the uploaded SAML metadata document), AssertionEncryptionMode ('Allowed' by default) and SAMLKeyHash (a list whose members pair IssuerURL with KeyHashes, derived from the signing keys in the metadata). Smithy: GetSAMLProviderResponse.ValidUntil (dateType) and .AssertionEncryptionMode (enum) are modelled; SAMLKeyHash is recorded behavior only (the checked-out model carries PrivateKeyList in its place), so its contract is taken from the recording. The recording also shows SAMLProviderUUID failing its shape assertion against the emulator's value. Emulator: ValidUntil, AssertionEncryptionMode and SAMLKeyHash are all absent from the response. Why: a client that reads the provider's expiry or encryption mode - the two fields that drive SAML rotation - gets nulls, and cannot verify which signing key the provider trusts. Fix: the GetSAMLProvider handler must derive and persist these fields when the metadata document is uploaded, and the response serializer must emit them.
Traces: iam-saml-provider-lifecycle#task#2; iam-identity-providers-saml-and-oidc-batch6#task#3.

Cause 2 of 3 - the provider list entries omit their flags. AWS: ListSAMLProviders entries carry ValidUntil and IsTrustedIdentityPropagationEnabled ('false' when not enabled), and ListOpenIDConnectProviders entries carry IsTrustedIdentityPropagationEnabled ('false'). Emulator: these members are absent from each list entry. Why: a client enumerating providers cannot tell which are trusted-identity-propagation enabled or when they expire without a per-provider Get, and a client that reads the member unconditionally (safe against AWS) throws. Fix: the ListSAMLProviders / ListOpenIDConnectProviders entry serializers.
Traces: iam-identity-providers-saml-and-oidc-batch6#task#4 (ListSAMLProviders); iam-identity-providers-saml-and-oidc-batch6#task#7 (ListOpenIDConnectProviders).

Cause 3 of 3 - GetOpenIDConnectProvider returns an empty ThumbprintList. AWS: the provider's ThumbprintList is returned as a member list containing each thumbprint supplied to CreateOpenIDConnectProvider / UpdateOpenIDConnectProviderThumbprint. Emulator: the element is serialised empty ('') - the thumbprints are not persisted or not read back - and Tags is omitted entirely. Why: a client verifying its provider's thumbprint before rotating it sees no thumbprint at all, so the rotation contract is unobservable. Fix: persist the thumbprint list in OIDC provider state and serialise it in the Get handler.
Traces: iam-add-client-id-to-openid-connect-provider#task#3.

## Evidence
Cause 1: expected (recorded AWS) GetSAMLProviderResult carries SAMLKeyHash/member{IssuerURL, KeyHashes/member <hex digest>}, ValidUntil and AssertionEncryptionMode 'Allowed'; actual all three `missing (expected ..., got None)`, and SAMLProviderUUID is reported value_mismatch against the recorded shape (emulator emits 'SAMLPBGXEQES5NRCCJJWAN').
Cause 2: expected ListSAMLProvidersResult/SAMLProviderList/member/IsTrustedIdentityPropagationEnabled 'false' and .../ValidUntil; actual both absent. Expected ListOpenIDConnectProvidersResult/OpenIDConnectProviderList/member/IsTrustedIdentityPropagationEnabled 'false'; actual absent.
Cause 3: expected GetOpenIDConnectProviderResult/ThumbprintList/member 'ab9d0263244dd0326eb67015705a667e79cfe998' (the thumbprint supplied at create time); actual ThumbprintList serialised as '' (type_mismatch), and Tags `missing (expected '', got None)`.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-saml-provider-lifecycle: task#1 CreateSAMLProvider with a metadata document, task#2 GetSAMLProvider - ValidUntil, AssertionEncryptionMode and SAMLKeyHash are all absent.
3. Replay iam-identity-providers-saml-and-oidc-batch6#task#4 and #task#7 - the list entries carry no IsTrustedIdentityPropagationEnabled.
4. Replay iam-add-client-id-to-openid-connect-provider#task#3 - GetOpenIDConnectProvider returns an empty ThumbprintList despite the thumbprint given at create time.
