Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.create-custom-key-store-requires-active-cluster-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.create-custom-key-store-requires-active-cluster-rejected|run#15|AssertionError|assert refused == expected, f"expected {expected!r}, got {refused!r}"
Issue-Commit: 64e28f4ec003624b84dff5d61ed4196a531f8dd0

# KMS CreateCustomKeyStore refuses an inactive CloudHSM cluster with an empty Error.Message instead of "Cluster is not active."

> _Note: this issue was identified against the code base at commit `64e28f4ec003624b84dff5d61ed4196a531f8dd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomKeyStore

## Description
One net-new root cause in the KMS crate.

Cause 1 of 1: CreateCustomKeyStore on an existing-but-uninitialized CloudHSM cluster raises the right error code but with no Error.Message, so the modelled `message` member is never populated.
Failing check ids for this cause: kms.create-custom-key-store-requires-active-cluster-rejected

AWS:      Smithy (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json): CreateCustomKeyStore models CloudHsmClusterNotActiveException as errorKind=client, httpError 400, awsQueryError code CloudHsmClusterNotActiveException, with one member `message` targeting `ErrorMessageType`; its documentation is "The request was rejected because the CloudHSM cluster associated with the CloudHSM key store is not active. Initialize and activate the cluster and try the command again." Recorded behavior shows that for this refusal KMS populates that member with the FIXED text `Cluster is not active.` — the whole string is fixed text, it carries no variable parts (no cluster id, no key store name, no limit is interpolated into it). The status is HTTP 400 and the code is CloudHsmClusterNotActiveException. Recorded behavior also shows that the sibling refusals exercised by the same check carry NO message: DescribeCustomKeyStores on an absent store answers CustomKeyStoreNotFoundException with an empty message, and CreateCustomKeyStore naming a syntactically valid but non-existent cluster id answers CloudHsmClusterNotFoundException with an empty message. So "empty message" is correct for those two codes and specifically wrong for CloudHsmClusterNotActiveException — the message is what distinguishes this refusal class, not an incidental per-run value.
Emulator: the call is correctly refused with code CloudHsmClusterNotActiveException, but with an empty Error.Message. Replay reports `AssertionError: expected ('CloudHsmClusterNotActiveException', 'Cluster is not active.'), got ('CloudHsmClusterNotActiveException', '')`. The handler constructs the error with no message at all: `CreateCustomKeyStoreErrorModeled::CloudHsmClusterNotActiveException(CloudHsmClusterNotActiveException { message: None })`, reached from the `Err(crate::state::cloud_hsm::ClusterRefusal::NotActive)` arm.
Why:      A client that surfaces or matches on Error.Message gets an empty string where AWS gives an actionable sentence, so operator-facing output and log/alert text built from the KMS refusal is blank, and any client or test that branches on the message cannot tell the not-active refusal from any other message-less KMS 400. Because the sibling CloudHSM/custom-key-store refusals legitimately send an empty message, an empty message here collapses three distinct refusal classes into one indistinguishable shape at the message level.
Fix:      The CreateCustomKeyStore handler's CloudHSM precondition branch in the KMS crate — the `ClusterRefusal::NotActive` arm must build the modelled error with its message populated with the fixed text for this error class, rather than `message: None`. This is the handler/validation layer, not the serializer: the serializer already emits the code and 400 correctly, and the neighbouring `ClusterRefusal`/not-found arms are correct in leaving the message absent. Note that the open issue 20261026T090118Z-kms-reopens-createcustomkeystore-external-key-store-still-answers-notimplemented.md also concerns CreateCustomKeyStore, but it is about the EXTERNAL_KEY_STORE branch still answering NotImplemented and the missing synchronous XKS reachability probe; it neither carries this check's replay key nor mentions the CloudHSM not-active message, so it does not cover this cause.
Traces:   kms.create-custom-key-store-requires-active-cluster-rejected, asserting at check_kms_create_custom_key_store_requires_active_cluster_rejected.py:71. Handler: services/service-com-amazonaws-kms-trentservice/src/ops/create_custom_key_store.rs (the `ClusterRefusal::NotActive` arm, around lines 648-655).

## Evidence
Replay output, verbatim assertion tail:

[mismatch] kms.create-custom-key-store-requires-active-cluster-rejected
  File "check_kms_create_custom_key_store_requires_active_cluster_rejected.py", line 71, in run
    assert refused == expected, f"expected {expected!r}, got {refused!r}"
  AssertionError: expected ('CloudHsmClusterNotActiveException', 'Cluster is not active.'), got ('CloudHsmClusterNotActiveException', '')

Request:  CreateCustomKeyStore(CustomKeyStoreName=<unique>, CloudHsmClusterId=<id of a CloudHSM cluster
          that exists in this account but was never initialized/activated — DescribeClusters reports a
          state other than ACTIVE>, TrustAnchorCertificate=<well-formed self-signed PEM>,
          KeyStorePassword=<valid-shaped password>)
Expected: HTTP 400, Error.Code = CloudHsmClusterNotActiveException,
          Error.Message = "Cluster is not active."  (entire string is fixed text; no request or
          state values are interpolated)
Actual:   HTTP 400, Error.Code = CloudHsmClusterNotActiveException, Error.Message = "" (absent)

Controls in the SAME check that establish the message is a requirement and not a per-run value:
  - DescribeCustomKeyStores(CustomKeyStoreName=<the rejected name>) must answer
    CustomKeyStoreNotFoundException with an EMPTY message (and prove the rejected create left no store).
  - CreateCustomKeyStore(CloudHsmClusterId="cluster-abcdefghijkl", i.e. well-formed but non-existent)
    must answer CloudHsmClusterNotFoundException with an EMPTY message, and its code must differ from
    the not-active code.
So the emulator's empty message makes the not-active refusal textually identical to the two refusals
that are supposed to be message-less.

Source of the defect in the checkout:
  services/service-com-amazonaws-kms-trentservice/src/ops/create_custom_key_store.rs
    Err(crate::state::cloud_hsm::ClusterRefusal::NotActive) => {
        return Err(
            CreateCustomKeyStoreErrorModeled::CloudHsmClusterNotActiveException(
                CloudHsmClusterNotActiveException { message: None },
            )
            .into(),
        );
    }

Smithy citation: CloudHsmClusterNotActiveException — errorKind client, httpError 400, awsQueryError
code CloudHsmClusterNotActiveException, member `message` -> ErrorMessageType; bound as a modeled error
of CreateCustomKeyStore (the handler already enumerates it in CreateCustomKeyStoreErrorModeled).

## Steps to Reproduce
Boot the emulator and replay the behavioral check:

  kms.create-custom-key-store-requires-active-cluster-rejected

Manual repro with any AWS SDK pointed at the emulator's endpoints:
1. Create a VPC with two subnets in two different Availability Zones (EC2 CreateVpc / CreateSubnet).
2. cloudhsmv2.CreateCluster(HsmType="hsm2m.medium", Mode="NON_FIPS", SubnetIds=<the two subnets>,
   BackupRetentionPolicy={Type="DAYS", Value="7"}) and poll DescribeClusters until the cluster leaves
   CREATE_IN_PROGRESS. Do NOT initialize or activate it — its state must not be ACTIVE.
3. kms.CreateCustomKeyStore(CustomKeyStoreName="cks-not-active-1", CloudHsmClusterId=<that cluster id>,
   TrustAnchorCertificate=<any well-formed self-signed PEM>, KeyStorePassword="Sim2RealPw1").
   Expect HTTP 400 with Error.Code=CloudHsmClusterNotActiveException AND
   Error.Message="Cluster is not active."; observe Error.Code=CloudHsmClusterNotActiveException with an
   empty Error.Message.
4. Regression guards that must keep holding after the fix:
   - kms.DescribeCustomKeyStores(CustomKeyStoreName="cks-not-active-1") answers
     CustomKeyStoreNotFoundException (400) with an EMPTY message, proving the rejected create made no store.
   - kms.CreateCustomKeyStore(CustomKeyStoreName="cks-not-active-2",
     CloudHsmClusterId="cluster-abcdefghijkl", same cert and password) answers
     CloudHsmClusterNotFoundException (400) with an EMPTY message, and that code differs from the
     not-active code.
