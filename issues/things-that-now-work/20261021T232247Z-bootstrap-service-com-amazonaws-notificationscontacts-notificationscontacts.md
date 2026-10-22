Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 96234e472a4dbed875261b6f3ae0a9ed15fa6e7c

# Bootstrap the notifications-contacts emulator

_Filed: 2026-10-21T23:22:47Z_
_Operation: service-com-amazonaws-notificationscontacts-notificationscontacts | Class: missing_

> _Note: this issue was identified against the code base at commit `96234e472a4dbed875261b6f3ae0a9ed15fa6e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 9 operations in `services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/ops/` is an unmodified scaffold (carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and returns `Scaffolded::not_implemented(..)` => HTTP 501), so the service holds no state at all. This issue defines the FIRST small, stateful slice: the email-contact lifecycle.

Happy path to implement, in order (restJson1, sigv4 signing name `notifications-contacts`):

1. **CreateEmailContact** — `POST /2022-09-19/emailcontacts` -> 201. Input: `name` (1..64, `[\w-.~]+`, required), `emailAddress` (6..254, `^(.+)@(.+)$`, required), `tags` (map, optional). Must mint and PERSIST a new contact: generate an ARN matching `^arn:aws:notifications-contacts::[0-9]{12}:emailcontact/[a-z0-9]{27}$` (12-digit account id from the request credentials/default account, plus a fresh 27-char lowercase-alnum id), initial `status = inactive`, `creationTime` = `updateTime` = now, store `tags`. Output: the generated `arn` — the SAME arn later reads resolve.
2. **GetEmailContact** — `GET /emailcontacts/{arn}` -> 200. Input: `arn` (@httpLabel, required). Output: `emailContact` (required) = EmailContact{`arn`, `name`, `address`, `status`, `creationTime`, `updateTime`} — all required, all read back from the stored record (note the output member is `address`, while the create input member is `emailAddress`). Unknown-but-well-formed arn => `ResourceNotFoundException` (404).
3. **SendActivationCode** — `POST /2022-10-31/emailcontacts/{arn}/activate/send` -> 200, empty body. Must look up the contact (404 if absent) and generate + store a 7-char `^[a-z0-9]{7}$` activation code against that contact so step 4 can verify it. Already-`active` contact => `ConflictException` (409).
4. **ActivateEmailContact** — `PUT /emailcontacts/{arn}/activate/{code}` -> 200, empty body. Must compare `code` against the code stored in step 3: on match flip the stored `status` to `active` and bump `updateTime`; on mismatch return a modeled failure (`ValidationException` 400) rather than 200; unknown arn => 404. Idempotent: re-activating an already-active contact with the valid code stays 200.
5. **ListEmailContacts** — `GET /emailcontacts` -> 200. Input: `maxResults`, `nextToken` (both @httpQuery). Output: `emailContacts` (required list<EmailContact>) + `nextToken`. Must enumerate the persisted contacts for the caller's account (so a contact created in step 1 appears, with the `active` status written in step 4), honour `maxResults` as a page bound and return an opaque `nextToken` that resumes where the page stopped; omit `nextToken` on the last page.
6. **DeleteEmailContact** — `DELETE /emailcontacts/{arn}` -> 200, empty body. Must remove the record (and its tags/activation code) from state; a subsequent GetEmailContact must return `ResourceNotFoundException` (404) and ListEmailContacts must no longer include it.

NEVER STUBS: each of these must compute its response from the persisted store. No canned ARNs, no echoing the input back as if it were stored, no random id without a stored record. A create followed by a get/list must return exactly what was created, and activation/deletion must be observable by later calls.

## State Model

In `crate::state` (behind the existing `service_state(&*self.engine)` mutex guard):

- `ContactsState { contacts: HashMap<String /* contact ARN */, EmailContactRecord>, next_id_seed: u64 }`
- `EmailContactRecord { arn: String, name: String, address: String, status: EmailContactStatus /* inactive | active */, creation_time: DateTime, update_time: DateTime, tags: BTreeMap<String,String>, pending_activation_code: Option<String> /* 7 chars, [a-z0-9] */ }`
- Pagination: `nextToken` encodes the position in the ARN-ordered iteration of `contacts` (e.g. the last-returned ARN, base64-encoded); invalid/undecodable token => `ValidationException` (400).
- `tags` lives on the record so the later TagResource/UntagResource/ListTagsForResource slice reads the same map.

Out of scope for this first slice (stay 501, filed separately): `TagResource`, `UntagResource`, `ListTagsForResource`.

## Evidence

`services/service-com-amazonaws-notificationscontacts-notificationscontacts/OPERATIONS.md` lists all 9 operations with routes, required input/output members, constraints and errors (CreateEmailContact 201 with required `arn` output; GetEmailContact required `emailContact`; ListEmailContacts @paginated(inputToken=nextToken, outputToken=nextToken, items=emailContacts) with required `emailContacts`; ActivateEmailContact `PUT /emailcontacts/{arn}/activate/{code}` with `code` len 7..7 `^[a-z0-9]{7}$`; SendActivationCode `POST /2022-10-31/emailcontacts/{arn}/activate/send`; DeleteEmailContact `DELETE /emailcontacts/{arn}`). `smithy_query(shape="EmailContact")` gives the required members `arn`, `name`, `address`, `status`, `creationTime`, `updateTime`; `smithy_query(shape="EmailContactStatus")` gives values `inactive` and `active` — grounding that a freshly created contact is `inactive` until activated. `src/ops/create_email_contact.rs` lines 4 and 171 show the scaffold header and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateEmailContact"))`; every sibling file under `src/ops/` is the same generated stub, so no state exists today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `notifications-contacts` at it.
2. CreateEmailContact with name="oncall-primary", emailAddress="oncall@example.com", tags={"team":"infra"} — expect 201 and an `arn` matching ^arn:aws:notifications-contacts::[0-9]{12}:emailcontact/[a-z0-9]{27}$ (today: HTTP 501 NotImplemented).
3. GetEmailContact with that arn — expect 200 with emailContact{arn (same), name="oncall-primary", address="oncall@example.com", status="inactive", creationTime, updateTime}.
4. SendActivationCode with that arn — expect 200 and an activation code persisted server-side for the contact.
5. ActivateEmailContact with that arn and the wrong code "zzzzzzz" — expect a 400 ValidationException, not 200, and status still "inactive".
6. ActivateEmailContact with that arn and the issued code — expect 200; GetEmailContact now returns status="active" with a bumped updateTime.
7. Create two more contacts, then ListEmailContacts with maxResults=2 — expect exactly 2 emailContacts plus a nextToken; pass that nextToken to get the remaining contact with no nextToken on the final page.
8. DeleteEmailContact with the first arn — expect 200; GetEmailContact on it now returns 404 ResourceNotFoundException and ListEmailContacts no longer includes it.
