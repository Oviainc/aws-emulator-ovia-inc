Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 9b93cbc54d897386c30bfd66ea5bd8f85f03a356

# ListEmailContacts accepts a nextToken it never issued, contradicting its own stated rule

_Filed: 2026-10-26T01:04:32Z_
_Operation: ListEmailContacts | Class: pagination_

> _Note: this issue was identified against the code base at commit `9b93cbc54d897386c30bfd66ea5bd8f85f03a356`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEmailContacts` documents — in its module header and on the token decoder itself — that a `nextToken` the operation did not issue is answered with the modelled 400 `ValidationException`. The implementation does not test issuance: `decode_page_token` hex-decodes the token, converts it to UTF-8 and then only checks that the resulting string LOOKS like one of this service's email-contact ARNs (`crate::state::contact_id_from_arn`, which validates the ARN prefix, the 27-character id length and the id alphabet, and nothing else). Any caller can therefore hand-build `hex("arn:aws:notifications-contacts::000000000000:emailcontact/" + 27 lowercase alphanumerics)` for a contact that was never created and get a 200 with a suffix of the list, instead of the 400 both comments promise. The refusal that does fire even words itself "is not a pagination token issued by ListEmailContacts", a claim the check cannot support: a request the code should refuse is turned into a success.

## Evidence

Stated rule, `src/ops/list_email_contacts.rs:8-9`:
```
//! the next call resumes beyond it;
//! the last page carries no token. A token this operation did not issue is the
//! modelled 400.
```
And on the decoder, `src/ops/list_email_contacts.rs:96-108`:
```
/// The ARN a page token names, or the modelled 400 when the token is not one this
/// operation issued.
fn decode_page_token(token: &str) -> Result<String, ListEmailContactsError> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|arn| crate::state::contact_id_from_arn(arn).is_some())
        .ok_or_else(|| {
            ListEmailContactsErrorModeled::ValidationException(
                crate::state::field_validation_failure(
                    "nextToken",
                    "is not a pagination token issued by ListEmailContacts",
```
What the filter actually checks, `src/state.rs:339-346`:
```
pub fn contact_id_from_arn(candidate: &str) -> Option<&str> {
    let id = candidate
        .strip_prefix(&contact_arn(""))
        .filter(|id| id.len() == CONTACT_ID_LEN)?;
    id.bytes()
        .all(|b| CONTACT_ID_ALPHABET.contains(&b))
        .then_some(id)
}
```
No stored issuance record exists for a token to be matched against (`ComAmazonawsNotificationscontactsNotificationscontactsState` holds only `contacts`, `id_counter`, `last_instant_millis`, `src/state.rs:237-251`).

Test coverage: `tests/all/email_contacts.rs:685-721` (`a_page_token_the_list_never_issued_is_refused`) exercises only tokens that are malformed in shape — "not hex at all", "hex of a non-arn", "hex of another service's arn", "hex of an arn whose id is too short" — so the well-formed-but-never-issued case is not pinned either way, and no test asserts that such a token must be accepted.

## Steps to Reproduce

1. Start the emulator and create no contacts, or create a few with `POST /2022-09-19/emailcontacts`.
2. Build a token for a contact that was never created: hex-encode the string `arn:aws:notifications-contacts::000000000000:emailcontact/aaaaaaaaaaaaaaaaaaaaaaaaaaa` (27 `a`s).
3. Call `GET /emailcontacts?nextToken=<that hex>`.
4. Observe HTTP 200 with a page of contacts, although the handler's own documentation at `src/ops/list_email_contacts.rs:8-9` and `:96-97` says a token this operation did not issue is the modelled 400 `ValidationException`.
