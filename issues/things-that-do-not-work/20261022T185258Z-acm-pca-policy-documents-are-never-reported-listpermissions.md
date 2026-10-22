Issue-Source: trace-sweep-divergence
Issue-Trace: acm-pca-ca-access-management-guards
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-pca-ca-access-management-guards acm-pca-put-policy-json-unicode-round-trip
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|ListPermissions|2xx->2xx|$.Permissions[*]
Issue-Key: acm-pca|GetPolicy|2xx->4xx|$.Policy
Issue-Key: acm-pca|GetPolicy|2xx->4xx|status
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# ACM PCA policy documents are never reported: ListPermissions omits Permission.Policy and an attached resource policy is unreadable

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPermissions

## Description
Two causes, both in the acm-pca policy-document surface: a response member that carries a policy document is dropped on the way out, and a policy attached to a CA cannot be read back at all. 2 causes in this issue; BOTH must be closed.

Cause 1 of 2: ListPermissions omits the modeled Permission$Policy member (and orders Actions by request order rather than the order AWS reports).
  AWS: Smithy: ListPermissions -> ListPermissionsResponse$Permissions (PermissionList, member Permission). Permission models six members, including Policy (target AWSPolicy, @length 1..81920, pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$) documented as "The name of the policy that is associated with the permission", plus Actions (ActionList, @length 1..3). The recorded AWS response for a permission granted to acm.amazonaws.com returns Policy populated with the policy document the grant implies - a JSON policy whose single statement allows the granted actions, prefixed acm-pca:, for Principal {"Service":"acm.amazonaws.com"} on the CA's own ARN, conditioned on aws:SourceAccount equal to the grant's SourceAccount. That document is derivable from the request (the CA ARN, the principal, the source account and the granted actions); the recorded Sid value and exact JSON spacing are an example, not a requirement. Recorded behavior only, for Actions: AWS reports the three actions as GetCertificate, IssueCertificate, ListPermissions, not in the order CreatePermission received them.
  Emulator: the Permission it serializes has no Policy key at all, and Actions come back in grant/request order (IssueCertificate, GetCertificate, ListPermissions). Status is 200 in both, and every other member (CertificateAuthorityArn, Principal, SourceAccount, CreatedAt) matches.
  Why: a client that reads permission.Policy to show or audit what ACM was granted on a private CA gets a missing/None field and either renders nothing or raises on the absent key; a client that compares Actions element-wise against what AWS reports sees a spurious difference. Both make an inspection path that works against AWS fail against the emulator.
  Fix: the ListPermissions reporting path - PermissionRecord::to_wire in the service's state module, which hard-codes policy: None and clones the stored action order; derive the policy document for the grant there (or at CreatePermission time) and report the actions in AWS's reported order.
  Traces: acm-pca-ca-access-management-guards#task#3 (ListPermissions, 200 vs 200, $.Permissions[0] differs).

