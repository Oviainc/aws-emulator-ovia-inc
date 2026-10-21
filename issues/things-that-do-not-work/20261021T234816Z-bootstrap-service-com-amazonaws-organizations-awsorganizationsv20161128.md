Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9e4273988d175f89e8a017aa0737ee7107d62a89

# Bootstrap the Organizations emulator

_Filed: 2026-10-21T23:48:16Z_
_Operation: service-com-amazonaws-organizations-awsorganizationsv20161128 | Class: missing_

> _Note: this issue was identified against the code base at commit `9e4273988d175f89e8a017aa0737ee7107d62a89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 63 operations in this crate is an untouched scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and `src/state.rs` holds no Organizations data. Nothing works end-to-end.

Implement a FIRST, SMALL, stateful slice: the organization + organizational-unit hierarchy lifecycle. Wire protocol is awsJson1_1, `POST /` with `X-Amz-Target: AWSOrganizationsV20161128.<OperationName>` (OPERATIONS.md 'Wire protocol').

Happy path, in order (field names from OPERATIONS.md / the Smithy model):

1. **CreateOrganization** — input: optional `FeatureSet` (ALL | CONSOLIDATED_BILLING, default ALL). Must create and PERSIST the singleton organization and its implicit ROOT. Output `Organization` with `Id` (`^o-[a-z0-9]{10,32}$`), `Arn` (`arn:aws:organizations::<mgmt-acct>:organization/o-...`), `FeatureSet` as requested, `MasterAccountId`/`MasterAccountArn`/`MasterAccountEmail` for the calling (management) account, and the management account registered as a child of the root. A second call while an organization exists must fail with `AlreadyInOrganizationException` (409), not succeed.
2. **DescribeOrganization** — no input. Must return the SAME `Organization` struct that CreateOrganization persisted (same `Id`, `Arn`, `FeatureSet`, master-account fields). When no organization exists, `AWSOrganizationsNotInUseException` (404).
3. **ListRoots** — returns `Roots: [Root{Id (^r-[0-9a-z]{4,32}$), Arn, Name ("Root"), PolicyTypes}]` for the stored organization, honoring `MaxResults` (1..20) and `NextToken`. 404 `AWSOrganizationsNotInUseException` when no org.
4. **CreateOrganizationalUnit** — input `ParentId` (root id or ou id, required), `Name` (required), optional `Tags`. Must persist a new OU under that parent and return `OrganizationalUnit{Id (^ou-<root-suffix>-[a-z0-9]{8,32}$), Arn, Name, Path}`. Unknown parent -> `ParentNotFoundException` (404); duplicate `Name` under the same parent -> `DuplicateOrganizationalUnitException` (409).
5. **ListOrganizationalUnitsForParent** — input `ParentId` (required), `MaxResults`, `NextToken`. Must list exactly the OUs persisted under that parent (so step 4's OU appears, with the identical `Id`/`Name`/`Arn`), paginating by `MaxResults` with an opaque `NextToken`. Unknown parent -> `ParentNotFoundException` (404).
6. **DeleteOrganizationalUnit** — input `OrganizationalUnitId` (required). Must remove the OU from state so the following `ListOrganizationalUnitsForParent` / `DescribeOrganizationalUnit` no longer see it. Unknown id -> `OrganizationalUnitNotFoundException` (404); OU that still has child OUs or accounts -> `OrganizationalUnitNotEmptyException` (409).

Also implement **DescribeOrganizationalUnit** (input `OrganizationalUnitId`, output the stored `OrganizationalUnit`, 404 `OrganizationalUnitNotFoundException` after delete) if it falls out for free from the same store — it is the natural read of step 4.

NEVER STUBS: every one of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)`. No canned ids, no echoing the input back without storing it, no random id returned without a record. A create followed by a describe/list must return exactly what was created, and a delete must be observable by the next read. Every one of these ops must also return `AWSOrganizationsNotInUseException` (404) when no organization has been created.

## State Model

In `src/state.rs` (the single shared per-service store, guarded by `state.lock()`):

