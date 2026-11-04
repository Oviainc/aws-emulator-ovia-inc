Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.access-entry-duplicate-principal-rejected eks.deploy-accessentry-via-cloudformation-duplicate-principal-rejected eks.cancel-update-unknown-update-id-not-found eks.cancel-update-wrong-state-blocked eks.describe-update-not-found eks.nodegroup-update-serialization eks.certificate-authority-activate-before-distribution-complete-rejected eks.certificate-authority-full-lifecycle eks.certificate-authority-delete-in-use-rejected eks.certificate-authority-delete-while-active-blocked eks.certificate-authority-lifecycle-transitions eks.delete-certificate-authority-active-state-blocked eks.delete-certificate-authority-in-use-blocked eks.cluster-insights-read-path eks.e2e-pod-identity-association-role-access-flow eks.e2e-pod-identity-association-role-assumption-flow eks.pod-identity-association-lifecycle eks.pod-identity-association-role-swap-updates-association eks.pod-identity-association-update-changes-role-binding eks.pod-identity-full-lifecycle eks.pod-identity-update-not-found eks.pod-identity-association-duplicate-rejected eks.pod-identity-duplicate-association-rejected eks.pod-identity-association-immutable-fields-rejected eks.nodegroup-duplicate-create-rejected eks.ekscc-delete-cluster-with-deletion-protection-rejected-until-disabled eks.ekscc-update-cluster-config-while-creating-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.access-entry-duplicate-principal-rejected|run#7|AssertionError|assert dup == (
Issue-Key: eks.deploy-accessentry-via-cloudformation-duplicate-principal-rejected|run#22|AssertionError|assert message == "The specified access entry resource is already in use on this cluster.", message
Issue-Key: eks.cancel-update-unknown-update-id-not-found|run#9|AssertionError|assert cancel == ("ResourceNotFoundException", f"Update {unknown_update_id} not found"), (
Issue-Key: eks.cancel-update-wrong-state-blocked|run#11|AssertionError|assert unknown == ("ResourceNotFoundException", f"Update {unknown_id} not found"), (
Issue-Key: eks.describe-update-not-found|run#18|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: eks.nodegroup-update-serialization|run#18|AssertionError|assert missing_msg == f"No update found for ID: {BOGUS_UPDATE_ID}", missing_msg
Issue-Key: eks.certificate-authority-activate-before-distribution-complete-rejected|run#27|AssertionError|assert redelete_msg == expected, f"expected re-delete message {expected!r}, got {redelete_msg!r}"
Issue-Key: eks.certificate-authority-full-lifecycle|run#25|AssertionError|assert message.startswith(
Issue-Key: eks.certificate-authority-delete-in-use-rejected|run#8|AssertionError|assert err == ("ResourceInUseException", "Cannot delete the active certificate authority. Activate a different CA first."), f"expected ResourceInUseException de
Issue-Key: eks.certificate-authority-delete-while-active-blocked|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: eks.certificate-authority-lifecycle-transitions|run#34|AssertionError|assert in_use_delete_msg == expected, f"expected message {expected!r}, got {in_use_delete_msg!r}"
Issue-Key: eks.delete-certificate-authority-active-state-blocked|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: eks.delete-certificate-authority-in-use-blocked|run#11|AssertionError|assert message == "Cannot delete the active certificate authority. Activate a different CA first.", message
Issue-Key: eks.cluster-insights-read-path|run#9|AssertionError|assert unknown_id_msg.startswith(f"Cannot find insight insightId={unknown_id} clusterId=") and (
Issue-Key: eks.e2e-pod-identity-association-role-access-flow|run#22|AssertionError|assert message == f"No service account role association found for ID: {association_id}", message
Issue-Key: eks.e2e-pod-identity-association-role-assumption-flow|run#27|AssertionError|assert msg == expected, f"expected message {expected!r}, got {msg!r}"
Issue-Key: eks.pod-identity-association-lifecycle|run#35|AssertionError|assert not_found_msg == gone_msg, f"describe after delete -> {not_found_msg!r}"
Issue-Key: eks|DescribePodIdentityAssociation|ResourceNotFoundException
Issue-Key: eks.pod-identity-full-lifecycle|run#30|AssertionError|assert describe_message_after_delete == not_found, describe_message_after_delete
Issue-Key: eks.pod-identity-update-not-found|run#19|AssertionError|assert messages == {op: not_found for op in errors}, messages
Issue-Key: eks.pod-identity-association-duplicate-rejected|run#11|AssertionError|assert msg == f"Association already exists: {first_id}", f"unexpected message: {msg!r}"
Issue-Key: eks.pod-identity-duplicate-association-rejected|run#16|AssertionError|assert dup_message == f"The service account is already associated with a different IAM role: {role_one}", (
Issue-Key: eks.pod-identity-association-immutable-fields-rejected|run#12|AssertionError|assert dup_msg == f"The service account is already associated with a different IAM role: {role_arn_1}", dup_msg
Issue-Key: eks.nodegroup-duplicate-create-rejected|run#14|AssertionError|assert msg == f"NodeGroup already exists with name {nodegroup_name} and cluster name {cluster_name}", (
Issue-Key: eks.ekscc-delete-cluster-with-deletion-protection-rejected-until-disabled|_assert_delete_rejected#8|AssertionError|assert got == ("InvalidRequestException",
Issue-Key: eks.ekscc-update-cluster-config-while-creating-rejected|run#12|AssertionError|assert message == want, f"expected message {want!r}, got {message!r}"
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# eks: 10 net-new root causes in not-found / already-in-use refusal wording (access entries, updates, certificate authorities, insights, pod identities, nodegroups, deletion protection, cluster state)

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessEntry

