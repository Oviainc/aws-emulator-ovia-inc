Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: b6cb1c1c1a438d218681c6214ad627b7359d5b65

# A nextToken ListProfilePermissions handed out is refused once the statement it names is removed, so paging cannot be finished

_Filed: 2026-10-28T17:07:33Z_
_Operation: ListProfilePermissions | Class: pagination_

> _Note: this issue was identified against the code base at commit `b6cb1c1c1a438d218681c6214ad627b7359d5b65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListProfilePermissions hands out `nextToken` as the RAW statementId the next page starts at, and on the way back in it requires that exact id to still be a key of the policy: `if !policy.statements.contains_key(token) { return Err(invalid_next_token(token)) }`. A statement id is not a stable cursor — `RemoveProfilePermission` takes statements out of the same policy — so a token this service itself issued stops working as soon as the statement it names is removed between two pages: the client gets 400 `ValidationException` / code `InvalidNextToken` and can never read the statements that followed it; it has to restart the enumeration from page one.

The sibling paginated read in this same crate does not have this hole. `ListSigningProfiles` decodes its cursor and then resumes at the first element AT OR AFTER the one the cursor names, precisely so that "a page is still served when the profile it named has since stopped passing the filters this listing carries", and it tells a cursor it issued from a value it never issued by means of the crate's own cursor helpers (`ProfileListing::page_token` / `ProfileListing::resume_at`, built on `rendered_page_token` / `decoded_resume_at`). `ListProfilePermissions` calls neither helper and has no at-or-after resumption, so for it the only way to recognise a cursor is "the element still exists" — which conflates a cursor the service never handed out with one it did hand out and whose element has since gone.

## Evidence

The strict existence check, and the handler's own claim about what it refuses — services/service-com-amazonaws-signer-wallabyservice/src/ops/list_profile_permissions.rs:210-218:
```
// `nextToken` is the statement id the next page starts at, so the only
// tokens this service hands out are statement ids of this policy; anything
// else is refused with the modelled 400 rather than silently answering an
// arbitrary suffix of the order.
if let Some(token) = input.next_token.as_deref() {
    if !policy.statements.contains_key(token) {
        return Err(invalid_next_token(token));
    }
}
```
A statement id is removable, by this crate's own write path — services/service-com-amazonaws-signer-wallabyservice/src/ops/remove_profile_permission.rs:234:
```
policy.statements.remove(&input.statement_id);
```
The sibling listing handles exactly this case instead of refusing — services/service-com-amazonaws-signer-wallabyservice/src/ops/list_signing_profiles.rs:250-259:
```
for (profile_name, profile) in store.profiles_in(&listing.region) {
    // Resume at the first name at or after the one the token carries, so a
    // page is still served when the profile it named has since stopped
    // passing the filters this listing carries.
    if resume_at
        .as_deref()
        .is_some_and(|resume_at| profile_name < resume_at)
    {
        continue;
    }
```
The cursor helpers this path skips already exist in the crate — services/service-com-amazonaws-signer-wallabyservice/src/state.rs:1580-1591:
```
pub fn page_token(&self, resume_at: &str) -> String {
    rendered_page_token(&self.canonical(), resume_at)
}
...
pub fn resume_at(&self, token: &str) -> Option<String> {
    let resume_at = decoded_resume_at(&self.canonical(), token)?;
    is_profile_name(&resume_at).then_some(resume_at)
}
```
and their purpose is stated at services/service-com-amazonaws-signer-wallabyservice/src/state.rs:1706-1708: "what a resumed page must never do is serve a position out of a DIFFERENT enumeration" — a guarantee that does not require the resumed element to still exist.

Test coverage: services/service-com-amazonaws-signer-wallabyservice/tests/all/profile_permissions.rs:1267-1336 (`a_next_token_this_service_never_handed_out_is_refused`) covers tokens the service NEVER handed out ("a", "zzz", "never-granted", a revision id) and :1166-1190 pages a policy that is not being modified; no test covers a token the service DID hand out whose statement was then removed, so the broken case is unpinned.

## Steps to Reproduce

1. PUT /signing-profiles/Paged_Profile with a valid platformId to create a profile.
2. POST /signing-profiles/Paged_Profile/permissions 30 times with statementIds s01..s30 (action signer:GetSigningProfile, principal 123456789012).
3. GET /signing-profiles/Paged_Profile/permissions — the answer carries 25 permissions (s01..s25) and nextToken="s26".
4. DELETE /signing-profiles/Paged_Profile/permissions/s26?revisionId=<current revisionId> — 200, s26 is gone.
5. GET /signing-profiles/Paged_Profile/permissions?nextToken=s26 — observed: 400 ValidationException with code InvalidNextToken, so s27..s30 are unreachable through the cursor the service handed out; the sibling ListSigningProfiles path (list_signing_profiles.rs:250-259) would instead resume at the first statement at or after s26.
