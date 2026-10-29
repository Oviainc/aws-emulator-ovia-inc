Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1f84dda0f3deaf97a2c7ee51f672a37b240346b3

# The ARN-to-tag-map dispatch is written out twice in state.rs, once per borrow kind

_Filed: 2026-10-28T03:54:09Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1f84dda0f3deaf97a2c7ee51f672a37b240346b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` resolves "which stored resource does this tagging ARN name" twice: `resource_tags` for the read path (`ListTagsForResource`) and `resource_tags_mut` for the write path (`TagResource`, `UntagResource`). Each one spells out the same five-arm match over `arn_resource_type(arn)` — `stage`, `composition`, `ingest-configuration`, `encoder-configuration`, `storage-configuration`, with `_ => None` — differing only in the accessor's borrow kind (`stage_in` vs `stage_in_mut`, and so on).

The two copies are the taggable-resource table of this service, and nothing ties them together, so they can drift: a sixth taggable resource (the `public-key` kind this crate's own `crate::service::OWN_RESOURCE_TYPES` already lists, pending the four `ImportPublicKey`/`GetPublicKey`/`ListPublicKeys`/`DeletePublicKey` stubs) has to be added in both arms, and `ListTagsForResource` would answer 404 for a resource `TagResource` happily tagged — or the reverse — if only one copy is touched. `resource_tags`' own doc already anticipates the edit ("Each remaining create is its own gap issue, and when one lands its record joins the match below"), and names only one of the two matches.

The fix is to parse the ARN into the resource kind once (an enum of the kinds this store holds) and have both functions dispatch on that one value, so the set of taggable kinds is stated in a single place. The two functions keep their deliberate difference — `resource_tags` skips a reclaimed composition while holding the store immutably, `resource_tags_mut` materialises the clock's writes first — so every response and every existing test stays unchanged.

## Evidence

The read copy — src/state.rs:5198-5225:
```
pub fn resource_tags(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
    region: &str,
) -> Option<HashMap<String, String>> {
    match arn_resource_type(arn)? {
        "stage" => state.stage_in(region, arn).map(|stage| stage.tags.clone()),
        "composition" => state
            .composition_in(region, arn)
            ...
        "ingest-configuration" => state
            .ingest_configuration_in(region, arn)
            .map(|configuration| configuration.tags.clone()),
        "encoder-configuration" => state
            .encoder_configuration_in(region, arn)
            .map(|configuration| configuration.tags.clone()),
        "storage-configuration" => state
            .storage_configuration_in(region, arn)
            .map(|configuration| configuration.tags.clone()),
        _ => None,
    }
}
```

The write copy — src/state.rs:5229-5253:
```
pub fn resource_tags_mut<'a>(
    state: &'a mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
    region: &str,
) -> Option<&'a mut HashMap<String, String>> {
    state.materialize_due_compositions(SystemTime::now());
    match arn_resource_type(arn)? {
        "stage" => state.stage_in_mut(region, arn).map(|stage| &mut stage.tags),
        "composition" => state
            .composition_in_mut(region, arn)
            .map(|composition| &mut composition.tags),
        "ingest-configuration" => state
            .ingest_configuration_in_mut(region, arn)
            .map(|configuration| &mut configuration.tags),
        "encoder-configuration" => state
            .encoder_configuration_in_mut(region, arn)
            .map(|configuration| &mut configuration.tags),
        "storage-configuration" => state
            .storage_configuration_in_mut(region, arn)
            .map(|configuration| &mut configuration.tags),
        _ => None,
    }
}
```

The doc that anticipates the drift, naming one match only — src/state.rs:5185-5192:
```
/// A stage, an ingest configuration, an encoder configuration, a storage configuration
/// and a composition are the resource types this store can hold: ...
/// and a well-formed ARN naming one is answered exactly as an ARN naming a deleted stage
/// is — not found. Each remaining create is its own gap issue, and when one lands its
/// record joins the match below.
```

The callers — src/ops/list_tags_for_resource.rs:171 (`crate::state::resource_tags(&state.lock(), ...)`), src/ops/tag_resource.rs:185 and src/ops/untag_resource.rs:181 (`crate::state::resource_tags_mut(&mut store, ...)`).

Test coverage that must stay green and unchanged: `tests/all/tagging.rs` drives all three tagging operations over every taggable resource kind (the 404/400 tables at tests/all/tagging.rs:500-503, :595-598, :613, :667-670, :723-724), plus `tests/all/region_scoped_resources.rs` and `tests/all/composition_tag_lifetime.rs`.

## Steps to Reproduce

1. Open src/state.rs and read `resource_tags` (lines 5198-5225) and `resource_tags_mut` (lines 5229-5253).
2. Observe the identical five-arm `match arn_resource_type(arn)?` in both, differing only in the borrow kind of the accessor each arm calls.
3. Note src/state.rs:5191-5192 ("when one lands its record joins the match below"), which names a single match while two exist, and src/service.rs:406-413, where `OWN_RESOURCE_TYPES` already lists a sixth kind (`public-key`) that neither match has.
4. Confirm the fix is behaviour-preserving: `cargo test -p service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime` (tests/all/tagging.rs, region_scoped_resources.rs, composition_tag_lifetime.rs) must pass unchanged after the two matches are collapsed onto one parsed resource kind.
