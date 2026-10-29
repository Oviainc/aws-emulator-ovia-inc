Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e2d1e8b9f8d33435d8188757aebd13b97dadeffd

# ListEnabledControls can never report a mandatory control, though the catalog's own doc says it reports them

_Filed: 2026-10-29T07:23:19Z_
_Operation: ListEnabledControls | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e2d1e8b9f8d33435d8188757aebd13b97dadeffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The control catalog in `crate::state` publishes 11 of its 23 entries with `enablable: false`, and the doc on that very field states that those mandatory controls are reported by `ListEnabledControls`. No code path can put one in that listing:

* `enabled_controls` is, by the store's own doc, "written by `EnableControl`" only, and `store.enabled_controls.insert(..)` appears exactly once in the crate (`enable_control.rs:358`).
* `EnableControl` refuses every catalog entry carrying `enablable: false` with `ValidationException` BEFORE it reaches that insert (`enable_control.rs:234`), so a mandatory control is never persisted.
* `ListEnabledControls` lists `store.enabled_controls.values()` and nothing else (`list_enabled_controls.rs:209-211`), and `GetEnabledControl` resolves against the same map.

So the only way a mandatory control can be observed through this emulator is as an `EnableControl` refusal: the 11 `enablable: false` catalog entries are unreachable in any successful response. A caller that lists the enabled controls on a governed organizational unit sees only the controls it enabled itself, and the field doc that justifies those entries existing in the catalog at all asserts the opposite. Either the listing has to surface the mandatory controls Control Tower applies itself, or the claim in the catalog doc is false and the crate contradicts itself about what the one store it reads holds.

This is distinct from the already-open `ListEnabledControls` `includeChildren` gap (`parameter-ignored`): that one is about widening the listing to a target's children, this one is about a whole class of catalog entry that is never in the listing for ANY target.

## Evidence

The claim, `services/service-com-amazonaws-controltower-awscontroltowerapis/src/state.rs:2869-2877` (doc on `CatalogControl::enablable`):
```
    /// Whether `EnableControl` may enable this control.
    ///
    /// `EnableControlInput$controlIdentifier` is documented as "Only Strongly recommended
    /// and Elective controls are permitted, with the exception of the Region deny
    /// control": the MANDATORY controls AWS Control Tower applies to every governed
    /// account itself are named by a `controlIdentifier` — `ListEnabledControls` reports
    /// them — but cannot be enabled by a caller, so a request naming one is refused rather
    /// than persisted. False for exactly those.
    pub enablable: bool,
```

The store that listing reads, and its only writer, `state.rs:1187-1191`:
```
    /// Enabled controls keyed by their own ARN — the single home for this service's
    /// control enablements, written by `EnableControl` and read by
    /// `ListEnabledControls`/`GetEnabledControl`. Holds at most
    /// [`MAX_ENABLED_CONTROLS`] entries.
    pub enabled_controls: HashMap<String, EnabledControl>,
```

The refusal that keeps every mandatory entry out of that store, `src/ops/enable_control.rs:234-246`:
```
        if !control.enablable {
            return Err(EnableControlErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "control {} is a mandatory control, which AWS Control Tower applies \
                         itself; only strongly recommended and elective controls may be \
                         enabled, with the exception of the Region deny control",
                        control.name
                    ),
                },
            )
            .into());
        }
```

The listing, which reads nothing but that store, `src/ops/list_enabled_controls.rs:209-212`:
```
        let mut controls: Vec<&crate::state::EnabledControl> = store
            .enabled_controls
            .values()
            .filter(|control| {
```

The only insertion site into that store, `src/ops/enable_control.rs:358`:
```
        store.enabled_controls.insert(
```
(`grep -n 'enabled_controls.insert' src/` reports this single line; every other reference is a read, a `get`, a `get_mut`, a `remove` in `settle`, or the `len()` cap check.)

The 11 mandatory catalog entries, `state.rs:2938,2945,2952,2959,2966,2973,2980,2987,2994,3001,3008` — each `enablable: false`.

Test coverage: `tests/all/enable_control_validation.rs:378` `a_mandatory_catalog_control_is_not_a_callers_to_enable` pins the refusal side only; no test asserts a mandatory control ever appears in a listing, and `tests/all/disable_control.rs:677` carries the opposite incidental assumption ("a pair naming a mandatory control, which no enablement can exist for") — i.e. the crate states both readings in two places.

## Steps to Reproduce

1. Start the emulator and POST /create-landingzone with a valid version and manifest, then wait past the settle span so the landing zone is ACTIVE.
2. POST /list-enabled-controls with { "targetIdentifier": "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a", "includeChildren": true } — the response is `enabledControls: []`.
3. POST /enable-control naming a mandatory catalog entry, e.g. { "controlIdentifier": "arn:aws:controltower:us-east-1::control/AWS-GR_CLOUDTRAIL_ENABLED", "targetIdentifier": "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a" } — refused 400 ValidationException ("is a mandatory control, which AWS Control Tower applies itself").
4. POST /list-enabled-controls again, with and without `targetIdentifier`, with `includeChildren` both ways, and with no filter at all: no entry ever reports a mandatory `controlIdentifier`, so none of the 11 `enablable: false` catalog entries is observable in any successful response — while state.rs:2874 says `ListEnabledControls` reports them.
5. grep -n 'enabled_controls.insert' services/service-com-amazonaws-controltower-awscontroltowerapis/src/ -r → one hit, enable_control.rs:358, which is unreachable for an `enablable: false` entry because of the refusal at enable_control.rs:234.