- `organization: Option<Organization>` — the singleton org: `{ id: String ("o-..."), arn: String, feature_set: OrganizationFeatureSet, master_account_id: String, master_account_arn: String, master_account_email: String, available_policy_types: Vec<PolicyTypeSummary> }`. `None` means "not in use" -> every op 404s with `AWSOrganizationsNotInUseException`.
- `roots: Vec<Root>` — `{ id: String ("r-..."), arn: String, name: String, policy_types: Vec<PolicyTypeSummary> }`; exactly one entry created by CreateOrganization.
- `organizational_units: HashMap<String /* ou id */, OrganizationalUnitRecord>` where `OrganizationalUnitRecord { id, arn, name, parent_id, path, tags: Vec<Tag> }`.
- `children: HashMap<String /* parent id: root or ou */, Vec<ChildRef>>` where `ChildRef { id: String, child_type: ORGANIZATIONAL_UNIT | ACCOUNT }` — the hierarchy edges that ListOrganizationalUnitsForParent, the not-empty check in DeleteOrganizationalUnit, and (later) ListChildren/ListParents read.
- `accounts: HashMap<String /* 12-digit id */, AccountRecord>` — seeded with the management account at CreateOrganization so `MasterAccountId` and the root's child list are real; fields `{ id, arn, email, name, status, joined_method, joined_timestamp }`.
- id generators: monotonic/random suffix helpers producing ids that satisfy the modeled patterns (`o-`, `r-`, `ou-<root-suffix>-`), stored so they are stable across calls.

The remaining 56 operations (accounts, policies, handshakes, tags, delegated administrators, responsibility transfers, service access) stay at 501 for now and are filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate: 63 operations, protocol awsJson1_1, target prefix `AWSOrganizationsV20161128`; CreateOrganization (input `FeatureSet`, output `Organization`, errors incl. `AlreadyInOrganizationException` 409), DescribeOrganization (no input, output `Organization`, `AWSOrganizationsNotInUseException` 404), ListRoots (`MaxResults` 1..20, `NextToken`, output `Roots`), CreateOrganizationalUnit (`ParentId`+`Name` required, `ParentNotFoundException` 404, `DuplicateOrganizationalUnitException` 409), ListOrganizationalUnitsForParent (@paginated, `ParentId` required), DeleteOrganizationalUnit (`OrganizationalUnitId` required, `OrganizationalUnitNotFoundException` 404, `OrganizationalUnitNotEmptyException` 409). smithy_query on shapes Organization / OrganizationalUnit / Root gives the exact output members and id/ARN patterns cited above. All handler files under src/ops/ match `^//! SCAFFOLDED, NOT IMPLEMENTED:` (grep: 63/63 files), so no behavior exists yet and no test asserts any of this.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSOrganizationsV20161128.CreateOrganization and body {"FeatureSet":"ALL"} -> expect 200 with Organization.Id matching ^o-[a-z0-9]{10,32}$; today: 501 NotImplemented.
2. POST / DescribeOrganization with {} -> expect the identical Organization (same Id, Arn, FeatureSet, MasterAccountId) returned from persisted state.
3. POST / ListRoots with {} -> expect exactly one Root whose Id matches ^r-[0-9a-z]{4,32}$.
4. POST / CreateOrganizationalUnit with {"ParentId":"<root id>","Name":"Sandbox"} -> expect OrganizationalUnit with a persisted ou- id and Name "Sandbox".
5. POST / ListOrganizationalUnitsForParent with {"ParentId":"<root id>"} -> expect the "Sandbox" OU with the same Id/Arn as step 4; repeat with {"MaxResults":1} after creating a second OU and expect a NextToken that pages the rest.
6. POST / CreateOrganizationalUnit again with the same ParentId and Name "Sandbox" -> expect 409 DuplicatePolicyAttachment-style DuplicateOrganizationalUnitException, not a second OU.
7. POST / DeleteOrganizationalUnit with {"OrganizationalUnitId":"<ou id>"} -> expect 200, then ListOrganizationalUnitsForParent no longer lists it and DescribeOrganizationalUnit on that id returns 404 OrganizationalUnitNotFoundException.
8. Against a server with no organization created, call DescribeOrganization -> expect 404 AWSOrganizationsNotInUseException rather than 501 or an empty 200.
