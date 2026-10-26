Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.managed-login-branding-and-ui-customization-lifecycle cognito-idp.managed-login-branding-lifecycle cognito-idp.update-managed-login-branding-concurrent-modification-conflict
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|DescribeManagedLoginBranding|NotImplemented
Issue-Key: cognito-idp.update-managed-login-branding-concurrent-modification-conflict|run#10|AssertionError|assert code == "ConcurrentModificationException", f"unexpected error code: {code}"
Issue-Commit: ac01852cc17d3270a54b28950effcfcf66838250

# cognito-idp: two causes — DescribeManagedLoginBranding refuses the merged view with 501 NotImplemented, and UpdateManagedLoginBranding is a 501 stub so no branding style can ever be modified

> _Note: this issue was identified against the code base at commit `ac01852cc17d3270a54b28950effcfcf66838250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedLoginBranding

## Description
Managed login branding is reachable for create/describe(customized)/delete, but two distinct defects make the style unreadable in its merged form and unwritable at all. Both causes are in the cognito-idp service and are numbered below; a fixer must close BOTH before submitting.

No existing open issue covers either cause: the open cognito-idp list contains stub issues for many other operations and `20261024T114943Z-cognito-idp-createmanagedloginbranding-stub.md` (already fixed) for CreateManagedLoginBranding, but nothing for DescribeManagedLoginBranding's merged view and nothing for UpdateManagedLoginBranding. The replay report marked neither of these failures ALREADY FILED or OWNED.

--- Cause 1 of 2: DescribeManagedLoginBranding with ReturnMergedResources=true answers 501 NotImplemented ---

AWS: DescribeManagedLoginBranding takes `ReturnMergedResources` (Smithy: DescribeManagedLoginBrandingRequest.ReturnMergedResources, BooleanType, @default false) documented as "When true, returns values for branding options that are unchanged from Amazon Cognito defaults. When false or when you omit this parameter, returns only values that you customized in your branding style." It is a normal success path: the operation returns HTTP 200 with `ManagedLoginBranding` (ManagedLoginBrandingType: ManagedLoginBrandingId, UserPoolId, UseCognitoProvidedValues, Settings, Assets, CreationDate, LastModifiedDate), and for a style created with UseCognitoProvidedValues=true the merged view is the only view that carries `Settings` at all. The operation models NO error for this input: its error set is InternalErrorException(500), InvalidParameterException(400), NotAuthorizedException(403), OperationNotEnabledException(400), ResourceNotFoundException(404), TooManyRequestsException(429). There is no modeled way for AWS to refuse a well-formed ReturnMergedResources=true request.

Emulator: the request is refused with HTTP 501 and error code `NotImplemented`, message `operation DescribeManagedLoginBranding with ReturnMergedResources true is scaffolded but not implemented in this emulator`. Fixed text of that message is the whole string; `DescribeManagedLoginBranding with ReturnMergedResources true` is the scaffold label. The handler short-circuits on the flag before it ever reads state, so the pool and the branding id in the request are never consulted.

Why: a client cannot read a branding style's effective settings, and therefore cannot round-trip them. Both failing checks use the merged describe exactly as AWS requires — to obtain `Settings` so they can call UpdateManagedLoginBranding with UseCognitoProvidedValues=false — so the whole managed-login-branding lifecycle (describe-by-client, update-persists, delete-removes-both) is unreachable. 501/NotImplemented is also not a modeled error for the operation, so an SDK sees an unmodeled failure rather than any AWS-shaped outcome.

Fix: the DescribeManagedLoginBranding handler must serve the merged view from the stored style plus the service's Cognito-provided default style, returning 200 with ManagedLoginBranding.Settings populated, instead of returning early on the flag.

Also in this handler (same cause, same file): the ResourceNotFoundException raised after the branding is deleted is built as `Managed login branding {branding_id} does not exist.` while AWS's message for that condition is the fixed text `ManagedLoginBranding does not exist.` with no variable part at all (both failing lifecycle checks assert that exact string, and the by-client form `ManagedLoginBranding for client {ClientId} does not exist.` where ClientId is the app client id from the request). The replay never reached those asserts because it died on the merged describe, so correct that message while fixing this cause.

Traces: cognito-idp.managed-login-branding-and-ui-customization-lifecycle, cognito-idp.managed-login-branding-lifecycle. Handler: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/describe_managed_login_branding.rs lines 178-182 (the ReturnMergedResources early return) and lines 213-220 (the not-found message).

--- Cause 2 of 2: UpdateManagedLoginBranding answers 501 NotImplemented for every call, so no branding style can be updated and the concurrent-modification contract is unobservable ---

AWS: UpdateManagedLoginBranding configures a user pool style and returns HTTP 200 with the updated `ManagedLoginBranding` (Smithy: input UserPoolId, ManagedLoginBrandingId, UseCognitoProvidedValues (BooleanType, @default false), Settings (Document), Assets (AssetListType, @length 0..40); output ManagedLoginBranding: ManagedLoginBrandingType). Amazon Cognito preserves style settings not named in the request. Its modeled errors are ConcurrentModificationException(400), InternalErrorException(500), InvalidParameterException(400), NotAuthorizedException(403), OperationNotEnabledException(400), ResourceNotFoundException(404), TooManyRequestsException(429). ConcurrentModificationException is documented as "This exception is thrown if two or more modifications are happening concurrently": when several modifications of the same branding style overlap, at least one wins with 200 and every loser is refused with error code `ConcurrentModificationException`, message (fixed text, no variable part) `Only one request to modify a ManagedLoginBranding can be processed at a time`. In an overlapping UpdateManagedLoginBranding/DeleteManagedLoginBranding pair on the same style exactly one side wins and the loser is refused with either that same ConcurrentModificationException or ResourceNotFoundException with the fixed text `ManagedLoginBranding does not exist.`, and an independent DescribeManagedLoginBranding agrees with whichever side won.

Emulator: every UpdateManagedLoginBranding call is refused with HTTP 501, error code `NotImplemented`, message `operation UpdateManagedLoginBranding is scaffolded but not implemented in this emulator`. The handler is an unmodified scaffold stub whose body is a single `Err(not_implemented("UpdateManagedLoginBranding"))`, so state is never touched. In the concurrency check the losing racers therefore report code `NotImplemented` where `ConcurrentModificationException` is required (`AssertionError: unexpected error code: NotImplemented`), and the mutual-exclusion/ordering contract is not implemented at all — there is no serialization of concurrent modifications of one branding style.

Why: a client can create a branding style and read it back, but can never change it; an opt-out of Cognito-provided values (UseCognitoProvidedValues=false with Settings) is impossible, so a branding style is write-once. `NotImplemented` is not in the operation's modeled error set, so SDK error handling for the real contention outcome never runs and a client that retries on ConcurrentModificationException retries nothing.

Fix: implement the UpdateManagedLoginBranding handler — apply UseCognitoProvidedValues/Settings/Assets to the stored style, preserve unnamed settings, bump LastModifiedDate, return 200 with the updated ManagedLoginBrandingType, return ResourceNotFoundException for an unknown pool or branding id, and serialize overlapping modifications of one style in the service state so losing writers get ConcurrentModificationException with AWS's message. Delete the SCAFFOLDED marker line on the file once implemented.

Traces: cognito-idp.update-managed-login-branding-concurrent-modification-conflict (and the two lifecycle checks above will exercise this operation once Cause 1 is fixed). Handler: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_managed_login_branding.rs lines 182-200 (scaffold stub body), marker at line 4.

## Evidence
Cause 1 — expected vs actual:
Expected: DescribeManagedLoginBranding(UserPoolId=<pool>, ManagedLoginBrandingId=<id>, ReturnMergedResources=True) -> HTTP 200, body {"ManagedLoginBranding": {"ManagedLoginBrandingId": <id>, "UserPoolId": <pool>, "UseCognitoProvidedValues": true, "Settings": {...}, "Assets": [...], "CreationDate": ..., "LastModifiedDate": ...}}.
Actual: HTTP 501, Error.Code=NotImplemented, Error.Message="operation DescribeManagedLoginBranding with ReturnMergedResources true is scaffolded but not implemented in this emulator".
Replay detail (both checks, identical):
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeManagedLoginBranding operation: operation DescribeManagedLoginBranding with ReturnMergedResources true is scaffolded but not implemented in this emulator
Source confirming the cause: describe_managed_login_branding.rs:178 `if input.return_merged_resources { return Err(Scaffolded::not_implemented("DescribeManagedLoginBranding with ReturnMergedResources true")); }`.
Secondary (same handler, not reached by this replay): deleted-branding refusal message is built as "Managed login branding {id} does not exist." (describe_managed_login_branding.rs:215-217); AWS's message is the fixed text "ManagedLoginBranding does not exist."

Cause 2 — expected vs actual:
Expected: with N=10 overlapping UpdateManagedLoginBranding(UserPoolId=<pool>, ManagedLoginBrandingId=<id>, UseCognitoProvidedValues=True) calls, >=1 call returns HTTP 200 with the updated ManagedLoginBranding and every refused call returns HTTP 400 with Error.Code=ConcurrentModificationException and Error.Message="Only one request to modify a ManagedLoginBranding can be processed at a time"; a subsequent independent DescribeManagedLoginBranding still returns the branding with UseCognitoProvidedValues=true. For an overlapping update/delete pair on the same style, exactly one side wins and the loser is ConcurrentModificationException (same message) or ResourceNotFoundException with "ManagedLoginBranding does not exist."
Actual: refused calls carry Error.Code=NotImplemented (HTTP 501, message "operation UpdateManagedLoginBranding is scaffolded but not implemented in this emulator").
Replay detail:
  File "check_cognito_idp_update_managed_login_branding_concurrent_modification_conflict.py", line 77, in run
  assert code == "ConcurrentModificationException", f"unexpected error code: {code}"
  AssertionError: unexpected error code: NotImplemented
Source confirming the cause: update_managed_login_branding.rs:199 `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateManagedLoginBranding"))`, with the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker still on line 4.

