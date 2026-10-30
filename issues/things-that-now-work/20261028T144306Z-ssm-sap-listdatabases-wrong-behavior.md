Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a13c4bd7833d49550a5f70ddd7b2f501243c6c8c

# Database identity is case-sensitive although the crate states SAP HANA database names are case-insensitive, so one database named in two cases derives two databases

_Filed: 2026-10-28T14:43:06Z_
_Operation: ListDatabases | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a13c4bd7833d49550a5f70ddd7b2f501243c6c8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`database_type_for` decides a database's `DatabaseType` with a case-INSENSITIVE comparison and justifies it in its own doc: HANA database names are case-insensitive. Every other place that decides WHICH database a name refers to is case-SENSITIVE: `database_id_from_name` preserves case, `StoredApplication::databases` deduplicates on that case-sensitive `DatabaseId`, and `same_credential` compares credentials by it.

So the two halves of the same rule disagree about whether two names differing only in case are one database:
- An application registered with credentials for `SYSTEMDB` and `systemdb` derives TWO `ApplicationDatabase` records, with two `DatabaseId`s and two distinct ARNs, and `ListDatabases` / `GetDatabase` report BOTH with `DatabaseType` `SYSTEM` — i.e. one HANA system with two system databases, which contradicts the singular rule `database_type_for` states ("The system database of a HANA system is the one named SYSTEMDB").
- `UpdateApplicationSettings` refuses to remove the credential of a database the application holds under another case: an application holding the `ADMIN` credential for `SYSTEMDB` answers a `CredentialsToRemove` entry naming `systemdb` with "Application X has no ADMIN credential for database systemdb", although by the case-insensitivity the crate asserts the two name one database.

Note the asymmetry is specific to CASE: `database_id_from_name` already normalises every character the modelled `DatabaseId` pattern rejects to `_`, so names differing in punctuation ARE collapsed to one database (pinned by `tests/all/list_databases.rs:118-126`), while names differing only in case are not.

## Evidence

The case-insensitive half, `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:3513-3524`:
```rust
/// The `DatabaseType` of the SAP HANA database named `database_name`.
///
/// The system database of a HANA system is the one named [`HANA_SYSTEM_DATABASE_NAME`]
/// (HANA database names are case-insensitive); every other database of the system is a
/// tenant database.
pub fn database_type_for(database_name: &str) -> &'static str {
    if database_name.eq_ignore_ascii_case(HANA_SYSTEM_DATABASE_NAME) {
        DATABASE_TYPE_SYSTEM
```

The case-sensitive half that decides identity, `state.rs:3465-3485`:
```rust
/// The `DatabaseId` of the SAP HANA database named `database_name`.
///
/// A database is identified by its name, so its id is a function of that name: every
/// character the model's `DatabaseId` pattern (`[\w\d]+$`) does not admit is replaced by
/// `_` ...
pub fn database_id_from_name(database_name: &str) -> String {
    database_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
```
(no case folding), used as the dedup key in `state.rs:799-845`:
```rust
    /// A database is identified by its name ([`database_id_from_name`]), so two of those
    /// mentions naming the same database are ONE database ...
        // Keyed by `DatabaseId`, which both deduplicates and puts the databases in id
        // order ...
        for credential in &self.credentials {
            let database_id = database_id_from_name(&credential.database_name);
            databases
                .entry(database_id.clone())
```
and as credential identity in `state.rs:3495-3498`:
```rust
pub fn same_credential(one: &ApplicationCredential, other: &ApplicationCredential) -> bool {
    one.credential_type == other.credential_type
        && database_id_from_name(&one.database_name) == database_id_from_name(&other.database_name)
```
which is what `UpdateApplicationSettings` refuses a removal through (`state.rs:641-649`): `"Application {} has no {} credential for database {}"`.

The listing that reports the duplicate: `src/ops/list_databases.rs:186-197` pages over `application.databases(&region)`.

Test coverage: `tests/all/list_databases.rs:100-108` pins only the single-name lower-case case (`database_names: &["systemdb"]` -> `("systemdb", "SYSTEM")`); no test registers the same database name in two cases, so nothing pins the duplicate.

## Steps to Reproduce

1. POST /register-application {"ApplicationId":"HDB-CASE","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"sap/hdb/s1"},{"DatabaseName":"systemdb","CredentialType":"ADMIN","SecretId":"sap/hdb/s2"}]}
2. POST /list-databases {"ApplicationId":"HDB-CASE"} — two DatabaseSummary entries come back, DatabaseId "SYSTEMDB" and DatabaseId "systemdb", each with its own ARN and both with DatabaseType "SYSTEM", so one HANA system reports two system databases
3. Register a second application with only the SYSTEMDB credential: POST /register-application {"ApplicationId":"HDB-CASE2","ApplicationType":"HANA","Instances":["i-0123456789abcdef1"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"sap/hdb/s1"}]}
4. Wait out TRANSITION_SPAN, then POST /update-application-settings {"ApplicationId":"HDB-CASE2","CredentialsToRemove":[{"DatabaseName":"systemdb","CredentialType":"ADMIN","SecretId":"sap/hdb/s1"}]} — refused 400 "Application HDB-CASE2 has no ADMIN credential for database systemdb", although `database_type_for` states the two names are the same HANA database
