Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the partnercentral-selling emulator

_Filed: 2026-10-21T23:18:24Z_
_Operation: service-com-amazonaws-partnercentralselling-awspartnercentralselling | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 45 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. This issue defines the FIRST buildable slice: the Opportunity lifecycle, which is the service's primary resource (`^O[0-9]{1,19}$` ids) and the entry point for every Partner Central co-sell flow.

Implement these 6 operations, in this order, each computing its response from persisted state (awsJson1_0, `POST /` with `X-Amz-Target: AWSPartnerCentralSelling.<Op>`):

1. **CreateOpportunity** — input `Catalog` (required, `^[a-zA-Z]+$`), `ClientToken` (required idempotency token, `^.{1,255}$`), plus `Customer`, `Project`, `LifeCycle`, `OpportunityType`, `Origin`, `Marketing`, `SoftwareRevenue`, `PrimaryNeedsFromAws`, `NationalSecurity`, `PartnerOpportunityIdentifier`, `OpportunityTeam` (max 10), `Tags` (1..200). Must mint a new `Id` matching `^O[0-9]{1,19}$`, store the WHOLE input under (Catalog, Id), stamp `CreatedDate`/`LastModifiedDate`, build an `Arn` (`^arn:.*$`, arn namespace `partnercentral`), and return `Id`, `PartnerOpportunityIdentifier`, `LastModifiedDate`. Replaying the same `ClientToken` with the same body must return the SAME `Id` (@idempotent), not a second record.
2. **GetOpportunity** — input `Catalog` + `Identifier`. Must return the stored record: `Catalog`, `Id`, `Arn`, `CreatedDate`, `LastModifiedDate`, `RelatedEntityIdentifiers` (required — empty lists when nothing is associated), `Customer`, `Project`, `LifeCycle`, `OpportunityTeam`, `OpportunityType`, `Marketing`, `SoftwareRevenue`, `PrimaryNeedsFromAws`, `NationalSecurity`, `PartnerOpportunityIdentifier`. Unknown `Identifier` (or wrong `Catalog`) → `ResourceNotFoundException` (404), never a hollow 200.
3. **UpdateOpportunity** — input `Catalog`, `Identifier`, `LastModifiedDate` (required, optimistic-concurrency token) plus the mutable fields. Must merge into the stored record, advance `LastModifiedDate`, and return `Id` + the new `LastModifiedDate`. A stale `LastModifiedDate` → `ConflictException` (409); unknown `Identifier` → `ResourceNotFoundException`.
4. **AssignOpportunity** — input `Catalog`, `Identifier`, `Assignee` (AssigneeContact, required). Empty output, but the assignee must be persisted so a following GetOpportunity shows it on the `OpportunityTeam`/assignee field and `LastModifiedDate` moves.
5. **SubmitOpportunity** — input `Catalog`, `Identifier`, `InvolvementType` (`Co-Sell` | `For Visibility Only`, required), `Visibility` (`Full` | `Limited`). Empty output, but it must transition the stored record's `LifeCycle.ReviewStatus` to the submitted state and record the involvement/visibility, observable via GetOpportunity. Re-submitting an already-submitted opportunity → `ValidationException`/`ConflictException` rather than silent success.
6. **ListOpportunities** — input `Catalog` (required), `MaxResults` (1..100), `NextToken`, `Sort`, and the filters `Identifier`, `LifeCycleStage`, `LifeCycleReviewStatus`, `CustomerCompanyName`, `CreatedDate`, `TargetCloseDate`, `LastModifiedDate`. Output `OpportunitySummaries` (required) + `NextToken`. Must enumerate only the opportunities actually created in the requested catalog, honour `MaxResults` as a page bound, return an opaque `NextToken` when more remain and resume exactly from it (no repeats, no skips), and apply at least the `Identifier` and `LifeCycleStage`/`LifeCycleReviewStatus` filters.

No canned or echoed responses: every field above must come out of the store, and a create followed by a get/list must return what was created. `MaxResults`/`Catalog` pattern/enum violations must raise `ValidationException` (400).

## State Model

In `src/state.rs` (`ComAmazonawsPartnercentralsellingAwspartnercentralsellingState`, reached via `crate::state::service_state(&*self.engine)`):