## Description
Ten net-new root causes share one shape: the operation refuses with the right HTTP status but a message (and in one case a sentence structure) that is the emulator's own prose rather than the sentence Amazon EKS returns. An error message is part of the API contract, so each of these is a distinct defect in the handler that produces that refusal. Every value quoted below that comes from a single recorded run (cluster names, ids, ARNs) is a template slot filled from the request/state, never a requirement.

Cause 1 of 10: CreateAccessEntry duplicate principal message.
  AWS: Smithy: CreateAccessEntry errors include ResourceInUseException (httpError 409). A second access entry for a principalArn that already has one is refused ResourceInUseException with the message 'The specified access entry resource is already in use on this cluster.' — the whole sentence is fixed text, with no variable part.
  Emulator: ResourceInUseException with 'The access entry for principal ARN <principalArn> already exists for cluster <clusterName>.' — the emulator's own sentence, interpolating the principal ARN and cluster name.
  Why: a client that matches EKS's documented sentence (or a CloudFormation provider that maps it to an already-exists condition) sees an unrecognised string and treats the conflict as an unknown failure.
  Fix: CreateAccessEntry's handler/validation — emit EKS's fixed already-in-use sentence.
  Traces: eks.access-entry-duplicate-principal-rejected, eks.deploy-accessentry-via-cloudformation-duplicate-principal-rejected.

Cause 2 of 10: an unknown updateId is reported with the emulator's own sentence.
  AWS: Smithy: DescribeUpdate/CancelUpdate declare ResourceNotFoundException (httpError 404). For an updateId that was never issued the message is 'Update <updateId> not found' — fixed text 'Update ' + ' not found', the only variable part being the requested updateId; there is no trailing period and no nodegroupName clause.
  Emulator: ResourceNotFoundException 'No update found for ID: <updateId>.' and, when a nodegroupName is supplied, 'No update found for ID: <updateId> with nodegroupName: <nodegroupName>.'
  Why: callers polling an update id (and CLI/console output) key on EKS's sentence; the differing text and trailing period break exact-match handling and leave the nodegroup variant with a clause AWS never emits.
  Fix: the not-found path of DescribeUpdate/CancelUpdate (cluster, nodegroup and addon variants).
  Traces: eks.cancel-update-unknown-update-id-not-found, eks.cancel-update-wrong-state-blocked, eks.describe-update-not-found, eks.nodegroup-update-serialization.

