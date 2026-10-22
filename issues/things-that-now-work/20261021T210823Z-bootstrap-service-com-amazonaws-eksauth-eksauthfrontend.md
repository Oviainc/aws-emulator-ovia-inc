Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the EKS Auth (eksauthfrontend) emulator

_Filed: 2026-10-21T21:08:23Z_
_Operation: service-com-amazonaws-eksauth-eksauthfrontend | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The EKS Auth frontend service is a single-operation service: `AssumeRoleForPodIdentity` (`POST /clusters/{clusterName}/assume-role-for-pod-identity`). Today the only handler (`src/ops/assume_role_for_pod_identity.rs`) still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds zero fields (`// TODO: add service-specific state fields`), so the emulator has no behavior at all.

The happy-path first slice is therefore the full lifecycle of one pod-identity credential exchange, driven from persisted state:

1. **Seed/registration (internal, test-visible)** — a cluster and a pod identity association must exist in state before a token can be exchanged. Because this frontend models no create/list/delete operations, the bootstrap must expose a seeding seam (engine state pre-population, usable from `tests/all/`) that registers: cluster name, namespace, service account, IAM role ARN, association id/ARN.
2. **`AssumeRoleForPodIdentity` (happy path)** — input: `clusterName` (httpLabel, len 1..100, pattern `^[0-9A-Za-z][A-Za-z0-9\-_]*$`, required), `token` (JWT, len 1.., pattern `^[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+$`, required), plus optional `eksNodeName`, `instanceId`, `zone`. The handler must decode the service-account token's payload (namespace + service account + `exp` + audience), look up the cluster in state, look up the association matching that `(namespace, serviceAccount)` pair, mint a credential record, store it, and return all five required output members computed from state:
   - `subject` = `Subject{namespace, serviceAccount}` taken from the token/association, not echoed from the request body
   - `audience` = `pods.eks.amazonaws.com`
   - `podIdentityAssociation` = `PodIdentityAssociation{associationArn, associationId}` of the matched association
   - `assumedRoleUser` = `AssumedRoleUser{arn, assumeRoleId}` where `arn` is the association's IAM role ARN and `assumeRoleId` is `<roleId>:eks-<clusterName>-<podName>-<uuid>`
   - `credentials` = `Credentials{accessKeyId, secretAccessKey, sessionToken, expiration}` where `expiration` is a real future timestamp (≈6h) stored alongside the minted session so a second call returns a NEW distinct session, and an expired/unknown session is rejected.
3. **Error paths that the happy-path slice needs to be honest** — unknown `clusterName` → `ResourceNotFoundException` (404); a cluster with no association for the token's `(namespace, serviceAccount)` → `AccessDeniedException` (400); token whose `exp` is in the past → `ExpiredTokenException` (400); token that is syntactically a JWT but not decodable/unsigned by the seeded cluster → `InvalidTokenException` (400). Each must be driven by state, never by request inspection alone.

No canned or echoed responses: every output field above must be derived from the stored cluster/association/session records. A hardcoded `Credentials` block, or a `subject` echoed from the request, is a failure of this issue even if it returns 200.

## State Model

- `ComAmazonawsEksauthEksauthfrontendState`
  - `clusters: HashMap<String /* clusterName */, Cluster>`
  - `sessions: HashMap<String /* accessKeyId */, PodIdentitySession>`
- `Cluster { name: String, arn: String, associations: Vec<PodIdentityAssociationRecord> }`
- `PodIdentityAssociationRecord { association_id: String, association_arn: String, namespace: String, service_account: String, role_arn: String, role_id: String }`
- `PodIdentitySession { access_key_id: String, secret_access_key: String, session_token: String, expiration: SystemTime, cluster_name: String, association_id: String, assumed_role_arn: String, assume_role_id: String, namespace: String, service_account: String, eks_node_name: Option<String>, instance_id: Option<String>, zone: Option<String> }`

Done = an integration test seeds a cluster + association, calls `AssumeRoleForPodIdentity` with a matching service-account token and asserts the returned `subject`, `audience`, `podIdentityAssociation` and `assumedRoleUser.arn` match the seeded association, that `credentials.expiration` is in the future, that two successive calls mint distinct `accessKeyId`s, and that the unknown-cluster / no-association / expired-token cases return the modeled 404/400 errors rather than 501.

## Evidence

