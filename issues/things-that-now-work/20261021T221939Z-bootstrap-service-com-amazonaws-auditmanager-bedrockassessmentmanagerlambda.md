Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7f122c3e96febe26bb2311916e16539819a4dcf1

# Bootstrap the AuditManager (bedrockassessmentmanagerlambda) emulator

_Filed: 2026-10-21T22:19:39Z_
_Operation: service-com-amazonaws-auditmanager-bedrockassessmentmanagerlambda | Class: stub_

> _Note: this issue was identified against the code base at commit `7f122c3e96febe26bb2311916e16539819a4dcf1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 62 operations in `services/service-com-amazonaws-auditmanager-bedrockassessmentmanagerlambda/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this emulator. Implement a FIRST, SMALL slice: the assessment lifecycle, the service's primary resource (restJson1, sdkId `AuditManager`, signing name `auditmanager`).

### Happy path to implement (in order)

1. **CreateAssessment** — `POST /assessments` -> 200. Input: `name` (required, 1..300, `^[^\\]*$`), `description` (0..1000), `assessmentReportsDestination` (AssessmentReportsDestination, required), `scope` (Scope, required), `roles` (list<Role>, required), `frameworkId` (UUID, required), `tags` (map, 0..50). Output: `assessment` (Assessment). Must mint a fresh lowercase-hex UUID matching `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, build `arn` as `arn:aws:auditmanager:<region>:<account>:assessment/<id>` (model constrains it to `^arn:.*:auditmanager:`), set `metadata.status = ACTIVE`, set `metadata.creationTime`/`metadata.lastUpdated`, echo the stored `name`/`description`/`scope`/`roles`/`assessmentReportsDestination` back from the stored record (not from the request object), and STORE it. `frameworkId` that names no known framework must be `ResourceNotFoundException` (404), so seed at least one framework id (or accept any well-formed id and record it on `assessment.framework.id` — but be consistent with GetAssessmentFramework if that is later implemented).
2. **GetAssessment** — `GET /assessments/{assessmentId}` -> 200. Output: `assessment` (Assessment), `userRole` (Role). Must return the PERSISTED assessment created in step 1 — same id, arn, name, description, status, scope, roles, tags, creationTime — not a synthesized one. Unknown but well-formed id => `ResourceNotFoundException` (404).
3. **ListAssessments** — `GET /assessments` -> 200, `@paginated(inputToken=nextToken, outputToken=nextToken)`. Input: `status` (ACTIVE|INACTIVE, query), `maxResults` (1..1000, query), `nextToken` (query). Output: `assessmentMetadata` (list<AssessmentMetadataItem>), `nextToken`. Must project every stored assessment into an AssessmentMetadataItem (`id`, `name`, `complianceType`, `status`, `roles`, `delegations`, `creationTime`, `lastUpdated`), filter by `status` when supplied, honour `maxResults` by truncating the page, and emit an opaque `nextToken` (base64, matching `^[A-Za-z0-9+/=]*$`) only when more items remain — resuming from that token must return the remaining items with no duplicates and no `nextToken` on the last page.
4. **UpdateAssessment** — `PUT /assessments/{assessmentId}` -> 200. Mutate the stored record's `name`, `description`, `scope`, `assessmentReportsDestination`, bump `metadata.lastUpdated`, and return the updated `assessment`. A following GetAssessment must observe the new values.
5. **UpdateAssessmentStatus** — `PUT /assessments/{assessmentId}/status` -> 200. Input: `assessmentId` (label), `status` (ACTIVE|INACTIVE, required). Must flip the stored `metadata.status`, bump `lastUpdated`, and return the stored `assessment`. After setting INACTIVE, `ListAssessments?status=ACTIVE` must no longer include it and `ListAssessments?status=INACTIVE` must.
6. **DeleteAssessment** — `DELETE /assessments/{assessmentId}` -> 200, empty body. Must remove the record; a subsequent GetAssessment must return `ResourceNotFoundException` (404) and ListAssessments must not include it. Deleting an already-deleted id => 404.

Every one of these six must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` — NO canned bodies, NO echoing the request struct straight back, NO minting an id that is never stored. A create followed by a read must return what was created; a delete followed by a read must 404.