Cause 3 of 10: an unknown certificate-authority id is reported with the emulator's own sentence.
  AWS: a certificate-authority id that does not exist on the cluster is refused ResourceNotFoundException with 'Certificate authority <id> not found for cluster <clusterName>' — fixed text 'Certificate authority ' / ' not found for cluster ', variables being the requested id and the cluster name, no trailing period.
  Emulator: 'No certificate authority found with id: <id> for cluster: <clusterName>.'
  Why: the same not-found condition is unrecognisable to a client matching EKS's wording; re-delete / post-delete probes cannot distinguish 'already gone' from an unexpected fault.
  Fix: the not-found path shared by the certificate-authority operations. (Related open issue 20261103T041817Z-eks-describecertificateauthority-reports-an-unknown-deleted-certificate-authority.md covers DescribeCertificateAuthority only; these failures are on Activate/Delete and are reported unowned.)
  Traces: eks.certificate-authority-activate-before-distribution-complete-rejected, eks.certificate-authority-full-lifecycle.

Cause 4 of 10: deleting the certificate authority that currently signs for the cluster is refused with the emulator's own sentence.
  AWS: the refusal is ResourceInUseException with 'Cannot delete the active certificate authority. Activate a different CA first.' — the entire sentence is fixed text, with no id or cluster name in it.
  Emulator: ResourceInUseException 'Certificate authority <id> is in IN_USE signing status; the certificate authority that signs for cluster <clusterName> cannot be deleted.'
  Why: clients that branch on EKS's sentence to decide 'activate another CA, then retry the delete' never take that branch.
  Fix: DeleteCertificateAuthority's in-use refusal. (Related open issue 20261103T150548Z-eks-5-net-new-root-causes-in-refusal-0ae4aa.md names a CA delete-in-use cause; these five checks are reported unowned.)
  Traces: eks.certificate-authority-delete-in-use-rejected, eks.certificate-authority-delete-while-active-blocked, eks.certificate-authority-lifecycle-transitions, eks.delete-certificate-authority-active-state-blocked, eks.delete-certificate-authority-in-use-blocked.

Cause 5 of 10: DescribeInsight on an unknown insightId uses the emulator's own sentence.
  AWS: the message starts 'Cannot find insight insightId=<insightId> clusterId=<clusterId>' — fixed text 'Cannot find insight insightId=' / ' clusterId=', with the requested insight id and the cluster id as the variable parts.
  Emulator: 'No insight found with id: <insightId> for cluster: <clusterName>.'
  Why: the key=value form AWS emits is what operators and log scrapers parse; the emulator's prose carries the cluster NAME where AWS reports the cluster id.
  Fix: DescribeInsight's not-found path.
  Traces: eks.cluster-insights-read-path.

Cause 6 of 10: an unknown pod-identity associationId is reported with the wrong noun.
  AWS: Describe/Update/DeletePodIdentityAssociation refuse an unknown associationId with ResourceNotFoundException and 'No service account role association found for ID: <associationId>' — fixed text 'No service account role association found for ID: ', variable being the requested association id, with no trailing period.
  Emulator: 'No pod identity association found for ID: <associationId>.' — different noun phrase plus a trailing period.
  Why: every teardown/absence probe in a pod-identity workflow matches EKS's sentence; the emulator's variant makes a successful delete look like an unknown error.
  Fix: the shared not-found path of the pod-identity association operations.
  Traces: eks.e2e-pod-identity-association-role-access-flow, eks.e2e-pod-identity-association-role-assumption-flow, eks.pod-identity-association-lifecycle, eks.pod-identity-association-role-swap-updates-association, eks.pod-identity-association-update-changes-role-binding, eks.pod-identity-full-lifecycle, eks.pod-identity-update-not-found.

