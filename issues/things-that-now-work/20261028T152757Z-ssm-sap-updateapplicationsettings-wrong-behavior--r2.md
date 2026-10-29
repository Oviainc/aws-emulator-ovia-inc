Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ea262c1f0c770608da2cb6c8011fa8bda70e1973

# Application$AssociatedApplicationArns is never written by the handler the crate names as its owner, so the member is permanently dead

_Filed: 2026-10-28T15:27:57Z_
_Operation: UpdateApplicationSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ea262c1f0c770608da2cb6c8011fa8bda70e1973`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StoredApplication::associated_application_arns` is written exactly once in the whole crate — as `Vec::new()` by `RegisterApplication` — and the comment there states WHO is supposed to fill it: "Associating applications with one another is `UpdateApplicationSettings`' business, so a freshly registered application is associated with none."

`UpdateApplicationSettings`' handler never touches it. The change it files (`PendingChange::Settings(PendingSettings { credentials, backint, database_arn })`) carries only those three members, and `StoredApplication::settle` applies only those three. No other handler writes the field either (grep over the crate: three mentions in `state.rs`, one in `register_application.rs`, none anywhere else).

The consequence is that `Application$AssociatedApplicationArns` ("The Amazon Resource Names of the associated AWS Systems Manager for SAP applications") is absent from EVERY `GetApplication`/`RegisterApplication` response for the lifetime of the store, and the non-empty branch of the projection in `StoredApplication::view` is unreachable code: a field whose stored half can only ever be empty is projected through a conditional that can only ever take the `None` arm. Either the handler the comment names has to write it, or the comment and the dead projection branch are wrong about where it comes from — as it stands the crate documents a rule one of its handlers owns and that handler does not implement it.

## Evidence

The comment that assigns the behaviour, `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs:292-294`:
```rust
            // Associating applications with one another is `UpdateApplicationSettings`'
            // business, so a freshly registered application is associated with none.
            associated_application_arns: Vec::new(),
```

The handler it names, which never writes the field — `src/ops/update_application_settings.rs:298-305` is the whole of what an update changes:
```rust
        application.pending = Some(crate::state::PendingTransition {
            due_at: now + crate::state::TRANSITION_SPAN,
            change: crate::state::PendingChange::Settings(crate::state::PendingSettings {
                credentials,
                backint: input.backint,
                database_arn: input.database_arn,
            }),
        });
```
and `src/state.rs:585-597` applies only those three on settling (`self.credentials = settings.credentials;` / `if let Some(backint) ...` / `if let Some(database_arn) ...`).

Every mention of the field in the crate (grep `associated_application_arns`): `src/state.rs:532` (the declaration), `src/state.rs:749-753` (the projection), `src/ops/types/shared_types.rs:50` (the wire member), `src/ops/register_application.rs:294` (the only write, empty). So the unreachable branch is `src/state.rs:749-753`:
```rust
            associated_application_arns: if self.associated_application_arns.is_empty() {
                None
            } else {
                Some(self.associated_application_arns.clone())
            },
```
The member it feeds is modelled: `Application$AssociatedApplicationArns` — "The Amazon Resource Names of the associated AWS Systems Manager for SAP applications."

Test coverage: `tests/all/update_application_settings.rs` covers credentials, `Backint` and `DatabaseArn` only (the three members `PendingSettings` carries); no test in the crate asserts anything about `AssociatedApplicationArns`, so nothing pins the field's permanent absence.

## Steps to Reproduce

1. POST /register-application {"ApplicationId":"hdb-assoc","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"sap/hdb/system"}]}
2. Register a second application of type SAP_ABAP whose DatabaseArn names the first application's SAP HANA database ARN
3. Wait out TRANSITION_SPAN, then POST /update-application-settings {"ApplicationId":"abap-assoc","DatabaseArn":"<the HANA database ARN>"} — 200
4. POST /get-application {"ApplicationId":"abap-assoc"} (and for the HANA application): Application.AssociatedApplicationArns is absent in both, and no sequence of calls can make it present, although register_application.rs:292-294 states UpdateApplicationSettings is what associates applications
