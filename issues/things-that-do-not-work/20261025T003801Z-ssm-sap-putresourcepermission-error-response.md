Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# PutResourcePermission stores a grant for an ARN the crate itself reports as nonexistent, justified by a stale comment

_Filed: 2026-10-25T00:38:01Z_
_Operation: PutResourcePermission | Class: error-response_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutResourcePermission` does not resolve its `ResourceArn` against stored state at all: any well-formed ARN gets a grant and a 200 with a rendered `Policy`, so the `ResourceNotFoundException` this operation models is unreachable. The handler justifies that with a comment whose factual claim is FALSE in this crate: it says the emulator "holds no database or component registry (both `GetDatabase` and `ListDatabases` are still scaffolds), so it cannot tell a target that exists from one that does not". `ListDatabases` is implemented in this very crate (`list_databases.rs` has a real handler and no `SCAFFOLDED, NOT IMPLEMENTED` header), databases ARE derived from the registration by `StoredApplication::databases`, and `RegionState::holds_resource` already answers exactly the question the comment claims cannot be answered — resolving an ARN to either a registered application or one of its derived databases.

The sibling handler needing the same resolution does call it: `ListTagsForResource` rejects an ARN that names neither with the modelled `ResourceNotFoundException` built by `crate::state::resource_not_found_message`. So one path in this crate uses the existing helper and the resource-permission paths skip it, which means the same ARN is simultaneously "doesn't exist" to `ListTagsForResource` (404) and a legitimate permission target to `PutResourcePermission` (200, grant persisted). `GetResourcePermission` carries the identical stale comment and the identical omission (`get_resource_permission.rs:162-167`), so the fix belongs on both paths.

Note for the fixer: `tests/all/resource_permissions.rs` grants on database ARNs of applications that were never registered, so its fixtures must register the target application first; the rule that module states it pins ("the `Policy` either operation reports is PROJECTED from the triples held for that one target") is unaffected by resolving the target.

## Evidence

The false justification, `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/put_resource_permission.rs:161-167`:
```
// `ActionType`, `ResourceArn` and `SourceResourceArn` are all modelled
// `@required` with their own constraints, so reaching here means each is
// present and well formed. Nothing else about the two ARNs can be checked
// against state: this emulator holds no database or component registry (both
// `GetDatabase` and `ListDatabases` are still scaffolds), so it cannot tell a
// target that exists from one that does not, and answering the modelled
// `ResourceNotFoundException` here would refuse requests that are valid.
```
But `ListDatabases` is NOT a scaffold — `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_databases.rs:3-10`:
```
//! @op ListDatabases
//!
//! Reports one `DatabaseSummary` per SAP HANA database of the registered applications
//! this request reaches, computed from state: the databases an application has are the
//! ones its registration named ([`crate::state::StoredApplication::databases`]),
```
(the scaffold header `//! SCAFFOLDED, NOT IMPLEMENTED:` appears in `get_database.rs:4` but not in `list_databases.rs`).

The helper the comment says does not exist, `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:980-1006`:
```
/// Whether `arn` names a resource THIS Region holds.
///
/// The resources AWS Systems Manager for SAP holds are the registered SAP
/// application and the SAP HANA databases of that application, so an ARN resolves
/// here when it is a registered application's own ARN
/// ([`StoredApplication::arn`]) or the ARN of one of the databases derived from
/// that registration ([`StoredApplication::databases`])
pub fn holds_resource(&self, region: &str, arn: &str) -> bool {
```
The sibling that calls it and answers the modelled 404, `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_tags_for_resource.rs:171-184`:
```
// `ResourceArn` is modelled `@required` with a `@pattern`, so reaching here means
// the reference is a well-formed ARN. It still has to NAME something: ...
// so an ARN naming neither is the
// `ResourceNotFoundException` this operation models rather than an empty tag set
// reported for a resource that does not exist.
if !region_state.holds_resource(&region, &input.resource_arn) {
    return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
        crate::state::not_found(crate::state::resource_not_found_message(
```
While `put_resource_permission.rs:184-188` writes unconditionally:
```
let outcome = state.region_entry(&region).grant_resource_permission(
    &input.resource_arn,
    &input.action_type,
    &input.source_resource_arn,
);
```
The same stale claim is repeated in `src/ops/get_resource_permission.rs:162-167` ("`GetDatabase` and `ListDatabases` are still scaffolds").

Test coverage today: `tests/all/resource_permissions.rs:59-65` grants on `arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/HDB-TEST/HDB` without ever registering `HDB-TEST`, and asserts HTTP 200 — i.e. the unresolved-target path is exercised but never asserted as a documented rule.

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"HDB-TEST","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"secret"}]} — 200.
2. POST /list-databases with {"ApplicationId":"HDB-TEST"} — the derived database and its ARN are reported, proving the crate knows which database ARNs exist.
3. GET /tags/arn%3Aaws%3Assm-sap%3Aus-east-1%3A<account>%3Adatabase%2FNOPE%2FNOPE — 404 ResourceNotFoundException ("Resource ... doesn't exist."), because ListTagsForResource calls `holds_resource`.
4. POST /put-resource-permission with {"ResourceArn":"arn:aws:ssm-sap:us-east-1:<account>:database/NOPE/NOPE","ActionType":"RESTORE","SourceResourceArn":"arn:aws:ssm-sap:us-east-1:<account>:database/HDB-TEST/HDB"} — 200 with a Policy, and the grant is persisted for the SAME ARN step 3 just reported as nonexistent.
5. POST /get-resource-permission with {"ResourceArn":"arn:aws:ssm-sap:us-east-1:<account>:database/NOPE/NOPE"} — the policy is reported back; the modelled ResourceNotFoundException is never reachable on either path.
