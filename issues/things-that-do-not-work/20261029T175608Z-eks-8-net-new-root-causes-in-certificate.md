Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.activate-certificate-authority-already-active-rejected eks.certificate-authority-activation-is-service-managed eks.certificate-authority-resource-limit-exceeded-rejected eks.create-certificate-authority-resource-limit-exceeded-rejected eks.delete-inuse-certificate-authority-rejected eks.create-pod-identity-association-conflicting-role-rejected eks.duplicate-podidentity-association-rejected eks.e2e-pod-identity-role-assumption-flow eks.pod-identity-addon-dependency-teardown-order eks.create-podidentity-association-malformed-role-arn-rejected eks.op-validation-precedence-is-layered eks.update-pod-identity-association-malformed-target-rejected eks.update-pod-identity-association-invalid-namespace-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.activate-certificate-authority-already-active-rejected|run#9|AssertionError|assert err == (
Issue-Key: eks.certificate-authority-activation-is-service-managed|run#11|AssertionError|assert err == (
Issue-Key: eks.certificate-authority-resource-limit-exceeded-rejected|run#13|AssertionError|assert err == ("InvalidRequestException", "Cluster already has the maximum number of certificate authorities (2). Delete an existing CA before creating a new on
Issue-Key: eks.create-certificate-authority-resource-limit-exceeded-rejected|run#20|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: eks.delete-inuse-certificate-authority-rejected|run#9|AssertionError|assert message == "Cannot delete the active certificate authority. Activate a different CA first.", message
Issue-Key: eks.create-pod-identity-association-conflicting-role-rejected|run#14|AssertionError|assert message == f"The service account is already associated with a different IAM role: {role_a}", message
Issue-Key: eks.duplicate-podidentity-association-rejected|run#13|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: eks.e2e-pod-identity-role-assumption-flow|run#29|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: eks.pod-identity-addon-dependency-teardown-order|run#21|AssertionError|assert msg_after == assoc_gone_msg, f"describe of a deleted association -> {msg_after!r}"
Issue-Key: eks.create-podidentity-association-malformed-role-arn-rejected|run#6|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: eks.op-validation-precedence-is-layered|run#14|AssertionError|assert code == "AccessDeniedException", f"pod-identity cross-account role -> {code}: {msg}"
Issue-Key: eks.update-pod-identity-association-malformed-target-rejected|run#10|AssertionError|assert message == "The provided targetRoleArn not-a-valid-iam-role-arn is invalid.", message
Issue-Key: eks.update-pod-identity-association-invalid-namespace-rejected|run#9|KeyError|assert created["disableSessionTags"] is False
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# eks: 8 net-new root causes in certificate-authority and pod-identity refusals — wrong error codes, wrong messages, PassRole ordering and a missing response member

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePodIdentityAssociation

## Description
Replaying simparity-behavioral-aws-v1 against the eks emulator (crate services/service-com-amazonaws-eks-awswesleyfrontend) exposes EIGHT distinct net-new root causes in the certificate-authority and pod-identity surfaces. All eight must be closed before this issue is complete; each carries its own failing check ids.

