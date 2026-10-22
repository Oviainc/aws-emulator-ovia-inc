Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the Amazon Connect Cases emulator

_Filed: 2026-10-21T21:58:09Z_
_Operation: service-com-amazonaws-connectcases-amazonconnectcases | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 43 operations in `services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/` are unmodified scaffolds: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the crate holds no state. Nothing works end-to-end.

Implement a FIRST, SMALL slice: the core case-management lifecycle of the primary resource (Domain -> Field -> Template -> Case), statefully, so a realistic restJson1 client script succeeds end-to-end. Every one of these must compute its response from persisted state in `ComAmazonawsConnectcasesAmazonconnectcasesState` reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back, no returning a fresh random id that is never stored.

### Happy path to implement (in order)

1. **CreateDomain** — `POST /domains`. Input: `name` (required, len 1..100, pattern `^.*[\S]$`). Must mint and STORE a `domainId` and `domainArn` (`arn:aws:cases:<region>:<account>:domain/<domainId>`), record `createdTime` and `domainStatus = "Active"`. Output: `domainId`, `domainArn`, `domainStatus` — all three required. A second CreateDomain with the same `name` must not silently clobber the first (`ConflictException` 409 is modeled).
2. **CreateField** — `POST /domains/{domainId}/fields`. Input: `domainId` (@httpLabel), `name`, `type` (`Boolean|DateTime|Number|SingleSelect|Text|Url|User`), optional `description`, `attributes`. Must store the field under the domain and return the stored `fieldId`/`fieldArn`. Unknown `domainId` -> `ResourceNotFoundException` (404), not a success.
3. **CreateTemplate** — `POST /domains/{domainId}/templates`. Input: `domainId` (@httpLabel), `name`, optional `description`, `layoutConfiguration`, `requiredFields` (list<RequiredField>), `status` (`Active|Inactive`). Must store the template (defaulting `status` to `Active`, `deleted=false`, `createdTime`/`lastModifiedTime`) and return `templateId`/`templateArn`. `requiredFields` referencing a field id that was never created in that domain must be rejected with `ValidationException` (400).
4. **CreateCase** — `POST /domains/{domainId}/cases`. Input: `domainId` (@httpLabel), `templateId` (required), `fields` (required list<FieldValue> = `{id, value: FieldValueUnion{stringValue|doubleValue|booleanValue|emptyValue|userArnValue}}`), optional `clientToken` (@idempotencyToken), `performedBy`, `tags`. Must verify the domain and template exist, PERSIST the field values exactly as given (preserving the union variant), stamp `createdTime`, and return a stored `caseId`/`caseArn`. Because the op is `@idempotent` with an `@idempotencyToken`, a replay with the same `clientToken` must return the SAME `caseId` rather than creating a second case.
5. **GetCase** — `POST /domains/{domainId}/cases/{caseId}`. Input: `caseId`, `domainId` (@httpLabel), `fields` (required list<FieldIdentifier>). Must read the persisted case and return ONLY the requested field ids as `fields` (list<FieldValue>, required) plus the case's stored `templateId` (required) and `tags`. Values must byte-for-byte match what CreateCase/UpdateCase last wrote; an id with no stored value should come back as `emptyValue`. Unknown `caseId` or `domainId` -> `ResourceNotFoundException` (404).
6. **UpdateCase** — `PUT /domains/{domainId}/cases/{caseId}`. Input: `domainId`, `caseId` (@httpLabel), `fields` (required), optional `performedBy`. Must MERGE the supplied field values into the stored case (overwriting same-id values, leaving untouched ids alone), bump `lastModifiedTime`, and return an empty body (200). The change must be visible to the very next GetCase. Unknown case -> 404.

Optionally round the slice out with **DeleteCase** (`DELETE /domains/{domainId}/cases/{caseId}`), which must remove the case so a following GetCase returns `ResourceNotFoundException` instead of the old data. Everything else (SearchCases, related items, case rules, layouts, list/tag ops, case event configuration) stays 501 and will be filed as individual gap issues later.

## State Model

In `src/state.rs`, behind the shared engine (one lockable store per service):

- `domains: HashMap<String /*domainId*/, Domain>`
  - `Domain { domain_id: String, domain_arn: String, name: String, created_time: SystemTime, domain_status: "Active", tags: HashMap<String,String>, fields: HashMap<String /*fieldId*/, Field>, templates: HashMap<String /*templateId*/, Template>, cases: HashMap<String /*caseId*/, Case>, event_bridge: Option<EventBridgeConfiguration> }`
