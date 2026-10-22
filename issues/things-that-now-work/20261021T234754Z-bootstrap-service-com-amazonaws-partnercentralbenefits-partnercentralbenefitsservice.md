Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9e4273988d175f89e8a017aa0737ee7107d62a89

# Bootstrap the PartnerCentral Benefits emulator

_Filed: 2026-10-21T23:47:54Z_
_Operation: service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice | Class: missing_

> _Note: this issue was identified against the code base at commit `9e4273988d175f89e8a017aa0737ee7107d62a89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 17 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields, so no client workflow can run. Implement a FIRST, SMALL slice: the benefit-application lifecycle, statefully, on the `awsJson1_0` wire protocol (`POST /` with `X-Amz-Target: PartnerCentralBenefitsService.<OperationName>`).

### Happy path to implement (in order)

1. **ListBenefits** — input `Catalog` (required), optional `Programs`, `FulfillmentTypes`, `Status` (ACTIVE|INACTIVE), `MaxResults`, `NextToken`; output `BenefitSummaries` + `NextToken`. The benefit catalog is service-owned (there is no CreateBenefit operation), so seed a small fixed set of benefits per catalog (e.g. catalog `AWS`) at state init and return them from the store, filtered by the supplied filters. This is what gives a client a valid `BenefitIdentifier` for step 2.
2. **CreateBenefitApplication** (@idempotent) — input `Catalog` (required), `ClientToken` (required), `BenefitIdentifier` (required), optional `Name` (1..255), `Description` (1..1000), `FulfillmentTypes`, `BenefitApplicationDetails` (document), `Tags`, `AssociatedResources`, `PartnerContacts`, `FileDetails`; output `Id` matching `^benappl-[0-9a-z]{14}$`, `Arn`, `Revision`. Must mint a new id, persist EVERY supplied input member, set `Status = PENDING_SUBMISSION`, set `CreatedAt`/`UpdatedAt`, and set an initial `Revision`. Replaying the same `ClientToken` must return the identical `Id`/`Arn`/`Revision` without creating a second application. An unknown `BenefitIdentifier` must be `ResourceNotFoundException` (404).
3. **GetBenefitApplication** — input `Catalog`, `Identifier` (id or ARN, `^(arn:.+|benappl-[0-9a-z]{14})$`); output must be read back from the stored record: `Id`, `Arn`, `Catalog`, `BenefitId`, `Name`, `Description`, `FulfillmentTypes`, `BenefitApplicationDetails`, `Status`, `StatusReason`, `CreatedAt`, `UpdatedAt`, `Revision`, `AssociatedResources`, `PartnerContacts`, `FileDetails`. Unknown identifier -> `ResourceNotFoundException` (404).
4. **UpdateBenefitApplication** — input `Catalog`, `ClientToken`, `Identifier`, `Revision` (all required) plus `Name`, `Description`, `BenefitApplicationDetails`, `PartnerContacts`, `FileDetails`; output `Id`, `Arn`, `Revision`. Must apply the supplied fields to the stored record, bump `Revision` and `UpdatedAt`, and reject a stale/mismatched `Revision` with `ConflictException` (409). A subsequent GetBenefitApplication must show the new values and the new `Revision`.
5. **SubmitBenefitApplication** — input `Catalog`, `Identifier`; empty output. Must transition the stored record from `PENDING_SUBMISSION` to `IN_REVIEW` and bump `UpdatedAt`; submitting an application that is not in `PENDING_SUBMISSION` is `ConflictException` (409). The transition must be observable via GetBenefitApplication and ListBenefitApplications.
6. **ListBenefitApplications** (@paginated inputToken=NextToken, outputToken=NextToken, items=BenefitApplicationSummaries) — input `Catalog` (required) plus the `Programs`/`FulfillmentTypes`/`BenefitIdentifiers`/`Status`/`Stages`/`AssociatedResourceArns` filters, `MaxResults`, `NextToken`; output `BenefitApplicationSummaries` + `NextToken`. Must enumerate the applications actually stored for that catalog, honour `Status`/`BenefitIdentifiers` filtering, cap the page at `MaxResults`, and return an opaque `NextToken` that resumes exactly where the previous page stopped (absent when the listing is exhausted).
7. **CancelBenefitApplication** — input `Catalog`, `ClientToken`, `Identifier`, optional `Reason`; empty output. Must set stored `Status = CANCELED`, record `Reason` as `StatusReason`, bump `UpdatedAt`/`Revision`. GetBenefitApplication must then report `CANCELED`; cancelling an already-canceled application is `ConflictException` (409).

None of these may return a canned, zeroed or echoed response: each one must compute its response from the persisted store, and a create followed by a get/list must return what was created. `Catalog` must be validated against `^[A-Za-z0-9_-]+$` and identifiers against their modeled patterns, with `ValidationException` (400) on violation; lookups of unknown ids are `ResourceNotFoundException` (404).

## State Model

Add to `ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState`:

- `catalogs: HashMap<String, Catalog>` — keyed by the `Catalog` input string.
- `Catalog { benefits: HashMap<String, Benefit>, applications: HashMap<String, BenefitApplication>, client_tokens: HashMap<String, String> /* ClientToken -> application id */ }`
- `Benefit { id: String /* ben-<14 lowercase alnum> */, arn: String, name: String, description: String, programs: Vec<String>, fulfillment_types: Vec<FulfillmentType>, benefit_request_schema: Option<Document>, status: BenefitStatus }` — seeded read-only catalog data, served by ListBenefits and GetBenefit.
- `BenefitApplication { id: String /* benappl-<14 lowercase alnum> */, arn: String, catalog: String, benefit_id: String, name: Option<String>, description: Option<String>, fulfillment_types: Vec<FulfillmentType>, benefit_application_details: Option<Document>, programs: Vec<String>, status: BenefitApplicationStatus, stage: Option<String>, status_reason: Option<String>, status_reason_codes: Vec<String>, created_at: SystemTime, updated_at: SystemTime, revision: String, associated_resources: Vec<String>, partner_contacts: Vec<Contact>, file_details: Vec<FileDetail>, tags: Vec<Tag> }`
- An ARN index `arns: HashMap<String, (String /*catalog*/, String /*id*/)>` so identifiers given as ARNs resolve to the same record as the bare id.

Identifier resolution must accept either the bare `benappl-...` id or the full ARN. Everything else in the service (AmendBenefitApplication, RecallBenefitApplication, Associate/DisassociateBenefitApplicationResource, GetBenefit, GetBenefitAllocation, ListBenefitAllocations, TagResource/UntagResource/ListTagsForResource) stays stubbed at 501 for now and will be filed as individual gap issues — but the state model above should leave room for them (the `tags` field and the allocation types).

Done = a client script can ListBenefits -> CreateBenefitApplication -> GetBenefitApplication -> UpdateBenefitApplication -> SubmitBenefitApplication -> ListBenefitApplications -> CancelBenefitApplication and see correct, stateful bodies at each step, not just 2xx.

## Evidence

OPERATIONS.md for this crate lists all 17 operations with their input/output members and errors (CreateBenefitApplication output `Id` pattern `^benappl-[0-9a-z]{14}$`, `Arn`, `Revision`; GetBenefitApplication output `Status` enum ACTION_REQUIRED|APPROVED|CANCELED|IN_REVIEW|PENDING_SUBMISSION|REJECTED; ListBenefitApplications/@paginated inputToken=NextToken outputToken=NextToken items=BenefitApplicationSummaries; UpdateBenefitApplication requiring `Revision` and declaring ConflictException 409; wire protocol awsJson1_0 with X-Amz-Target PartnerCentralBenefitsService.<Op>). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 17 handler files, so every operation returns HTTP 501 today. `src/state.rs` contains only `// TODO: add service-specific state fields`, i.e. there is no store at all. There are no tests asserting any of this behavior.

