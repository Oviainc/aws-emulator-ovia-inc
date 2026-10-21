Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the sts emulator

_Filed: 2026-10-21T21:53:16Z_
_Operation: service-com-amazonaws-sts-awssecuritytokenservicev20110615 | Class: missing_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 11 STS operations is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state (e.g. `src/ops/get_caller_identity.rs` returns `Err(crate::scaffolded::Scaffolded::not_implemented("GetCallerIdentity"))` under the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker). STS is the credential-vending service every other emulated service's clients call first, so nothing works until a first stateful slice exists.

Implement this minimal happy path (awsQuery protocol, sigv4 signing name `sts`), in order:

1. **GetCallerIdentity** — no input members. Must resolve the caller from the request's SigV4 `Credential=<AccessKeyId>/...` Authorization header against the credential registry and return `UserId`, `Account`, `Arn` for THAT identity. For an unknown/root access key, return the configured default account identity (e.g. `Account=000000000000`, `Arn=arn:aws:iam::000000000000:user/<name>`, `UserId=<a stable 21-char AIDA...>`). It must NOT return a hardcoded identity for every caller: after step 2 the same call made with the assumed-role credentials must return the assumed-role identity (`UserId=<RoleId>:<RoleSessionName>`, `Arn=arn:aws:sts::<account>:assumed-role/<RoleName>/<RoleSessionName>`).

2. **AssumeRole** — input `RoleArn` (required), `RoleSessionName` (required), optional `DurationSeconds` (900..43200, default 3600), `Policy`/`PolicyArns`, `Tags`, `ExternalId`, `SourceIdentity`. Must mint a NEW, unique `Credentials{AccessKeyId (16..128, ^[\w]*$, ASIA-prefixed), SecretAccessKey, SessionToken, Expiration}` with `Expiration = now + DurationSeconds`, derive `AssumedRoleUser{AssumedRoleId = "<generated role id>:<RoleSessionName>", Arn = arn:aws:sts::<account>:assumed-role/<role name from RoleArn>/<RoleSessionName>}`, echo `SourceIdentity` only when supplied, set `PackedPolicySize` when `Policy` is supplied, and PERSIST the session keyed by the minted `AccessKeyId` so GetCallerIdentity and GetAccessKeyInfo can resolve it. Account id must be parsed out of `RoleArn`.

3. **GetSessionToken** — optional `DurationSeconds` (900..129600, default 43200), `SerialNumber`, `TokenCode`. Must mint and persist a new `Credentials` bound to the calling identity (so GetCallerIdentity with those credentials returns the ORIGINAL caller identity, not an assumed role).

4. **GetAccessKeyInfo** — required `AccessKeyId` (16..128, `^[\w]*$`). Must return the `Account` recorded for that access key in the registry (the account of the session minted in step 2/3), not a constant.

5. **GetFederationToken** — required `Name` (2..32), optional `Policy`/`PolicyArns`, `DurationSeconds` (900..129600, default 43200), `Tags`. Must mint and persist credentials plus `FederatedUser{FederatedUserId = "<account>:<Name>", Arn = arn:aws:sts::<account>:federated-user/<Name>}`, and set `PackedPolicySize` when a policy is supplied.

Requirements that apply to all five:
- Responses must be COMPUTED FROM PERSISTED STATE. No canned bodies, no echoing the input back as the whole answer, no minting a random access key id that is never stored — a credential returned by AssumeRole/GetSessionToken/GetFederationToken must be resolvable afterwards by GetCallerIdentity and GetAccessKeyInfo.
- Expiration must be a real wall-clock instant derived from `DurationSeconds`; expired sessions must be treated as expired (`ExpiredTokenException`, HTTP 400) rather than still valid.
- Modeled constraint violations are rejected by the framework before `handle` runs; for anything else refuse with `Scaffolded::validation(..)` or the modeled error (`MalformedPolicyDocumentException`, `PackedPolicyTooLargeException`, `RegionDisabledException`) — never 501.
- awsQuery serialization: responses wrapped as `<AssumeRoleResponse><AssumeRoleResult>...` with a `ResponseMetadata/RequestId`.

