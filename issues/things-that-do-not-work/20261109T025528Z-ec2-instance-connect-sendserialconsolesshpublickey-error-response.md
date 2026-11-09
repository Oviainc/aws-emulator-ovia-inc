Issue-Severity: HIGH
Issue-Gap-Class: error-response
Issue-Commit: aba2adfa3005273c5f6b7f8438c028c27604934b

# A long-term access key the IAM store holds as Inactive is treated as a valid credential, so a deactivated key still pushes a key with Success: true

_Filed: 2026-11-09T02:55:28Z_
_Operation: SendSerialConsoleSSHPublicKey | Class: error-response_

> _Note: this issue was identified against the code base at commit `aba2adfa3005273c5f6b7f8438c028c27604934b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both pushes go through `crate::state::authorization::refusal` -> `caller()`, which the crate documents as refusing "A request whose credential the identity store does not hold as VALID" with the modelled `AuthException`. `caller()` implements that rule for ONE credential kind only: an STS session past its validity window is mapped to `Refusal::InvalidCredential`. For a LONG-TERM IAM access key it looks the key up in IAM's `access_keys` map and uses the record solely to find the owning user — `record.status` is never read — so a key whose stored `statusType` is `Inactive` (what IAM's own `UpdateAccessKey` writes) resolves to the user, is judged on that user's policies, and an allowing policy produces a 200 `Success: true` push with the key written into `serial_console_keys` / `pushed_keys`.

The IAM store this crate reads already owns the decision and exposes it as a public accessor — `access_key_is_usable` — whose doc states that a key the account holds "authenticates a request only while its status is ACCESS_KEY_ACTIVE_STATUS, and deactivating one makes it unusable for API calls while leaving it listed". The gate's own module doc says it reads "through the IAM service's OWN accessors", yet this one path indexes `store.access_keys` directly and skips the accessor that answers whether the credential is usable at all. So the crate's documented step 1 is enforced for an expired session and silently skipped for a deactivated long-term key.

The defect lives in the shared gate, so `SendSSHPublicKey` behaves identically (it needs no account toggle, so it is the shortest reproduction). It is filed on `SendSerialConsoleSSHPublicKey` because that handler's own decision order states the rule it breaks, and because the open `SendSSHPublicKey` issues already occupy that operation's classes.

## Evidence

The rule, this crate's own — `services/service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice/src/state/authorization.rs:56-62`:
```
//! A request whose credential the identity store does not hold as VALID is a different
//! failure, and gets a different answer: it speaks for no principal, so there is nothing to
//! read policies for and no policy is consulted, and the model declares an error for exactly
//! that case — `AuthException`, "Either your AWS credentials are not valid or you do not have
//! access to the EC2 instance"
```
and the handler's own decision order — `src/ops/send_serial_console_ssh_public_key.rs:12-14`:
```
//! 1. the request's credential is one the identity store holds as valid, else the modelled
//!    `AuthException` — a credential that is not valid speaks for no principal, so no policy
//!    is consulted at all;
```

The code that skips it — `src/state/authorization.rs:458-467` (the record is used only for `user_key`; `record.status` is never read):
```
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
        }
    }
```
The sibling path in the SAME function, which does apply the rule — `src/state/authorization.rs:437-439`:
```
    // The session credential has expired, so it is not a valid credential and speaks for
    // nobody: there is no principal to read policies for.
    let identity = identity.map_err(|_expired| InvalidCredential)?;
```
and the gate's claim about how it reads that store — `src/state/authorization.rs:20-26`: "read through the IAM service's OWN accessors (`principal_policies`, `permissions_boundary_document`), where the collection rule itself lives".

The accessor in the store it reads, not called — `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state/account.rs:1173-1194`:
```
    /// Whether a signed request presenting `access_key_id` carries a credential this
    /// account will accept.
    ///
    /// `UpdateAccessKey$Status` documents what the stored value means: "Active means
    /// that the key can be used for programmatic calls to Amazon Web Services, while
    /// Inactive means that the key cannot be used" — so a key the account holds
    /// authenticates a request only while its status is
    /// [`ACCESS_KEY_ACTIVE_STATUS`], and deactivating one makes it unusable for API
    /// calls while leaving it listed.
    pub fn access_key_is_usable(&self, access_key_id: &str) -> bool {
        self.access_keys
            .get(access_key_id)
            .is_none_or(|record| record.status == ACCESS_KEY_ACTIVE_STATUS)
    }
```
Proof the `Inactive` status is reachable through the emulator's own APIs — `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/update_access_key.rs:307`:
```
        let previous = ::std::mem::replace(&mut record.status, input.status.clone());
```
(`AccessKeyRecord.status` is defined at `.../src/state.rs:991-996`.)

Test coverage: nothing in this crate's tests deactivates a key — `rg "UpdateAccessKey|Inactive" services/service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice/tests` returns no matches — while `tests/all/credential_validity_auth_exception.rs:15-17` asserts the premise this gap rests on: "The invalid credential this emulator can hold is a session whose validity window has run out". That harness mints long-term keys through IAM's own `CreateAccessKey` (`:273-275`) and signs EIC pushes with them (`:142-152`), so the deactivated-key case is one `UpdateAccessKey` call away from an existing test and is pinned by nothing.

## Steps to Reproduce

1. Mount EC2 Instance Connect, IAM and STS on one shared engine, as `tests/all/credential_validity_auth_exception.rs:98-107` does.
2. Through IAM's own API: `CreateUser(UserName=probe-user)`, `PutUserPolicy` with `{"Effect":"Allow","Action":"ec2-instance-connect:*","Resource":"*"}`, then `CreateAccessKey(UserName=probe-user)` and keep the returned `AccessKeyId`.
3. Call IAM `UpdateAccessKey(UserName=probe-user, AccessKeyId=<that id>, Status=Inactive)` — IAM now holds the key as unusable (`access_key_is_usable` would answer false).
4. Grant the account serial-console access (EC2 `EnableSerialConsoleAccess`, or set `serial_console_access_enabled = true` as the test helper does).
5. POST `/` with `X-Amz-Target: AWSEC2InstanceConnectService.SendSerialConsoleSSHPublicKey`, an `Authorization` credential scope naming the DEACTIVATED access key id, and body `{"InstanceId":"i-0a1b2c3d4e5f60718","SerialPort":0,"SSHPublicKey":"<80+ char key>"}`.
6. Observed: HTTP 200 with `Success: true`, and the key present in `serial_console_keys` — the deactivated credential performed the push. Expected per the crate's own documented step 1: the modelled `AuthException` with "Either your AWS credentials are not valid or you do not have access to the EC2 instance.", with nothing stored.
7. Repeat step 5 against `AWSEC2InstanceConnectService.SendSSHPublicKey` (no account toggle needed): same 200 `Success: true`, confirming the gap is in the shared gate rather than in one handler.
