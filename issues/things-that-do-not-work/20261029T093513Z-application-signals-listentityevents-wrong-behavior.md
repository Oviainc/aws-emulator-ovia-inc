Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0184958fe7a8a2f43d9a0a5d46b055181f8d4624

# An `Entity` map that names THIS store's own account in `AwsAccountId` selects no change events, because the account id is matched as an entity attribute instead of through the crate's one account rule

_Filed: 2026-10-29T09:35:13Z_
_Operation: ListEntityEvents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0184958fe7a8a2f43d9a0a5d46b055181f8d4624`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEntityEvents.Entity` is documented to carry an `AwsAccountId` field ("specifies the account where this object is in"), and the modelled `Attributes` map admits it (key pattern `^[a-zA-Z]{1,50}$`, up to 4 entries, so `{Type, Name, Environment, AwsAccountId}` passes the boundary validator).

The handler never reads that entry through `crate::state::names_other_account` — the place the crate declares EVERY reader that takes an account id goes through. Instead the whole request map, `AwsAccountId` included, is handed to `entity_change_events`, which selects a record only when the record's own identity holds every entry the map states (`attributes_contain`). No stored identity ever carries an `AwsAccountId` entry: an instrumentation record's identity is built by `service_entity_attributes` from `Type`/`Name`/`Environment` only, and an SLO record's identity is its SLI `KeyAttributes`.

So an `Entity` naming the account this store holds answers an EMPTY `ChangeEvents` list, while the identical request without `AwsAccountId` answers the events — the exact opposite of the rule the crate states and of what the sibling audit read does with the very same fact. `ListAuditFindings` resolves its audited entity's account correctly: `audited_entity_attributes` builds the identity map from `Type`/`Name`/`Environment` and runs the account id through `names_other_account` separately. `ListEntityEvents` is the one reader of an account id that does neither, and it is the one reader missing from the suite that pins the rule.

## Evidence

THE MODEL MEMBER THAT CARRIES THE ACCOUNT — `smithy_query(query="doc", shape="ListEntityEventsInput", member="Entity")`:
```
This is a string-to-string map. It can include the following fields.
 ... <code>Environment</code> specifies the location where this object is hosted, or what it belongs to.
 ... <code>AwsAccountId</code> specifies the account where this object is in.
```
and `smithy_query(query="shape", shape="Attributes")`: `"length": {"min": 1, "max": 4}`, `"key": {"target": "KeyAttributeName", "pattern": "^[a-zA-Z]{1,50}$"}` — `AwsAccountId` is 12 letters, so the entry reaches the handler.

THE RULE THE CRATE STATES — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:565-580:
```
/// Whether an account id a request supplies names an account OTHER than the one this store
/// holds — the single place every account-scoped read of this service decides that.
...
/// Every reader that takes an account id — a list's own `AwsAccountId`, an SLO list's
/// `SloOwnerAwsAccountId`, the account an audited entity names, and the account field of a
/// `ResourceArn` (always present, so `Some`) — reads it through here, so the reads that must
/// agree about which account a request names cannot drift apart.
pub fn names_other_account(account: Option<&str>) -> bool {
    account.is_some_and(|account| account != ::engine::emulator_account_id())
}
```

THE SIBLING THAT APPLIES IT TO THE SAME KIND OF FACT — src/state.rs:3374-3394 (`audited_entity_attributes`, used by `ListAuditFindings`):
```
/// `None` for an entity that names NO attribute at all, since an empty map would match every
/// stored identity rather than one, and `None` for an entity in another account: ... so another
/// account's entity is one it holds nothing for ([`names_other_account`], the one reader of
/// that rule).
fn audited_entity_attributes(entity: &ServiceEntity) -> Option<HashMap<String, String>> {
    if names_other_account(entity.aws_account_id.as_deref()) {
        return None;
    }
    let mut attributes = HashMap::new();
    for (attribute, value) in [
        (TYPE_ATTRIBUTE, entity.r#type.as_deref()),
        (NAME_ATTRIBUTE, entity.name.as_deref()),
        (ENVIRONMENT_ATTRIBUTE, entity.environment.as_deref()),
    ] {
```
(the account is taken OUT of the identity and judged separately)