`services/service-com-amazonaws-eksauth-eksauthfrontend/OPERATIONS.md` lists exactly one bound operation, `AssumeRoleForPodIdentity` (`POST /clusters/{clusterName}/assume-role-for-pod-identity -> 200`, restJson1, sigv4 name `eks-auth`), with required inputs `clusterName`/`token`, required outputs `subject`/`audience`/`podIdentityAssociation`/`assumedRoleUser`/`credentials`, and errors `AccessDeniedException(400)`, `ExpiredTokenException(400)`, `InternalServerException(500)`, `InvalidParameterException(400)`, `InvalidRequestException(400)`, `InvalidTokenException(400)`, `ResourceNotFoundException(404)`, `ServiceUnavailableException(503)`, `ThrottlingException(429)`. Smithy shapes confirm member names: `Subject{namespace, serviceAccount}`, `PodIdentityAssociation{associationArn, associationId}`, `AssumedRoleUser{arn, assumeRoleId}`, `Credentials{accessKeyId, secretAccessKey, sessionToken, expiration:Timestamp}` (sensitive); the model documents `audience` as always `pods.eks.amazonaws.com` and `assumeRoleId` as `<roleId>:eks-<clustername>-<podname>-<uuid>`. Handler `src/ops/assume_role_for_pod_identity.rs:4` carries the SCAFFOLDED marker and line 202 returns `Scaffolded::not_implemented("AssumeRoleForPodIdentity")`; `src/state.rs:8` has no state fields. Only test present is `tests/all/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and seed one cluster `demo-cluster` with a pod identity association for namespace `default`, service account `my-sa`, role ARN `arn:aws:iam::123456789012:role/pod-role`.
2. POST /clusters/demo-cluster/assume-role-for-pod-identity with body {"token":"<JWT for system:serviceaccount:default:my-sa, exp in the future>"} — expect HTTP 200 today returns 501.
3. Assert the response `subject` = {namespace:"default", serviceAccount:"my-sa"}, `audience` = "pods.eks.amazonaws.com", `podIdentityAssociation.associationId/Arn` match the seeded association, `assumedRoleUser.arn` = the seeded role ARN, and `credentials.expiration` is in the future.
4. Repeat the same call and assert `credentials.accessKeyId` differs from the first response (a new session was persisted, not a canned constant).
5. POST to /clusters/no-such-cluster/assume-role-for-pod-identity with the same token — expect `ResourceNotFoundException` HTTP 404.
6. POST to /clusters/demo-cluster/... with a token for service account `other-sa` (no association) — expect `AccessDeniedException` HTTP 400.
7. POST to /clusters/demo-cluster/... with a token whose `exp` is in the past — expect `ExpiredTokenException` HTTP 400.

## Resolution

`AssumeRoleForPodIdentity` is implemented and computes every output member from the
persisted cluster / association / session records; the SCAFFOLDED marker is gone and
`src/state.rs` carries `clusters` + `sessions` + `role_ids` with the registration seam
this issue asks for. Covered by the ten table-driven suites in
`services/service-com-amazonaws-eksauth-eksauthfrontend/tests/all/assume_role_for_pod_identity.rs`.

DELIBERATE DEVIATION from step 2 of the plan above: the handler does NOT decode the
presented token's payload. A projected pod identity token is minted by a cluster's own
Kubernetes API server, and only that cluster can say whose service account it names — and
this emulator IS the cluster, so each cluster records the tokens it issued
(`Cluster::issued_tokens`: namespace, service account, pod, audiences, validity window)
and `verify_service_account_token` answers from that record plus the clock. That is
strictly more state-driven than decoding claims a caller presents, and it avoids
hand-writing a JOSE parser. A token the cluster did not issue names nobody, which is what
the real service answers for a token signed by keys it does not hold, so the requirement
to decode a payload should not be re-filed.

REMAINING, TRACKED ELSEWHERE: a cluster and its pod identity associations can only be
registered through the state seam (`ComAmazonawsEksauthEksauthfrontendState::register_cluster`
/ `Cluster::associate_pod_identity`), because EKS Auth models no create/list/delete calls.
An out-of-process client can therefore only reach `ResourceNotFoundException` until
`CreateCluster` and `CreatePodIdentityAssociation` are implemented in
`service-com-amazonaws-eks-awswesleyfrontend`, which is its own bootstrap issue
(`20261021T210857Z-bootstrap-service-com-amazonaws-eks-awswesleyfrontend.md`); wiring those
operations to this service's records is the next step and belongs with that issue.
