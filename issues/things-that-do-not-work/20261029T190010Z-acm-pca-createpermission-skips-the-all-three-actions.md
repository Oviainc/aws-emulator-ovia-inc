Issue-Source: trace-sweep-divergence
Issue-Trace: acm-pca-create-permission-requires-all-three-actions-and-is-idempotent
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-pca-create-permission-requires-all-three-actions-and-is-idempotent
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|CreatePermission|4xx->2xx|$
Issue-Key: acm-pca|CreatePermission|4xx->2xx|status
Issue-Key: acm-pca|CreatePermission|2xx->4xx|$
Issue-Key: acm-pca|CreatePermission|2xx->4xx|status
Issue-Key: acm-pca|ListPermissions|2xx->2xx|$.Permissions[*].Actions[*]
Issue-Key: acm-pca|ListPermissions|2xx->2xx|$.Permissions[*].Policy
Issue-Commit: 473105e5105ba5cee28620541f40434a5fe4ef94

# ACM PCA CreatePermission skips the all-three-actions requirement and refuses an identical re-create, so ListPermissions serves the wrong Actions and Policy

> _Note: this issue was identified against the code base at commit `473105e5105ba5cee28620541f40434a5fe4ef94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePermission

## Description
THREE net-new causes in the acm-pca permissions surface (CreatePermission / ListPermissions). The emulator accepts a permission AWS refuses, then refuses the permission AWS accepts, and consequently reports a permission whose Actions and generated Policy do not match what was granted. No open acm-pca issue covers the permissions surface: the open 20261025T011542Z-acm-pca-resource-policy-contract-diverges-putpolicy-rejection.md is about PutPolicy/DeletePolicy/authorization, and the fixed 20261022T185258Z-acm-pca-policy-documents-are-never-reported-listpermissions.md was about Permission.Policy being absent entirely, not about its contents being wrong.

Cause 1 of 3: CreatePermission accepts an Actions list that does not contain all three actions, instead of refusing it.
AWS: Smithy (services/service-com-amazonaws-acmpca-acmprivateca/smithy-model/com-amazonaws-acmpca-acmprivateca.json) models CreatePermission.Actions as required, target ActionList @length 1..3, and documents on the Permission shape that "in order for ACM to automatically renew private certificates, you must give the ACM service principal all available permissions (IssueCertificate, GetCertificate, and ListPermissions)". The modeled refusal classes include ValidationException-style front-door refusal; the message is recorded behavior only: HTTP 400 with the fixed text "Permissions must contain all three actions [IssueCertificate, GetCertificate, ListPermissions] for ACM to perform renewals." — that sentence, including the bracketed action list, is entirely fixed text with no values taken from the request.
Emulator: answers HTTP 200 with an empty body and STORES a partial permission (the ListPermissions read at task#5 later shows only two actions persisted). No refusal is emitted at all.
Why: the whole point of the three-action rule is that ACM cannot renew a certificate from a CA whose acm.amazonaws.com permission is incomplete. A caller that provisions an incomplete permission gets success from the emulator and a hard 400 in production, so an onboarding/IaC path is validated green against a configuration AWS will never allow — and the partial grant is then silently treated as the CA's real permission.
Fix: CreatePermission's request validation in the acm-pca service crate — reject before mutating state when the Actions list is not the full set {IssueCertificate, GetCertificate, ListPermissions}, with HTTP 400 and AWS's fixed message.
Traces: acm-pca-create-permission-requires-all-three-actions-and-is-idempotent#task#2.

Cause 2 of 3: CreatePermission is not idempotent — a request granting the full action set is refused with PermissionAlreadyExistsException instead of succeeding.
AWS: Smithy models PermissionAlreadyExistsException ("The designated permission has already been given to the user") as one of CreatePermission's errors; when it applies is recorded behavior only. The recording shows CreatePermission for principal acm.amazonaws.com with the full three-action set on a CA succeeding with HTTP 200 and an empty body, and a REPEAT of the identical request (same CertificateAuthorityArn, Principal, SourceAccount and Actions) ALSO succeeding with HTTP 200 — i.e. re-granting the permission already in effect is idempotent, and PermissionAlreadyExistsException is not raised for a request that asks for exactly the state that already holds.
Emulator: both calls are refused with HTTP 400 PermissionAlreadyExistsException, message 'a permission on <ca-arn> for principal <principal> and source account <account> already exists' (an emulator-authored sentence; its ARN, principal and account are values taken from the request). The refusal is driven purely by the existence of any permission for that (CA, principal) pair — including the partial one cause 1 should never have stored — rather than by the requested grant differing from the existing one.
Why: re-asserting a desired permission is the normal converge/retry pattern for IaC and for any client retrying after a timeout; against the emulator that second call turns into a hard 400, so idempotent provisioning cannot be modelled. Worse, because cause 1 leaves a partial permission behind, the correct full-action request can never be made to succeed at all — the CA is permanently stuck with an incomplete grant.
Fix: CreatePermission's handler/state in the acm-pca service crate — compare the requested grant with the stored one for the (CertificateAuthorityArn, Principal, SourceAccount) key and return success when they match (upserting the action set), reserving PermissionAlreadyExistsException for the cases AWS uses it for.
Traces: acm-pca-create-permission-requires-all-three-actions-and-is-idempotent#task#3 (and the same divergence repeats at task#4, the idempotent repeat).

Cause 3 of 3: ListPermissions reports a permission whose Actions are incomplete and whose generated Policy document omits the corresponding action, so the read does not reflect the granted permission.
AWS: Smithy models ListPermissions.Permissions as a list of Permission, whose Actions member targets ActionList and whose Policy member is the policy document associated with the permission. Recorded behavior: after the full three-action grant, the read returns Actions containing all three action values, and Policy as a document whose single statement has Effect Allow, Principal.Service the granted principal, Action the list of the granted actions rendered with the "acm-pca:" prefix in sorted order (acm-pca:GetCertificate, acm-pca:IssueCertificate, acm-pca:ListPermissions), Resource the CA's ARN and a Condition StringEquals aws:SourceAccount of the source account. The ARNs, account id and principal in that document are values taken from the request/state; the structure and the action-name derivation are the rule.
Emulator: returns a permission with only two Actions (the third, ListPermissions, is missing) and a Policy whose Action array likewise carries only acm-pca:GetCertificate and acm-pca:IssueCertificate — the read faithfully reflects the wrong state produced by causes 1 and 2, i.e. the Actions->Policy derivation itself is consistent but the stored grant is not the granted one.
Why: a client that reads back its permission to verify renewal eligibility sees a grant it never asked for and a policy document missing an action, so verification logic (and anything that diffs desired vs actual permissions) reports drift that cannot be corrected, because the corrective CreatePermission is refused by cause 2.
Fix: primarily the permissions state written by CreatePermission (causes 1 and 2); re-verify afterwards that ListPermissions renders the full Actions list and derives the Policy document's Action array from it.
Traces: acm-pca-create-permission-requires-all-three-actions-and-is-idempotent#task#5.

## Evidence
Fixture acm-pca-create-permission-requires-all-three-actions-and-is-idempotent (service acm-pca).

Cause 1 — step task#2, CreatePermission:
  expected: HTTP 400, body {"__type": "ValidationException", "message": "Permissions must contain all three actions [IssueCertificate, GetCertificate, ListPermissions] for ACM to perform renewals."}
  actual:   HTTP 200, empty body ({}) — the incomplete permission was accepted and stored.

Cause 2 — step task#3 (and identically at task#4), CreatePermission with the full action set:
  expected: HTTP 200, empty body {}
  actual:   HTTP 400, body {"__type": "PermissionAlreadyExistsException", "message": "a permission on arn:aws:acm-pca:<region>:<account>:certificate-authority/<ca-id> for principal acm.amazonaws.com and source account <account> already exists"} — error class raised for a request that asks for the state AWS treats as idempotent; the message sentence is emulator-authored (AWS does not refuse here at all).

Cause 3 — step task#5, ListPermissions, HTTP 200 vs 200, body diverges:
  $.Permissions[0].Actions[2]: expected 'ListPermissions', got None (member absent).
  $.Permissions[0].Policy: expected a document whose statement Action is ["acm-pca:GetCertificate","acm-pca:IssueCertificate","acm-pca:ListPermissions"]; got the same document with Action ["acm-pca:GetCertificate","acm-pca:IssueCertificate"] — ListPermissions dropped from both the Actions list and the derived policy document. (The Resource ARN, Principal.Service and aws:SourceAccount condition value in that document are state-derived values, not part of the requirement.)

## Steps to Reproduce
1. CreateCertificateAuthority (CertificateAuthorityType ROOT, KeyAlgorithm RSA_2048, SigningAlgorithm SHA256WITHRSA, a Subject) and keep the returned CertificateAuthorityArn.

2. Cause 1: CreatePermission(CertificateAuthorityArn=<ca>, Principal='acm.amazonaws.com', SourceAccount=<caller account>, Actions=['IssueCertificate','GetCertificate']) — a two-action subset. It MUST be refused with HTTP 400 ValidationException and the message "Permissions must contain all three actions [IssueCertificate, GetCertificate, ListPermissions] for ACM to perform renewals.", and no permission may be stored (a following ListPermissions must return an empty Permissions list).

3. Cause 2: CreatePermission(same CA/Principal/SourceAccount, Actions=['IssueCertificate','GetCertificate','ListPermissions']) — must return HTTP 200 with an empty body. Repeat the identical call — it must ALSO return HTTP 200, not PermissionAlreadyExistsException.

4. Cause 3: ListPermissions(CertificateAuthorityArn=<ca>) — the single Permission must carry all three Actions, and its Policy document's statement Action must list acm-pca:GetCertificate, acm-pca:IssueCertificate and acm-pca:ListPermissions for Principal.Service acm.amazonaws.com on the CA ARN with the aws:SourceAccount condition.

Replay: acm-pca-create-permission-requires-all-three-actions-and-is-idempotent steps task#2, task#3, task#4, task#5.