THE SIDE THAT SKIPS IT — src/ops/list_entity_events.rs:192-212 (the only two things the handler does with `input.entity`):
```
        if let Some(message) =
            crate::state::entity_type_refusal_at(crate::state::ENTITY_MEMBER_PATH, &input.entity)
        {
            return Err(refused(message));
        }
...
            store.expire_instrumentation_configurations(&now);
            store.entity_change_events(&input.entity, &input.start_time, &input.end_time)
```
(no `names_other_account` anywhere in the file: `grep -n names_other_account src/ops/list_entity_events.rs` returns nothing, while list_services.rs:194, list_service_states.rs:221, list_grouping_attribute_definitions.rs:180 and list_service_level_objectives.rs:208 each call it)

WHY THAT EMPTIES THE ANSWER — src/state.rs:5256-5259:
```
        let mut dated: Vec<(ChangeOrder<'store>, ChangeEvent)> = self
            .change_records()
            .into_iter()
            .filter(|(attributes, _)| attributes_contain(Some(attributes), entity))
```
over src/state.rs:1656-1658:
```
    filter
        .iter()
        .all(|(key, value)| candidate.get(key) == Some(value))
```
and the stored identities, which carry no account entry — src/state.rs:4989-4995:
```
pub fn service_entity_attributes(service: &str, environment: &str) -> HashMap<String, String> {
    HashMap::from([
        (TYPE_ATTRIBUTE.to_string(), SERVICE_ENTITY_TYPE.to_string()),
        (NAME_ATTRIBUTE.to_string(), service.to_string()),
        (ENVIRONMENT_ATTRIBUTE.to_string(), environment.to_string()),
    ])
}
```

TEST COVERAGE: tests/all/account_scoped_reads.rs:3-12 states the rule as universal — "the ONE account rule every account-scoped read of this service holds: an account id a request supplies selects this store's records when it names the account this store holds, and nothing when it names any other account" — and its `READERS` table (lines 144-150) drives `Services`, `ServiceStates`, `ServiceLevelObjectives`, `GroupingAttributeDefinitions` and `AuditFindings`. `ListEntityEvents` is absent from that table, so nothing pins the current behaviour; tests/all/entity_events.rs never sends an `AwsAccountId` entry.

## Steps to Reproduce

1. Start the emulator with this crate mounted and sign requests for this emulator's own account (engine::emulator_account_id()).
2. POST /slo with {"Name":"checkout-latency","SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"prod"},"OperationName":"POST /checkout","MetricType":"LATENCY"}}} — 200.
3. POST /events with {"Entity":{"Type":"Service","Name":"checkout","Environment":"prod"},"StartTime":<now-3600>,"EndTime":<now+3600>} — 200 with one CreateServiceLevelObjective change event.
4. POST /events again with the SAME body but one extra documented entry: "AwsAccountId":"<this emulator's own account id>" — observe 200 with ChangeEvents == [] instead of the same event, because the account id was matched as an entity attribute no stored record carries.
5. Contrast with every sibling reader of an account id: POST /service/states with "AwsAccountId":"<this account>" and GET /services?...&AwsAccountId=<this account> both answer exactly what naming no account answers (tests/all/account_scoped_reads.rs).
6. Contrast with the sibling that takes the account inside an entity: POST /auditFindings?StartTime=..&EndTime=.. with {"AuditTargets":[{"Type":"service","Data":{"Service":{"Type":"Service","Name":"checkout","Environment":"prod","AwsAccountId":"<this account>"}}}]} — answers the findings, because that path runs the account through names_other_account instead of matching it as an attribute.
