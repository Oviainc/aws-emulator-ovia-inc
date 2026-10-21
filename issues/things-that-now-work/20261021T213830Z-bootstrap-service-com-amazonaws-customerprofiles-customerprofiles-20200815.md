Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the Customer Profiles emulator

_Filed: 2026-10-21T21:38:30Z_
_Operation: service-com-amazonaws-customerprofiles-customerprofiles-20200815 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 115 operations in `service-com-amazonaws-customerprofiles-customerprofiles-20200815` is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. The service holds no state, so no client workflow works.

Implement a FIRST SLICE of six operations (restJson1, signing name `profile`) that makes the primary-resource lifecycle — domain -> profile — work end-to-end against persisted state:

1. **CreateDomain** — `POST /domains/{DomainName}`. Input: `DomainName` (@httpLabel, 1..64, `^[a-zA-Z0-9_-]+$`, required), `DefaultExpirationDays` (1..1098, required), optional `DefaultEncryptionKey`, `DeadLetterQueueUrl`, `Matching`, `RuleBasedMatching`, `Tags`. Must STORE the domain and return the stored record: `DomainName`, `DefaultExpirationDays`, `DefaultEncryptionKey`, `DeadLetterQueueUrl`, `Matching`, `RuleBasedMatching`, `CreatedAt`, `LastUpdatedAt` (both required, server-generated), `Tags`.
2. **GetDomain** — `GET /domains/{DomainName}`. Must read the stored domain and return exactly the values CreateDomain persisted (same `CreatedAt`/`LastUpdatedAt`, same `Tags`, same expiration days). Unknown `DomainName` -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **CreateProfile** — `POST /domains/{DomainName}/profiles`. Input: the profile attribute set (`FirstName`, `LastName`, `EmailAddress`, `PhoneNumber`, `AccountNumber`, `PartyType`, `Gender`, `Address`/`ShippingAddress`/`MailingAddress`/`BillingAddress`, `Attributes` map, `ProfileType`, ...). Must mint a NEW `ProfileId` matching `^[a-f0-9]{32}$` (required output), store the full profile under the domain, and index its searchable keys. Unknown domain -> `ResourceNotFoundException`.
4. **SearchProfiles** — `POST /domains/{DomainName}/profiles/search` with `KeyName` + `Values` (both required), optional `MaxResults` (1..100, @httpQuery `max-results`) and `NextToken` (@httpQuery `next-token`). Must look the profiles up from state by the standard key (at minimum `_profileId`; ideally `_email`/`_phone`/`_fullName` too) and return `Items` as the stored `Profile` structures — every field set by CreateProfile must come back, plus the `ProfileId`. A key with no match returns an EMPTY `Items`, not a 404 and not a fabricated profile. Honour `MaxResults` and emit/accept `NextToken` when more results remain.
5. **DeleteProfile** — `POST /domains/{DomainName}/profiles/delete` with `ProfileId` (required). Must remove the profile and its key index entries from state and return `Message`; a subsequent SearchProfiles for that profile must return empty.
6. **DeleteDomain** — `DELETE /domains/{DomainName}`. Must remove the domain and all nested profiles/keys and return `Message`; a subsequent GetDomain must return `ResourceNotFoundException` (404) and the domain must disappear from ListDomains.

Rules for the slice: NEVER STUBS — each of the six must compute its response from the stored state. No hardcoded fields, no echoing the request back without storing it, no random `ProfileId` that is not persisted and retrievable. Timestamps are server-generated once at create and stable across reads. `DomainName`/`ProfileId` pattern and length constraints and `DefaultExpirationDays` range must be enforced with `BadRequestException` (400); references to a missing domain/profile must yield `ResourceNotFoundException` (404).

Optionally include **ListDomains** (`GET /domains`, output `Items` of `ListDomainItem` + `NextToken`) if cheap — it is the natural verification step that a created domain is visible and a deleted one is gone.

## State Model

In `ComAmazonawsCustomerprofilesCustomerprofiles20200815State` (src/state.rs), keyed per account/region:

