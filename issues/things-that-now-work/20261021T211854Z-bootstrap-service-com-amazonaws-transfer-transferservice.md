Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the AWS Transfer Family (transfer) emulator

_Filed: 2026-10-21T21:18:54Z_
_Operation: service-com-amazonaws-transfer-transferservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 71 handlers under `services/service-com-amazonaws-transfer-transferservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST SMALL SLICE: the server+user lifecycle, which is the primary resource pair of AWS Transfer Family.

Happy path to implement, in order (awsJson1_1, `POST /` with `X-Amz-Target: TransferService.<Op>`):

1. **CreateServer** — input: `Domain` (EFS|S3, default S3), `IdentityProviderType` (API_GATEWAY|AWS_DIRECTORY_SERVICE|AWS_LAMBDA|SERVICE_MANAGED, default SERVICE_MANAGED), `EndpointType` (PUBLIC|VPC|VPC_ENDPOINT, default PUBLIC), `Protocols` (list of SFTP|FTP|FTPS|AS2, default [SFTP]), `LoggingRole`, `SecurityPolicyName`, `Tags`, `IdentityProviderDetails`, `EndpointDetails`, `PreAuthenticationLoginBanner`, `PostAuthenticationLoginBanner`. Must MINT and STORE a new id matching `^s-([0-9a-f]{17})$` (19 chars) and return it as `ServerId`. Initial `State` = `OFFLINE`, `UserCount` = 0, `Arn` = `arn:aws:transfer:<region>:<account>:server/<ServerId>`.
2. **CreateUser** — input: `ServerId` (required), `UserName` (required, `^[\w][\w@.-]{2,99}$`), `Role` (required, `^arn:.*role/\S+$`), `HomeDirectory`, `HomeDirectoryType`, `HomeDirectoryMappings`, `Policy`, `PosixProfile`, `SshPublicKeyBody`, `Tags`. Output: `ServerId` + `UserName`. Must reject an unknown `ServerId` with `ResourceNotFoundException` (404) and a duplicate `UserName` on the same server with `ResourceExistsException` (409). An `SshPublicKeyBody` supplied here becomes the user's first stored SSH key (minted `SshPublicKeyId`, `DateImported`).
3. **DescribeServer** — input `ServerId`; output `Server` (DescribedServer) rebuilt FROM STORED STATE: `Arn`, `ServerId`, `Domain`, `IdentityProviderType`, `EndpointType`, `Protocols`, `LoggingRole`, `SecurityPolicyName`, `Tags`, `State`, and `UserCount` computed as the live count of that server's users (must become 1 after step 2). Unknown id → `ResourceNotFoundException` (404).
4. **DescribeUser** — input `ServerId` + `UserName`; output `ServerId` and `User` (DescribedUser) with `Arn` (`.../user/<ServerId>/<UserName>`), `UserName`, `Role`, `HomeDirectory`, `HomeDirectoryType`, `HomeDirectoryMappings`, `Policy`, `PosixProfile`, `SshPublicKeys`, `Tags` — exactly the values persisted by CreateUser (not echoed from the request). Unknown server or user → 404.
5. **ListUsers** — input `ServerId` (required), `MaxResults` (1..1000), `NextToken` (1..6144); output `ServerId`, `Users` (list<ListedUser>: `Arn`, `UserName`, `Role`, `HomeDirectory`, `HomeDirectoryType`, `SshPublicKeyCount`) and `NextToken` only when more remain. Honour `MaxResults` as a real page bound and resume from an opaque `NextToken`; a token that does not decode → `InvalidNextTokenException` (400).
6. **ListServers** — `MaxResults`/`NextToken` paging over the stored servers; output `Servers` (list<ListedServer>: `Arn`, `ServerId`, `Domain`, `IdentityProviderType`, `EndpointType`, `LoggingRole`, `State`, `UserCount`). A server created in step 1 must appear, and must disappear after step 8.
7. **DeleteUser** — input `ServerId` + `UserName`; empty output, @idempotent. Removes the user so DescribeUser → 404 and the server's `UserCount` drops back to 0. Unknown server/user → `ResourceNotFoundException`.
8. **DeleteServer** — input `ServerId`; empty output, @idempotent. Removes the server AND its users/keys; subsequent DescribeServer → `ResourceNotFoundException` (404) and ListServers no longer lists it.

Every response must be computed from persisted state held in the shared engine store reached via `crate::state::service_state(&*self.engine)`. NO canned responses, NO echoing the request back, NO minting an id that is never stored. `ServerId` must satisfy the modeled pattern/length so real SDKs accept it. Input constraints (`ServerId` pattern, `UserName` pattern, `Role` pattern, `MaxResults` range) that the framework does not already enforce should surface as `InvalidRequestException` (400).

## State Model

In `src/state.rs` (`ComAmazonawsTransferTransferserviceState`, behind the existing lock):

- `servers: BTreeMap<String /* ServerId, "s-" + 17 hex */, Server>` where
  `Server { server_id, arn, domain, identity_provider_type, identity_provider_details, endpoint_type, endpoint_details, protocols: Vec<String>, protocol_details, logging_role, security_policy_name: String /* default "TransferSecurityPolicy-2018-11-05" */, pre_authentication_login_banner, post_authentication_login_banner, host_key_fingerprint, structured_log_destinations: Vec<String>, s3_storage_options, ip_address_type, workflow_details, certificate, state: ServerState /* OFFLINE|STARTING|ONLINE|STOPPING */, tags: Vec<Tag>, created_at }`
- `users: BTreeMap<(String /* ServerId */, String /* UserName */), User>` where
  `User { server_id, user_name, arn, role, home_directory, home_directory_type, home_directory_mappings: Vec<HomeDirectoryMapEntry>, policy, posix_profile, ssh_public_keys: Vec<SshKey>, tags: Vec<Tag>, created_at }`
  and `SshKey { ssh_public_key_id /* "key-" + 17 hex */, ssh_public_key_body, date_imported }`
- `next_id_seq: u64` — monotonic counter used to mint the 17-hex-digit suffix for server/user-key ids so ids are unique and reproducible.
- `UserCount` and `SshPublicKeyCount` are DERIVED at read time from `users` / `User.ssh_public_keys`, never stored as a stale field.
- Pagination tokens are opaque strings encoding the last-seen key of the `BTreeMap` iteration order so List* calls resume deterministically.

All other operations (connectors, agreements, profiles, certificates, host keys, workflows/executions, web apps, accesses, Start*/Test* actions, Tag/Untag/ListTagsForResource, StartServer/StopServer, UpdateServer/UpdateUser) stay 501 for now and will be filed as individual gap issues.

## Evidence

Spec: `services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json` plus the denormalized `OPERATIONS.md` at the crate root — CreateServer (line 346: output `ServerId` required, pattern `^s-([0-9a-f]{17})$`), DescribeServer (line 352: output `Server` DescribedServer required, errors include ResourceNotFoundException 404), DeleteServer (line 365, @idempotent), ListServers (line 372: @paginated inputToken=NextToken/outputToken=NextToken/items=Servers, InvalidNextTokenException), CreateUser (line 379: required `Role`/`ServerId`/`UserName`, ResourceExistsException 409), DescribeUser (line 386), DeleteUser (line 399), ListUsers (line 406: @paginated items=Users, requires ServerId). Shape members for DescribedServer/ListedServer/DescribedUser/ListedUser confirmed via smithy_query (DescribedServer.UserCount = "number of users assigned to a server you specified with the ServerId"; ListedUser.SshPublicKeyCount). Implementation: all 71 files in `src/ops/` carry `//! SCAFFOLDED, NOT IMPLEMENTED:` at line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501; no handler touches `crate::state`. Wire protocol awsJson1_1 with `X-Amz-Target: TransferService.<Op>` per OPERATIONS.md line 10-11. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `transfer` client at it.
2. `aws transfer create-server --domain S3 --identity-provider-type SERVICE_MANAGED --protocols SFTP --tags Key=env,Value=dev` → today HTTP 501 NotImplemented; expected 200 with a `ServerId` matching ^s-([0-9a-f]{17})$.
3. `aws transfer create-user --server-id <ServerId> --user-name alice --role arn:aws:iam::123456789012:role/transfer-s3 --home-directory /bucket/alice` → expected 200 echoing ServerId + UserName, with the user persisted.
4. `aws transfer describe-server --server-id <ServerId>` → expected 200 with Server.Domain=S3, Server.Protocols=[SFTP], Server.State=OFFLINE, Server.UserCount=1, Server.Tags containing env=dev.
5. `aws transfer describe-user --server-id <ServerId> --user-name alice` → expected 200 with User.Role and User.HomeDirectory equal to the values created, and a User.Arn derived from the stored ids.
6. `aws transfer list-users --server-id <ServerId> --max-results 1` → expected alice in Users with SshPublicKeyCount, and NextToken only if more users exist.
7. `aws transfer list-servers` → expected the new server listed with UserCount=1.
8. `aws transfer create-user --server-id <ServerId> --user-name alice --role arn:aws:iam::123456789012:role/transfer-s3` again → expected ResourceExistsException (HTTP 409).
9. `aws transfer describe-server --server-id s-00000000000000000` (never created) → expected ResourceNotFoundException (HTTP 404).
10. `aws transfer delete-user --server-id <ServerId> --user-name alice` then `describe-user` → expected 404, and `describe-server` UserCount back to 0.
11. `aws transfer delete-server --server-id <ServerId>` then `describe-server` → expected ResourceNotFoundException (404) and the server absent from `list-servers`.
