Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5941084f5d4a21e8b5693768e8fcfa390372a113

# An expired session credential is refused as AccessDeniedException "no identity-based policy allows" instead of the modelled AuthException

_Filed: 2026-11-08T01:07:44Z_
_Operation: SendSSHPublicKey | Class: error-response_

> _Note: this issue was identified against the code base at commit `5941084f5d4a21e8b5693768e8fcfa390372a113`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both handlers take their authorization decision through `crate::state::authorization::refusal`. That function's caller resolution has a dedicated branch for a credential STS reports as expired: it fabricates a pseudo-principal (`"an expired session credential (<akid>)"`) with an EMPTY document list and `account_credential: false`, which then falls through the ordinary policy evaluation and comes back as the synthetic, NON-modelled `AccessDeniedException` (HTTP 403) whose message blames a missing identity-based policy.

Two things are wrong with that, both provable from this crate's own text:

1. The model declares `AuthException` (HTTP 403) for exactly this failure mode — "Either your AWS credentials are not valid ..." — and this crate's own `scaffolded.rs` says so in as many words while justifying the synthetic shape: the synthetic `AccessDeniedException` is scoped there to "a call the caller's IAM policies do not allow", and `AuthException` is named as "the one AWS states for a credential that is not valid". An expired session credential is a credential that is not valid, not a policy refusal, yet it is answered with the policy-refusal shape. `AuthException` is declared in both operations' error enums and is never constructed anywhere in `src/` (grep: only the enum variants and the doc headers match).
2. The refusal text misreports the cause. `not_authorized` is documented as naming "who was refused, which action, on which resource, and which rule withheld the grant. Every value in it comes from the request and the stored policies", but for this branch the "who" is an English sentence rather than a principal and the stated reason is "because no identity-based policy allows the ec2-instance-connect:SendSSHPublicKey action" — a policy that was never consulted, because the credential carried no principal at all.

The same branch serves `SendSerialConsoleSSHPublicKey`, which calls the identical gate. No test in the crate pins the expired-credential answer (grep for `expired` in `tests/` matches only the pushed-key TTL assertions), so nothing justifies the divergence.

## Evidence

Model (`smithy_query(query="operation",operation="SendSSHPublicKey")`): `{"shape": "AuthException", "errorKind": "client", "httpError": 403, ...}`, documented — and quoted verbatim in the handler's own header, `src/ops/send_ssh_public_key.rs:107-108`:
```
//! * `AuthException` (HTTP 403) — Either your AWS credentials are not valid or you do not
//!   have access to the EC2 instance.
```

The crate's own scoping of the two shapes — `src/scaffolded.rs:223-228`:
```
/// In the `smithy.framework` namespace for the same reason as `NotImplemented`: AWS answers a
/// call the caller's IAM policies do not allow with this error code on both of this service's
/// operations, whose model declares no error for an authorization refusal at all (the
/// modelled `AuthException` is the one AWS states for a credential that is not valid), so
/// there is no modeled shape to use and the synthetic one must not collide with a modeled
/// name.
```

The branch that breaks that scoping — `src/state/authorization.rs:70-71` (module doc):
```
//! An expired session credential speaks for nobody: it carries no document and no principal
//! spelling, so every action it asks for is refused.
```
and `src/state/authorization.rs:351-363`:
```
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            }
        }
    };
```
which reaches the policy-refusal wording at `src/state/authorization.rs:270-277`:
```
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Some(not_authorized(
            &caller.arn,
            action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
```
against its own stated contract, `src/state/authorization.rs:200-202`:
```
/// The refusal AWS words an authorization failure with: who was refused, which action, on
/// which resource, and which rule withheld the grant. Every value in it comes from the request
/// and the stored policies.
```
and is carried to the wire as the synthetic shape by `src/ops/send_ssh_public_key.rs:253-261`:
```
        if let Some(refusal) = crate::state::authorization::refusal(
            &*self.engine, ctx, Self::NAME, &resource, &conditions,
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(refusal));
        }
```
rendered as `__type` `AccessDeniedException` / HTTP 403 (`src/scaffolded.rs:392`, `:413`).

The modelled shape is wired but dead: `src/ops/send_ssh_public_key.rs:139` `AuthException(AuthException),` and `src/ops/send_serial_console_ssh_public_key.rs:151` are the only non-doc matches for `AuthException` in `src/` (grep `AuthException` over the crate returns the two enum variants, the two doc headers, the generated struct in `src/ops/types/shared_types.rs:14-16`, and the `scaffolded.rs:226` comment — no construction site).

Test coverage: nothing pins this path. `tests/all/identity_policy_authorization.rs:12` states the rule only for policy refusals ("A refusal carries the error code `AccessDeniedException` with HTTP 403"), and its tables (`:498`, `:505`) cover a user whose policies deny or do not allow; grep for `expired` in `tests/` matches only `instance_connect_key_push.rs:548-555` (pushed-key TTL pruning). `src/state/authorization.rs`'s unit tests never build the `Err(expired)` caller.

## Steps to Reproduce

1. Mount this service with IAM and STS on one shared engine, exactly as `tests/all/identity_policy_authorization.rs:98-117` does.
2. Mint a short-lived session credential through STS (e.g. `GetSessionToken`/`AssumeRole` with the smallest `DurationSeconds`) and let it expire, so `sts_store::resolve_caller` answers `Err(expired)` for its access key id.
3. POST `/` with `X-Amz-Target: AWSEC2InstanceConnectService.SendSSHPublicKey`, that expired session's access key id in the SigV4 credential scope, and body `{"InstanceId":"i-0a1b2c3d4e5f60718","InstanceOSUser":"ec2-user","SSHPublicKey":"<80+ char key>"}` (the seeded running Nitro instance, `src/state.rs:508-514`).
4. Observed: HTTP 403 with `__type` `AccessDeniedException` and the message "User: an expired session credential (<akid>) is not authorized to perform: ec2-instance-connect:SendSSHPublicKey on resource: arn:aws:ec2:us-east-1:<account>:instance/i-0a1b2c3d4e5f60718 because no identity-based policy allows the ec2-instance-connect:SendSSHPublicKey action" — a policy refusal for a request in which no principal and no policy was ever resolved.
5. Expected per the modelled error this crate itself names for an invalid credential (`src/ops/send_ssh_public_key.rs:107-108`, `src/scaffolded.rs:226`): the modelled `AuthException` (declared at `src/ops/send_ssh_public_key.rs:139`, never constructed), not the synthetic `AccessDeniedException` whose own doc scopes it to a call the caller's IAM policies do not allow.
6. Repeat with `X-Amz-Target: AWSEC2InstanceConnectService.SendSerialConsoleSSHPublicKey`: the same gate (`src/ops/send_serial_console_ssh_public_key.rs:254-262`) gives the same answer.
