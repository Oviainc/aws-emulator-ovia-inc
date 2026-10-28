Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc8acfad318c0cd148284152a9b68532fd3f8c27

# RegisterApplication stores duplicate credentials for one (DatabaseName, CredentialType), breaking the crate's own credential identity rule

_Filed: 2026-10-28T15:54:19Z_
_Operation: RegisterApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc8acfad318c0cd148284152a9b68532fd3f8c27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegisterApplication` persists the request's `Credentials` list verbatim, with no collapsing of two entries that name the SAME credential. This crate defines a credential's identity as (SAP HANA database, `CredentialType`) and every OTHER path maintains "at most one credential per identity": `UpdateApplicationSettings` (via `StoredApplication::credentials_after`) REPLACES a credential of an identity it already holds instead of appending a second one, and the registration path itself collapses repeated declarations of every other kind (components, EC2 instance ids, configuration check ids, databases).

Since `CredentialType` has exactly one modelled value (`ADMIN`), two credential entries naming the same `DatabaseName` are always the same credential by `same_credential`. A registration carrying two such entries with different `SecretId`s is accepted and both are stored, so the application ends up holding two contradictory secrets for ONE database:

* `GetDatabase` reports both in `Database$Credentials` (`database_view` filters `self.credentials` by database id, so both match), while `ListDatabases`/`GetDatabase` report ONE database for them (`StoredApplication::databases` deduplicates by `DatabaseId`).
* A later `UpdateApplicationSettings` with `CredentialsToAddOrUpdate` for that database replaces only the FIRST copy (`credentials_after` uses `find`), so the application keeps two different `SecretId`s for the same database and type — a state the crate's own documentation says cannot exist.
* The same duplicate pair sent through `UpdateApplicationSettings` instead of `RegisterApplication` yields ONE credential, so the two write paths disagree about the same input.

The registration path should collapse credentials by the identity `same_credential` already defines (the way it collapses a repeated component declaration), so the stored set honours the invariant the update path enforces.

## Evidence

`services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs:299` stores the list unchanged:
```rust
            credentials: input.credentials.unwrap_or_default(),
```

`services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:610-616` states the rule the stored set is supposed to satisfy:
```
    /// A credential is identified by the SAP HANA database it reaches and its
    /// `CredentialType`, by the same identity a database itself has here
    /// ([`database_id_from_name`]), so a credential named for the database and type of
    /// one this application already holds REPLACES it (the secret the new one names is
    /// the one that reaches that database from then on) and one naming a database and
    /// type it holds none for is ADDED, after the ones it keeps.
```
and `state.rs:672-680` implements it for the update path, replacing only the first match (correct only if at most one exists):
```rust
        for named in add_or_update {
            match credentials
                .iter_mut()
                .find(|held| same_credential(held, named))
            {
                Some(held) => *held = named.clone(),
                None => credentials.push(named.clone()),
            }
        }
```

The same registration handler DOES collapse every other repeated declaration, which is the sibling treatment credentials skip — `state.rs:3359-3363` (`registered_components`):
```rust
    for component in components_info {
        let id = component_id(&component.sid, &component.component_type);
        if components.iter().any(|stored| stored.id == id) {
            continue;
        }
```
`state.rs:729` (`ec2_instance_ids`): `if id.is_empty() || ids.iter().any(|known| known == id) { continue; }`; `state.rs:810-812` (`databases`): "Keyed by `DatabaseId`, which both deduplicates and puts the databases in id order".

Identity is defined once, in `state.rs:3495-3498`:
```rust
pub fn same_credential(one: &ApplicationCredential, other: &ApplicationCredential) -> bool {
    one.credential_type == other.credential_type
        && database_id_from_name(&one.database_name) == database_id_from_name(&other.database_name)
}
```
and `CredentialType` has a single value (`src/ops/types/shared_types.rs:161`: `pub const CREDENTIAL_TYPE_VALUES: &[&str] = &["ADMIN"];`), so same database name ⇒ same credential.

Test coverage: the crate pins the collapsing of every other repeated registration input — `tests/all/application_lifecycle.rs:563` ("Two components, one of them a duplicate declaration of the first."), `tests/all/ec2_instance_shutdown.rs:291` (`instance-named-twice`), `tests/all/configuration_checks.rs:172` ("the same check named twice is one check to perform") — but no test registers two credentials for one database, which is why this path is unguarded (`grep -i duplicate\|twice tests/` shows no credential case).

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"HDB-DUP","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"secret-one"},{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"secret-two"}]} — the call is accepted.
2. POST /list-databases with {"ApplicationId":"HDB-DUP"} — ONE database (`DatabaseId` HDB) is reported, confirming the two entries name one database.
3. POST /get-database with {"ApplicationId":"HDB-DUP","DatabaseId":"HDB"} — observe `Database.Credentials` holding TWO ADMIN credentials for that one database, with `SecretId` `secret-one` and `secret-two`. Expected (per state.rs:610-616): one credential for that (database, type).
4. POST /update-application-settings with {"ApplicationId":"HDB-DUP","CredentialsToAddOrUpdate":[{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"secret-three"}]}, wait out TRANSITION_SPAN, then POST /get-database again — only the first copy was replaced, so the application now reports `secret-three` AND `secret-two` for the same database and type.
5. Contrast: register the application with a single credential and send the SAME duplicate pair through /update-application-settings `CredentialsToAddOrUpdate` — the update path collapses it to one credential, so the two write paths disagree about identical input.
