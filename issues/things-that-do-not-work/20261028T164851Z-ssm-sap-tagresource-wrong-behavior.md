Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 78c035fb932ec52ae00c0e559fb4c57d057723ed

# An ARN of another Region resolves in this Region's partition, contradicting the resolver's own documented invariant

_Filed: 2026-10-28T16:48:51Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `78c035fb932ec52ae00c0e559fb4c57d057723ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegionState::holds_resource` is the single place this crate decides whether an ARN names something, and it is the gate for `TagResource`, `UntagResource`, `ListTagsForResource`, `PutResourcePermission`, `GetResourcePermission` and `DeleteResourcePermission`. Its own doc comment states the invariant "an ARN of another Region therefore resolves to nothing here", resting on the premise that every ARN it compares against embeds the partition's own Region. `RegionState::resolve_database` (the gate for `GetDatabase`) states the same thing even more explicitly: "`None` when nothing this Region holds answers the reference — including a reference to a database of another Region".

That premise is false. `StoredApplication::databases` keeps the ARN a registration supplied in `RegisterApplication$DatabaseArn` (or `UpdateApplicationSettings$DatabaseArn`) VERBATIM rather than minting it with the partition's Region — `arn: arn.to_string()` — and `StoredApplication::resource_arns` then reports that ARN as one of the registration's resources. `holds_resource` compares it with plain string equality and never consults `arn_region`, the helper this crate already has for reading the Region out of an ARN. So a registration made at the us-east-1 endpoint whose `DatabaseArn` carries another Region makes that foreign ARN resolve in the us-east-1 partition.

The observable consequences: `TagResource` signed for us-east-1 accepts the eu-west-1 ARN and persists tags under it in the us-east-1 partition; `ListTagsForResource` signed for eu-west-1 then answers 404 `ResourceNotFoundException` for the very same ARN, so tag state for an ARN of one Region lives only at another Region's endpoint. `PutResourcePermission` / `DeleteResourcePermission` behave the same way for resource policies, and `GetDatabase` signed for us-east-1 resolves a `DatabaseArn` of another Region, which is exactly what `resolve_database`'s doc says it will not do.

Either the two doc comments overstate the invariant or the resolver is missing the Region comparison; as the crate stands the two halves disagree. Note that `tests/all/registered_database_arn.rs:77` pins the ARN itself being reported "AsRegistered" (including `arn:aws:rds:us-east-1:000000000000:db:sap-prd` at :99), so that reporting must stay as it is — the inconsistency is in what the resolver claims about Regions, not in how the ARN is stored.

## Evidence

The documented invariant, `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2802-2819`:
```rust
    /// `region` is the Region this partition holds, which is the Region those ARNs
    /// embed: an ARN of another Region therefore resolves to nothing here, exactly as
    /// an application of another Region does ([`Self::resolve_application_id`]).
    pub fn holds_resource(&self, region: &str, arn: &str) -> bool {
        if self.resolve_application_id(None, Some(arn), None).is_some() {
            return true;
        }
        self.applications.values().any(|application| {
            application
                .resource_arns(region)
                .iter()
                .any(|held| held == arn)
        })
    }
```
and the same claim at `src/state.rs:2832-2839` for the database resolver:
```rust
    /// `None` when nothing this Region holds answers the reference — including a reference
    /// to a database of another Region, since the lookup only ever reaches this partition.
    ...
    /// `region` is the Region this partition holds, which is the Region the ARNs the
    /// databases carry embed.
```

The code that breaks the premise, `src/state.rs:816-832` (`StoredApplication::databases`) — the registration-supplied ARN is stored and reported unchanged, not minted from `region`:
```rust
        let named_by_arn = self
            .database_arn
            .as_deref()
            .and_then(|arn| database_name_from_arn(arn).map(|name| (arn, name)));
        if let Some((arn, database_name)) = named_by_arn {
            let database_id = database_id_from_name(database_name);
            databases.insert(
                database_id.clone(),
                ApplicationDatabase {
                    application_id: self.id.clone(),
                    component_id: component_id.clone(),
                    arn: arn.to_string(),
```
and `src/state.rs:959-967`, which feeds exactly that ARN to the resolver:
```rust
    pub fn resource_arns(&self, region: &str) -> Vec<String> {
        let mut arns = ::std::vec![self.arn.clone()];
        arns.extend(self.databases(region).into_iter().map(|db| db.arn));
```
The crate already owns the helper the check would need and does not use it here — `src/state.rs:3461-3463`:
```rust
pub fn arn_region(arn: &str) -> &str {
    arn.split(':').nth(3).unwrap_or_default()
}
```

The handlers that gate on the resolver and therefore inherit the contradiction: `src/ops/tag_resource.rs:178-188`, `src/ops/untag_resource.rs:180-190`, `src/ops/list_tags_for_resource.rs:179-186`, `src/ops/put_resource_permission.rs:183-193`, `src/ops/get_resource_permission.rs:169-176`, `src/ops/delete_resource_permission.rs:194-206`, and `src/ops/get_database.rs:185-191` for `resolve_database`. `tag_resource.rs:174-177` repeats the claim at the call site: "the resources this service holds are the registered SAP application and the resources derived from that registration ... Resolved through [`crate::state::RegionState::holds_resource`], the one place this service decides what an ARN names".

Test coverage: `tests/all/region_scoping.rs` pins Region scoping for applications, operation records and listings only (`REGIONS` at :48, cases at :292-344 cover GetApplication / Start / Stop / Deregister / ListComponents by ApplicationId and ApplicationArn). No row registers a `DatabaseArn` of another Region, and `tests/all/registered_database_arn.rs` keeps every `DatabaseArn` in `us-east-1` (`grep -n 'eu-west|Region' tests/all/registered_database_arn.rs` matches only the doc prose), so nothing pins the current cross-Region acceptance.

## Steps to Reproduce

1. POST /register-application signed for us-east-1 with {"ApplicationId":"APP1","ApplicationType":"SAP_ABAP","Instances":["i-0123456789abcdef0"],"DatabaseArn":"arn:aws:ssm-sap:eu-west-1:000000000000:database/APP1/HDB"} — expect 200
2. POST /list-databases signed for us-east-1 {"ApplicationId":"APP1"} — the database is reported with Arn exactly as registered, i.e. carrying eu-west-1
3. POST /tags/arn%3Aaws%3Assm-sap%3Aeu-west-1%3A000000000000%3Adatabase%2FAPP1%2FHDB signed for us-east-1 with {"tags":{"env":"dev"}} — expected per the quoted invariant: 404 ResourceNotFoundException, since the ARN names another Region. Actual: 200, and the tag is persisted in the us-east-1 partition keyed by a eu-west-1 ARN
4. GET /tags/arn%3Aaws%3Assm-sap%3Aeu-west-1%3A000000000000%3Adatabase%2FAPP1%2FHDB signed for eu-west-1 — 404 ResourceNotFoundException, so the tag set for a eu-west-1 ARN is readable only at the us-east-1 endpoint
5. POST /get-database signed for us-east-1 {"DatabaseArn":"arn:aws:ssm-sap:eu-west-1:000000000000:database/APP1/HDB"} — resolves 200, contradicting src/state.rs:2832-2833 ("including a reference to a database of another Region")
6. POST /put-resource-permission signed for us-east-1 with that same eu-west-1 ResourceArn, ActionType RESTORE and any SourceResourceArn — 200, and GetResourcePermission at eu-west-1 for the same ARN answers 404
