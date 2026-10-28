Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7027025c5fd5dfe34e91978241957b132a94afec

# The profile ARN is minted from the raw credential-scope region, so a created profile can carry an ARN this crate's own ARN parser then calls malformed

_Filed: 2026-10-28T19:04:01Z_
_Operation: PutSigningProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7027025c5fd5dfe34e91978241957b132a94afec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PutSigningProfile builds and persists the profile's ARN by pasting the request's credential-scope region into the ARN template, with no check that the value can survive a round trip through the ONE ARN parser this crate uses to resolve a `resourceArn` back to a profile.

`crate::state::signer_region_and_resource` takes an ARN apart by splitting on `:` into exactly six fields (`arn : partition : service : region : account : resource`). A credential-scope region that itself contains a `:` therefore shifts every later field: the parser reads the account out of the second half of the region and the resource out of the account, the `/signing-profiles/` prefix no longer strips, and the parse fails outright.

The consequence is a profile this service created, stores, and happily reports through GetSigningProfile, whose own ARN — the one PutSigningProfile returned as `arn`, and the one GetSigningProfile reports as `arn` — is refused by the three tagging handlers as "not a valid signing profile resource ARN" (400 BadRequestException), and by GetRevocationStatus as "is not a valid signing profile version ARN" (400 ValidationException). The profile can never be tagged, its tags can never be read or removed, and its versions can never be revocation-checked: one handler mints an identifier the crate's other handlers declare impossible.

Nothing in the crate reconciles the two sides: `request_region` returns the credential-scope region verbatim, and `profile_arn` is documented as building the exact form `profile_region_and_name_in_arn` claims to parse.

## Evidence

The mint (region taken verbatim, pasted into the ARN):

- `src/ops/put_signing_profile.rs:203` — `let region = crate::state::request_region(ctx.parts());`
- `src/ops/put_signing_profile.rs:306` — `let arn = crate::state::profile_arn(&input.profile_name, &region);`
- `src/state.rs:1746-1750` — `pub fn request_region(parts: &::http::request::Parts) -> String { ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.region).unwrap_or_else(|| DEFAULT_REGION.to_string()) }` (the scope parser accepts ANY non-empty region field: `vendor/server/src/lib.rs:1800` only rejects an empty one)
- `src/state.rs:801-811` — "/// The ARN of the signing profile `profile_name` names. /// /// Signer's profile ARNs carry a leading `/` in the resource portion: /// `arn:aws:signer:<region>:<account>:/signing-profiles/<name>`." then `::engine::emulator_arn("signer", region, &format!("/signing-profiles/{profile_name}"))`

The parser that then refuses that same string, in the same crate:

- `src/state.rs:907-927` — "/// The ONE place this service takes an ARN apart …" `let mut fields = arn.splitn(6, ':'); … let region = fields.next()?; if region.is_empty() || fields.next()?.is_empty() { return None; } Some((region, fields.next()?))`
- `src/state.rs:835-839` — `let name = resource.strip_prefix(PROFILE_ARN_RESOURCE_PREFIX)?;` (fails once the fields have shifted)
- `src/state.rs:938-944` — `TaggingArnProblem::Malformed` = "/// `resourceArn` is not a signing-profile ARN."; `src/state.rs:958-960` — message "`{resource_arn} is not a valid signing profile resource ARN`"

The handlers that answer with it:

- `src/ops/tag_resource.rs:219-221` — `store.profile_for_tagging_mut(&region, &input.resource_arn).map_err(|problem| refused(problem, &input.resource_arn))?` → `BadRequestException` (`src/ops/tag_resource.rs:130-132`)
- `src/ops/untag_resource.rs:217-219`, `src/ops/list_tags_for_resource.rs:194-196` — the same refusal
- `src/ops/get_revocation_status.rs:207-212` — `if !crate::state::is_signer_arn(&input.profile_version_arn) { return Err(validation(format!("{} is not a valid signing profile version ARN", input.profile_version_arn))); }`

Test coverage: `tests/all/resource_tags.rs` and `tests/all/region_scoping.rs` exercise the tagging round trip only for credential scopes whose region is a plain label (e.g. `tests/all/profile_permissions.rs:40-42`, `Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request`), so no test covers a region the ARN grammar cannot carry.

## Steps to Reproduce

1. Send `PUT /signing-profiles/Colon_Region_Profile` with body `{"platformId":"AWSLambda-SHA384-ECDSA"}` and header `Authorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us:east-1/signer/aws4_request, SignedHeaders=host, Signature=abc`
2. Observe HTTP 200 and `arn` = `arn:aws:signer:us:east-1:000000000000:/signing-profiles/Colon_Region_Profile` (the stored ARN; `GET /signing-profiles/Colon_Region_Profile` with the same credential scope reports the identical value)
3. Send `POST /tags/arn%3Aaws%3Asigner%3Aus%3Aeast-1%3A000000000000%3A%2Fsigning-profiles%2FColon_Region_Profile` with body `{"tags":{"team":"signing"}}` and the same Authorization header
4. Observe HTTP 400 `BadRequestException` with message "arn:aws:signer:us:east-1:000000000000:/signing-profiles/Colon_Region_Profile is not a valid signing profile resource ARN" — the service refuses the ARN it just minted; `GET /tags/{sameArn}` and `DELETE /tags/{sameArn}?tagKeys=team` fail identically, and `GET /revocations?...&profileVersionArn=<arn>/<profileVersion>&...` answers 400 ValidationException "is not a valid signing profile version ARN"
