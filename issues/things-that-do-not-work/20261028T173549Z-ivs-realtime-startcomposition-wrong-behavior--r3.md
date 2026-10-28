Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# Layout members whose documentation states `Default: ""` are reported absent, while every other empty-string documented default of this crate is materialised

_Filed: 2026-10-28T17:35:49Z_
_Operation: StartComposition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::effective_layout` is this crate's single place for applying "the model's documented default for every member the request left out" to a composition's layout, and it materialises every documented default of `GridConfiguration`/`PipConfiguration` — `omitStoppedVideo`, `videoAspectRatio`, `videoFillMode`, `gridGap`, `pipBehavior`, `pipPosition` — EXCEPT the three whose documented default is the empty string:

* `GridConfiguration$featuredParticipantAttribute` — "Default: `""` (no featured participant)."
* `PipConfiguration$featuredParticipantAttribute` — "Default: `""` (no featured participant)."
* `PipConfiguration$pipParticipantAttribute` — "Default: `""` (no PiP participant)."

For those three, `effective_grid` copies the requested `Option<String>` through unchanged and `effective_pip` passes them through with `..requested`, so a `StartComposition` whose layout omits them makes `Composition$layout` carry NO value for the member at all. Every reader of the composition (`StartComposition`, `GetComposition`) then reports a layout in which an absent member and the documented `""` cannot be told apart, even though the crate materialises exactly this kind of empty-string documented default everywhere else: `AutoParticipantRecordingConfiguration$storageConfigurationArn` ('Default: `""` (empty string, no storage configuration is specified)') is always reported through `NO_STORAGE_CONFIGURATION_ARN`, as are `IngestConfiguration$stageArn` (`NO_ASSOCIATED_STAGE_ARN`), `Stage$activeSessionId` (`NO_ACTIVE_SESSION_ID`) and `Participant$recordingS3BucketName`/`recordingS3Prefix`.

The test that pins the current output states the opposite rule in its own doc comment — it claims each absent member is reported "at the default its own documentation states" — and then builds the expected grid without `featuredParticipantAttribute`.

## Evidence

Model, `smithy_query(query="shape",shape="GridConfiguration")` — `featuredParticipantAttribute`: "This attribute name identifies the featured slot. A participant with this attribute set to \"true\" (as a string value) in ParticipantTokenConfiguration is placed in the featured slot. **Default: `""` (no featured participant).**"

Model, `smithy_query(query="shape",shape="PipConfiguration")` — `pipParticipantAttribute`: "Specifies the participant for the PiP window. … **Default: `""` (no PiP participant).**"; `featuredParticipantAttribute`: "… **Default: `""` (no featured participant).**"

The handler/state copies them through instead of defaulting them — `src/state.rs:783-805`:
```
fn effective_grid(requested: GridConfiguration) -> GridConfiguration {
    GridConfiguration {
        featured_participant_attribute: requested.featured_participant_attribute,
        omit_stopped_video: Some(documented_default(
            requested.omit_stopped_video,
            DEFAULT_OMIT_STOPPED_VIDEO,
        )),
```
and `src/state.rs:819-838`:
```
fn effective_pip(requested: PipConfiguration) -> PipConfiguration {
    PipConfiguration {
        video_fill_mode: Some(...),
        pip_behavior: Some(...),
        pip_position: Some(...),
        ..requested
    }
}
```
(`..requested` carries `pip_participant_attribute` and `featured_participant_attribute` as they arrived, i.e. `None` when absent.)

The same crate materialises the identical empty-string documented default for a sibling member — `src/state.rs:71-75`:
```
/// `storageConfigurationArn` of a stage with no storage configuration attached.
///
/// Documented on `AutoParticipantRecordingConfiguration$storageConfigurationArn`:
/// "Default: `\"\"` (empty string, no storage configuration is specified)."
pub const NO_STORAGE_CONFIGURATION_ARN: &str = "";
```
used at `src/state.rs:189-198` (`default_recording_configuration`), so a stage always reports the member.

Test coverage: `tests/all/composition_layout_default_lifetime.rs:158-172` states the rule and omits the member from the expectation:
```
/// The grid layout a composition reports when the request configured `members` and left
/// every other member out: each absent member at the default its own documentation
/// states.
fn grid_reporting(members: Value) -> Value {
    let mut grid = json!({
        "omitStoppedVideo": DEFAULT_OMIT_STOPPED_VIDEO,
        "videoAspectRatio": DEFAULT_GRID_ASPECT_RATIO,
        "videoFillMode": DEFAULT_GRID_FILL_MODE,
        "gridGap": DEFAULT_GRID_GAP,
    });
```
and `tests/all/composition_layout_default_lifetime.rs:313-317` asserts the whole layout object equals that, so the absence is pinned against the test's own stated rule.

## Steps to Reproduce

1. CreateStage; note the stage ARN.
2. StartComposition with stageArn=<stage>, destinations=[{"channel":{"channelArn":"arn:aws:ivs:us-east-1:<account>:channel/live"}}] and layout={"grid":{}} (no featuredParticipantAttribute).
3. Read `composition.layout.grid` in the response: it carries `omitStoppedVideo`, `videoAspectRatio`, `videoFillMode` and `gridGap` at their documented defaults, but carries NO `featuredParticipantAttribute` member, although the member documents `Default: "" (no featured participant)`.
4. Repeat with layout={"pip":{}}: `composition.layout.pip` carries no `pipParticipantAttribute` and no `featuredParticipantAttribute`, both of which document `Default: ""`.
5. GetComposition on the returned ARN reports the same layout, so the omission is persisted, not just a response-shaping accident.