Cause 7 of 10: a duplicate pod-identity association names the namespace/serviceAccount instead of the existing association id.
  AWS: Smithy: CreatePodIdentityAssociation declares ResourceInUseException (409). A create for a namespace/serviceAccount pair that already has an association is refused ResourceInUseException with 'Association already exists: <associationId>' — fixed text 'Association already exists: ', the single variable being the id of the EXISTING association.
  Emulator: 'Association already exists with namespace: <namespace> and service account: <serviceAccount>' — the request's inputs echoed back, and the existing association's id (the only datum the caller needs to adopt it) is absent.
  Why: a caller that reconciles by adopting the existing association parses the id out of this message; with the emulator's sentence it has nothing to adopt and must re-list.
  Fix: CreatePodIdentityAssociation's conflict path (also reached by the immutable-field update path).
  Traces: eks.pod-identity-association-duplicate-rejected, eks.pod-identity-duplicate-association-rejected, eks.pod-identity-association-immutable-fields-rejected.

Cause 8 of 10: the duplicate-nodegroup message inserts colons AWS does not emit.
  AWS: Smithy: CreateNodegroup declares ResourceInUseException (409). A duplicate nodegroupName is refused with 'NodeGroup already exists with name <nodegroupName> and cluster name <clusterName>' — fixed text 'NodeGroup already exists with name ' / ' and cluster name ', variables being the nodegroup and cluster names.
  Emulator: 'NodeGroup already exists with name: <nodegroupName> and cluster name: <clusterName>' — two extra colons.
  Why: exact-match handling of EKS's sentence fails on a one-character difference, so an idempotent create-or-adopt path mis-classifies the conflict.
  Fix: CreateNodegroup's conflict path.
  Traces: eks.nodegroup-duplicate-create-rejected.

Cause 9 of 10: the deletion-protection refusal message is the emulator's own.
  AWS: Smithy: DeleteCluster declares InvalidRequestException (httpError 400). DeleteCluster on an ACTIVE cluster with deletionProtection=true is refused InvalidRequestException with 'Cluster <name> cannot be deleted while DeletionProtection is enabled.' — fixed text 'Cluster ' / ' cannot be deleted while DeletionProtection is enabled.', variable being the cluster name.
  Emulator: InvalidRequestException 'Cluster <name> has deletion protection enabled. Disable deletion protection before deleting the cluster.'
  Why: automation that recognises EKS's sentence to disable protection and retry cannot recognise the emulator's.
  Fix: DeleteCluster's deletion-protection guard. (The open issue 20261029T175608Z-eks-5-net-new-root-causes-in-refusal.md names a deletion-protection cause; this check is reported unowned.)
  Traces: eks.ekscc-delete-cluster-with-deletion-protection-rejected-until-disabled.

Cause 10 of 10: UpdateClusterConfig on a non-ACTIVE cluster uses the emulator's own sentence.
  AWS: UpdateClusterConfig against a cluster that is not ACTIVE is refused InvalidRequestException with 'Cluster is currently not in Active State, so it cannot be updated' — the entire sentence is fixed text; AWS names neither the cluster nor its current status.
  Emulator: InvalidRequestException 'Cluster <name> is in CREATING status; it must be ACTIVE to update its configuration'.
  Why: a client waiting for ACTIVE branches on EKS's sentence; the emulator's text (and the status it leaks into it) is a different contract.
  Fix: UpdateClusterConfig's cluster-state guard.
  Traces: eks.ekscc-update-cluster-config-while-creating-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 151/342 passed; service eks 142/306.

