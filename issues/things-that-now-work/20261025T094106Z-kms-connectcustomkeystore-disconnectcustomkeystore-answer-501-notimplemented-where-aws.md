Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.op-custom-key-store-not-found-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.op-custom-key-store-not-found-rejected|run#10|AssertionError|assert code2 == "CustomKeyStoreNotFoundException", f"ConnectCustomKeyStore -> got {code2}"
Issue-Commit: ee3608f46372fd0499ccb380262b1cbf197ec697

# KMS ConnectCustomKeyStore/DisconnectCustomKeyStore answer 501 NotImplemented where AWS refuses an unknown store id with 400 CustomKeyStoreNotFoundException (1 cause)

> _Note: this issue was identified against the code base at commit `ee3608f46372fd0499ccb380262b1cbf197ec697`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConnectCustomKeyStore

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the custom-key-store connect/disconnect lifecycle operations are unimplemented scaffold stubs, so the not-found refusal contract of the custom-key-store family is only half present ===

AWS: a custom-key-store operation naming a store id that does not exist is refused with the modeled error CustomKeyStoreNotFoundException. Smithy (com.amazonaws.kms): ConnectCustomKeyStore takes a single required member CustomKeyStoreId (CustomKeyStoreIdType, @length 1..64) and models errors CloudHsmClusterInvalidConfigurationException, CloudHsmClusterNotActiveException, CustomKeyStoreInvalidStateException, CustomKeyStoreNotFoundException (all httpError 400) and KMSInternalException (500); DisconnectCustomKeyStore takes the same required member and models CustomKeyStoreInvalidStateException, CustomKeyStoreNotFoundException (400) and KMSInternalException (500). CustomKeyStoreNotFoundException is documented 'The request was rejected because KMS cannot find a custom key store with the specified key store name or ID.' The requirement here is therefore HTTP 400 with error code CustomKeyStoreNotFoundException, consistently across the whole family. Recorded behavior additionally shows these refusals arrive with an EMPTY message body for this family; the check asserts that, and the sibling DescribeCustomKeyStores leg of the same check now passes, which establishes the shape the emulator already produces for one member of the family.

Emulator: DescribeCustomKeyStores(CustomKeyStoreId=<unknown>) is now refused correctly (that assertion passes), but ConnectCustomKeyStore(CustomKeyStoreId=<unknown>) answers error code NotImplemented ('operation ConnectCustomKeyStore is scaffolded but not implemented in this emulator', HTTP 501) instead of 400 CustomKeyStoreNotFoundException. DisconnectCustomKeyStore is never reached because the check aborts on the first divergence, but it is the same unimplemented stub and will answer the same way. Both handlers still return crate::scaffolded::Scaffolded::not_implemented(..) and still carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker.

Why: a client that probes or reconciles custom key stores cannot distinguish 'this store id does not exist' from 'this API is unavailable'. NotImplemented is not a modeled KMS error, so an SDK-generated error type for it does not exist and retry/branching logic keyed on CustomKeyStoreNotFoundException never fires; a 501 is also commonly treated as a transient server fault, so callers retry an input error forever. Because DescribeCustomKeyStores now refuses correctly while Connect/Disconnect do not, the family is internally inconsistent: the same bogus id yields two different error classes depending on which operation is called.

Fix: the two operation handlers, which must resolve CustomKeyStoreId against custom-key-store state and return the modeled CustomKeyStoreNotFoundException (400, empty message) when no store matches, instead of the scaffold's 501. Note this is NOT covered by the existing open stub issues 20261024T102105Z-kms-createcustomkeystore-stub.md or 20261024T102105Z-kms-updatecustomkeystore-stub.md (different operations), and the fixed 20261024T102105Z-kms-describecustomkeystores-stub.md covers only the Describe leg, which is why the check now gets past Describe and fails at Connect.

Traces: kms.op-custom-key-store-not-found-rejected (handler files, for reference: services/service-com-amazonaws-kms-trentservice/src/ops/connect_custom_key_store.rs:210 and .../disconnect_custom_key_store.rs:180).

## Evidence
Failing check kms.op-custom-key-store-not-found-rejected, verdict mismatch:

  File "check_kms_op_custom_key_store_not_found_rejected.py", line 45, in run
    assert code2 == "CustomKeyStoreNotFoundException", f"ConnectCustomKeyStore -> got {code2}"
  AssertionError: ConnectCustomKeyStore -> got NotImplemented

The check first establishes a positive control: kms.describe_custom_key_stores() returns a well-formed listing containing 'CustomKeyStores', and describe_custom_key_stores(CustomKeyStoreId='cks-1234567890abcdef0') is refused with code CustomKeyStoreNotFoundException and an empty message — both of those assertions PASS against the emulator. It then calls connect_custom_key_store(CustomKeyStoreId='cks-1234567890abcdef0').

Expected (AWS / Smithy model): HTTP 400, Error.Code = 'CustomKeyStoreNotFoundException', Error.Message = '' (the family sends these refusals with no message text).
Actual (emulator): Error.Code = 'NotImplemented', message 'operation ConnectCustomKeyStore is scaffolded but not implemented in this emulator' (HTTP 501).

The third leg, disconnect_custom_key_store(CustomKeyStoreId='cks-1234567890abcdef0'), asserting the same 400/CustomKeyStoreNotFoundException, was not reached. Static confirmation of both stubs in the checkout: services/service-com-amazonaws-kms-trentservice/src/ops/connect_custom_key_store.rs line 4 ('SCAFFOLDED, NOT IMPLEMENTED') and line 210 (Err(Scaffolded::not_implemented("ConnectCustomKeyStore"))); services/service-com-amazonaws-kms-trentservice/src/ops/disconnect_custom_key_store.rs line 4 and line 180. This check was NOT marked 'ALREADY FILED' or 'OWNED by' in the replay report, and no open issue names ConnectCustomKeyStore or DisconnectCustomKeyStore.

## Steps to Reproduce
1. kms:DescribeCustomKeyStores with no filter — succeeds, returns a CustomKeyStores list (positive control: the custom-key-store surface is reachable).
2. kms:DescribeCustomKeyStores(CustomKeyStoreId='cks-1234567890abcdef0') — correctly refused today with 400 CustomKeyStoreNotFoundException and an empty message.
3. kms:ConnectCustomKeyStore(CustomKeyStoreId='cks-1234567890abcdef0') — AWS answers HTTP 400 Error.Code='CustomKeyStoreNotFoundException' with an empty message; the emulator answers HTTP 501 Error.Code='NotImplemented'.
4. kms:DisconnectCustomKeyStore(CustomKeyStoreId='cks-1234567890abcdef0') — same expectation (400 CustomKeyStoreNotFoundException); the handler is the same kind of scaffold stub and answers 501 NotImplemented.
