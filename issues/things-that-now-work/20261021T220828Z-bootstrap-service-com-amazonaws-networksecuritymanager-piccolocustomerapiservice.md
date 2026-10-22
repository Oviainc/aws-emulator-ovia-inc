Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the service-com-amazonaws-networksecuritymanager-piccolocustomerapiservice emulator

_Filed: 2026-10-21T22:08:28Z_
_Operation: service-com-amazonaws-networksecuritymanager-piccolocustomerapiservice | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `src/ops/` is an unmodified scaffold (`//! SCAFFOLDED, NOT IMPLEMENTED:` + HTTP 501 `NotImplemented`) and the service holds no state, so no client workflow works at all. Implement a FIRST minimal slice: the stateful lifecycle of the service's primary resource, the **Policy** (restJson1, signing name `network-security-manager`).

### Happy path to implement (in order)

1. **CreatePolicy** — `POST /policies -> 201`. Required input: `policyName` (1..128, `^[a-zA-Z0-9][a-zA-Z0-9 _.:/=+\-@]*$`), `priority` (integer >= 1), `firewallType` (`SHIELD_ADVANCED|WAF`), `policyConfiguration` (PolicyConfiguration), `isPublished` (boolean, default true); optional `policyDescription`, `associatedTemplateAndRuleList` (list<TemplateOrRuleReference>, 0..100), `tags` (map, <=200), `clientToken` (@idempotencyToken). Must MINT and PERSIST: `policyId` matching `^[a-z0-9]{1,64}$`, `policyArn` matching the modeled ARN pattern (`arn:aws:network-security-manager:<region>:<account>:policy/<policyId>`), `version` starting at `"1"`, an `updateToken` UUID (`^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$`), `status` = `ACTIVE` when `isPublished` is true else `DRAFT`, `hasPublishedVersion`, `isSnapshot=false`, `updatedAt`, and the echo-back of the stored name/description/priority/firewallType/policyConfiguration, with `associatedTemplateAndRuleList` returned as the required list<AssociatedTemplateOrRule>. Reusing the same `clientToken` with identical input must return the SAME policyId (idempotent), not a new one.
2. **GetPolicy** — `GET /policies/{policyIdentifier} -> 200`. `policyIdentifier` accepts either the policyId or the policyArn. Must return the full persisted record created in step 1 (same id/arn/name/description/priority/firewallType/policyConfiguration/version/updateToken/status). Unknown identifier -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListPolicies** — `GET /policies -> 200`. Must return one `PolicySummary` per persisted policy in `policies` (required member, always present even when empty), honour `maxResults` (1..100) by truncating and emitting an opaque `nextToken` (1..4096, `^[0-9A-Za-z:/+=_-]+$`) that resumes exactly where the previous page stopped, and filter by the `status` query param (`ACTIVE|DISABLED|DRAFT`).
4. **UpdatePolicy** — `PATCH /policies/{policyIdentifier} -> 200`. Requires `updateToken` and `isPublished`; optional `policyDescription`, `priority`, `associatedTemplateAndRuleList`, `policyConfiguration`. Must apply the provided fields to the stored record (absent fields unchanged), bump `version` and MINT A NEW `updateToken`, refresh `updatedAt`, and recompute `status` from `isPublished`. A stale/mismatched `updateToken` must fail with `ConflictException` (HTTP 409); unknown identifier -> `ResourceNotFoundException` (404). A following GetPolicy must observe the update.
5. **DeletePolicy** — `DELETE /policies/{policyIdentifier} -> 204` (empty body, @idempotent). Must remove the record from state; a following GetPolicy must return `ResourceNotFoundException` (404) and ListPolicies must no longer include it. Deleting an already-absent policy must stay successful (204) per the @idempotent trait.

No canned, hardcoded or echo-only responses: every one of these five responses must be computed from the persisted store (a random id returned without storing it, or a GetPolicy that echoes its input, counts as a failure of this issue). Input validation already enforced by the generated model layer (length/pattern/range) should surface as `ValidationException` (400).

## State Model

