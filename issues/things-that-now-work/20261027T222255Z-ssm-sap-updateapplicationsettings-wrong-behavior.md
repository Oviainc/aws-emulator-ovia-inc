Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# UpdateApplicationSettings destroys a derived database without dropping the tags and resource permissions held on its ARN

_Filed: 2026-10-27T22:22:55Z_
_Operation: UpdateApplicationSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An application's SAP HANA databases are DERIVED from the registration record: `StoredApplication::databases` builds one entry per `database_arn` and per `ApplicationCredential$DatabaseName`, and each entry's ARN is the key its tags (`RegionState::tags_by_arn`) and its grants (`RegionState::resource_permissions`) are held under.

`UpdateApplicationSettings` can DESTROY such a database: `CredentialsToRemove` drops the credential that was the only mention of a database, and `DatabaseArn` REPLACES the application's SAP HANA connection, so the previously named database stops being derived. The crate already owns the cleanup for this situation and calls it on exactly one path — `RegionState::settle` drops `tags_by_arn` and `resource_permissions` for every ARN of `StoredApplication::resource_arns` when a deregistration completes, because "a database ... cannot outlive the registration it came from and neither can what was tagged on it or granted on it". The settings path calls neither `remove_tags` nor `revoke_resource_permission` and touches neither map.

Three observable consequences:

1. Unreachable-but-retained state. After the credential is removed, `RegionState::holds_resource` no longer resolves that ARN, so `ListTagsForResource`, `UntagResource`, `GetResourcePermission` and `DeleteResourcePermission` all answer 404 for it — the caller can neither read nor delete what is still held.
2. Resurrection onto a NEW resource. The minted ARN is a pure function of (region, application id, database id) (`database_arn`), so re-adding a credential for the same `DatabaseName` recreates the database with the same ARN, and `ListDatabases`/`GetDatabase` then report it carrying the tags that were set on the destroyed database (and `GetResourcePermission` re-renders the old RESTORE policy). A tag/grant outlives the resource it was attached to.
3. The Region partition is never reclaimed. `RegionState::is_empty` counts `tags_by_arn`/`resource_permissions`, so the orphan entry survives even the later `DeregisterApplication` (its cleanup enumerates the resource ARNs the application has AT REMOVAL TIME, which no longer include the destroyed database), leaving a partition that holds something no API can observe or remove and that keeps counting against `MAX_REGIONS`.

## Evidence

The cleanup the crate performs when a derived database disappears by deregistration — `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2528`:

```
// What is held PER RESOURCE goes with it for every resource of that
// registration — the application's own ARN and the ARNs of the resources
// DERIVED from it ([`StoredApplication::resource_arns`]), since a database or a
// component cannot outlive the registration it came from and neither can what
// was tagged on it or granted on it.
```

and the code under it, `state.rs:2545`:

```
for (id, arns) in completed {
    self.applications.remove(&id);
    for arn in arns {
        self.tags_by_arn.remove(&arn);
        // A resource permission is held ON a resource, so it cannot outlive
        // that resource either ...
        self.resource_permissions.remove(&arn);
    }
```

The settings path destroys the same kind of resource and runs no such cleanup — `state.rs:585` (`StoredApplication::settle`):

```
PendingChange::Settings(settings) => {
    self.credentials = settings.credentials;
    ...
    if let Some(database_arn) = settings.database_arn {
        // The named database REPLACES the application's current SAP HANA
        // connection, so it is this application's database from here on
        // ([`Self::databases`]).
        self.database_arn = Some(database_arn);
    }
}
```

written there by `src/ops/update_application_settings.rs:298`:

```
application.pending = Some(crate::state::PendingTransition {
    due_at: now + crate::state::TRANSITION_SPAN,
    change: crate::state::PendingChange::Settings(crate::state::PendingSettings {
        credentials,
        backint: input.backint,
        database_arn: input.database_arn,
    }),
});
```