## State Model

```text
StsState {
    default_account: String,              // e.g. "000000000000"
    default_identity: Identity,           // the ambient caller when no session matches
    sessions: HashMap<String, Session>,   // keyed by minted AccessKeyId
    role_ids: HashMap<String, String>,    // RoleArn -> stable generated "AROA..." role id
    next_key_seq: u64,                    // for unique ASIA.../ AKIA... key generation
}

Session {
    access_key_id: String,      // ASIA… , unique, the registry key
    secret_access_key: String,
    session_token: String,      // opaque, unique
    expiration: SystemTime,     // created_at + duration_seconds
    duration_seconds: i32,
    account: String,
    identity: Identity,         // what GetCallerIdentity returns for this credential
    kind: SessionKind,          // AssumedRole | SessionToken | FederationToken
    source_identity: Option<String>,
    tags: Vec<Tag>,             // from AssumeRole/GetFederationToken Tags
    transitive_tag_keys: Vec<String>,
    packed_policy_size: Option<i32>,
    external_id: Option<String>,
}

Identity { user_id: String, account: String, arn: String }
```

The other six operations (AssumeRoleWithSAML, AssumeRoleWithWebIdentity, AssumeRoot, GetDelegatedAccessToken, GetWebIdentityToken, DecodeAuthorizationMessage) stay 501 for this slice and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md for service-com-amazonaws-sts-awssecuritytokenservicev20110615 lists 11 awsQuery operations with these input/output members: AssumeRole (RoleArn/RoleSessionName required, DurationSeconds 900..43200 -> Credentials, AssumedRoleUser, PackedPolicySize, SourceIdentity), GetSessionToken (DurationSeconds 900..129600 -> Credentials), GetFederationToken (Name required -> Credentials, FederatedUser, PackedPolicySize), GetCallerIdentity (no input -> UserId, Account, Arn), GetAccessKeyInfo (AccessKeyId required -> Account). smithy_query on `Credentials` shows all four members required: AccessKeyId (16..128, ^[\w]*$), SecretAccessKey (sensitive), SessionToken, Expiration (timestamp); `AssumedRoleUser` requires AssumedRoleId (2..193) + Arn; `FederatedUser` requires FederatedUserId + Arn. Modeled errors include ExpiredTokenException (400), MalformedPolicyDocumentException (400), PackedPolicyTooLargeException (400), RegionDisabledException (403). All handlers under src/ops/ carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Scaffolded::not_implemented(..)` (verified in src/ops/get_caller_identity.rs line 154); no tests assert any STS behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI at the sts endpoint with placeholder credentials.
2. `aws sts get-caller-identity` -> expect 200 with UserId/Account/Arn for the default caller identity (currently HTTP 501 NotImplemented).
3. `aws sts assume-role --role-arn arn:aws:iam::000000000000:role/demo --role-session-name s1 --duration-seconds 900` -> expect 200 with Credentials{AccessKeyId, SecretAccessKey, SessionToken, Expiration = now+900s} and AssumedRoleUser{AssumedRoleId='<roleid>:s1', Arn='arn:aws:sts::000000000000:assumed-role/demo/s1'}.
4. Re-run `aws sts get-caller-identity` using the credentials returned in the previous step -> expect the ASSUMED-ROLE identity (Arn ends in assumed-role/demo/s1), proving the session was persisted and lookup is credential-driven.
5. `aws sts get-access-key-info --access-key-id <AccessKeyId from assume-role>` -> expect Account '000000000000' resolved from the stored session.
6. `aws sts get-session-token --duration-seconds 900` -> expect fresh distinct Credentials; GetCallerIdentity with them returns the ORIGINAL caller identity.
7. `aws sts get-federation-token --name bob --policy '{"Version":"2012-10-17","Statement":[]}'` -> expect Credentials plus FederatedUser{FederatedUserId='000000000000:bob', Arn='arn:aws:sts::000000000000:federated-user/bob'} and a non-zero PackedPolicySize.
8. Assume a role with --duration-seconds 900, advance past expiration, and call an operation with the expired credentials -> expect ExpiredTokenException (HTTP 400), not success.
