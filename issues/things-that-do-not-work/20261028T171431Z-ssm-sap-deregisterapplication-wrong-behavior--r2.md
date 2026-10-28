Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5437c407f1bdac467f7974ac312544841a51f124

# A completed deregistration keeps the tags and resource permissions held on the application's own ARN when another registration's DatabaseArn names that ARN

_Filed: 2026-10-28T17:14:31Z_
_Operation: DeregisterApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5437c407f1bdac467f7974ac312544841a51f124`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegionState::settle` is the one place a completed deregistration forgets what was held PER RESOURCE (tags and resource-permission grants, both keyed by the resource's ARN). Since the `UpdateApplicationSettings` fix it does so through a `destroyed` / `live` diff: an ARN that stopped existing is forgotten only when no application the Region still holds reports that same ARN among its own resources (`StoredApplication::resource_arns`).

That diff compares bare ARN STRINGS and never asks what KIND of resource an ARN names. `StoredApplication::databases` keeps the ARN a registration supplied in `RegisterApplication$DatabaseArn` / `UpdateApplicationSettings$DatabaseArn` VERBATIM (`arn: arn.to_string()`) and no handler checks that the ARN names a database — the model's `SsmSapArn` pattern admits any ARN, and `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs:300-303` stores it as given. So a second registration whose `DatabaseArn` is ANOTHER application's own ARN (or one of its component ARNs) puts that ARN into `live`, and the deregistration of the application that ARN belongs to then forgets nothing that was held on it.

Three observable consequences, all of which the crate's own prose says cannot happen:

1. The tag set and the RESTORE policy of a deregistered application survive it: `ListTagsForResource` and `GetResourcePermission` keep answering 200 for the gone application's ARN (it now resolves through `holds_resource` as the OTHER application's "database"), although `src/state.rs:2567-2570` says "what was held FOR it goes with it — its tags and the resource permissions granted on it, both keyed by its ARN" and the module doc at `src/state.rs:105-108` says "the grants go when it does".
2. A later registration under the same `ApplicationId` in the same Region mints the same application ARN and therefore INHERITS the stale tags and grants: `GetApplication` reports tags the new registration never set and `GetResourcePermission` re-renders a RESTORE grant nobody made on it. `forget_resource`'s doc states the opposite guarantee verbatim: "a resource later derived under the SAME ARN is a new resource that carries neither — which is what this guarantees" (`src/state.rs:2657-2659`). The stale tags also count against `MAX_TAGS_PER_RESOURCE` for the new application (`apply_tags` merges into the entry already held).
3. Before the deregistration the same bare-string keying already makes two DIFFERENT resources share one tag set and one policy: the application `VICTIM` and the "database" the other registration derives under `VICTIM`'s ARN are tagged and granted as one resource.

Note for a fixer: the `live` guard must NOT be removed — `tests/all/settings_destroyed_databases.rs:463-513` pins that a DATABASE two registrations both name by ARN keeps everything held on it. What is wrong is that the guard is applied to ARNs that are not databases at all: an ARN that is the removed application's own ARN, or one of its component ARNs, is a resource of THAT registration and nothing another registration's `DatabaseArn` names can keep it alive.

## Evidence

The guarantee the cleanup documents, `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2651-2668`:
```rust
    /// Forget everything this Region holds ON the resource `arn` names, because that
    /// resource has stopped existing.
    ///
    /// Tags and resource permissions are held PER RESOURCE, keyed by exactly the ARN the
    /// resource is reported under, so neither can outlive the resource it was attached to:
    /// once the ARN names nothing, no tag set is held for it and no policy is rendered
    /// from grants keyed by it. The ARNs of this service are a pure function of the ids
    /// they embed, so a resource later derived under the SAME ARN is a new resource that
    /// carries neither — which is what this guarantees.
    ...
    fn forget_resource(&mut self, arn: &str) {
        self.tags_by_arn.remove(arn);
        self.resource_permissions.remove(arn);
    }
```
and what the removal pass says it does, `src/state.rs:2567-2574`:
```rust
        // A deregistration whose removal has fallen due is COMPLETE: from that instant
        // the application stops resolving, whether or not anybody called, and what was
        // held FOR it goes with it — its tags and the resource permissions granted on
        // it, both keyed by its ARN, ...
```
The code that breaks both, `src/state.rs:2598-2617` — the diff is over bare strings, and its own comment claims the opposite of what it achieves ("so a destroyed ARN is one no call that takes an ARN resolves any more"):
```rust
        if !destroyed.is_empty() {
            // The ARNs this partition STILL holds, enumerated once: the same enumeration
            // [`Self::holds_resource`] resolves an ARN through, so a destroyed ARN is one
            // no call that takes an ARN resolves any more. It is asked of the set rather
            // than of the resolver per ARN because an ARN can be named by more than one
            // registration (`RegisterApplication$DatabaseArn` and
            // `UpdateApplicationSettings$DatabaseArn` take any ARN), ...
            let live: BTreeSet<String> = self
                .applications
                .values()
                .flat_map(|application| application.resource_arns(arn_region(&application.arn)))
                .collect();
            for arn in &destroyed {
                if !live.contains(arn) {
                    self.forget_resource(arn);
                }
            }
        }
```
What puts a foreign ARN into that `live` set — `src/state.rs:816-832` (`StoredApplication::databases`), which keeps the registration's ARN unchanged whatever it names:
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
fed to the diff and to the resolver by `src/state.rs:959-967`:
```rust
    pub fn resource_arns(&self, region: &str) -> Vec<String> {
        let mut arns = ::std::vec![self.arn.clone()];
        arns.extend(self.databases(region).into_iter().map(|db| db.arn));
```
and stored unchecked by the handler, `src/ops/register_application.rs:300-303`:
```rust
            // The SAP HANA database the registration named by ARN: one of this
            // application's databases, which `ListDatabases` reports under that ARN
            // (`crate::state::StoredApplication::databases`).
            database_arn: input.database_arn,
```
`database_name_from_arn` (`src/state.rs:3507-3511`) takes "the last non-empty segment", so `arn:aws:ssm-sap:us-east-1:000000000000:application/VICTIM` yields the database name `VICTIM` and a database whose ARN is that application ARN; `holds_resource` (`src/state.rs:2805-2819`) then resolves it for every ARN-taking call.