Cause 1: eks.access-entry-duplicate-principal-rejected asserted dup == ('ResourceInUseException', 'The specified access entry resource is already in use on this cluster.'); got ('ResourceInUseException', 'The access entry for principal ARN arn:aws:iam::000000000000:role/sim2real-behav-eks-principal-<id> already exists for cluster sim2real-behav-eks-aedup-<id>.').
Cause 2: eks.describe-update-not-found — "expected message 'No update found for ID: <uuid>', got 'No update found for ID: <uuid>.'" (check normalises to AWS's 'Update <id> not found' in the sibling checks: eks.cancel-update-unknown-update-id-not-found reports expected ResourceNotFoundException 'Update <id> not found', got ('ResourceNotFoundException', 'No update found for ID: <uuid>.')); eks.nodegroup-update-serialization got 'No update found for ID: <uuid> with nodegroupName: <ng>.'
Cause 3: eks.certificate-authority-activate-before-distribution-complete-rejected — "expected re-delete message 'Certificate authority <id> not found for cluster <cluster>', got 'No certificate authority found with id: <id> for cluster: <cluster>.'"
Cause 4: eks.delete-certificate-authority-in-use-blocked — expected 'Cannot delete the active certificate authority. Activate a different CA first.', got 'Certificate authority <id> is in IN_USE signing status; the certificate authority that signs for cluster <cluster> cannot be deleted.' (identical text in the other four checks).
Cause 5: eks.cluster-insights-read-path — assertion required message.startswith('Cannot find insight insightId=<id> clusterId='); got 'No insight found with id: <id> for cluster: <cluster>.'
Cause 6: eks.e2e-pod-identity-association-role-assumption-flow — "expected message 'No service account role association found for ID: a-<id>', got 'No pod identity association found for ID: a-<id>.'"
Cause 7: eks.pod-identity-association-duplicate-rejected — assertion msg == f'Association already exists: {first_id}'; got 'Association already exists with namespace: default and service account: svc-acct-<id>'.
Cause 8: eks.nodegroup-duplicate-create-rejected — assertion msg == f'NodeGroup already exists with name {nodegroup_name} and cluster name {cluster_name}'; got 'NodeGroup already exists with name: sim2real-eks-dd-ng-<id> and cluster name: sim2real-eks-dd-<id>'.
Cause 9: eks.ekscc-delete-cluster-with-deletion-protection-rejected-until-disabled — expected ('InvalidRequestException', 'Cluster <name> cannot be deleted while DeletionProtection is enabled.'); got ('InvalidRequestException', 'Cluster <name> has deletion protection enabled. Disable deletion protection before deleting the cluster.').
Cause 10: eks.ekscc-update-cluster-config-while-creating-rejected — "expected message 'Cluster is currently not in Active State, so it cannot be updated', got 'Cluster <name> is in CREATING status; it must be ACTIVE to update its configuration'".

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreateAccessEntry (POST /clusters/{clusterName}/access-entries; errors InvalidParameterException 400, InvalidRequestException 400, ResourceInUseException 409, ResourceLimitExceededException 400, ResourceNotFoundException 404); DescribeUpdate (GET /clusters/{name}/updates/{updateId}; ResourceNotFoundException 404); CreateNodegroup (ResourceInUseException 409); CreatePodIdentityAssociation (ResourceInUseException 409, ResourceNotFoundException 404); DeleteCluster (InvalidRequestException 400, ResourceInUseException 409, ResourceNotFoundException 404).

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run the check ids listed in trace_ids.
3. Manually, against the emulator's EKS endpoint:
   a. CreateAccessEntry twice for the same principalArn on one cluster -> expect 409 ResourceInUseException 'The specified access entry resource is already in use on this cluster.'
   b. DescribeUpdate / CancelUpdate with a never-issued updateId on a real ACTIVE cluster -> expect 404 'Update <updateId> not found'.
   c. ActivateCertificateAuthority / DeleteCertificateAuthority with an unknown id -> expect 'Certificate authority <id> not found for cluster <cluster>'; DeleteCertificateAuthority on the CA currently signing -> expect 409 'Cannot delete the active certificate authority. Activate a different CA first.'
   d. DescribeInsight with an unknown insightId -> expect 'Cannot find insight insightId=<id> clusterId=<clusterId>'.
   e. DescribePodIdentityAssociation with a deleted associationId -> expect 'No service account role association found for ID: <id>'; create a second association for the same namespace/serviceAccount -> expect 409 'Association already exists: <existingAssociationId>'.
   f. CreateNodegroup twice with one name -> expect 'NodeGroup already exists with name <ng> and cluster name <cluster>'.
   g. DeleteCluster on an ACTIVE cluster with deletionProtection=true -> expect 'Cluster <name> cannot be deleted while DeletionProtection is enabled.'; UpdateClusterConfig while the cluster is CREATING -> expect 'Cluster is currently not in Active State, so it cannot be updated'.
