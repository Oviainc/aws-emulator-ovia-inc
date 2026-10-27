Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.list-aws-managed-policies-includes-well-known-policies iam.put-role-permissions-boundary-non-attachable-rejected iam.put-user-permissions-boundary-non-attachable-rejected iam.op-contract-listPolicies-path-prefix
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.list-aws-managed-policies-includes-well-known-policies|run#9|AssertionError|assert not missing, f"expected catalog to include {_EXPECTED_NAMES}, missing {missing}"
Issue-Key: iam.put-role-permissions-boundary-non-attachable-rejected|run#6|AssertionError|raise AssertionError(
Issue-Key: iam.put-user-permissions-boundary-non-attachable-rejected|run#6|AssertionError|raise AssertionError("no non-attachable AWS managed policy discoverable in this partition")
Issue-Key: iam.op-contract-listPolicies-path-prefix|run#16|AssertionError|assert error.get("Message") == expected, f"expected message {expected!r}, got {error.get('Message')!r}"
Issue-Commit: ee5f79e932caffa01c897fc87f80cd65e8d793a6

# iam: 3 net-new root causes — the built-in AWS-managed policy catalog omits well-known policies and carries no non-attachable policy, and ListPolicies' Scope enum validation message diverges from AWS's text

> _Note: this issue was identified against the code base at commit `ee5f79e932caffa01c897fc87f80cd65e8d793a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPolicies

## Description
Three net-new root causes in the iam emulator, all reached through ListPolicies and the AWS-managed-policy catalog it reports from. None of them is an unimplemented scaffold stub: every operation involved answers 200. No open issue covers them — the nearest neighbours are 20261025T000319Z-iam-9-net-new-behavioral-root-causes-cloudformation.md (refusal messages are generated prose instead of AWS's text) and 20261025T050617Z-iam-2-net-new-root-causes-listentitiesforpolicy-s.md (ListEntitiesForPolicy / CreateSAMLProvider messages); neither names ListPolicies, the AWS-managed catalog, or the permissions-boundary attachability contract, so they do not cover any cause below.

Cause 1 of 3: the built-in AWS-managed policy catalog omits well-known AWS managed policies.
AWS: ListPolicies (Smithy: iam#ListPolicies, input Scope -> policyScopeType enum [All, AWS, Local], output Policies -> policyListType, errors: ServiceFailureException only) "lists all the managed policies that are available in your Amazon Web Services account, including your own customer-defined managed policies and all Amazon Web Services managed policies" (model doc). In a real account, Scope=AWS enumerates AWS's full managed-policy catalog, which always contains the long-standing job-function and service policies — among them AdministratorAccess, ReadOnlyAccess and AWSLambdaBasicExecutionRole, each with Arn arn:aws:iam::aws:policy/<Name> (or arn:aws:iam::aws:policy/service-role/<Name>) and IsAttachable true.
Emulator: the Scope=AWS enumeration returns a catalog that contains AdministratorAccess and AWSLambdaBasicExecutionRole but NOT ReadOnlyAccess; the replay reports `expected catalog to include {'AWSLambdaBasicExecutionRole', 'ReadOnlyAccess', 'AdministratorAccess'}, missing {'ReadOnlyAccess'}`. The call itself succeeds with HTTP 200, so the gap is in the catalog contents, not in the operation.
Why: a client that resolves an AWS managed policy by listing the catalog and matching on PolicyName (the standard way CDK/CFN/SDK helpers find arn:aws:iam::aws:policy/ReadOnlyAccess) finds nothing and either fails or attaches no policy. Anything that attaches, boundary-scopes or simulates ReadOnlyAccess against this emulator silently diverges from the account it is standing in for.
Fix: the AWS-managed policy catalog this service's state exposes — the AWS_MANAGED_POLICIES table read by the Scope=AWS branch of the ListPolicies handler — must carry the well-known AWS managed policies with their documented ARNs, names and IsAttachable flags. The catalog entry set is the defect, not the handler's filtering.
Traces: iam.list-aws-managed-policies-includes-well-known-policies.

Cause 2 of 3: the AWS-managed policy catalog contains no non-attachable policy, so the PolicyNotAttachable refusal of PutRolePermissionsBoundary / PutUserPermissionsBoundary is unreachable.
AWS: AWS's managed-policy catalog contains policies that are NOT attachable to an identity — service-role and service-linked-role policies report IsAttachable false in ListPolicies/GetPolicy. Smithy: iam#PutRolePermissionsBoundary (input RoleName required, PermissionsBoundary -> arnType required) and iam#PutUserPermissionsBoundary both model PolicyNotAttachableException, errorKind client, awsQueryError code PolicyNotAttachable, HTTP 400 — the refusal AWS gives when the named policy ARN resolves to a policy whose IsAttachable is false. The boundary must be left untouched by that refusal.
Emulator: enumerating the whole catalog yields no policy at all with IsAttachable false, so a caller cannot name a non-attachable AWS managed policy; the replay fails with `AssertionError: no non-attachable AWS managed policy discoverable in this partition` before PutRolePermissionsBoundary/PutUserPermissionsBoundary is ever called. The modeled PolicyNotAttachable (400) path is therefore unreachable and unexercised in both operations.
Why: a client cannot discover which managed policies may be used as a permissions boundary or attached to an identity, because every policy the emulator reports claims to be attachable. Code that relies on IsAttachable to pre-screen a boundary ARN sees a uniformly permissive catalog, and code that relies on IAM to refuse a non-attachable boundary with PolicyNotAttachable 400 gets no refusal to handle. Both legs are overly permissive relative to AWS.
Fix: the same AWS-managed policy catalog in this service's state must include the non-attachable (service-role / service-linked-role) AWS managed policies with IsAttachable false, and PutRolePermissionsBoundary's and PutUserPermissionsBoundary's handlers must consult that flag and answer PolicyNotAttachableException (awsQueryError code PolicyNotAttachable, HTTP 400) without mutating the entity's boundary.
Traces: iam.put-role-permissions-boundary-non-attachable-rejected, iam.put-user-permissions-boundary-non-attachable-rejected.

Cause 3 of 3: ListPolicies' Scope enum validation message is not AWS's message — it echoes the rejected value and lists the enum set in the wrong order.
AWS: recorded behavior only (the Smithy model declares the enum but no message text). For a Scope value outside policyScopeType, IAM answers a ValidationError whose message is the fixed text `1 validation error detected: Value at 'scope' failed to satisfy constraint: Member must satisfy enum value set: [All, Local, AWS]` — i.e. the clause is `Value at '<memberName>'` with NO echo of the offending value, the member is named in its wire (lower-camel) form `scope`, and the admissible set is rendered in the order `[All, Local, AWS]`. The only variable part of that message is the member name; the bracketed set is fixed for this member.
Emulator: the same request is refused with `1 validation error detected: Value 'Bogus' at 'scope' failed to satisfy constraint: Member must satisfy enum value set: [All, AWS, Local]` — it interpolates the rejected value (`Value 'Bogus' at 'scope'`, where `Bogus` is the request's value) and renders the set in the model's declaration order `[All, AWS, Local]` instead of AWS's `[All, Local, AWS]`. The replay reports the check's error code as ValidationException/ValidationError with that body.
Why: a client that matches or logs IAM's validation message — or a test that asserts on it — sees text AWS never produces, and the echoed input value makes the message vary with the request where AWS's does not. Anything deriving the admissible value set by parsing the bracketed list gets a differently ordered list than AWS's.
Fix: the request-validation layer that renders enum-constraint failures for this service's operations (the boundary validator that produces `1 validation error detected: ...` before the ListPolicies handler runs) must emit AWS's fixed clause without the offending value and with this member's documented set ordering.
Traces: iam.op-contract-listPolicies-path-prefix.

## Evidence
All four failing checks come from suite simparity-behavioral-ga-prod-v1; iam is in the bootstrapped set, and every operation involved answers 200 (no scaffold stub is in the path).

Cause 1 — iam.list-aws-managed-policies-includes-well-known-policies [mismatch]:
  AssertionError: expected catalog to include {'AWSLambdaBasicExecutionRole', 'ReadOnlyAccess', 'AdministratorAccess'}, missing {'ReadOnlyAccess'}
  Expected: the Scope=AWS catalog contains ReadOnlyAccess at arn:aws:iam::aws:policy/ReadOnlyAccess.
  Actual: ReadOnlyAccess is absent from the emulator's catalog; the ListPolicies call itself returns HTTP 200.

Cause 2 — iam.put-role-permissions-boundary-non-attachable-rejected [mismatch] and iam.put-user-permissions-boundary-non-attachable-rejected [mismatch]:
  AssertionError: no non-attachable AWS managed policy discoverable in this partition
  Expected: at least one AWS managed policy reports IsAttachable=false, and PutRolePermissionsBoundary / PutUserPermissionsBoundary with that ARN answer error code PolicyNotAttachable (HTTP 400, awsQueryError code PolicyNotAttachable per iam#PutRolePermissionsBoundary / iam#PutUserPermissionsBoundary) leaving the entity's PermissionsBoundary absent.
  Actual: the whole catalog reports IsAttachable=true, so the refusal path is never reached; the check aborts in discovery (file check_iam_put_role_permissions_boundary_non_attachable_rejected.py line 54).

Cause 3 — iam.op-contract-listPolicies-path-prefix [mismatch]:
  AssertionError: expected message "1 validation error detected: Value at 'scope' failed to satisfy constraint: Member must satisfy enum value set: [All, Local, AWS]", got "1 validation error detected: Value 'Bogus' at 'scope' failed to satisfy constraint: Member must satisfy enum value set: [All, AWS, Local]"
  Fixed text of AWS's message: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [All, Local, AWS]". Variable part: the member name ("scope"), taken from the request's rejected member. The rejected VALUE ("Bogus" in this run) is not part of AWS's template at all — the emulator adds it.
  Smithy grounding: iam#policyScopeType is an enum with values [All, AWS, Local]; iam#ListPolicies models only ServiceFailureException, so the refusal is the unmodelled-bad-request ValidationError, as the handler's own comment at services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_policies.rs:223-226 states.

Code citations: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_policies.rs:282-288 reads the AWS-managed catalog via crate::state::AWS_MANAGED_POLICIES (causes 1 and 2); lines 223-226 of the same file document that enum values are refused by the boundary validator before the handler (cause 3).

## Steps to Reproduce
Boot the iam emulator and sign requests as a principal permitted to call IAM.

Cause 1:
1. Call ListPolicies with Scope=AWS and page through every result (MaxItems/Marker).
2. Collect the PolicyName of each returned policy.
3. Observe that 'ReadOnlyAccess' is missing from the collected set, while 'AdministratorAccess' and 'AWSLambdaBasicExecutionRole' are present. Real IAM returns all three.
   Replay: iam.list-aws-managed-policies-includes-well-known-policies

Cause 2:
1. Call ListPolicies with Scope=AWS and page through every result.
2. Look for any returned policy with IsAttachable=false. Observe there is none; real IAM's catalog contains service-role policies that report IsAttachable=false.
3. (Blocked by step 2) CreateRole, then call PutRolePermissionsBoundary with that non-attachable policy ARN and observe error code PolicyNotAttachable with HTTP 400 and GetRole still reporting no PermissionsBoundary; repeat for CreateUser + PutUserPermissionsBoundary.
   Replay: iam.put-role-permissions-boundary-non-attachable-rejected, iam.put-user-permissions-boundary-non-attachable-rejected

Cause 3:
1. Call ListPolicies with Scope set to a value outside the enum, e.g. Scope=Bogus.
2. Read the error message. Observe "1 validation error detected: Value 'Bogus' at 'scope' failed to satisfy constraint: Member must satisfy enum value set: [All, AWS, Local]" instead of AWS's "1 validation error detected: Value at 'scope' failed to satisfy constraint: Member must satisfy enum value set: [All, Local, AWS]".
   Replay: iam.op-contract-listPolicies-path-prefix
