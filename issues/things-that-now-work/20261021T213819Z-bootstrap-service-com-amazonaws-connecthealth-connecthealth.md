Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the ConnectHealth (health-agent) emulator

_Filed: 2026-10-21T21:38:19Z_
_Operation: service-com-amazonaws-connecthealth-connecthealth | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 16 modeled operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, and the service holds no state. `StartMedicalScribeListeningSession` is not even wired (see `src/ops/SKIPPED_STUBS.md`: `@streaming` event-stream input).

This issue defines the FIRST, SMALL slice: the Domain -> Subscription lifecycle, implemented statefully over the shared engine store (`crate::state::service_state(&*self.engine)`), on the restJson1 wire protocol (sigv4 signing name `health-agent`). Implement exactly these 6 operations, in this order:

1. **CreateDomain** — `POST /domain` -> 201. Input: `name` (required, 1..64), `kmsKeyArn`, `webAppSetupConfiguration` (CreateWebAppConfiguration{ehrRole, idcInstanceId, idcRegion}), `tags`. Must mint a new `domainId` matching `^(hai-|dom-)[a-z0-9]+$` with length 20..25 (e.g. `dom-` + 16 lowercase alphanumerics), build `arn` as `arn:aws:health-agent:<region>:<account>:domain/<domainId>`, set `status=ACTIVE`, set `createdAt` to now, and PERSIST name/kmsKeyArn/tags/web-app config. When `webAppSetupConfiguration` is supplied, derive and store the output `webAppConfiguration` (WebAppConfiguration{ehrRole, idcApplicationId, idcRegion}) and a `webAppUrl`. Return the stored record, not the echoed input.
2. **GetDomain** — `GET /domain/{domainId}` -> 200. Must return the PERSISTED domain: `domainId`, `arn`, `name`, `kmsKeyArn`, `encryptionContext`, `status`, `webAppUrl`, `webAppConfiguration`, `createdAt`, `tags`. Unknown or already-deleted-and-purged id -> `ResourceNotFoundException` (HTTP 404).
3. **ListDomains** — `GET /domain` -> 200. Must return one `DomainSummary{domainId, arn, name, status, createdAt}` per persisted domain (required list, never a canned empty list once a domain exists), filtered by the `status` query parameter when present, honouring `maxResults` with an opaque `nextToken` that resumes exactly where the previous page stopped.
4. **CreateSubscription** — `POST /domains/{domainId}/subscriptions` -> 201. Must 404 (`ResourceNotFoundException`) when the domain does not exist. Otherwise mint `subscriptionId` matching `^sub-[a-zA-Z0-9]{21}$` (exactly 25 chars), build `arn` as `<domainArn>/subscription/<subscriptionId>`, set `status=INACTIVE`, `createdAt`/`lastUpdatedAt`=now, and store the subscription under its domain.
5. **ActivateSubscription / DeactivateSubscription** — `POST /domains/{domainId}/subscriptions/{subscriptionId}/activate|deactivate` -> 200. Must mutate the persisted subscription: activate sets `status=ACTIVE` and `activatedAt`=now; deactivate sets `status=INACTIVE` and `deactivatedAt`=now; both bump `lastUpdatedAt` and return the stored `SubscriptionDescription`. Unknown domain or subscription -> 404. (Both are `@idempotent`: repeating the call must not error.) `GetSubscription` / `ListSubscriptions` must observe the new status.
6. **DeleteDomain** — `DELETE /domain/{domainId}` -> 200. Must transition the persisted domain to `status=DELETED` (or remove it), return `domainId`, `arn`, `status`, and make a subsequent `GetDomain` report the deletion (404 once purged). Unknown id -> `ResourceNotFoundException` (404). `@idempotent`.

NO canned or echoed responses: every field above must be computed from the persisted record. A generated id that is not stored, a hardcoded `status`, or a `GetDomain` that reflects its own input are all failures of this issue. Also enforce the model's `@httpLabel` constraints — a `domainId` that fails the length/pattern constraint must produce `ValidationException` (HTTP 400), not a 404 or a 200.

## State Model

In `src/state.rs` (`ComAmazonawsConnecthealthConnecthealthState`, reached via `state.lock()`):

- `domains: BTreeMap<String /* domainId */, Domain>` where
  `Domain { domain_id: String, arn: String, name: String, kms_key_arn: Option<String>, encryption_context: Option<EncryptionContext>, status: DomainStatus /* ACTIVE | DELETING | DELETED */, web_app_url: Option<String>, web_app_configuration: Option<WebAppConfiguration{ehr_role, idc_application_id, idc_region}>, created_at: SystemTime, tags: BTreeMap<String,String>, subscriptions: BTreeMap<String /* subscriptionId */, Subscription> }`