- `Field { field_id, field_arn, name, field_type /*Boolean|DateTime|Number|SingleSelect|Text|Url|User*/, description: Option<String>, attributes: Option<FieldAttributes>, deleted: bool, created_time, last_modified_time }`
- `Template { template_id, template_arn, name, description: Option<String>, layout_configuration: Option<LayoutConfiguration>, required_fields: Vec<RequiredField>, status /*Active|Inactive*/, deleted: bool, tags, created_time, last_modified_time }`
- `Case { case_id, case_arn, domain_id, template_id, field_values: HashMap<String /*fieldId*/, FieldValueUnion>, tags: HashMap<String,String>, created_time, last_modified_time, performed_by: Option<UserUnion>, related_items: HashMap<String, RelatedItem> /*reserved for the next slice*/ }`
- `idempotency: HashMap<String /*clientToken*/, String /*caseId*/>` so `@idempotent` CreateCase replays resolve to the existing case.
- Counters/uuid source for minting ids, plus the account id/region used to build ARNs, so `caseArn`/`domainArn`/`templateArn` are stable and consistent with the ids actually stored.

Done = a restJson1 integration script walking steps 1-6 passes with full fidelity: the ids returned by each create are the ids later reads accept, GetCase echoes the persisted field values (not the request), UpdateCase's merge is observable, and unknown ids produce the modeled 404/400 errors rather than 200 or 501.

## Evidence

Scaffold census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-connectcases-amazonconnectcases/src/ops/` matches all 43 operation files (create_domain.rs, create_field.rs, create_template.rs, create_case.rs, get_case.rs, update_case.rs, ... ), each returning `Scaffolded::not_implemented(..)` => HTTP 501, so no operation holds or reads state. Contracts taken from the crate's generated `OPERATIONS.md` (restJson1, SDK id `ConnectCases`, sigv4 name `cases`): CreateDomain lines 177-183 (`name` -> `domainId`/`domainArn`/`domainStatus`, ConflictException 409); CreateField 232-238; CreateTemplate 327-333 and GetTemplate 335-341 (`status`, `deleted` default false, `createdTime`); CreateCase 50-56 (`templateId`+`fields` required, `clientToken` @idempotencyToken, ResourceNotFoundException 404, ConflictException 409); GetCase 58-64 (`fields` list<FieldIdentifier> in, `fields`+`templateId` required out, @readonly); UpdateCase 66-71; DeleteCase 73-79. Shape detail from smithy_query: `FieldValue{id required, value: FieldValueUnion required}`, `FieldValueUnion{stringValue|doubleValue|booleanValue|emptyValue|userArnValue}`, `FieldIdentifier{id required}`, `TemplateStatus=[Active,Inactive]`. No tests exist beyond the generated `tests/state_roundtrip.rs` state-API example, so nothing asserts any of this today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `cases` client at it (restJson1, signing name `cases`).
2. CreateDomain(name="bootstrap-domain") -> expect 200 with a domainId/domainArn and domainStatus="Active"; actual: HTTP 501 NotImplemented.
3. CreateField(domainId=<from step 2>, name="customer-note", type="Text") -> expect 200 with a stored fieldId/fieldArn.
4. CreateTemplate(domainId=<domainId>, name="bootstrap-template", status="Active", requiredFields=[{fieldId:<fieldId>}]) -> expect 200 with templateId/templateArn.
5. CreateCase(domainId=<domainId>, templateId=<templateId>, clientToken="tok-1", fields=[{id:<fieldId>, value:{stringValue:"hello"}}]) -> expect 200 with caseId/caseArn; repeat the identical call with clientToken="tok-1" and expect the SAME caseId.
6. GetCase(domainId=<domainId>, caseId=<caseId>, fields=[{id:<fieldId>}]) -> expect fields=[{id:<fieldId>, value:{stringValue:"hello"}}] and templateId=<templateId> read back from state.
7. UpdateCase(domainId=<domainId>, caseId=<caseId>, fields=[{id:<fieldId>, value:{stringValue:"updated"}}]) -> expect 200 empty body; then GetCase again and expect stringValue="updated".
8. GetCase with caseId="does-not-exist" -> expect ResourceNotFoundException (HTTP 404), not 501 and not 200.
9. DeleteCase(domainId=<domainId>, caseId=<caseId>) then GetCase -> expect ResourceNotFoundException (HTTP 404).
