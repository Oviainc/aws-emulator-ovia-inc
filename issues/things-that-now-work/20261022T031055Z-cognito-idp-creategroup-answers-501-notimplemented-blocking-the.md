Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-group-permit-allows-member-denies-non-member cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-group-removal-applies-only-to-reissued-token cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-forged-group-claim-rejected cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-tampered-token-signature-rejected cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-foreign-pool-rejected cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-unlisted-client-rejected
Issue-Key: cognito-idp__verifiedpermissions|CreateGroup|NotImplemented
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# cognito-idp CreateGroup answers 501 NotImplemented, blocking the entire Cognito-token → Verified Permissions authorization suite (6 checks)

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
Cause 1 of 1: `CreateGroup` on cognito-idp is an unimplemented scaffold stub that answers HTTP 501 with error code `NotImplemented`, which makes every cross-service Cognito-ID-token → Verified Permissions authorization check unexercisable. All six failing checks die on the first `CreateGroup` call, before any Verified Permissions behaviour is reached.

NOTE ON ATTRIBUTION: these checks carry the composite id prefix `cognito-idp__verifiedpermissions`, which is not a single service token. The failing operation is unambiguously cognito-idp's `CreateGroup`, so this issue is filed against service `cognito-idp` (bootstrapped as services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice). The report does NOT mark these failures as ALREADY FILED, and no open issue mentions cognito or CreateGroup (list_existing_issues query='cognito' returns nothing), so the stub-issue auto-filer did not cover this operation — it only covered the verifiedpermissions-side stubs (BatchGetPolicy, BatchIsAuthorized, CreatePolicyStoreAlias, CreateIdentitySource, CreatePolicyTemplate, GetSchema, PutSchema, TagResource).

AWS: Smithy model com.amazonaws.cognitoidentityprovider#CreateGroup is a modeled operation returning `Group` (GroupType) on HTTP 200. Input: `UserPoolId` (required, len 1..55, pattern ^[\w-]+_[0-9a-zA-Z]+$), `GroupName` (required, len 1..128, pattern ^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$), optional `Description` (len 0..2048), `RoleArn`, `Precedence` (range 0..). Modeled errors: GroupExistsException (400), InvalidParameterException (400), LimitExceededException (400), NotAuthorizedException (403), OperationNotEnabledException (400), ResourceNotFoundException (404), TooManyRequestsException (429), InternalErrorException (500). `NotImplemented`/501 is not a modeled outcome for this operation. Group membership established through this operation is what populates the `cognito:groups` claim of issued ID tokens (model doc on CreateGroup.RoleArn: "Both ID and access tokens also contain a cognito:groups claim that list all the groups that a user is a member of"), which is the input these authorization checks depend on.

Emulator: `CreateGroup` returns HTTP 501 `NotImplemented` with message "operation CreateGroup is scaffolded but not implemented in this emulator" — the handler's `handle` body is still `Err(crate::scaffolded::Scaffolded::not_implemented("CreateGroup"))` and the file still carries the SCAFFOLDED marker.

Why: a client cannot create a user-pool group at all, so no group-scoped identity can be modelled. Downstream, nothing in the Cognito-token → AVP path can be validated: group-based permit policies (ALLOW for a member, DENY for a non-member), the group-removal-visible-only-on-reissued-token semantics, and the four token-rejection paths (forged `cognito:groups` claim, tampered signature, token from a foreign pool, token from an unlisted app client) are all unreachable. The dangerous shape of this gap is that the over-permissive failures these checks exist to catch — an AVP emulator that trusts claims without binding them to the token signature and turns a DENY into an ALLOW — stay completely unmeasured.

Fix: implement the handler in services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/create_group.rs (create the group in the user-pool state, return the created `Group`, reject a duplicate name with GroupExistsException and an unknown `UserPoolId` with ResourceNotFoundException) and delete the SCAFFOLDED marker. Group state must be readable by the token-issuing path so issued ID tokens carry `cognito:groups`.

Traces: cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-group-permit-allows-member-denies-non-member, cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-group-removal-applies-only-to-reissued-token, cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-forged-group-claim-rejected, cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-tampered-token-signature-rejected, cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-foreign-pool-rejected, cognito-idp__verifiedpermissions.xsvc-cognito-idp-2-avp-token-from-unlisted-client-rejected.

## Evidence
Expected (AWS / Smithy com.amazonaws.cognitoidentityprovider#CreateGroup): HTTP 200 with a `Group` member of type GroupType; failures only in the modeled set (GroupExistsException 400, ResourceNotFoundException 404, InvalidParameterException 400, ...).

Actual (emulator, all 6 checks): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateGroup operation: operation CreateGroup is scaffolded but not implemented in this emulator — HTTP 501.

Source: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/create_group.rs:4 (SCAFFOLDED marker) and :199 (`Err(crate::scaffolded::Scaffolded::not_implemented("CreateGroup"))`).

All six checks fail at the same point — the helper `_make_group(ctx, idp, pool_id, _ADMINS)`, which calls `idp.create_group(UserPoolId=pool_id, GroupName="admins")` — so no assertion about AVP decisions or token rejection in those checks has ever been evaluated.

## Steps to Reproduce
1. Boot the emulator and point a botocore cognito-idp client at it.
2. pool = idp.create_user_pool(PoolName="p1", DeletionProtection="INACTIVE") -> succeeds, note UserPool.Id.
3. idp.create_group(UserPoolId=<pool id>, GroupName="admins")
4. Observe HTTP 501 / error code NotImplemented instead of a 200 response carrying `Group`.
5. Re-replay the six behavioral checks listed in trace_ids; each aborts at step 3.