## Steps to Reproduce

1. Start the emulator and point an awsJson1_0 client at it (POST /, header X-Amz-Target: PartnerCentralBenefitsService.<OperationName>).
2. Call ListBenefits with {"Catalog":"AWS"} — expect a non-empty BenefitSummaries list from the seeded catalog; actual: HTTP 501 NotImplemented.
3. Call CreateBenefitApplication with {"Catalog":"AWS","ClientToken":"tok-1","BenefitIdentifier":<id from step 2>,"Name":"demo","Description":"demo app"} — expect an Id matching ^benappl-[0-9a-z]{14}$, an Arn and a Revision; actual: HTTP 501.
4. Repeat the identical CreateBenefitApplication call with ClientToken tok-1 — expect the same Id/Arn/Revision and no second application.
5. Call GetBenefitApplication with {"Catalog":"AWS","Identifier":<Id>} — expect Name="demo", BenefitId equal to the BenefitIdentifier sent, Status=PENDING_SUBMISSION, CreatedAt/UpdatedAt/Revision populated.
6. Call UpdateBenefitApplication with {"Catalog":"AWS","ClientToken":"tok-2","Identifier":<Id>,"Revision":<Revision>,"Name":"demo-2"} — expect a new Revision; then re-Get and expect Name="demo-2". Re-send the update with the OLD Revision and expect ConflictException (409).
7. Call SubmitBenefitApplication with {"Catalog":"AWS","Identifier":<Id>} — expect success, then Get shows Status=IN_REVIEW. Submit again and expect ConflictException (409).
8. Call ListBenefitApplications with {"Catalog":"AWS","MaxResults":1} after creating two applications — expect one summary plus a NextToken, and the second page to return the other application and no NextToken.
9. Call CancelBenefitApplication with {"Catalog":"AWS","ClientToken":"tok-3","Identifier":<Id>,"Reason":"no longer needed"} — expect success, then Get shows Status=CANCELED with StatusReason="no longer needed".
10. Call GetBenefitApplication with an unknown identifier benappl-00000000000000 — expect ResourceNotFoundException (404), and with Catalog "bad catalog!" expect ValidationException (400).
