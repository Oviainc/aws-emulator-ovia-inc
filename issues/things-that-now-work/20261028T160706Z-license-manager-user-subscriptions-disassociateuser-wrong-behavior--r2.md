Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0b62ce506db227f8753aee084e9b25daeb773a55

# DisassociateUser ignores InstanceId/Username/IdentityProvider when the request also carries InstanceUserArn, tearing down an association on an instance it never named

_Filed: 2026-10-28T16:07:06Z_
_Operation: DisassociateUser | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0b62ce506db227f8753aee084e9b25daeb773a55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `DisassociateUser` supplies `InstanceUserArn`, `crate::state::resolve_instance_user` returns the record that ARN names and then cross-checks only `Domain`. `InstanceId`, `Username` and `IdentityProvider` are never compared against the stored association, even though the handler passes all of them into the resolution and they are exactly the members that make up an association's identity (`instance_user_key(provider_key, instance_id, username, domain)`).

So a request naming the `InstanceUserArn` of `a.user` on instance `i-A` together with `InstanceId: i-B` / `Username: b.user` / that directory's `IdentityProvider` is accepted: the handler accepts the disassociation of a.user on i-A, answers HTTP 200 with an `InstanceUserSummary` whose `InstanceId`/`Username` are the STORED ones (not the ones the caller named), and `refresh_instance_view` is run for i-A while the association on i-B — the one the request actually named — is left alone. A request that names two mutually contradictory things is turned into a success against a resource the caller did not name.

Two rules inside this crate are broken by that:

1. the instance-authorization gate decides on the instance the request NAMES (`service.rs`: "the account's ownership of the EC2 instance a request names is what decides whether this service acts on that request at all"), so a request authorized for i-B lands its write on an association of i-A, an instance this call's gate never asked about;
2. the very same function states why `Domain` IS cross-checked on the ARN path — "`Domain` is part of which user a request names, so an association addressed by ARN is also only the one the caller asked for" — and that reason applies verbatim to `InstanceId` and `Username`, which the non-ARN addressing of this same function requires to match.

## Evidence

The ARN path checks only `Domain` — `src/state.rs:1123-1141` (`resolve_instance_user`):
```
        let record = if let Some(arn) = instance_user_arn {
            self.instance_users.get(arn)?
        } else {
            let arn = self.instance_user_arn_for(
                &provider_key(identity_provider?).ok()?,
                instance_id?,
                username?,
                domain,
            )?;
            self.instance_users.get(arn)?
        };
        // `Domain` is part of which user a request names, so an association addressed by
        // ARN is also only the one the caller asked for when the domain the request names
        // is the stored user's. The identity lookup above already matched on it.
        let named = domain_key(domain);
        if named.is_some() && named != domain_key(record.domain.as_deref()) {
            return None;
        }
        Some(record)
```
The handler hands every member to that resolution and uses none of them afterwards — `src/ops/disassociate_user.rs:216-230`:
```
        let arn = state
            .resolve_instance_user(
                input.instance_user_arn.as_deref(),
                input.identity_provider.as_ref(),
                input.instance_id.as_deref(),
                input.username.as_deref(),
                input.domain.as_deref(),
            )
            .ok_or_else(crate::scaffolded::Scaffolded::unauthorized)?
            .arn
            .clone();

        let record = state
            .begin_instance_user_disassociation(&arn, now)
```
The identity an association is stored under includes the instance and the user name — `src/state.rs:1151-1156` (`insert_instance_user`):
```
        let index_key = instance_user_key(
            &record.provider_key,
            &record.instance_id,
            &record.username,
            record.domain.as_deref(),
        );
```
The authorization the request passed is about the instance it NAMED — `src/service.rs:67-71`:
```
/// An operation belongs here exactly when its input carries an `InstanceId`: the account's
/// ownership of the EC2 instance a request names is what decides whether this service acts
/// on that request at all, so the decision is taken for every operation that names one.
```
and `src/ops/disassociate_user.rs:190-195` runs that same gate on `input.instance_id` only.

The sibling ARN-addressed teardown in this crate DOES refuse a mismatched secondary discriminator — `src/ops/delete_license_server_endpoint.rs:173-180`:
```
        // Fail closed: an ARN state holds no endpoint for, or one whose `ServerType` is
        // not the stored endpoint's, is refused without disclosing which it was.
        let arn = state
            .license_server_endpoints
            .get(&input.license_server_endpoint_arn)
            .filter(|record| record.server_type == input.server_type)
```

Test coverage: every ARN-addressed disassociation in the suite sends the ARN (plus at most `Domain`) and nothing else — `tests/all/user_disassociation.rs:145-158` builds `json!({ "InstanceUserArn": arn })` for the `by_arn` rows, and `tests/all/instance_gate_is_one_function.rs:74-77` calls `/user/DisassociateUser` with `&json!({ "InstanceUserArn": arn })`, whose rule is only "a request that names NO instance authorizes nothing" (line 207-209). No test pins an ARN combined with a conflicting `InstanceId`/`Username`.

## Steps to Reproduce

1. RegisterIdentityProvider for a seeded directory (e.g. DirectoryId d-9f8e7d6c5b) and Product=VISUAL_STUDIO_ENTERPRISE.
2. AssociateUser with Username=a.user, InstanceId=<seeded instance A>, that IdentityProvider — note the returned InstanceUserArn (call it ARN_A).
3. AssociateUser with Username=b.user, InstanceId=<seeded instance B>, the same IdentityProvider — note its InstanceUserArn (ARN_B).
4. Call DisassociateUser with {"InstanceUserArn": ARN_A, "InstanceId": <instance B>, "Username": "b.user", "IdentityProvider": <that provider>}.
5. Observe HTTP 200 and an InstanceUserSummary carrying InstanceUserArn=ARN_A, InstanceId=<instance A>, Username=a.user, Status=DISASSOCIATING — the association the request did not name was torn down.
6. ListUserAssociations for <instance B> shows b.user still ASSOCIATED, and for <instance A> shows a.user DISASSOCIATING, confirming the write landed on the instance the request never named (and whose ownership this call's gate never checked).
