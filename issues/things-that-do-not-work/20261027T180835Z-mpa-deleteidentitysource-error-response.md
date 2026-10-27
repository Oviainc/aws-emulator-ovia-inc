Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c9e2d684fc826ee70e56effd17777b7bc5b4fec7

# DeleteIdentitySource answers 200 for an ARN that addresses no identity source, where GetIdentitySource answers 400 Invalid Resource Arn

_Filed: 2026-10-27T18:08:35Z_
_Operation: DeleteIdentitySource | Class: error-response_

> _Note: this issue was identified against the code base at commit `c9e2d684fc826ee70e56effd17777b7bc5b4fec7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteIdentitySource` screens its `IdentitySourceArn` with the WEAK ARN predicate `crate::state::is_well_formed_arn`, which only asks whether the value is an ARN at all. The sibling `GetIdentitySource`, on the same member of the same resource, screens it with `crate::state::is_identity_source_arn` — the helper this crate added for exactly this member — and refuses anything that is not an identity-source ARN of this service with the malformed-ARN class (`ValidationException` 400, `state::INVALID_ARN_MESSAGE`).

The consequence is a false success. A value such as `arn:aws:mpa:us-east-1:<caller-account>:approval-team/some-team` (another resource kind of this very service), `arn:aws:sso:us-east-1:<caller-account>:identity-source/ids-1` (another service's namespace), `arn:not-aws:mpa:us-east-1:<caller-account>:identity-source/ids-1` (a partition outside the `aws` family) or `arn:aws:mpa:us-east-1:12345:identity-source/ids-1` (an account that is not twelve digits) passes `is_well_formed_arn`, passes the account gate when the fifth field happens to be the caller's account, resolves to no stored identity source, and is answered HTTP 200 with an empty body — the handler reporting that a delete the caller asked for was carried out on a value that addresses no identity source at all.

The crate already states the rule this skips: `is_service_resource_arn` (which `is_identity_source_arn` wraps) documents that an ARN "of another service's namespace, of another resource kind, or with no resource id names no resource of this kind, so it is refused as malformed (`invalid_arn`) rather than looked up", and `is_identity_source_arn` documents itself as the grammar "the `IdentitySourceArn` members ADDRESS" — the member `DeleteIdentitySource` carries too. Nothing in this handler, its doc header or the test suite states a reason for the two members to be screened differently. This is distinct from the already-open `DeleteIdentitySource` validation issue (the in-use check ignoring a pending update's approvers): that one is about which approver sets `team_arn_using_identity_source` scans, this one is about which values ever reach the lookup.

## Evidence

The weak screen, and the success it leads to — `src/ops/delete_identity_source.rs:203-211`:
```rust
        // `IdentitySourceArn` carries no ARN `@pattern` here, so the malformed-ARN class
        // is refused by this handler with the one sentence this service answers it with
        // (`crate::state::invalid_arn`, 400) rather than by the boundary.
        if !crate::state::is_well_formed_arn(&input.identity_source_arn) {
            return Err(DeleteIdentitySourceErrorModeled::ValidationException(
                crate::state::invalid_arn(),
            )
            .into());
        }
```
and `src/ops/delete_identity_source.rs:235-237`:
```rust
        if state.identity_source(&input.identity_source_arn).is_none() {
            return Ok(DeleteIdentitySourceOutput);
        }
```

The sibling handler on the same member, using the strict screen — `src/ops/get_identity_source.rs:201-211`:
```rust
        // `IdentitySourceArn` carries no ARN `@pattern` here, so the malformed-ARN class
        // is refused by this handler with the one sentence this service answers it with
        // (`crate::state::invalid_arn`, 400). A value that is not an identity source's own
        // ARN addresses no identity source, so it is refused as that class rather than
        // looked up and reported absent.
        if !crate::state::is_identity_source_arn(&input.identity_source_arn) {
            return Err(GetIdentitySourceErrorModeled::ValidationException(
                crate::state::invalid_arn(),
            )
            .into());
        }
```

The rule the crate states for this class — `src/state.rs:1764-1774`:
```rust
/// Whether `arn` is an ARN of this service's identity-source resource
/// ([`is_service_resource_arn`] for [`IDENTITY_SOURCE_RESOURCE`]) — the kind of ARN the
/// `IdentitySourceArn` members ADDRESS.
///
/// A value that is not one names no identity source whatever this store holds, so the
/// read addressed by it answers the malformed-ARN class ([`invalid_arn`]) rather than
/// reporting a lookup that found nothing.
pub fn is_identity_source_arn(arn: &str) -> bool {
    is_service_resource_arn(arn, IDENTITY_SOURCE_RESOURCE)
}
```
and `src/state.rs:3016-3020`: "Stricter than [`is_well_formed_arn`], which answers only whether a value is an ARN at all: this answers whether it is an ARN this service could have minted for that kind of resource, which is what a member documented to carry one ADDRESSES. An ARN of another service's namespace, of another resource kind, or with no resource id names no resource of this kind, so it is refused as malformed ([`invalid_arn`]) rather than looked up."

The handler's own doc states only the weak rule, with no reason for the divergence — `src/ops/delete_identity_source.rs:19-22`: "`IdentitySourceArn` carries no ARN `@pattern` of its own here (`length(max: 1000)` only), so a value that is not a well-formed ARN is refused by this handler with the one sentence this service answers that class with ([`crate::state::invalid_arn`]): `ValidationException` (400)."

Test coverage: `tests/all/identity_source_detail.rs:412-503` (`an_arn_that_names_no_identity_source_is_refused_as_malformed`) pins 400 + `state::INVALID_ARN_MESSAGE` for exactly the wrong-kind rows, including `("an approval team's ARN", format!("arn:aws:mpa:{REGION}:{account}:approval-team/some-team"))` (:462-466), `("another service's namespace", ...)` (:452-455), `("a partition outside the aws family", ...)` (:433-436) and `("an account that is not twelve digits", ...)` (:443-447). The delete suite's equivalent table, `tests/all/identity_source_deletion.rs:586-632` (`a_malformed_identity_source_arn_is_refused`), carries only the seven generic non-ARN rows (`"no colons at all"`, `"the arn prefix misspelled"`, `"too few fields"`, `"five fields, no resource"`, `"an empty resource"`, `"an empty partition"`, `"an empty service"`) — not one wrong-kind row, so the gap is unpinned.

## Steps to Reproduce

1. Start the emulator and point an MPA client at it in us-east-1.
2. Call `DELETE /identity-sources/{IdentitySourceArn}` with the URI-encoded value `arn:aws:mpa:us-east-1:<emulator-account-id>:approval-team/some-team` (an ARN of another resource kind of this same service, in the caller's account).
3. Observe HTTP 200 with an empty body: `is_well_formed_arn` accepts the value (src/ops/delete_identity_source.rs:206), the account gate passes, `state.identity_source(..)` resolves to nothing and the handler returns `Ok(DeleteIdentitySourceOutput)` (src/ops/delete_identity_source.rs:235-237).
4. Call `GET /identity-sources/{IdentitySourceArn}` with the SAME value: observe HTTP 400 `ValidationException` with `Message` = `Invalid Resource Arn` (src/ops/get_identity_source.rs:206).
5. Repeat step 2 with `arn:aws:sso:us-east-1:<emulator-account-id>:identity-source/ids-000000000001` and with `arn:not-aws:mpa:us-east-1:<emulator-account-id>:identity-source/ids-000000000001`: both are answered 200 by the delete and 400 by the get.
