Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9d4ea5e78ce612465906f9a67fc5f9d068061c14

# Grid layout reports gridGap 0 instead of the documented default 2

_Filed: 2026-10-25T00:21:15Z_
_Operation: StartComposition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9d4ea5e78ce612465906f9a67fc5f9d068061c14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartComposition` fills in every layout member the request left out through `crate::state::effective_layout` -> `effective_grid`. `effective_grid` applies the prose defaults for `videoAspectRatio` and `videoFillMode` and passes everything else through with `..requested`. `GridConfiguration$gridGap` is generated as a non-optional `i32` because of the Smithy `@default 0`, so a request that never mentions `gridGap` arrives as `0` and the composition is persisted and reported with `grid.gridGap: 0`.

The model documents a different value for this member: `GridConfiguration$gridGap` says "Default: `2`". (The `PipConfiguration$gridGap` doc does say "Default: `0`", which is why the pip branch is fine — the two members have different documented defaults and the crate treats them as one.)

The comment above `effective_grid` states the rule the code is following and gets the model wrong: it asserts that `gridGap` is one of the members that need no prose default, when the model documents one for the GRID shape. Because the Smithy `@default` makes `gridGap: 0` on the wire indistinguishable from an omitted member, an SDK-generated request can never ask for 0 explicitly, so every grid composition started without an explicit non-zero `gridGap` reports the wrong tile spacing.

## Evidence

Smithy, `GridConfiguration$gridGap` (`smithy_query(query="shape", shape="GridConfiguration")`):
> "Specifies the spacing between participant tiles in pixels. Default: `2`."

and for contrast `PipConfiguration$gridGap`:
> "Specifies the spacing between participant tiles in pixels. Default: `0`."

The comment that mis-states it, and the code under it — services/.../src/state.rs:646-667:
```
/// The grid layout `requested` describes, with the documented default for every member
/// it left out.
///
/// The members the model carries a `@default` trait for (`omitStoppedVideo`, `gridGap`)
/// arrive already filled in by the framework's deserializer — they are not optional on
/// the Rust shape — so only the two the model documents a default for in prose are
/// applied here.
fn effective_grid(requested: GridConfiguration) -> GridConfiguration {
    GridConfiguration {
        video_aspect_ratio: Some(...DEFAULT_GRID_VIDEO_ASPECT_RATIO...),
        video_fill_mode: Some(...DEFAULT_GRID_VIDEO_FILL_MODE...),
        ..requested
    }
}
```
`gridGap` IS a member "the model documents a default for in prose" ("Default: `2`"), so the stated reason for skipping it does not hold.

The field is non-optional, so `..requested` yields the framework-materialised 0 — services/.../src/ops/types/shared_types.rs:495-500:
```
pub struct GridConfiguration {
    pub featured_participant_attribute: Option<String>,
    pub omit_stopped_video: bool,
...
    pub grid_gap: i32,
```

The handler reaches it at services/.../src/ops/start_composition.rs:257: `let layout = crate::state::effective_layout(input.layout);`.

The current value is pinned by a test whose stated reason is the model's documented defaults — services/.../tests/all/compositions.rs:602-621:
```
/// `Composition$layout` is `@required`, so every composition reports one: the documented
/// grid fallback when the request configures no layout, and the documented default for
/// every layout member the request left out.
...
            "no layout at all: the documented grid fallback",
            None,
            json!({ "grid": {
                "omitStoppedVideo": false,
                "videoAspectRatio": DEFAULT_GRID_ASPECT_RATIO,
                "videoFillMode": DEFAULT_GRID_FILL_MODE,
                "gridGap": 0,
            }}),
```
The quoted model sentence contradicts that stated reason.

## Steps to Reproduce

1. `CreateStage {}` and keep `stage.arn`.
2. `StartComposition {"stageArn":"<stage arn>","destinations":[{"channel":{"channelArn":"arn:aws:ivs:us-east-1:<acct>:channel/live"}}]}` — no `layout` member at all.
3. Read `composition.layout.grid.gridGap` in the response (or via `GetComposition`): it is `0`.
4. `GridConfiguration$gridGap` is documented "Default: `2`"; the same request against a grid is the documented-fallback case the handler's own `effective_layout` comment cites.
5. Repeat with `"layout":{"pip":{}}` — `pip.gridGap` 0 is correct there, which is why the single shared code path is the defect.
