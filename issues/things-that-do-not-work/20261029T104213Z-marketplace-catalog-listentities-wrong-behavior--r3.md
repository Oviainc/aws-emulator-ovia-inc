Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6bb9010ea97ecc711aeaca7897e143f375b8cacc

# Per-type EntitySummary members are copied verbatim out of a change's details, outside the enums, lengths and patterns the model declares for them

_Filed: 2026-10-29T10:42:13Z_
_Operation: ListEntities | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6bb9010ea97ecc711aeaca7897e143f375b8cacc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`EntityRecord::to_summary` — the projection `ListEntities` answers with — fills `EntitySummary$Name` and every member of the per-type summaries (`AmiProductSummary$ProductTitle`, `OfferSummary$State`, `OfferSummary$ReleaseDate`, `OfferSummary$OfferSetId`, `ExpressPrivateOfferConfigurationSummary$State`, …) by copying whatever STRING the applied change's details carried at the member of that name. Nothing on the write path or the read path judges those values against the member's own modelled contract, so a modelled-valid `StartChangeSet` makes `ListEntities` answer 200 with summary members the model says cannot hold them:

* `{"ProductTitle": ""}` -> `EntitySummary$Name` and `AmiProductSummary$ProductTitle` are the empty string, though both are `@length(min: 1, …)`; the empty string also fails `EntityNameString`'s `@pattern`, which requires a non-whitespace first character.
* `{"ProductTitle": "<300 chars>"}` -> both members exceed their `@length(max: 255)`.
* `{"State": "bogus"}` on an `Offer` -> `OfferSummary$State` reports a value outside the `OfferStateString` enum (`Draft | Released`).
* `{"ReleaseDate": "tomorrow"}` -> `OfferSummary$ReleaseDate` reports a value outside `DateTimeISO8601` (`@length(20, 20)` plus a strict pattern).

This is the crate contradicting its own rule rather than an unwritten feature: the one sibling attribute it DOES read into a record field, `Visibility`, is judged twice — `details_defects` fails the change set for a `Visibility` outside the published value set, and `detail_visibility` drops any value outside it — while the name member beside it is checked only for being a string, and the remaining summary members are not checked at all. The result is a response a client validating against the model cannot read.

## Evidence

Model constraints on the reported members (smithy_query query="shape"):
- `"EntityNameString": {"type": "string", "length": {"min": 1, "max": 255}, "pattern": "^\\S+[\\S\\s]*$"}` (the target of `EntitySummary$Name`)
- `"AmiProductSummary": {"ProductTitle": {"target": "AmiProductTitleString", "length": {"min": 1, "max": 255}, "pattern": "^(.)+$"}}`
- `"OfferSummary": {"State": {"target": "OfferStateString", "type": "enum"}, "ReleaseDate": {"target": "DateTimeISO8601", "length": {"min": 20, "max": 20}, "pattern": "^([\\d]{4})\\-…Z$"}, "OfferSetId": {"target": "OfferSetIdString", "length": {"min": 1, "max": 50}, "pattern": "^offerset-[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$"}}` and `"OfferStateString": {"type": "enum", "values": ["Draft", "Released"]}`

What the handler reports — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/list_entities.rs:410-411`:
```
        let summaries: Vec<EntitySummary> =
            matched.iter().map(|record| record.to_summary()).collect();
```
and `src/state.rs:919-925` / `:959-972`:
```
    pub fn to_summary(&self) -> EntitySummary {
        EntitySummary {
            name: self.name.clone(),
...
            offer_summary: self.is_type("Offer").then(|| OfferSummary {
                name: self.name.clone(),
                product_id: self.detail_text("ProductId"),
                resale_authorization_id: self.detail_text("ResaleAuthorizationId"),
                release_date: self.detail_text("ReleaseDate"),
```
with `src/state.rs:1013-1015` reading any string at all:
```
    pub fn detail_text(&self, member: &str) -> Option<String> {
        self.details.get(member)?.as_str().map(str::to_string)
    }
```

Where the name is taken from the submitted details — `src/state.rs:2428-2433`:
```
fn detail_name(members: &Map<String, Value>) -> Option<String> {
    ENTITY_NAME_DETAIL_MEMBERS
        .iter()
        .filter_map(|member| members.get(*member))
        .find_map(|value| value.as_str().map(str::to_string))
}
```
and stored unchanged — `src/state.rs:1827-1833` (`apply_changes`):
```
                if let Some(Value::Object(members)) = details {
                    if let Some(name) = detail_name(&members) {
                        entry.name = Some(name);
                    }
                    if let Some(visibility) = detail_visibility(&members) {
                        entry.visibility = visibility;
                    }
```

The sibling rule this path skips, in the SAME helper — `src/state.rs:2484-2502` (`details_defects`):
```
    for member in ENTITY_NAME_DETAIL_MEMBERS {
        if let Some(value) = members.get(*member) {
            if !value.is_string() {
                defects.push(invalid_detail_value(member, "a string", value));
            }
        }
    }
    if let Some(value) = members.get(ENTITY_VISIBILITY_DETAIL_MEMBER) {
        if value
            .as_str()
            .is_none_or(|text| !ENTITY_VISIBILITY_VALUES.contains(&text))
        {
            defects.push(invalid_detail_value(
```
(the name member is judged only for its TYPE, the visibility member for its published VALUE SET) and `src/state.rs:2440-2445` drops a visibility outside that set a second time.

Test coverage: `tests/all/entity_type_projections.rs` exercises the projections only with modelled-valid values — `:215` `json!({"ProductTitle": "alpha ami", "Visibility": "Limited"})`, `:226-229` `"State": "Released", … "ReleaseDate": "2026-01-01T00:00:00Z"` — so no test pins an out-of-contract summary value.

## Steps to Reproduce

1. POST /StartChangeSet with {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct"},"Details":"{\"ProductTitle\":\"\"}"}]} and observe 200.
2. Wait CHANGE_SET_PREPARING + CHANGE_SET_APPLYING for the change set to reach SUCCEEDED.
3. POST /ListEntities with {"Catalog":"AWSMarketplace","EntityType":"AmiProduct"} and observe the summary carries Name="" and AmiProductSummary.ProductTitle="", both outside `@length(min: 1)` of EntityNameString / AmiProductTitleString.
4. Repeat step 1 with "Details":"{\"ProductTitle\":\"<300 'a' characters>\"}" and observe Name/ProductTitle longer than the modelled max of 255.
5. Repeat step 1 with Entity.Type "Offer", ChangeType "CreateOffer" and Details {"Name":"o","State":"bogus","ReleaseDate":"tomorrow"}; ListEntities then reports OfferSummary.State="bogus" (outside the OfferStateString enum Draft|Released) and OfferSummary.ReleaseDate="tomorrow" (outside DateTimeISO8601's 20-character pattern).
