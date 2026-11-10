Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 475da5d994ddfaf7edb8005d68a27ae31cbab3bb

# AccountlessArn::parse admits any partition beginning "aws", though its own doc says the segment is checked for membership of the `aws(?:[-a-z]*)?` family

_Filed: 2026-10-29T15:24:42Z_
_Operation: GetControl | Class: validation_

> _Note: this issue was identified against the code base at commit `475da5d994ddfaf7edb8005d68a27ae31cbab3bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every caller-supplied ARN this service reads goes through `AccountlessArn::parse` (`GetControl`'s `resolve_control`, and from there `canonical_catalog_arn` for every list filter). Its doc states the rule it implements for the partition segment: the segment "is checked for membership of the family" `aws(?:[-a-z]*)?`, after which it "carries no information about which resource the ARN names". The code under that doc does not check family membership — it checks a prefix: `if !partition.starts_with("aws")`. That admits partitions the family does not contain (`awsfoo`, `aws1`, `aws-CN`, `awsX`).

Because the partition is then DROPPED (`canonical_catalog_arn` rewrites the ARN into `arn:aws:controlcatalog:::{kind}/{id}` and `resolve_control` looks the identifier up directly), an ARN written in a partition the model rejects is not merely tolerated, it is resolved as though it had been written in `aws`: `resolve_control("arn:awsfoo:controlcatalog:::control/<a published identifier>")` returns the published control, and `canonical_catalog_arn("objective", "arn:awsX:controlcatalog:::objective/<id>")` returns the stored objective ARN, so the filter narrows as if the caller had named a modelled ARN.

From the wire the modelled `@pattern` on `ControlArn` / `DomainArn` / `ObjectiveArn` / `CommonControlArn` refuses such an ARN before the handler runs, so the divergence is reachable only in-process. The crate states, for the one other check it duplicates from the model, that this is precisely a case worth refusing anyway: `PageError::MaxResultsOutOfRange`'s doc says the modelled `@range` makes the value "unreachable from the wire; refusing it here too means an in-process caller gets the same answer as a client, in every build profile". Either the doc at state.rs:243-246 or the check at state.rs:278-280 has to change; today they disagree.

## Evidence

The rule the helper's own doc states — services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs:239-246:
```
/// The segments that carry meaning in an ARN this service accepts: none of the forms
/// its modeled ARN patterns admit carries an account id, so each is
/// `arn:{PARTITION}:{SERVICE}:{REGION}::{RESOURCE}`.
///
/// Parsing drops the partition because no modeled ARN pattern here distinguishes one:
/// every pattern restricts that segment to the `aws` family (`aws(?:[-a-z]*)?`) and
/// says nothing more about it, so the segment is checked for membership of the family
/// and then carries no information about which resource the ARN names.
```

The check actually performed — src/state.rs:277-280:
```
        if !partition.starts_with("aws") {
            return None;
        }
```
`starts_with("aws")` is satisfied by `awsfoo`, `aws1`, `aws-CN` and `awsX`, none of which the quoted family `aws(?:[-a-z]*)?` matches (the model's own spelling, e.g. `ControlArn`: `"pattern": "^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\\-]+$"`, `smithy_query(query="shape", shape="ControlArn")`).

The partition is then dropped, so a non-family spelling resolves a published resource — src/state.rs:330-334 and :338-344:
```
pub fn canonical_catalog_arn(kind: &str, requested_arn: &str) -> String {
    catalog_identifier(kind, requested_arn)
        .map(|identifier| catalog_arn(kind, identifier))
        .unwrap_or_else(|| requested_arn.to_string())
}
```
```
fn catalog_identifier<'a>(kind: &str, arn: &'a str) -> Option<&'a str> {
    let parsed = AccountlessArn::parse(arn)?;
    // The catalog is global, so its own ARNs carry no Region segment.
    if parsed.service != "controlcatalog" || !parsed.region.is_empty() {
        return None;
    }
    kind_identifier(kind, parsed.resource)
```

The handler path that reaches it — src/ops/get_control.rs:219 into src/state.rs:191-195:
```
        let Some(control) = state.resolve_control(&input.control_arn) else {
```
```
    pub fn resolve_control(&self, requested_arn: &str) -> Option<&Control> {
        match ControlArnRef::parse(requested_arn)? {
            ControlArnRef::Catalog(identifier) => {
                self.controls.get(&catalog_arn("control", identifier))
            }
```

The crate's own statement that an in-process caller must get the client's answer for a check the model already enforces — src/state.rs:612-617:
```
    /// A `MaxResults` outside the operation's modeled range. The modeled `@range`
    /// refuses such a value before a handler runs, so it is unreachable from the
    /// wire; refusing it here too means an in-process caller gets the same answer as
    /// a client, in every build profile, rather than a page quietly sized differently
    /// from the one it asked for.
    MaxResultsOutOfRange { requested: i32, modeled_max: i32 },
```

Test coverage: the suite that pins partition handling walks only in-family partitions — tests/all/global_catalog_arn_reading.rs:33-37:
```
/// Partitions a catalog ARN can be written in. Source: the modeled `ControlArn`,
/// `DomainArn`, `ObjectiveArn` and `CommonControlArn` patterns, each of which opens
/// `^arn:(aws(?:[-a-z]*)?):` — the whole `aws` partition family
/// (`smithy_query(query="shape", shape="ControlArn,DomainArn,ObjectiveArn,CommonControlArn")`).
const PARTITIONS: [&str; 4] = ["aws", "aws-cn", "aws-us-gov", "aws-iso-e"];
```
All four are inside `aws(?:[-a-z]*)?`, as are the partitions used by tests/all/catalog_resource_identity.rs and tests/all/control_arn_resolution.rs, so making the check a family-membership test leaves every existing assertion (and every wire response) unchanged.

## Steps to Reproduce

1. Read services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs:243-246: the doc says the partition segment "is checked for membership of the family" `aws(?:[-a-z]*)?`.
2. Read src/state.rs:278-280 directly under it: the check is `if !partition.starts_with("aws")`, a prefix test, which accepts `awsfoo`, `aws1`, `aws-CN` and `awsX` — strings the quoted family does not match.
3. Follow the use: src/ops/get_control.rs:219 calls `state.resolve_control(&input.control_arn)` (src/state.rs:191-203), which parses through `ControlArnRef::parse` (src/state.rs:227-236) -> `catalog_identifier` (src/state.rs:338-344) -> `AccountlessArn::parse`; the partition is discarded and the identifier is looked up in `controls`.
4. In-process (as src/state.rs:1033-1142's own unit tests drive the state), call `ComAmazonawsControlcatalogControlcatalogState::default().resolve_control("arn:awsfoo:controlcatalog:::control/<any published control identifier>")`: it returns the published control, i.e. an ARN in a partition outside the modelled family names a catalog resource.
5. Do the same for a filter: `crate::state::canonical_catalog_arn("objective", "arn:awsX:controlcatalog:::objective/<a published objective identifier>")` returns the stored `arn:aws:controlcatalog:::objective/<id>`, so `SingleResourceFilter` would narrow `ListObjectives` as though a modelled ARN had been sent.
6. Compare with the rule the crate applies to the other model-duplicated check (src/state.rs:612-617): a value the model refuses at the boundary is refused in-process too "so an in-process caller gets the same answer as a client, in every build profile".