The removal itself is what `src/ops/deregister_application.rs:187-190` schedules:
```rust
        application.pending = Some(crate::state::PendingTransition {
            due_at: now + crate::state::TRANSITION_SPAN,
            change: crate::state::PendingChange::Removal,
        });
```

Test coverage: `tests/all/settings_destroyed_databases.rs:463-513` (`a_database_another_registration_still_names_keeps_what_is_held_on_it`) pins the shared-DATABASE case the `live` guard exists for, and it uses an RDS ARN (`REPLACEMENT_DATABASE_ARN` at :64, `COLON_SEGMENT_DATABASE_ARN` imported at :43) — never an ssm-sap application or component ARN, so nothing pins the current behaviour for those. `tests/all/get_database.rs:799-803` shows the crate already treats "an application ARN rather than a database ARN" as naming no database (400) when no registration named it in `DatabaseArn`, but no test registers one that way. `grep -n 'DatabaseArn' tests/all/*.rs` shows every registered `DatabaseArn` is an rds/ssm-sap DATABASE ARN.

## Steps to Reproduce

1. POST /register-application signed for us-east-1 with {"ApplicationId":"VICTIM","ApplicationType":"SAP_ABAP","Instances":["i-0123456789abcdef0"]} -> 200; its ARN is arn:aws:ssm-sap:us-east-1:000000000000:application/VICTIM
2. POST /tags/arn%3Aaws%3Assm-sap%3Aus-east-1%3A000000000000%3Aapplication%2FVICTIM with {"tags":{"env":"prod"}} -> 200, and POST /put-resource-permission with {"ActionType":"RESTORE","ResourceArn":"arn:aws:ssm-sap:us-east-1:000000000000:application/VICTIM","SourceResourceArn":"arn:aws:ssm-sap:eu-west-1:111111111111:application/PEER"} -> 200
3. POST /register-application with {"ApplicationId":"KEEPER","ApplicationType":"SAP_ABAP","Instances":["i-0123456789abcdef1"],"DatabaseArn":"arn:aws:ssm-sap:us-east-1:000000000000:application/VICTIM"} -> 200 (the ARN is stored unchecked); POST /list-databases {"ApplicationId":"KEEPER"} reports a database whose Arn is VICTIM's application ARN and whose Tags are {"env":"prod"} — one tag set shared by two different resources
4. POST /deregister-application {"ApplicationId":"VICTIM"} -> 200; wait longer than TRANSITION_SPAN and make any request so the removal settles. POST /get-application {"ApplicationId":"VICTIM"} -> 400, the application is gone
5. GET /tags/arn%3Aaws%3Assm-sap%3Aus-east-1%3A000000000000%3Aapplication%2FVICTIM -> 200 with {"tags":{"env":"prod"}}, and POST /get-resource-permission {"ResourceArn":"arn:aws:ssm-sap:us-east-1:000000000000:application/VICTIM"} -> 200 still rendering the RESTORE statement. Expected per src/state.rs:2567-2570 and 2651-2659: both were forgotten when the removal completed
6. POST /register-application again with {"ApplicationId":"VICTIM","ApplicationType":"SAP_ABAP","Instances":["i-0123456789abcdef0"]} -> 200, then POST /get-application {"ApplicationId":"VICTIM"} -> Tags {"env":"prod"} the new registration never set, and GetResourcePermission for its ARN still reports the grant made on the previous incarnation