Also delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each of the six handler files as they are implemented, since that line is the stub census.

## State Model

In `src/state.rs`, on the service state struct:

- `assessments: HashMap<String /* assessmentId (UUID) */, Assessment>` where `Assessment { id, arn, aws_account: AWSAccount{ id, emailAddress, name }, metadata: AssessmentMetadata, framework: AssessmentFramework, tags: HashMap<String,String> }`.
- `AssessmentMetadata { name, id, description, compliance_type, status: AssessmentStatus (ACTIVE|INACTIVE), assessment_reports_destination: AssessmentReportsDestination{ destinationType, destination }, scope: Scope{ awsAccounts, awsServices }, roles: Vec<Role{ roleType, roleArn }>, delegations: Vec<Delegation>, creation_time: SystemTime, last_updated: SystemTime }`.
- `frameworks: HashMap<String /* frameworkId */, Framework>` — at minimum the id/name/complianceType/controlSets needed to validate `CreateAssessment.frameworkId` and to populate `assessment.framework`.
- A monotonic `list_tokens`/offset encoding for pagination: `nextToken` = base64 of the index (or the last-seen assessment id) into the stable `assessments` ordering used by ListAssessments (sort by `creationTime`, then `id`, so paging is deterministic).

Everything else in the service (frameworks CRUD, controls, evidence, evidence folders, assessment reports, delegations, insights, share requests, settings, account registration, tagging) stays 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-auditmanager-bedrockassessmentmanagerlambda/OPERATIONS.md` lines 1-24 (62 operations, restJson1, sdkId AuditManager, endpoint prefix `auditmanager`) and the per-operation contracts cited above: CreateAssessment (lines 68-73), DeleteAssessment (96-101), GetAssessment (159-164), ListAssessments (315-321), UpdateAssessmentStatus (448-453), UpdateAssessment (413-419). Shape members for the state model come from `smithy_query shape=Assessment,AssessmentMetadata,AssessmentMetadataItem` (arn/awsAccount/metadata/framework/tags; metadata: name,id,description,complianceType,status,assessmentReportsDestination,scope,roles,delegations,creationTime,lastUpdated). Stub proof: `src/ops/create_assessment.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 187 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateAssessment"))`; all 62 op files are the same scaffold. No tests assert any assessment behaviour (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. POST /assessments with {name:"demo-assessment", frameworkId:<a framework uuid>, assessmentReportsDestination:{destinationType:"S3",destination:"s3://bucket/prefix"}, scope:{awsAccounts:[{id:"123456789012"}],awsServices:[]}, roles:[{roleType:"PROCESS_OWNER",roleArn:"arn:aws:iam::123456789012:role/owner"}], tags:{env:"test"}} — expect 200 with assessment.metadata.id a fresh uuid, metadata.status=ACTIVE, arn beginning arn:aws:auditmanager:, name/scope/roles echoed from storage (today: 501 NotImplemented)
2. GET /assessments/{assessmentId} with the returned id — expect 200 and the SAME name, description, arn, status, scope, roles, tags, creationTime that CreateAssessment returned
3. GET /assessments?status=ACTIVE — expect 200 and assessmentMetadata containing exactly one item whose id/name/status match the created assessment
4. GET /assessments?maxResults=1 with two assessments created — expect 1 item plus a nextToken; GET /assessments?maxResults=1&nextToken=<token> returns the other item and no nextToken
5. PUT /assessments/{assessmentId} with a new name/description — expect 200, then GET /assessments/{assessmentId} shows the new values and a later lastUpdated
6. PUT /assessments/{assessmentId}/status with {status:"INACTIVE"} — expect 200 with metadata.status=INACTIVE; GET /assessments?status=ACTIVE no longer lists it, GET /assessments?status=INACTIVE does
7. DELETE /assessments/{assessmentId} — expect 200 empty body; GET /assessments/{assessmentId} now returns 404 ResourceNotFoundException and GET /assessments omits it
8. GET /assessments/00000000-0000-0000-0000-000000000000 (never created) — expect 404 ResourceNotFoundException, not 501 and not 200
