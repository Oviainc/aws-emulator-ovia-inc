Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7d7bdea90bc70459c4b589287302dc4eaf39117d

# Bootstrap the account emulator

_Filed: 2026-10-21T22:16:06Z_
_Operation: service-com-amazonaws-account-account | Class: missing_

> _Note: this issue was identified against the code base at commit `7d7bdea90bc70459c4b589287302dc4eaf39117d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 18 operations of the AWS Account service (`restJson1`, sigv4 signing name `account`) are unmodified scaffolds answering HTTP 501 and holding zero state. Implement a FIRST SMALL SLICE: the alternate-contact / account-settings happy path, which is this service's primary stateful resource lifecycle.

Happy path (implement these 5 in order):

1. **PutAccountName** — `POST /putAccountName`. Input: `AccountName` (required, len 1..50, pattern `^[ -;=?-~]+$`), optional `AccountId` (`^\d{12}$`). Empty output body. Must PERSIST the name against the resolved account id (caller's account when `AccountId` is absent).
2. **GetAccountInformation** — `POST /getAccountInformation`. Input: optional `AccountId`. Output: `AccountId`, `AccountName`, `AccountCreatedDate` (timestamp), `AccountState` (ACTIVE|CLOSED|PENDING_ACTIVATION|SUSPENDED). `AccountName` must be exactly what the preceding PutAccountName stored; `AccountCreatedDate` must be a stable per-account value (set once at first touch, not re-generated per call); `AccountState` ACTIVE for a known account.
3. **PutAlternateContact** — `POST /putAlternateContact`. Input: `AlternateContactType` (BILLING|OPERATIONS|SECURITY, required), `Name`, `Title`, `EmailAddress`, `PhoneNumber` (all required, with the modeled length/pattern constraints), optional `AccountId`. Empty output. Must upsert the contact keyed by (account id, contact type) — idempotent (`@idempotent`): a second call with the same type overwrites, never duplicates.
4. **GetAlternateContact** — `POST /getAlternateContact`. Input: `AlternateContactType` (required), optional `AccountId`. Output: `AlternateContact{Name, Title, EmailAddress, PhoneNumber, AlternateContactType}` read back byte-for-byte from the stored record. If no contact of that type was stored: `ResourceNotFoundException` (HTTP 404) — NOT an empty 200.
5. **DeleteAlternateContact** — `POST /deleteAlternateContact`. Input: `AlternateContactType` (required), optional `AccountId`. Removes the stored record; a subsequent GetAlternateContact for that type must return `ResourceNotFoundException` (404). Deleting a type that is not set returns `ResourceNotFoundException` (404).

Also required for all five: reject inputs violating the modeled constraints (missing required member, `AccountId` not matching `^\d{12}$`, `AccountName` outside 1..50 or failing its pattern, `EmailAddress`/`PhoneNumber` pattern violations, unknown `AlternateContactType` value) with `ValidationException` (HTTP 400), not a 200.

NEVER STUBS: every one of these five must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (the shared engine) — no canned bodies, no echoing the request back without storing it, no values regenerated per call. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each handler file as it is implemented.

## State Model

`ComAmazonawsAccountAccountState` (src/state.rs, currently an empty struct) gains:

- `accounts: HashMap<String /* 12-digit account id */, Account>`
- `Account { account_id: String, account_name: Option<String>, created_date: SystemTime /* set once on first insert */, state: AccountState /* ACTIVE */, alternate_contacts: HashMap<AlternateContactType, AlternateContactRecord> }`
- `AlternateContactRecord { name: String, title: String, email_address: String, phone_number: String, contact_type: AlternateContactType }`
- `AlternateContactType` enum: `Billing | Operations | Security` (map 1:1 to the modeled enum values).
- A `default_account_id` (the caller's account, e.g. from the request context / a fixed 12-digit emulator account) used whenever the optional `AccountId` input member is absent, so standalone and management-account calls land on the same record.

All mutation goes through `state.lock()` so the five operations share one store across requests.

Out of scope for this slice (stay 501, filed separately later): PutContactInformation / GetContactInformation (ContactInformation structure with required FullName, AddressLine1, City, PostalCode, CountryCode, PhoneNumber), the primary-email flow (StartPrimaryEmailUpdate / AcceptPrimaryEmailUpdate / GetPrimaryEmail / GetPrimaryEmailUpdateStatus), phone verification (SendPhoneNumberVerification / VerifyPhoneNumber), region opt-in (EnableRegion / DisableRegion / GetRegionOptStatus / ListRegions with its `NextToken`/`MaxResults` pagination), and GetGovCloudAccountInformation.

## Evidence

services/service-com-amazonaws-account-account/OPERATIONS.md lists all 18 operations with routes, members and errors (PutAccountName lines 25-31, GetAccountInformation 33-39, PutAlternateContact 41-47, GetAlternateContact 49-55, DeleteAlternateContact 57-63), including the `ResourceNotFoundException (HTTP 404)` and `ValidationException (HTTP 400)` the contact reads/deletes declare and the `@idempotent`/`@readonly` traits. `smithy_query shape=AlternateContact` gives the output structure's members (Name, Title, EmailAddress, PhoneNumber, AlternateContactType, all sensitive, with the length/pattern constraints). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-account-account/src/ops/` matches all 18 handler files, each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. src/state.rs line 8 is a `// TODO: add service-specific state fields` placeholder, so the service holds no state at all. No tests assert any of this behavior (only the generated tests/state_roundtrip.rs example).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `account` client at it.
2. `aws account put-account-name --account-name "Demo Account"` → expect HTTP 200 with an empty body (today: 501 NotImplemented).
3. `aws account get-account-information` → expect `AccountName` == "Demo Account", a 12-digit `AccountId`, a stable `AccountCreatedDate`, and `AccountState` == ACTIVE.
4. `aws account put-alternate-contact --alternate-contact-type BILLING --name "Ada" --title "CFO" --email-address ada@example.com --phone-number "+1 206-555-0100"` → 200, empty body.
5. `aws account get-alternate-contact --alternate-contact-type BILLING` → expect the AlternateContact structure with exactly the Name/Title/EmailAddress/PhoneNumber just written and AlternateContactType == BILLING.
6. `aws account get-alternate-contact --alternate-contact-type SECURITY` → expect ResourceNotFoundException with HTTP 404.
7. Re-run the BILLING put with `--name "Grace"`, then get BILLING again → expect the single record updated to "Grace" (idempotent upsert, no duplicate).
8. `aws account delete-alternate-contact --alternate-contact-type BILLING` → 200; then `get-alternate-contact --alternate-contact-type BILLING` → ResourceNotFoundException (404); then delete BILLING again → ResourceNotFoundException (404).
9. `aws account get-account-information --account-id 12345` (malformed) → expect ValidationException with HTTP 400, not a 200.
