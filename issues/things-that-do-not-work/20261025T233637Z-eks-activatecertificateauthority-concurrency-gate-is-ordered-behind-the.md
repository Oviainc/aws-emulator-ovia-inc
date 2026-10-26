Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.activate-certificate-authority-wrong-state-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.activate-certificate-authority-wrong-state-rejected|run#8|AssertionError|assert code == (
Issue-Commit: e1450fe0eeb41d31a6a3d4e37ffe159fbfa51c57

# EKS ActivateCertificateAuthority: concurrency gate is ordered behind the CA distribution gate, and its ResourceInUseException message is not AWS's wording (2 causes)

> _Note: this issue was identified against the code base at commit `e1450fe0eeb41d31a6a3d4e37ffe159fbfa51c57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateCertificateAuthority

## Description
ActivateCertificateAuthority refuses an immediate activation of a just-appended successor CA, but it refuses it for the wrong reason and with the wrong message text. Two distinct causes, both in services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/activate_certificate_authority.rs, both exercised by the single failing check eks.activate-certificate-authority-wrong-state-rejected.

Cause 1 of 2: the per-cluster "one update in flight" gate is evaluated AFTER the CA's distributionStatus gate, so the wrong refusal wins.
AWS: CreateCertificateAuthority is asynchronous — it returns an `update` object (Smithy: CreateCertificateAuthority output members `update` (Update) and `certificateAuthority` (CertificateAuthoritySummary); POST /clusters/{clusterName}/certificate-authorities) and EKS allows at most one update in progress per cluster. An ActivateCertificateAuthority issued while that append update is still in flight is therefore refused on the CLUSTER's concurrency, as ResourceInUseException, naming the cluster and the in-flight update id. This refusal class is recorded behavior only for this operation: the Smithy model for ActivateCertificateAuthority declares only InvalidParameterException (400), ResourceNotFoundException (404), ServerException (500) and ServiceUnavailableException (503) — ResourceInUseException is not modeled on it (it IS modeled on CreateCertificateAuthority, httpError 409), and the handler already widens it in via `extra_errors = ["ClientException", "ResourceInUseException"]`, so the class itself is right.
Emulator: the handler resolves the cluster and the CA, then checks `signing_status != NOT_USED`, then `distribution_status == IN_PROGRESS` and only afterwards `cluster.in_flight_update()`. Because the append update that is in flight is exactly what leaves the new CA's distribution IN_PROGRESS, the distribution branch always fires first and the cluster-concurrency branch is unreachable in this (the normal) sequence. The observed refusal was ResourceInUseException / "Certificate authority <ca-id> is not in a valid state for activation. Its distributionStatus must be COMPLETE." where AWS reports the concurrent-update refusal.
Why: a client (or an operator runbook) that distinguishes "wait for the cluster's current update to finish, then retry" from "this CA can never be activated in its current distribution state" is told the wrong thing, and it is told it about the wrong resource — the message names a CA id when the blocking resource is the cluster and its update id. Retry logic keyed on the in-flight update id (DescribeUpdate on that id) has nothing to key on.
Fix: the gate ordering in the ActivateCertificateAuthority handler — the cluster-level in-flight-update check must be judged before the CA's distributionStatus check, so the CA-state refusal is only reported when the cluster is otherwise idle.
Traces: eks.activate-certificate-authority-wrong-state-rejected.

Cause 2 of 2: the in-flight-update ResourceInUseException message does not use AWS's wording.
AWS: recorded behavior only (the model carries no message template). The recorded refusal message is of the form "Cannot CertificateAuthorityUpdate because cluster <clusterName> currently has update <updateId> in progress". Fixed text: "Cannot ", then the update type being attempted, then " because cluster ", " currently has update ", " in progress". Variable parts: the update type (CertificateAuthorityUpdate here), <clusterName> taken from the request's clusterName @httpLabel, and <updateId> taken from emulator state — the id of the update currently in flight on that cluster. No literal cluster name or update id from any one run is part of the requirement.
Emulator: the corresponding branch formats "Cluster {cluster_name} already has update {id} in progress" — different fixed text, and it omits the update-type prefix entirely. (The error code, ResourceInUseException, and the cluster-name payload member are correct.)
Why: the message is a requirement of the error class; a client that matches on it, logs it, or surfaces it to an operator sees text real AWS never emits, and loses the statement of WHICH update type was refused.
Fix: the message construction in the in-flight-update branch of the ActivateCertificateAuthority handler (the shared `in_use` helper's caller), so the refusal is worded as AWS words it and names the attempted update type.
Traces: eks.activate-certificate-authority-wrong-state-rejected.

No existing open issue covers this operation: the only open EKS certificate-authority issue is 20261024T081355Z-eks-listcertificateauthorities-stub.md, which is the unimplemented ListCertificateAuthorities scaffold and a different operation.

## Evidence
Failing check: [mismatch] eks.activate-certificate-authority-wrong-state-rejected

AssertionError: expected ResourceInUseException, got ('ResourceInUseException', 'Certificate authority 915bd5a4-bac3-4a52-b74f-38ae133f8985 is not in a valid state for activation. Its distributionStatus must be COMPLETE.')

Expected (real AWS, us-west-2 recording), immediately after CreateCertificateAuthority on an ACTIVE cluster:
  Code:    ResourceInUseException
  Message: "Cannot CertificateAuthorityUpdate because cluster <clusterName> currently has update <appendUpdateId> in progress"
           fixed text: "Cannot " / " because cluster " / " currently has update " / " in progress";
           variable: the update type (CertificateAuthorityUpdate), clusterName from the request,
           appendUpdateId = the id returned in CreateCertificateAuthority's `update.id`.

Actual (emulator):
  Code:    ResourceInUseException  (correct class)
  Message: "Certificate authority <caId> is not in a valid state for activation. Its distributionStatus must be COMPLETE."
           — the distribution-state refusal, naming the CA instead of the cluster and its in-flight update.

Handler evidence (services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/activate_certificate_authority.rs):
  - lines 297-305: `if distribution_status == CA_DISTRIBUTION_IN_PROGRESS` -> "...is not in a valid state for activation. Its distributionStatus must be COMPLETE."
  - lines 330-338: `if let Some(in_flight) = cluster.in_flight_update()` -> "Cluster {cluster_name} already has update {id} in progress"
    The second branch is ordered after the first, hence unreachable in the recorded sequence, and its text differs from AWS's.

Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json):
  ActivateCertificateAuthority POST /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId}/activate,
  errors = InvalidParameterException(400), ResourceNotFoundException(404), ServerException(500), ServiceUnavailableException(503)
  — ResourceInUseException is NOT modeled here (it is modeled on CreateCertificateAuthority, httpError 409), so the
  refusal class and its message are recorded behavior only.

## Steps to Reproduce
1. Create an EKS cluster and wait for status ACTIVE.
2. Call CreateCertificateAuthority(clusterName=<cluster>). Keep both the returned certificateAuthority.id and update.id; the append update is now in flight and the new CA's distributionStatus is IN_PROGRESS.
3. Immediately call ActivateCertificateAuthority(clusterName=<cluster>, certificateAuthorityId=<caId>).
4. Observe the error. Real AWS: ResourceInUseException with "Cannot CertificateAuthorityUpdate because cluster <cluster> currently has update <appendUpdateId> in progress". Emulator: ResourceInUseException with "Certificate authority <caId> is not in a valid state for activation. Its distributionStatus must be COMPLETE."
5. Confirm the effect is absent on both: DescribeCertificateAuthority still reports signingStatus NOT_USED.

Replay: run the behavioral check eks.activate-certificate-authority-wrong-state-rejected.