- `Subscription { domain_id: String, subscription_id: String, arn: String, status: SubscriptionStatus /* ACTIVE | INACTIVE | DELETED */, created_at: SystemTime, last_updated_at: SystemTime, activated_at: Option<SystemTime>, deactivated_at: Option<SystemTime> }`
- `tags_by_arn: BTreeMap<String /* resourceArn */, BTreeMap<String,String>>` — the single tag store that CreateDomain seeds and that TagResource/UntagResource/ListTagsForResource will later share (keyed by ARN so domain and subscription ARNs both work).
- counters/seeds for id minting (`next_domain_seq`, RNG) so ids are unique per process and pattern-valid.

Insertion-ordered (or key-ordered) maps so `ListDomains`/`ListSubscriptions` paginate deterministically and a `nextToken` is a stable cursor.

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): `StartPatientInsightsJob`, `GetPatientInsightsJob`, `StartMedicalScribeListeningSession`, `GetMedicalScribeListeningSession`, `TagResource`, `UntagResource`, `ListTagsForResource`, and `ListSubscriptions`/`GetSubscription` pagination edge cases beyond the basics.

Done = a client script can create a domain, read it back with identical field values, see it in ListDomains, create + activate + deactivate a subscription under it, and delete the domain, with every response computed from the store.

## Evidence

OPERATIONS.md (crate root) lists all 16 operations with routes, input/output members and errors: CreateDomain `POST /domain -> 201` (output requires domainId/arn/name/status/createdAt), GetDomain `GET /domain/{domainId}` (ResourceNotFoundException 404), ListDomains `GET /domain` (@paginated inputToken=nextToken, items=domains, required `domains` list), CreateSubscription `POST /domains/{domainId}/subscriptions -> 201`, ActivateSubscription/DeactivateSubscription `POST .../activate|deactivate -> 200` returning `subscription` (SubscriptionDescription), DeleteDomain `DELETE /domain/{domainId}`. Shape detail from smithy_query: DomainSummary{domainId, arn, name, status, createdAt all required}; SubscriptionDescription{domainId, subscriptionId, arn, status, createdAt, lastUpdatedAt required; activatedAt, deactivatedAt optional}; WebAppConfiguration{ehrRole, idcApplicationId, idcRegion}. Current implementation: `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-connecthealth-connecthealth/src/ops/` matches all 15 generated handlers (create_domain.rs, get_domain.rs, list_domains.rs, create_subscription.rs, activate_subscription.rs, deactivate_subscription.rs, delete_domain.rs, ...), each returning `Scaffolded::not_implemented(..)` -> HTTP 501; `src/ops/SKIPPED_STUBS.md` records StartMedicalScribeListeningSession as modeled-but-unwired. No tests beyond the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/curl client at the `health-agent` (restJson1) endpoint.
2. POST /domain with {"name":"clinic-a"} — expect 201 with a fresh `domainId` matching ^(hai-|dom-)[a-z0-9]+$ (20..25 chars), an `arn` of the form arn:aws:health-agent:<region>:<account>:domain/<domainId>, `status`=ACTIVE and a `createdAt`; today returns HTTP 501 NotImplemented.
3. GET /domain/{domainId} — expect 200 with name="clinic-a" and the same domainId/arn/status/createdAt as the create response; today 501.
4. GET /domain — expect 200 with a `domains` list containing a DomainSummary for the created domain; today 501.
5. POST /domains/{domainId}/subscriptions — expect 201 with a `subscriptionId` matching ^sub-[a-zA-Z0-9]{21}$, status=INACTIVE; today 501.
6. POST /domains/{domainId}/subscriptions/{subscriptionId}/activate — expect 200 with subscription.status=ACTIVE and activatedAt set; then GET /domains/{domainId}/subscriptions/{subscriptionId} must report ACTIVE; today both 501.
7. POST /domains/{domainId}/subscriptions/{subscriptionId}/deactivate — expect 200 with status=INACTIVE and deactivatedAt set.
8. GET /domain/dom-doesnotexist00000 — expect ResourceNotFoundException with HTTP 404 (not 501, not 200).
9. DELETE /domain/{domainId} — expect 200 with status reflecting deletion, then GET /domain/{domainId} must no longer report an ACTIVE domain.