Cause 1 of 8 — ActivateCertificateAuthority on a CA that is already the cluster's signer answers the wrong error class and the wrong message.
AWS: Smithy (com.amazonaws.eks#ActivateCertificateAuthority) models only InvalidParameterException (400), ResourceNotFoundException (404), ServerException (500), ServiceUnavailableException (503) — ResourceInUseException is NOT in its error list. Recorded behavior: re-activating the CA that is already IN_USE for a cluster is refused with error code InvalidRequestException (httpError 400) and the message "Certificate authority <certificateAuthorityId> is already active for cluster <clusterName>". Fixed text: "Certificate authority " ... " is already active for cluster "; variable parts: the certificateAuthorityId from the request and the clusterName from the request. The refusal must also leave signingStatus/activatedBy/activatedAt untouched and start no new Update.
Emulator: answers error code ResourceInUseException with message "Certificate authority <id> is in IN_USE signing status; only a certificate authority in NOT_USED signing status can be activated." — an error shape the operation does not model, plus different wording.
Why: a client branching on InvalidRequestException (invalid state transition) never matches; it sees a 409-class conflict error that the SDK's modeled error list for this operation does not even contain, so typed catch blocks fall through to a generic handler.
Fix: the ActivateCertificateAuthority handler's state-transition check in the eks crate's ops module (error selection and message construction).
Traces: eks.activate-certificate-authority-already-active-rejected, eks.certificate-authority-activation-is-service-managed.

Cause 2 of 8 — CreateCertificateAuthority past the per-cluster CA cap answers the wrong error class and message.
AWS: recorded behavior (the modeled list for CreateCertificateAuthority does include ResourceLimitExceededException, but the recording shows AWS does not use it here): a third CreateCertificateAuthority on a cluster that already holds two CAs is refused with error code InvalidRequestException (400) and the message "Cluster already has the maximum number of certificate authorities (2). Delete an existing CA before creating a new one." — that sentence is entirely fixed text apart from the cap value (2).
Emulator: answers ResourceLimitExceededException with "Cluster <clusterName> already has the maximum of 2 certificate authorities".
Why: a client that treats ResourceLimitExceeded as a quota problem (retry/raise a quota ticket) reacts wrongly to what AWS reports as an invalid request about existing state, and the message a user sees loses the remediation sentence.
Fix: the CreateCertificateAuthority handler's cap enforcement (error selection + message).
Traces: eks.certificate-authority-resource-limit-exceeded-rejected, eks.create-certificate-authority-resource-limit-exceeded-rejected.

Cause 3 of 8 — DeleteCertificateAuthority on the IN_USE signer returns the wrong message.
AWS: Smithy: DeleteCertificateAuthority models ResourceInUseException (httpError 409). Recorded behavior: deleting the CA that currently signs for the cluster is refused with the fixed sentence "Cannot delete the active certificate authority. Activate a different CA first." — no variable parts.
Emulator: refuses with "Certificate authority <id> is in IN_USE signing status; the certificate authority that signs for cluster <clusterName> cannot be deleted."
Why: the message is part of the contract surfaced to operators and to tests that assert the remediation text; the emulator's wording omits the remediation ("Activate a different CA first").
Fix: the DeleteCertificateAuthority handler's in-use guard message.
Traces: eks.delete-inuse-certificate-authority-rejected.

Cause 4 of 8 — the duplicate-pod-identity-association conflict reports the wrong message (and does not name the role already bound).
AWS: Smithy: CreatePodIdentityAssociation models ResourceInUseException (409). Recorded behavior: a second CreatePodIdentityAssociation for the same namespace + serviceAccount pair is refused with the message "The service account is already associated with a different IAM role: <roleArn>" — fixed text "The service account is already associated with a different IAM role: " plus the roleArn of the EXISTING association taken from emulator state.
Emulator: refuses with "Association already exists with namespace: <namespace> and service account: <serviceAccount>" — it echoes the request's namespace/serviceAccount and never names the role already bound.
Why: callers (and reconciliation loops) parse this message to learn which role currently owns the service account so they can decide between update and delete; the emulator's text makes that undiscoverable.
Fix: the CreatePodIdentityAssociation handler's conflict detection — build the message from the stored association's roleArn.
Traces: eks.create-pod-identity-association-conflicting-role-rejected, eks.duplicate-podidentity-association-rejected.

Cause 5 of 8 — the not-found message for a pod identity association uses the wrong noun and a trailing period.
AWS: recorded behavior: describing (or otherwise resolving) an associationId that does not exist — including one just deleted — is refused ResourceNotFoundException with the message "No service account role association found for ID: <associationId>" (fixed text "No service account role association found for ID: " + the associationId from the request, NO trailing period).
Emulator: answers "No pod identity association found for ID: <associationId>." (different noun phrase, plus a trailing period).
Why: an exact-match assertion on the refusal text — the normal way these flows are verified, and what both failing checks do — fails against the emulator even though the code is right.
Fix: the message construction shared by DescribePodIdentityAssociation / DeletePodIdentityAssociation / UpdatePodIdentityAssociation association resolution.
Traces: eks.e2e-pod-identity-role-assumption-flow, eks.pod-identity-addon-dependency-teardown-order.

Cause 6 of 8 — CreatePodIdentityAssociation validates/resolves before it authorizes iam:PassRole on roleArn.
AWS: recorded behavior (CreatePodIdentityAssociation's modeled error list does not include AccessDeniedException, so this is recording-grounded): EKS authorizes the pass-role of roleArn FIRST. A roleArn in another account, and equally a syntactically malformed roleArn ("not-an-arn-at-all"), are both refused with error code AccessDeniedException and the fixed sentence "Cross-account pass role is not allowed." — even when the clusterName in the same request does not exist and even though the ARN is malformed. No association is left behind.
Emulator: for a malformed roleArn it answers InvalidParameterException (body validation ran first); for a cross-account roleArn on a missing cluster it answers ResourceNotFoundException "No cluster found for name: <clusterName>." (cluster resolution ran first).
Why: a client or policy test asserting that cross-account role passing is denied sees a parameter complaint or a not-found instead, and an emulator-backed suite concludes cross-account PassRole is permitted for pod identity. Related open issue 20261022T154613Z-eks-control-plane-operations-answer-501-notimplemented-or.md documents the same ordering defect for CreateCluster (its cause 10); it does not cover CreatePodIdentityAssociation.
Fix: the CreatePodIdentityAssociation handler — move roleArn pass-role authorization ahead of body validation and cluster resolution.
Traces: eks.create-podidentity-association-malformed-role-arn-rejected, eks.op-validation-precedence-is-layered.

Cause 7 of 8 — UpdatePodIdentityAssociation reports a malformed targetRoleArn with the generic constraint message instead of EKS's own sentence.
AWS: Smithy: UpdatePodIdentityAssociation.targetRoleArn is a String member and the operation models InvalidParameterException (400). Recorded behavior: a targetRoleArn that is not an IAM role ARN is refused with the message "The provided targetRoleArn <value> is invalid." — fixed text "The provided targetRoleArn " ... " is invalid." with the offending value copied from the request.
Emulator: answers the generic validator text "Value 'not-a-valid-iam-role-arn' at 'targetRoleArn' failed to satisfy constraint: Member must be an IAM role ARN".
Why: the emulator substitutes its framework-level validation message for the operation's documented refusal, so message assertions and user-facing error reporting diverge.
Fix: the UpdatePodIdentityAssociation handler's targetRoleArn validation (operation-specific message, not the shared constraint formatter).
Traces: eks.update-pod-identity-association-malformed-target-rejected.

Cause 8 of 8 — the PodIdentityAssociation returned by CreatePodIdentityAssociation omits disableSessionTags.
AWS: Smithy: com.amazonaws.eks#PodIdentityAssociation has member disableSessionTags (BoxedBoolean, "The state of the automatic sessions tags. The value of true disables these tags."), and CreatePodIdentityAssociation returns `association`: PodIdentityAssociation. Recorded behavior: a create that does not pass disableSessionTags comes back with disableSessionTags present and false.
Emulator: the association object in the create response has no disableSessionTags key at all — the check dies with KeyError: 'disableSessionTags' on created["disableSessionTags"].
Why: a client reading the flag off the create response (rather than re-describing) gets a missing-key crash instead of the default value, and cannot tell whether session tags are enabled.
Fix: the CreatePodIdentityAssociation serializer/state — materialise disableSessionTags (default false) on the stored association and emit it.
Traces: eks.update-pod-identity-association-invalid-namespace-rejected.

## Evidence
Suite simparity-behavioral-aws-v1: 64/310 passed; eks 60/282 (222 failed). Failures below are from the suite's failing-check list and are NOT marked ALREADY FILED or OWNED.

Cause 1: eks.activate-certificate-authority-already-active-rejected — AssertionError: expected the already-active refusal, got ('ResourceInUseException', 'Certificate authority 004bd519-7382-44be-b338-1b6850d11799 is in IN_USE signing status; only a certificate authority in NOT_USED signing status can be activated.'); expected ('InvalidRequestException', 'Certificate authority <id> is already active for cluster <cluster>'). eks.certificate-authority-activation-is-service-managed — same pair.
Cause 2: eks.certificate-authority-resource-limit-exceeded-rejected — AssertionError: expected InvalidRequestException for a third CA past the two-CA cap, got ('ResourceLimitExceededException', 'Cluster sim2real-eks-cluster-c0aa50429bc5 already has the maximum of 2 certificate authorities'). eks.create-certificate-authority-resource-limit-exceeded-rejected — AssertionError: expected InvalidRequestException, got 'ResourceLimitExceededException'.
Cause 3: eks.delete-inuse-certificate-authority-rejected — AssertionError: 'Certificate authority 54ce27c7-fb25-45a8-822b-b08e8ecfc3e5 is in IN_USE signing status; the certificate authority that signs for cluster sim2real-eks-c5a12a1a22c0 cannot be deleted.' (expected 'Cannot delete the active certificate authority. Activate a different CA first.').
Cause 4: eks.create-pod-identity-association-conflicting-role-rejected — AssertionError: 'Association already exists with namespace: default and service account: sim2real-sa-b5fa656bea42' (expected 'The service account is already associated with a different IAM role: <roleArn>'). eks.duplicate-podidentity-association-rejected — AssertionError: expected message 'The service account is already associated with a different IAM role: arn:aws:iam::000000000000:role/sim2real-eks-podid-role-ccdfaad48efb', got 'Association already exists with namespace: default and service account: sim2real-sa-04f80c5a15ec'.
Cause 5: eks.e2e-pod-identity-role-assumption-flow — AssertionError: expected message 'No service account role association found for ID: a-dl9780kgk9o8gsdoy', got 'No pod identity association found for ID: a-dl9780kgk9o8gsdoy.'. eks.pod-identity-addon-dependency-teardown-order — AssertionError: describe of a deleted association -> 'No pod identity association found for ID: a-33d9mya3wnikzc9ld.'.
Cause 6: eks.create-podidentity-association-malformed-role-arn-rejected — AssertionError: expected AccessDeniedException, got InvalidParameterException (roleArn='not-an-arn-at-all'). eks.op-validation-precedence-is-layered — AssertionError: pod-identity cross-account role -> ResourceNotFoundException: No cluster found for name: sim2real-behav-eks-ghost-40e934202bbc. (expected AccessDeniedException 'Cross-account pass role is not allowed.'); legs (a) (b) (c) of that check passed, so only the PassRole layer is missing.
Cause 7: eks.update-pod-identity-association-malformed-target-rejected — AssertionError: Value 'not-a-valid-iam-role-arn' at 'targetRoleArn' failed to satisfy constraint: Member must be an IAM role ARN (expected "The provided targetRoleArn not-a-valid-iam-role-arn is invalid.").
Cause 8: eks.update-pod-identity-association-invalid-namespace-rejected — KeyError: 'disableSessionTags' at `assert created["disableSessionTags"] is False`.

## Steps to Reproduce
1. Boot the emulator and point a boto3 eks client at it.
2. Cause 1: create a cluster, list_certificate_authorities, take the CA whose signingStatus is IN_USE, then activate_certificate_authority(clusterName=<cluster>, certificateAuthorityId=<that id>) -> expect 400 InvalidRequestException 'Certificate authority <id> is already active for cluster <cluster>'; observe 409 ResourceInUseException with the IN_USE/NOT_USED wording.
3. Cause 2: create_certificate_authority twice beyond the cluster's two-CA cap -> expect 400 InvalidRequestException 'Cluster already has the maximum number of certificate authorities (2). Delete an existing CA before creating a new one.'; observe ResourceLimitExceededException.
4. Cause 3: delete_certificate_authority on the IN_USE CA -> expect 409 ResourceInUseException 'Cannot delete the active certificate authority. Activate a different CA first.'.
5. Cause 4: create_pod_identity_association twice for the same namespace+serviceAccount with different roleArns -> expect the conflict message naming the EXISTING roleArn.
6. Cause 5: delete_pod_identity_association, then describe_pod_identity_association with the same associationId -> expect ResourceNotFoundException 'No service account role association found for ID: <id>' (no trailing period).
7. Cause 6: create_pod_identity_association(clusterName=<absent cluster>, namespace='default', serviceAccount='sim2real-sa', roleArn='arn:aws:iam::111122223333:role/x') and the same call with roleArn='not-an-arn-at-all' -> both expect 403-class AccessDeniedException 'Cross-account pass role is not allowed.' and no association created.
8. Cause 7: update_pod_identity_association(..., targetRoleArn='not-a-valid-iam-role-arn') -> expect InvalidParameterException 'The provided targetRoleArn not-a-valid-iam-role-arn is invalid.'.
9. Cause 8: create_pod_identity_association without disableSessionTags -> the returned association must contain disableSessionTags=false.
