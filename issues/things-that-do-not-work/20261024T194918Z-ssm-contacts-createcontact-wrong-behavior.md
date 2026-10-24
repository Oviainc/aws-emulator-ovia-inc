Issue-Severity: CRITICAL
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# Default account is never onboarded, so every replication-set-gated SSM Contacts call answers 400 with no wire path to fix it

_Filed: 2026-10-24T19:49:18Z_
_Operation: CreateContact | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Out of the box the emulator refuses essentially the whole SSM Contacts API. `ComAmazonawsSsmcontactsSsmcontactsState` derives `Default` with `replication_set: None` (src/state.rs:284-293), and almost every handler evaluates `require_active_replication_set()` / `require_active_replication_set_for()` as its first step (CreateContact, CreateContactChannel, CreateRotation, CreateRotationOverride, Get*/Describe*, Delete*, Update*, Tag*/Untag*, PutContactPolicy/GetContactPolicy, StartEngagement/StopEngagement, AcceptPage, ListContactChannels, ListRotations, ListRotationOverrides, ListTagsForResource, ActivateContactChannel/DeactivateContactChannel/SendActivationCode). With no active set every one of those answers HTTP 400 `ValidationException` (`Invalid value provided - Account not found for the request`, or `Invalid value provided - Invalid resource Arn <arn>`).

Nothing in ANY emulated service writes that field: `grep` over `services/**` shows `replication_set` is only ever assigned from test code, and the one AWS API that mints a set — `ssm-incidents` `CreateReplicationSet` — is implemented to refuse every call unconditionally as deprecated (services/service-com-amazonaws-ssmincidents-ssmincidents/src/ops/create_replication_set.rs:324-331). `ssm-incidents`' own `replication_sets` map is also never inserted into, so `GetReplicationSet` can never report one either. The result: a client driving the emulator over HTTP (SDK/CLI) can never reach a single contact, channel, rotation, engagement or page operation — the CRUD lifecycle and every realistic workflow are unreachable, while real AWS (and any trace recorded against a real, onboarded account) answers 200.

Expected: an emulated account must be able to use the service — either the store starts with an ACTIVE replication set for the emulated account (what a recording-backed account looks like), or there is an over-the-wire way to onboard that the gate reads (e.g. the ssm-incidents replication-set operations writing the state the contacts gate consults, i.e. cross-service propagation). Actual: the gate can only be satisfied by in-process state mutation from a Rust test, so over HTTP the service is permanently closed.

Secondary symptom of the same root cause: the gate is also applied inconsistently across account-wide listings — `ListRotations` is gated (src/ops/list_rotations.rs:194-196) while `ListContacts` (src/ops/list_contacts.rs) and `ListEngagements` (src/ops/list_engagements.rs) are not, so two listings with no ARN subject answer differently on the same un-onboarded account. At most one of those can match the real service.

## Evidence

Spec/model: every operation listed declares `ValidationException` (HTTP 400), so the refusal is representable — but the modeled happy path (CreateContact -> ContactArn, GetContact, ListContacts, StartEngagement, ...) is unreachable. Handler evidence: src/state.rs:284-293 (`#[derive(Debug, Default)] ... pub replication_set: Option<AccountReplicationSet>` = `None` = 'an account that was never onboarded'), src/state.rs:390-409 (the gate), and the first statements of each gated handler. Cross-service evidence: services/service-com-amazonaws-ssmincidents-ssmincidents/src/ops/create_replication_set.rs:324-331 refuses every CreateReplicationSet as deprecated. Test coverage: core/router/tests/all/ssm_contacts_replication_set.rs asserts exactly this — CreateContact is 400 over the router, CreateReplicationSet is 400, and the suite then onboards by writing `engine.state::<ComAmazonawsSsmcontactsSsmcontactsState>().lock().replication_set = Some(...)` in-process, i.e. the only onboarding path is not available to a client.

## Steps to Reproduce

1. Start the emulator with a fresh in-memory engine (no test-only state mutation).
2. POST / with `X-Amz-Target: SSMContacts.CreateContact` and body {"Alias":"oncall-primary","Type":"PERSONAL","Plan":{"Stages":[{"DurationInMinutes":5,"Targets":[]}]}}.
3. Observe HTTP 400 ValidationException 'Invalid value provided - Account not found for the request' instead of a 200 carrying a ContactArn.
4. Try to onboard over the wire: POST /createReplicationSet against ssm-incidents — it answers 400 (deprecated), so the gate can never be satisfied.
5. Repeat with GetContact, ListContactChannels, CreateRotation, StartEngagement, TagResource: all 400 for the same reason.
6. Note the inconsistency: ListContacts and ListEngagements answer 200 on that same account while ListRotations answers 400.