That the database set is exactly those two mentions — `state.rs:790` (`databases`): the `database_arn` entry is inserted at `state.rs:805` and one entry per credential at `state.rs:819`, with `arn: database_arn(region, &self.id, &database_id)` (`state.rs:826`); `state.rs:944` (`resource_arns`) then feeds `holds_resource` (`state.rs:2724`), the one resolver the tagging and permission calls use (`src/ops/tag_resource.rs:178`, `src/ops/untag_resource.rs:180`, `src/ops/list_tags_for_resource.rs:179`, `src/ops/put_resource_permission.rs:183`, `src/ops/delete_resource_permission.rs:194`).

The maps are touched nowhere else: `grep -n 'remove_tags|tags_by_arn.remove|revoke_resource_permission|resource_permissions.remove' src` yields only `state.rs:2548`, `state.rs:2552`, `src/ops/untag_resource.rs:198` (`remove_tags`) and `src/ops/delete_resource_permission.rs:214` (`revoke_resource_permission`) — no settings path.

Reclamation is blocked because `state.rs:2591` counts the orphan:

```
pub fn is_empty(&self) -> bool {
    self.applications.is_empty()
        && self.operations.is_empty()
        && self.configuration_check_operations.is_empty()
        && self.tags_by_arn.is_empty()
        && self.resource_permissions.is_empty()
}
```

which `state.rs:3151` (`self.regions.retain(|_, region| !region.is_empty());`) reads, under the doc at `state.rs:3144` ("a partition that holds nothing is not one this store has to keep").

Test coverage: `tests/all/resource_tags.rs:137` pins that a database sub-resource ARN (`arn:aws:ssm-sap:{region}:{ACCOUNT}:database/{application_id}/{database_id}`) is a taggable resource, and `tests/all/update_application_settings.rs` exercises credential add/update/remove — but `grep -n 'tag|Tag' tests/all/update_application_settings.rs` returns no matches, so no test combines the two and nothing pins the current leak.

## Steps to Reproduce

1. POST /register-application {"ApplicationId":"HDB-A","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"s0"},{"DatabaseName":"TNT1","CredentialType":"ADMIN","SecretId":"s1"}]}
2. POST /list-databases {"ApplicationId":"HDB-A"} and note the ARN reported for TNT1: arn:aws:ssm-sap:us-east-1:<account>:database/HDB-A/TNT1
3. POST /tags/<that ARN> {"tags":{"env":"dev"}} -> 200; GET /tags/<that ARN> -> {"tags":{"env":"dev"}}
4. POST /update-application-settings {"ApplicationId":"HDB-A","CredentialsToRemove":[{"DatabaseName":"TNT1","CredentialType":"ADMIN","SecretId":"s1"}]} -> 200, then make any request after TRANSITION_SPAN so the settings settle
5. POST /list-databases {"ApplicationId":"HDB-A"} -> TNT1 is gone. GET /tags/<that ARN> -> 404 ResourceNotFoundException and DELETE /tags/<that ARN>?tagKeys=env -> 404: the tag set is still held but no call can read or remove it
6. POST /update-application-settings {"ApplicationId":"HDB-A","CredentialsToAddOrUpdate":[{"DatabaseName":"TNT1","CredentialType":"ADMIN","SecretId":"s2"}]}, let it settle, then POST /list-databases {"ApplicationId":"HDB-A"} -> TNT1 is reported again carrying Tags {"env":"dev"}, the tags of the database that was destroyed in step 4 (expected: a freshly derived database carries no tags, as the deregistration path guarantees for every derived resource)
7. POST /deregister-application {"ApplicationId":"HDB-A"} after step 4 (without step 6) and let the removal settle: the Region's partition still holds the orphan tags_by_arn entry, so RegionState::is_empty stays false and the partition is never reclaimed even though nothing observable remains in it
8. The same sequence with PutResourcePermission on the TNT1 ARN instead of TagResource leaves an unrevocable grant that is re-rendered by GetResourcePermission once the credential is re-added