- `opportunities: HashMap<(String /*Catalog*/, String /*Id, O…*/), Opportunity>`
- `Opportunity { id: String, arn: String, catalog: String, partner_opportunity_identifier: Option<String>, customer: Option<Customer>, project: Option<Project>, life_cycle: Option<LifeCycle>, opportunity_type: Option<OpportunityType>, origin: Option<OpportunityOrigin>, marketing: Option<Marketing>, software_revenue: Option<SoftwareRevenue>, primary_needs_from_aws: Vec<PrimaryNeedFromAws>, national_security: Option<NationalSecurity>, opportunity_team: Vec<Contact>, assignee: Option<AssigneeContact>, involvement_type: Option<SalesInvolvementType>, visibility: Option<Visibility>, related_entity_identifiers: RelatedEntityIdentifiers, tags: Vec<Tag>, created_date: SystemTime, last_modified_date: SystemTime }`
- `next_opportunity_seq: u64` — monotonic counter used to mint `O<seq>` ids
- `idempotency: HashMap<(String /*Catalog*/, String /*ClientToken*/), String /*Id*/>` — makes CreateOpportunity replay-safe
- `list_tokens: HashMap<String /*NextToken*/, usize /*offset into the sorted id list*/>` — or encode the offset in the opaque token; either way pagination must be resumable across calls

All other operations (Engagements, EngagementInvitations, ResourceSnapshots/Jobs, the Start*/List*Task families, Tagging, SellingSystemSettings, ListSolutions, GetAwsOpportunitySummary) stay at 501 for now and are tracked as individual gap issues.

## Evidence

OPERATIONS.md at the crate root lists 45 operations with protocol `awsJson1_0`, target prefix `AWSPartnerCentralSelling.`, signing name `partnercentral-selling`; the Opportunity ops are documented at lines 182-247 (CreateOpportunity requires `Catalog`+`ClientToken` and returns `Id` matching `^O[0-9]{1,19}$`; GetOpportunity requires `Catalog`+`Identifier` and must return required `Catalog`, `Id`, `CreatedDate`, `LastModifiedDate`, `RelatedEntityIdentifiers`; UpdateOpportunity requires `LastModifiedDate` and declares `ConflictException`; AssignOpportunity requires `Assignee`; SubmitOpportunity requires `InvolvementType`; ListOpportunities is `@paginated(inputToken=NextToken, outputToken=NextToken, items=OpportunitySummaries)` with `MaxResults` range 1..100). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-partnercentralselling-awspartnercentralselling/src/ops/` matches ALL 45 handler files, including create_opportunity.rs, get_opportunity.rs, update_opportunity.rs, assign_opportunity.rs, submit_opportunity.rs and list_opportunities.rs — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and touches no state. The only test present is the generated tests/state_roundtrip.rs; no behavioral coverage exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK PartnerCentral Selling client at it with catalog "Sandbox".
2. CreateOpportunity(Catalog="Sandbox", ClientToken="tok-1", Customer={Account:{CompanyName:"Acme"}}, Project={Title:"Migration"}, LifeCycle={Stage:"Prospect"}) → expect 200 with Id matching ^O[0-9]{1,19}$; today it returns HTTP 501 NotImplemented.
3. Repeat the identical CreateOpportunity with ClientToken="tok-1" → expect the SAME Id (idempotent replay), not a second opportunity.
4. GetOpportunity(Catalog="Sandbox", Identifier=<Id>) → expect the Customer/Project/LifeCycle exactly as created, plus Arn, CreatedDate, LastModifiedDate and RelatedEntityIdentifiers.
5. UpdateOpportunity(Catalog="Sandbox", Identifier=<Id>, LastModifiedDate=<value from Get>, Project={Title:"Migration v2"}) → expect 200 and a newer LastModifiedDate; re-send with the stale LastModifiedDate → expect ConflictException (409).
6. AssignOpportunity(Catalog="Sandbox", Identifier=<Id>, Assignee={Email:"rep@acme.test",FirstName:"Rep",LastName:"One",BusinessTitle:"AccountManager"}) then GetOpportunity → expect the assignee to be visible.
7. SubmitOpportunity(Catalog="Sandbox", Identifier=<Id>, InvolvementType="Co-Sell", Visibility="Full") then GetOpportunity → expect LifeCycle.ReviewStatus to show the submitted state.
8. Create two more opportunities, then ListOpportunities(Catalog="Sandbox", MaxResults=2) → expect exactly 2 summaries plus a NextToken; call again with that NextToken → expect the remaining opportunity and no repeats.
9. GetOpportunity(Catalog="Sandbox", Identifier="O999999999") → expect ResourceNotFoundException (404).
10. ListOpportunities(Catalog="Sandbox", MaxResults=0) → expect ValidationException (400).