- `domains: HashMap<String /*DomainName*/, Domain>`
- `Domain { name: String, default_expiration_days: i32, default_encryption_key: Option<String>, dead_letter_queue_url: Option<String>, matching: Option<MatchingResponse>, rule_based_matching: Option<RuleBasedMatchingResponse>, tags: HashMap<String,String>, created_at: SystemTime, last_updated_at: SystemTime, profiles: HashMap<String /*ProfileId*/, Profile>, key_index: HashMap<(String /*KeyName*/, String /*Value*/), HashSet<String /*ProfileId*/>> }`
- `Profile { profile_id: String /*32 hex chars*/, account_number, additional_information, party_type, business_name, first_name, middle_name, last_name, birth_date, gender, phone_number, mobile_phone_number, home_phone_number, business_phone_number, email_address, personal_email_address, business_email_address, address, shipping_address, mailing_address, billing_address, attributes: HashMap<String,String>, profile_type, engagement_preferences, created_at, last_updated_at }`
- `key_index` is populated at CreateProfile time with the standard search keys (`_profileId` -> the id, `_email`/`_phone`/`_fullName` where the corresponding fields are set) and is what SearchProfiles reads; DeleteProfile removes the profile's entries from it.

All other operations (calculated attributes, segments, event streams/triggers, integrations, workflows, recommenders, upload jobs, identity resolution, merge, object types) stay 501 for now and will be filed as individual gap issues.

Done = an integration script performing CreateDomain -> GetDomain -> CreateProfile -> SearchProfiles -> DeleteProfile -> DeleteDomain succeeds with correct, stateful bodies (not merely 2xx).

## Evidence

OPERATIONS.md (crate root) documents the wire protocol (`restJson1`, SDK id `Customer Profiles`, signing name `profile`) and each operation's contract: CreateDomain (line 68: `POST /domains/{DomainName}`, required `DomainName`+`DefaultExpirationDays`, required output `CreatedAt`/`LastUpdatedAt`), GetDomain (line 318, `GET /domains/{DomainName}`, required output `DomainName`/`CreatedAt`/`LastUpdatedAt`), CreateProfile (line 103, `POST /domains/{DomainName}/profiles`, required output `ProfileId` pattern `^[a-f0-9]{32}$`), SearchProfiles (line 762, `POST /domains/{DomainName}/profiles/search`, required `KeyName`+`Values`, output `Items` of `Profile` + `NextToken`, query params `max-results`/`next-token`), DeleteProfile (line 210, `POST /domains/{DomainName}/profiles/delete`, required `ProfileId`), DeleteDomain (line 167, `DELETE /domains/{DomainName}`, required output `Message`), ListDomains (line 554). All six declare `ResourceNotFoundException (HTTP 404)` and `BadRequestException (HTTP 400)`. Current state: `src/ops/create_domain.rs` line 4 has the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and line 226 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDomain"))`; all 115 op files in `src/ops/` are the same scaffold. No tests assert any behavior (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. POST /domains/testdom with {"DefaultExpirationDays": 90, "Tags": {"env":"dev"}} -> expect 200 with DomainName=testdom, DefaultExpirationDays=90, Tags echoed from storage, CreatedAt/LastUpdatedAt set (today: 501 NotImplemented)
2. GET /domains/testdom -> expect 200 with the exact values persisted above, same CreatedAt
3. GET /domains/nosuchdom -> expect 404 ResourceNotFoundException
4. POST /domains/testdom/profiles with {"FirstName":"Ada","LastName":"Lovelace","EmailAddress":"ada@example.com","Attributes":{"tier":"gold"}} -> expect 200 with a fresh ProfileId matching ^[a-f0-9]{32}$
5. POST /domains/testdom/profiles/search with {"KeyName":"_profileId","Values":["<that ProfileId>"]} -> expect 200 with Items[0] carrying FirstName=Ada, LastName=Lovelace, EmailAddress=ada@example.com, Attributes.tier=gold and the same ProfileId
6. POST /domains/testdom/profiles/search with {"KeyName":"_profileId","Values":["00000000000000000000000000000000"]} -> expect 200 with empty Items
7. POST /domains/testdom/profiles/delete with {"ProfileId":"<that ProfileId>"} -> expect 200 with Message; re-running the search returns empty Items
8. DELETE /domains/testdom -> expect 200 with Message; GET /domains/testdom then returns 404 and ListDomains no longer lists testdom