## Steps to Reproduce
Cause 1:
1. CreateUserPool (DeletionProtection=INACTIVE), CreateUserPoolClient, CreateUserPoolDomain and wait for the domain to be ACTIVE.
2. CreateManagedLoginBranding(UserPoolId=<pool>, ClientId=<client>, UseCognitoProvidedValues=true) and keep the returned ManagedLoginBrandingId.
3. DescribeManagedLoginBranding(UserPoolId=<pool>, ManagedLoginBrandingId=<id>) — succeeds (customized view).
4. DescribeManagedLoginBranding(UserPoolId=<pool>, ManagedLoginBrandingId=<id>, ReturnMergedResources=true) — observe HTTP 501 NotImplemented instead of HTTP 200 with ManagedLoginBranding.Settings.
5. (After fix) DeleteManagedLoginBranding then DescribeManagedLoginBranding the same id — the refusal must be ResourceNotFoundException with message "ManagedLoginBranding does not exist.", and DescribeManagedLoginBrandingByClient must be ResourceNotFoundException with "ManagedLoginBranding for client <ClientId> does not exist."

Cause 2:
1. CreateUserPool, CreateUserPoolClient, CreateManagedLoginBranding(UseCognitoProvidedValues=true).
2. UpdateManagedLoginBranding(UserPoolId=<pool>, ManagedLoginBrandingId=<id>, UseCognitoProvidedValues=true) once — observe HTTP 501 NotImplemented instead of HTTP 200 with the updated ManagedLoginBranding.
3. Fire 10 such calls simultaneously (threads released from one barrier) — expect >=1 HTTP 200 and every other call ConcurrentModificationException with the message above; observe all of them answering NotImplemented.
4. Fire one UpdateManagedLoginBranding and one DeleteManagedLoginBranding on the same branding id simultaneously — expect exactly one winner and the loser refused with ConcurrentModificationException or ResourceNotFoundException, then confirm with DescribeManagedLoginBranding.

Replay to confirm: cognito-idp.managed-login-branding-and-ui-customization-lifecycle, cognito-idp.managed-login-branding-lifecycle, cognito-idp.update-managed-login-branding-concurrent-modification-conflict.
