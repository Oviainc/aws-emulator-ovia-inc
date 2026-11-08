Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.update-custom-key-store-reconfigures-cluster
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.update-custom-key-store-reconfigures-cluster|run#11|AssertionError|assert unreachable == "XksProxyUriUnreachableException", (
Issue-Commit: bd2652b5ddba3816e4efa759e4736f1b7b87af71

# REOPENS: CreateCustomKeyStore(EXTERNAL_KEY_STORE) still answers NotImplemented instead of probing the XKS proxy synchronously and refusing XksProxyUriUnreachableException (1 cause)

> _Note: this issue was identified against the code base at commit `bd2652b5ddba3816e4efa759e4736f1b7b87af71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomKeyStore

## Description
REOPENS 20261024T140708Z-kms-error-semantics-checks-are-blocked-by-five.md: its fix left the call refused. That resolved issue's Cause 5 explicitly named "the CreateCustomKeyStore XKS-reachability precondition" alongside the UpdateCustomKeyStore target resolution. The UpdateCustomKeyStore half landed — the check now gets past the absent/malformed-id collapse at line 58 and fails further along at line 73 — but CreateCustomKeyStore is still refused with Error.Code NotImplemented, so the contract remains ungraded.

Cause 1 of 1: CreateCustomKeyStore does not perform the synchronous XKS proxy reachability probe, so no XksProxyUriUnreachableException can ever be produced.
AWS:      Smithy: CreateCustomKeyStore takes CustomKeyStoreName (required, @length 1..256), CustomKeyStoreType (CustomKeyStoreType enum, value EXTERNAL_KEY_STORE), XksProxyUriEndpoint (@length 10..128, pattern ^https://[a-zA-Z0-9.-]+$), XksProxyUriPath (pattern ^(/[a-zA-Z0-9\/_-]+/kms/xks/v\d{1,2})$|^(/kms/xks/v\d{1,2})$), XksProxyConnectivity and XksProxyAuthenticationCredential, and models XksProxyUriUnreachableException (client, httpError 400, awsQueryError XksProxyUriUnreachableException) alongside XksProxyInvalidConfigurationException(400), XksProxyInvalidResponseException(400), XksProxyIncorrectAuthenticationCredentialException(400), XksProxyUriEndpointInUseException(400), XksProxyVpcEndpointServiceNotFoundException(400), CloudHsmClusterNotActiveException(400), CustomKeyStoreNameInUseException(400), LimitExceededException(400) and KMSInternalException(500). The contract: for CustomKeyStoreType=EXTERNAL_KEY_STORE, KMS contacts the external key store proxy SYNCHRONOUSLY during CreateCustomKeyStore and refuses a well-formed but unreachable endpoint with XksProxyUriUnreachableException (HTTP 400), rather than accepting the store and deferring the probe to ConnectCustomKeyStore; the rejected create must bring no key store into existence. Recorded behavior shows an empty Error.Message on the sibling custom-key-store refusals in this check, so the requirement here is the status and code; the message has no variable parts to template.
Emulator: the call returns Error.Code NotImplemented ("operation CreateCustomKeyStore is scaffolded but not implemented in this emulator"), so the probe never happens and the XKS error taxonomy is unreachable.
Why:      A client building an external key store cannot distinguish "your proxy endpoint is unreachable" — the single most common XKS setup failure, and the only one AWS surfaces at create time — from a broken service. Worse, an emulator that later implements the create without the synchronous probe would silently ACCEPT a custom key store that real KMS refuses outright, which is the overly-permissive failure class this check exists to catch.
Fix:      The CreateCustomKeyStore handler's EXTERNAL_KEY_STORE branch and its XKS-reachability precondition, in the KMS crate. The open issue 20261024T102105Z-kms-createcustomkeystore-stub.md names the bare stub but carries neither this check's replay key nor the synchronous-probe contract; the replay report attributes that stub to a different check (kms.custom-key-store-connection-state-server-managed) only.
Traces:   kms.update-custom-key-store-reconfigures-cluster, asserting at check_kms_update_custom_key_store_reconfigures_cluster.py:73. Handler: services/service-com-amazonaws-kms-trentservice/src/ops/create_custom_key_store.rs.

## Evidence
Replay output, verbatim assertion tail:

[mismatch] kms.update-custom-key-store-reconfigures-cluster
  File "check_kms_update_custom_key_store_reconfigures_cluster.py", line 73, in run
    assert unreachable == "XksProxyUriUnreachableException", (
  AssertionError: expected CreateCustomKeyStore to test the XKS proxy synchronously and fail with XksProxyUriUnreachableException, got 'NotImplemented'

Request:  CreateCustomKeyStore(CustomKeyStoreName=<unique>, CustomKeyStoreType="EXTERNAL_KEY_STORE",
                               XksProxyUriEndpoint="https://<well-formed but unreachable host>",
                               XksProxyUriPath="/kms/xks/v1", XksProxyConnectivity=...,
                               XksProxyAuthenticationCredential=...)
Expected: HTTP 400, Error.Code = XksProxyUriUnreachableException, and no custom key store created
Actual:   Error.Code = NotImplemented ("operation CreateCustomKeyStore is scaffolded but not implemented in this emulator")

Progress relative to the resolved issue it reopens: the same check previously failed at line 58 with
  AssertionError: expected CustomKeyStoreNotFoundException for an absent key store id, got 'NotImplemented'
so the UpdateCustomKeyStore absent/malformed-id collapse named by Cause 5 of 20261024T140708Z-kms-error-semantics-checks-are-blocked-by-five.md now holds; only the CreateCustomKeyStore XKS leg of that same cause is still refused.

Smithy citation (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json): CreateCustomKeyStore input CustomKeyStoreName required (@length 1..256), CustomKeyStoreType enum, XksProxyUriEndpoint @length 10..128 pattern ^https://[a-zA-Z0-9.-]+$, XksProxyUriPath pattern ^(/[a-zA-Z0-9\/_-]+/kms/xks/v\d{1,2})$|^(/kms/xks/v\d{1,2})$; modeled error XksProxyUriUnreachableException (client, httpError 400, awsQueryError XksProxyUriUnreachableException). The synchronous-at-create-time nature of the probe is recorded behavior.

## Steps to Reproduce
Boot the emulator and replay the behavioral check:

  kms.update-custom-key-store-reconfigures-cluster

Manual repro with any AWS SDK pointed at the emulator's KMS endpoint:
1. CreateCustomKeyStore(CustomKeyStoreName="xks-probe-1", CustomKeyStoreType="EXTERNAL_KEY_STORE",
   XksProxyUriEndpoint="https://xks.example.invalid", XksProxyUriPath="/kms/xks/v1",
   XksProxyConnectivity="PUBLIC_ENDPOINT",
   XksProxyAuthenticationCredential={AccessKeyId=<any valid-shaped id>, RawSecretAccessKey=<any valid-shaped secret>}).
   Expect HTTP 400 Error.Code=XksProxyUriUnreachableException; observe Error.Code=NotImplemented.
2. DescribeCustomKeyStores() must show that no key store was created by the rejected call.
3. Regression guard for the already-landed half of the reopened issue: UpdateCustomKeyStore(CustomKeyStoreId="cks-0000000000000000d", NewCustomKeyStoreName="x") and UpdateCustomKeyStore(CustomKeyStoreId="not-a-cks", NewCustomKeyStoreName="x") must BOTH still answer CustomKeyStoreNotFoundException (400) with an empty message, and DescribeCustomKeyStores(CustomKeyStoreId="cks-0000000000000000d") must agree.
