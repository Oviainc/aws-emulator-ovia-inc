Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the route53domains emulator

_Filed: 2026-10-21T21:27:52Z_
_Operation: service-com-amazonaws-route53domains-route53domains-v20140515 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 35 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Route 53 Domains client workflow can run against the emulator. Implement a FIRST, SMALL stateful slice: the register -> inspect -> list -> delete lifecycle of the primary resource (a registered domain), plus the asynchronous operation record that every mutating Route 53 Domains call returns.

Protocol (README/OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: Route53Domains_v20140515.<OperationName>`, JSON body using the modeled member names.

### Happy path to implement (in order)

1. **CheckDomainAvailability** — in: `DomainName` (required), `IdnLangCode`; out: `Availability` (enum `AVAILABLE | UNAVAILABLE | ...`). Must be computed from state: a name already present in the domain store returns `UNAVAILABLE`; an unregistered, well-formed name in a supported TLD returns `AVAILABLE`. After step 2 the same name must flip to `UNAVAILABLE`.
2. **RegisterDomain** — in: `DomainName` (required), `DurationInYears` (required, range 1..10), `AdminContact`/`RegistrantContact`/`TechContact` (required `ContactDetail`), `AutoRenew`, `PrivacyProtect*Contact`, `IdnLangCode`, `BillingContact`; out: `OperationId`. Must insert a `Domain` record into the store (contacts, privacy flags, auto-renew, creation/expiration dates derived from now + `DurationInYears`, default nameservers, status list) AND append a pending/succeeded `Operation` record of `Type = REGISTER_DOMAIN` whose id is the returned `OperationId`. Re-registering a name already in the store must fail with `DuplicateRequest`, not succeed.
3. **GetOperationDetail** — in: `OperationId` (required); out: `OperationId`, `Status` (`SUBMITTED | IN_PROGRESS | SUCCESSFUL | ERROR | FAILED`), `DomainName`, `Type`, `SubmittedDate`, `LastUpdatedDate`. Must look the id up in the operation store and return the stored domain name/type/timestamps; an unknown id must return `InvalidInput`, never a fabricated success.
4. **GetDomainDetail** — in: `DomainName` (required); out at minimum `DomainName`, `Nameservers`, `AutoRenew`, `AdminContact`, `RegistrantContact`, `TechContact`, `AdminPrivacy`, `RegistrantPrivacy`, `TechPrivacy`, `CreationDate`, `ExpirationDate`, `StatusList`. Every field must come from the stored `Domain` written by RegisterDomain — the contacts returned must be byte-for-byte the contacts registered, not echoed from the current request. A name not in the store must return `InvalidInput`.
5. **ListDomains** — in: `Marker`, `MaxItems` (range ..100), `FilterConditions`, `SortCondition`; out: `Domains` (list<DomainSummary>: `DomainName`, `AutoRenew`, `TransferLock`, `Expiry`), `NextPageMarker`. Must enumerate the domain store, honour `MaxItems` and return/consume `NextPageMarker` so a paginating client terminates; omit `NextPageMarker` on the last page.
6. **DeleteDomain** — in: `DomainName` (required); out: `OperationId`. Must remove the domain from the store and append a `DELETE_DOMAIN` operation record. A subsequent GetDomainDetail must fail, ListDomains must no longer include it, and CheckDomainAvailability must report `AVAILABLE` again. Deleting an unknown domain must return `InvalidInput`.

Optionally round out the slice with **EnableDomainAutoRenew/DisableDomainAutoRenew** (flip the stored `AutoRenew` flag, visible in GetDomainDetail) and **ListOperations** (enumerate the operation store, paginated by `Marker`/`MaxItems`) if cheap — nothing else.

No canned, hardcoded, or echoed responses: each of the six must compute its response from persisted state, and a random `OperationId` returned without storing an operation record is a failure. Validation to enforce in this slice: `DomainName` non-empty and <= 255 chars, `DurationInYears` within 1..10, required contacts present — all violations as `InvalidInput (HTTP 400)` in the awsJson1_1 error shape.

## State Model

In `src/state.rs` (`ComAmazonawsRoute53domainsRoute53domainsV20140515State`), behind the shared engine handle reached via `crate::state::service_state(&*self.engine)`:

- `domains: BTreeMap<String /* lowercased DomainName */, Domain>` where `Domain { name: String, auto_renew: bool, transfer_lock: bool, admin_contact: ContactDetail, registrant_contact: ContactDetail, tech_contact: ContactDetail, billing_contact: Option<ContactDetail>, admin_privacy: bool, registrant_privacy: bool, tech_privacy: bool, billing_privacy: bool, nameservers: Vec<Nameserver>, status_list: Vec<String>, registrar_name: String, creation_date: Timestamp, updated_date: Timestamp, expiration_date: Timestamp, auth_code: String, duration_in_years: i32, tags: BTreeMap<String, String> }`
- `operations: Vec<OperationRecord>` (plus an id index) where `OperationRecord { operation_id: String, domain_name: String, op_type: OperationType, status: OperationStatus, message: Option<String>, submitted_date: Timestamp, last_updated_date: Timestamp }` — written by RegisterDomain and DeleteDomain, read by GetOperationDetail/ListOperations.
- a monotonic counter used to mint unique `OperationId` values that are keys into `operations`.

Everything outside this list (TransferDomain and the cross-account transfer family, DNSSEC signer ops, GetDomainSuggestions, ListPrices/GetDomainPrices, ViewBilling, tags ops, contact-reachability ops, PushDomain, RenewDomain, UpdateDomain* ) stays scaffolded at 501 and will be filed as individual gap issues later.

Done = an integration test that drives the six operations in order over awsJson1_1 and asserts the response bodies (not just 2xx): availability flips, registered contacts come back unchanged, the operation id resolves, the domain appears in a paginated ListDomains, and after DeleteDomain it is gone.

## Evidence

OPERATIONS.md for this crate lists all 35 operations with these exact input/output members (CheckDomainAvailability -> `Availability`; RegisterDomain -> required `DomainName`/`DurationInYears` 1..10/`AdminContact`/`RegistrantContact`/`TechContact`, out `OperationId`, error `DuplicateRequest`; GetOperationDetail -> `OperationId` in, `Status`/`Type`/`SubmittedDate` out; GetDomainDetail -> full domain record out; ListDomains -> @paginated(inputToken=Marker, outputToken=NextPageMarker, items=Domains), `MaxItems` range ..100; DeleteDomain -> `OperationId` out) and gives the wire protocol as awsJson1_1 with target `Route53Domains_v20140515.<Op>`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-route53domains-route53domains-v20140515/src/ops/` matches all 35 handler files, including register_domain.rs, get_domain_detail.rs, list_domains.rs, delete_domain.rs, get_operation_detail.rs and check_domain_availability.rs — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and touches no state. No tests exist beyond the generated tests/state_roundtrip.rs example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Route 53 Domains client at it.
2. CheckDomainAvailability{DomainName:"example-bootstrap.com"} -> expect Availability=AVAILABLE; actual: HTTP 501 NotImplemented.
3. RegisterDomain{DomainName:"example-bootstrap.com", DurationInYears:1, AutoRenew:true, AdminContact/RegistrantContact/TechContact:<contact>} -> expect 200 with a non-empty OperationId.
4. GetOperationDetail{OperationId:<from above>} -> expect DomainName="example-bootstrap.com", Type=REGISTER_DOMAIN, a Status from the modeled enum and SubmittedDate set.
5. GetDomainDetail{DomainName:"example-bootstrap.com"} -> expect AutoRenew=true, the registered contacts returned verbatim, CreationDate/ExpirationDate ~1 year apart, Nameservers non-empty.
6. CheckDomainAvailability{DomainName:"example-bootstrap.com"} again -> expect Availability=UNAVAILABLE.
7. RegisterDomain for the same name again -> expect DuplicateRequest (HTTP 400).
8. ListDomains{MaxItems:1} repeatedly, following NextPageMarker -> expect the domain listed exactly once and pagination to terminate with no NextPageMarker.
9. DeleteDomain{DomainName:"example-bootstrap.com"} -> expect 200 with an OperationId resolvable via GetOperationDetail with Type=DELETE_DOMAIN.
10. GetDomainDetail for the deleted name -> expect InvalidInput (HTTP 400); ListDomains -> empty; CheckDomainAvailability -> AVAILABLE.