Cause 2 of 2: a resource policy attached to a private CA is never readable - GetPolicy answers ResourceNotFoundException 400 where AWS answers 200 with the stored policy.
  AWS: Smithy: PutPolicy(Policy: AWSPolicy @length 1..81920, pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$; ResourceArn: Arn) attaches a resource-based policy to a private CA, and "The policy can be displayed with GetPolicy" (PutPolicy docs). GetPolicy then returns 200 with GetPolicyResponse$Policy holding the document that was attached, byte-for-byte - the pattern admits the whole Latin-1 printable range, so a statement Sid containing non-ASCII Latin-1 characters is a valid policy that must round-trip unchanged. ResourceNotFoundException (HTTP 400) is for a CA or policy that is not there, which is not the case right after a successful attach.
  Emulator: after the policy is attached, GetPolicy on the same ResourceArn answers HTTP 400 and $.Policy is absent, so nothing was persisted for the attach to read back. GetPolicy itself reads record.resource_policy and refuses when it is None; the attach side never sets it, because PutPolicy is still an unimplemented scaffold that answers 501 and persists nothing (see open issue 20261022T143159Z-acm-pca-putpolicy-stub.md, which covers PutPolicy's own 501 but carries no wire replay key for this GetPolicy step, so this read-back divergence is not exercised by it).
  Why: the documented cross-account-access flow is attach-then-display. A client that attaches a policy and reads it back to confirm, or that reads the current policy before editing it, gets a 400 "not found" for a policy it just created - indistinguishable from a CA that has no policy - and concludes the attach failed or wipes the policy it was about to amend.
  Fix: the PutPolicy handler and the CA record's resource_policy state field - persist the submitted document verbatim (no transcoding or ASCII folding, so Latin-1 Sid text survives) so the already-implemented GetPolicy read path finds it.
  Traces: acm-pca-put-policy-json-unicode-round-trip#task#3 (GetPolicy, expected 200, got 400, $.Policy missing).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (5 fixtures, 53 steps, 35 passed, score 66%, emulator_up=true, bootstrap_signal=false, state_reset_failures=0).

Cause 1 - fixture acm-pca-ca-access-management-guards, step task#3, ListPermissions, status 200 -> 200:
  expected $.Permissions[0] = {Actions: [GetCertificate, IssueCertificate, ListPermissions], CertificateAuthorityArn: <ca arn>, CreatedAt: <shaped-not-asserted>, Policy: '{"Version":"2012-10-17","Statement":[{"Sid":"1","Effect":"Allow","Principal":{"Service":"acm.amazonaws.com"},"Action":["acm-pca:GetCertificate","acm-pca:IssueCertificate","acm-pca:ListPermissions"],"Resource":"<ca arn>","Condition":{"StringEquals":{"aws:SourceAccount":"<account>"}}}]}', Principal: acm.amazonaws.com, SourceAccount: <account>}
  actual   $.Permissions[0] = {CertificateAuthorityArn: <ca arn>, CreatedAt: <epoch>, Principal: acm.amazonaws.com, SourceAccount: <account>, Actions: [IssueCertificate, GetCertificate, ListPermissions]}  -- no Policy key; Actions in a different order.
  Code: PermissionRecord::to_wire sets policy: None and actions: Some(self.actions.clone()) (services/service-com-amazonaws-acmpca-acmprivateca/src/state.rs, approx. lines 363-378).

Cause 2 - fixture acm-pca-put-policy-json-unicode-round-trip, step task#3, GetPolicy:
  expected status 200 with $.Policy = the previously attached policy document (statement Sid containing non-ASCII Latin-1 text, e.g. 'Café-Zürich-Straße-Ñoño-Allow'), allowing acm-pca:GetCertificate and acm-pca:DescribeCertificateAuthority to arn:aws:iam::<account>:root on the CA ARN.
  actual   status 400, $.Policy missing (ResourceNotFoundException).
  Code: GetPolicy refuses when record.resource_policy is None (services/service-com-amazonaws-acmpca-acmprivateca/src/ops/get_policy.rs, approx. lines 181-193); the writer, services/service-com-amazonaws-acmpca-acmprivateca/src/ops/put_policy.rs, still returns Scaffolded::not_implemented("PutPolicy") (line ~207) and so never sets state.rs's CertificateAuthorityRecord::resource_policy (line ~431).

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.

Cause 1: replay fixture acm-pca-ca-access-management-guards and look at step task#3 - create a private CA, grant the ACM service principal IssueCertificate/GetCertificate/ListPermissions with CreatePermission, then call ListPermissions on that CA ARN. The single Permission comes back without a Policy member and with Actions in request order.

Cause 2: replay fixture acm-pca-put-policy-json-unicode-round-trip and look at step task#3 - create a private CA, call PutPolicy on its ARN with a policy document whose statement Sid contains non-ASCII Latin-1 characters, then call GetPolicy with the same ResourceArn. It answers HTTP 400 ResourceNotFoundException instead of 200 with the attached document.
