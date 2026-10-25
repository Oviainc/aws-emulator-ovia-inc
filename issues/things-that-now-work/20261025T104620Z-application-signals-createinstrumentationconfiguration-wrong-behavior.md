Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# A second create at the same service/environment/signal type/location is accepted when InstrumentationType differs, minting a duplicate ARN whose tag set is then shared and silently destroyed

_Filed: 2026-10-25T10:46:20Z_
_Operation: CreateInstrumentationConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The create handler keys a configuration's identity on an `InstrumentationScope` that INCLUDES `InstrumentationType`, but mints the record's `ARN` from service + environment + signal type + location hash only — the instrumentation type is not part of the ARN. Two consequences follow, both wrong:

1. Conflict is not detected where the model says it must be. The operation's own documentation states the conflict key as four things and does not mention the instrumentation type: "If a configuration already exists for the same service, environment, signal type, and location, this operation returns a conflict instead of overwriting it." Because the lookup is scoped by `InstrumentationType`, a `PROBE` create at exactly the same service, environment, signal type (`SNAPSHOT` is the only modelled value) and location as an existing `BREAKPOINT` configuration is answered 200 instead of the modelled `ConflictException`.

2. The two accepted records then share ONE ARN, and the ARN is the only key this crate's tag store and tagging operations use. So:
   - `put_tags(&configuration.arn, ...)` merges the second record's create-time tags into the first record's tag set; `ListTagsForResource`/`TagResource`/`UntagResource` cannot address either record's tags separately (`tagged_resource_arn` returns the first stored configuration whose `arn` matches).
   - Deleting OR expiring either record removes `self.tags[arn]`, which is also the still-live sibling's tag set. A `PROBE` configuration that "persists until explicitly deleted" therefore loses every tag it carries the moment the co-located `BREAKPOINT` configuration lapses or is deleted — stored state a later caller can observe is destroyed, while `GetInstrumentationConfiguration` still serves the record.
   - `ListEntityEvents` reports the two records as two indistinguishable change events: `entity_change_events` builds the tie-break key as `{arn}\u{1d}{event_name}`, which is identical for both, so one logical location yields two identical `CreateInstrumentationConfiguration` events.

The fix belongs at one place: either the create must conflict on the documented four-part identity, or the identity that mints the ARN and the identity that keys the store (and therefore the tag set) must be the same. Today they disagree.

## Evidence

Smithy model (operation doc, `smithy_query(query="doc", shape="CreateInstrumentationConfiguration")`):
  "If a configuration already exists for the same service, environment, signal type, and location, this operation returns a conflict instead of overwriting it."

The handler scopes that check by instrumentation type —
services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/create_instrumentation_configuration.rs:226-232:
```
        let scope = crate::state::InstrumentationScope::new(
            &input.instrumentation_type,
            &input.service,
            &input.environment,
        );
        let location_hash = crate::state::location_hash(&input.location);
        let key = crate::state::instrumentation_key(&input.signal_type, &location_hash);
```
…:243-250:
```
        if let Some(existing) = store.instrumentation_configuration(&scope, &key) {
            return Err(
                CreateInstrumentationConfigurationErrorModeled::ConflictException(
```
while the ARN it mints omits the instrumentation type —
…:278-283:
```
            arn: crate::state::instrumentation_configuration_arn(
                &region,
                &scope,
                &input.signal_type,
                &location_hash,
            ),
```
src/state.rs:3542-3556:
```
pub fn instrumentation_configuration_arn(
    region: &str,
    scope: &InstrumentationScope,
    signal_type: &str,
    location_hash: &str,
) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!(
            "{INSTRUMENTATION_CONFIG_ARN_RESOURCE}/{}/{}/{signal_type}/{location_hash}",
            scope.service, scope.environment
        ),
    )
}
```
(the scope that DOES carry the type: src/state.rs:2920-2925 `pub struct InstrumentationScope { pub instrumentation_type: String, pub service: String, pub environment: String, }`)

Tags are keyed by that shared ARN, and both removal paths delete the shared entry:
- src/ops/create_instrumentation_configuration.rs:290: `store.put_tags(&configuration.arn, input.tags.as_deref().unwrap_or_default());`
- src/state.rs:282-287 (`tagged_resource_arn`): `.find(|configuration| configuration.arn == resource_arn)` — the FIRST match wins, so one of the two records is unaddressable.
- src/state.rs:3375-3377 (expiry pass): "// Tags are addressed by resource ARN, so a record that is gone takes its tags with it rather than leaving them for the next resource minted at that ARN." / `self.tags.remove(&arn);`
- src/state.rs:3779-3781 (`remove_instrumentation_configuration`): `self.tags.remove(&removed.arn);`
- src/state.rs:3948-3952 (`entity_change_events`): `format!("{arn}\u{1d}{event_name}")` as the only tie-break key.

Existing test coverage proves the two co-located creates are both accepted today (and no test pins the tag/ARN collision):
tests/all/get_instrumentation_configuration.rs:496-520:
```
    // One BREAKPOINT configuration, which expires, and one PROBE configuration at the same
    // location, which does not.
    ...
    probe_request["InstrumentationType"] = json!(PROBE);
    probe_request["Tags"] = json!([]);
    ...
    let (status, permanent) = create(&router, &probe_request).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the PROBE create, got {status} with {permanent}"
    );
```
That test sets the PROBE's `Tags` to `[]`, so its `store.tags.is_empty()` assertion at tests/all/get_instrumentation_configuration.rs:569-573 never exercises the collision.

## Steps to Reproduce

1. POST /create-instrumentation-configuration with {"InstrumentationType":"BREAKPOINT","Service":"checkout","Environment":"ec2:production","SignalType":"SNAPSHOT","Location":{"CodeLocation":{"Language":"Java","FilePath":"src/main/java/com/example/CartService.java","LineNumber":142}},"CaptureConfiguration":{"CodeCapture":{"CaptureLimits":{}}},"Tags":[{"Key":"team","Value":"payments"}]} — note the returned ARN.
2. POST the same body again with "InstrumentationType":"PROBE" and "Tags":[{"Key":"owner","Value":"probe"}] — observe HTTP 200 and an ARN IDENTICAL to step 1's, where the operation doc requires a ConflictException for the same service, environment, signal type and location.
3. GET /tags?ResourceArn=<that ARN> — observe both `team` and `owner` reported: the two stored records share one tag set and neither can be addressed on its own.
4. POST /delete-instrumentation-configuration naming the BREAKPOINT record (or wait for its 24-hour ExpiresAt to lapse and make any call that runs the expiry pass).
5. POST /get-instrumentation-configuration naming the PROBE record — still HTTP 200, the record exists.
6. GET /tags?ResourceArn=<that ARN> — the surviving PROBE record now reports an empty tag list: its tags were removed with the sibling record.