In `src/state.rs`, on the service state struct reached via `crate::state::service_state(&*self.engine)`:

- `policies: BTreeMap<String /* policyId */, Policy>` — insertion-ordered-by-key store, the single source of truth.
- `struct Policy { policy_id: String, policy_arn: String, policy_name: String, policy_description: Option<String>, status: EntityStatus /* ACTIVE|DISABLED|DRAFT */, priority: i32, firewall_type: PolicyFirewallType /* SHIELD_ADVANCED|WAF */, policy_configuration: Option<PolicyConfiguration>, associated_template_and_rule_list: Vec<AssociatedTemplateOrRule>, version: u32 /* rendered as "1","2",... */, update_token: String /* UUID, rotated on every mutation */, is_snapshot: bool, has_published_version: bool, tags: BTreeMap<String,String>, created_at: SystemTime, updated_at: SystemTime }`
- `policy_name_index: BTreeMap<String /* policyName */, String /* policyId */>` — for name collision detection (`ConflictException`) and arn/name lookup from `policyIdentifier`.
- `idempotency: BTreeMap<String /* clientToken */, String /* policyId */>` — makes repeated CreatePolicy/UpdatePolicy with the same token return the first result.
- `next_policy_seq: u64` — monotonic counter feeding the lowercase-alphanumeric `policyId` generator.

Out of scope for this first slice (stay 501, filed separately later): Rule/Scope/Template/Deployment lifecycles, all `Create*Snapshot`/`List*Snapshots`, `GenerateRuleConfiguration`, admin-account ops, tagging ops, and the resource/synchronization listing ops.

## Evidence

`services/service-com-amazonaws-networksecuritymanager-piccolocustomerapiservice/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-networksecuritymanager-piccolocustomerapiservice.json`) lists 46 operations, restJson1, sdkId `Network Security Manager`. Policy routes and shapes cited above come from its CreatePolicy (line 169: `POST /policies -> 201`, @idempotent), GetPolicy (line 177: `GET /policies/{policyIdentifier} -> 200`, @readonly, declares ResourceNotFoundException 404), UpdatePolicy (line 185: `PATCH ... -> 200`, required `updateToken`, declares ConflictException 409), DeletePolicy (line 193: `DELETE ... -> 204`, @idempotent), ListPolicies (line 201: @paginated(inputToken=nextToken, outputToken=nextToken, items=policies), required `policies`) sections. All policy handlers are confirmed unmodified scaffolds: `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/*policy*.rs` matches create_policy.rs, get_policy.rs, update_policy.rs, delete_policy.rs, create_policy_snapshot.rs, list_policy_snapshots.rs — i.e. every one answers HTTP 501 and persists nothing. No tests exercise policy state (only the generated `tests/state_roundtrip.rs` example).

## Steps to Reproduce

1. Start the emulator and point an AWS client at the `network-security-manager` endpoint.
2. POST /policies with {"policyName":"demo-policy","priority":1,"firewallType":"WAF","policyConfiguration":{...},"isPublished":true} — expect HTTP 201 with a minted policyId/policyArn, version "1", an updateToken UUID and status ACTIVE; today it returns HTTP 501 NotImplemented.
3. GET /policies/{policyId} — expect HTTP 200 echoing the persisted record (name, priority, firewallType, policyConfiguration, version, updateToken).
4. GET /policies — expect HTTP 200 with the new policy present in `policies`; repeat with ?maxResults=1 and follow nextToken to page through two created policies without duplicates or omissions.
5. PATCH /policies/{policyId} with the updateToken from the GET plus {"policyDescription":"updated","priority":5,"isPublished":true} — expect HTTP 200 with version bumped to "2" and a NEW updateToken; replaying the PATCH with the now-stale updateToken must return ConflictException (409).
6. GET /policies/{policyId} again — expect the updated description/priority/version, proving state persisted across calls.
7. DELETE /policies/{policyId} — expect HTTP 204 with an empty body.
8. GET /policies/{policyId} — expect ResourceNotFoundException (HTTP 404), and GET /policies must no longer list the policy.
