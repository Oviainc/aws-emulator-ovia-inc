Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# A malformed attachPoint is answered as a missing resource (404) instead of the invalid-ARN validation refusal this crate already words

_Filed: 2026-10-28T21:56:13Z_
_Operation: CreateConnection | Class: error-response_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateConnection's attach-point resolution never holds the `AttachPoint` union's members to their own modelled constraints and never separates "this value is not an attach-point identifier at all" from "this identifier names no attach point of the account".

* `attachPoint.arn` is handed straight to `store.connection_in_by_arn`, which starts with `connection_id_from_arn` -> `parse_connection_arn`; anything that is not an Interconnect connection ARN (a bare word, another service's ARN, a 10-character string) parses to `None`, so `is_none()` is true and the request is refused with `ResourceNotFoundException` (404) worded "<value> is not an attach point valid for environment <id>" — i.e. a malformed request is reported as a missing resource.
* `attachPoint.directConnectGateway` is never length/pattern checked either (the member is `@length(36,36)` with the UUID `@pattern`); a value outside that goes out over the cross-service Direct Connect walk and comes back as the same 404 (or, when the walk fails, as `InterconnectServerException` 500).

This crate states both halves of the rule it is skipping. First, that the framework does not do this for a union member, which is why it hand-checks the sibling union `RemoteAccountIdentifier` in the very same handler (`remote_account_violation`). Second, that an `arn` which "is not the ARN of an Interconnect connection" is "a fault in the request rather than a missing resource" — the crate owns a dedicated message for it (`not_a_connection_arn_message`) and all three ARN-addressed tagging handlers call it to answer `InterconnectValidationException` (400) before they ever look a resource up. The result is that one and the same malformed ARN is a 400 validation fault when sent to TagResource/UntagResource/ListTagsForResource and a 404 "no such attach point" when sent as `CreateConnection$attachPoint.arn`, and a caller cannot tell "I sent garbage" from "the account does not hold that attach point".

The identical branch is copied into AcceptConnectionProposal (accept_connection_proposal.rs:278-292), so the fix belongs in the shared resolution both writers use.

Distinct from the two existing CreateConnection/validation issues: 20261028T013820Z-...-validation--r2 was about which REGION an otherwise well-formed ARN names, and 20261028T151652Z-...-validation--r3 is about the STATE of the connection a well-formed ARN resolves to. Neither touches a value that is not an attach-point identifier at all, and neither is about the class of the answer.

## Evidence

The create path, which turns every unparseable `arn` into a not-found — services/service-com-amazonaws-interconnect-interconnect/src/ops/create_connection.rs:308-320:
```
    if let AttachPoint::Arn(arn) = &input.attach_point {
        if store.connection_in_by_arn(region, arn).is_none() {
            return Err(CreateConnectionErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::attach_point_not_valid_message(
                        arn,
                        &environment.environment_id,
                    )),
```
what `connection_in_by_arn` does with a value that is not an ARN — src/state.rs:442-447 and src/state.rs:816-821:
```
    pub fn connection_in_by_arn(&self, region: &str, arn: &str) -> Option<&StoredConnection> {
        let id = connection_id_from_arn(arn)?;
```
```
pub fn parse_connection_arn(arn: &str) -> Option<ConnectionArn<'_>> {
    let (qualifier, id) = arn.rsplit_once('/')?;
    let mut fields = qualifier.split(':');
    if fields.next() != Some("arn") {
        return None;
```
the classification the crate states for exactly that value, and never applies here — src/state.rs:850-854:
```
/// How the service words an `arn` that is not an Interconnect connection ARN at
/// all, which is a fault in the request rather than a missing resource.
pub fn not_a_connection_arn_message(arn: &str) -> String {
    format!("{REFUSAL_ENVELOPE}{arn} is not the ARN of an Interconnect {ARN_RESOURCE_TYPE}")
}
```
the three sibling handlers that do apply it — src/ops/tag_resource.rs:159-168 (identical at src/ops/untag_resource.rs:161-168 and src/ops/list_tags_for_resource.rs:165-174):
```
        // An ARN the boundary validator let through that still does not
        // parse as one is refused as invalid INPUT, never with a 501.
        let Some(parsed) = crate::state::parse_connection_arn(&input.arn) else {
            return Err(TagResourceErrorModeled::InterconnectValidationException(
                InterconnectValidationException {
                    message: crate::state::not_a_connection_arn_message(&input.arn),
                },
            )
            .into());
```
the crate's own statement that a union member's modelled constraints are NOT enforced before the handler, which is why the sibling union in this same operation is hand-checked — src/state.rs:569-578:
```
/// Whether `identifier` is built from the characters the member's pattern
/// (`^[-a-zA-Z0-9_@\.]+$`) allows: ASCII letters and digits, `-`, `_`, `@` and `.`.
///
/// Checked here because the boundary validator holds a top-level member to its
/// target's constraints but not a member one level down, inside a union.
fn is_remote_identifier_text(identifier: &str) -> bool {
```
called from create_connection.rs:271-278 (`remote_account_violation`), whose refusal is `InterconnectValidationException`.

The model's constraints on the members nothing checks — `smithy_query(query="shape",shape="AttachPoint")`: `arn` -> `{"length": {"min": 59, "max": 150}, "pattern": "^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$"}`, `directConnectGateway` -> `{"length": {"min": 36, "max": 36}, "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"}`.

Test coverage: the attach-point suite spans only WELL-FORMED identifiers that resolve to nothing — tests/all/create_connection_attach_point_validation.rs:153-182 lists v4/all-digit/all-hex gateway UUIDs and three syntactically valid connection ARNs, all asserted 404 at :189-197. No row sends a value outside the members' own constraints, so the behaviour above is unpinned. The crate's companion rule for the other union IS pinned, at tests/all/create_connection_remote_account.rs:266-288 ("A value outside the member's own modelled constraints is refused", asserting 400 and a message naming `remoteAccount.identifier`).

## Steps to Reproduce

1. Mount Interconnect beside Direct Connect (as tests/all/emulator.rs does) and create one ready Direct Connect gateway.
2. Pick a seeded environment that takes connections (e.g. env-csp-oraclecloud-eu-west-1, which asks for no remoteAccount) and a bandwidth it offers.
3. Call TagResource with arn="not-an-arn" and tags={"k":"v"}: observe HTTP 400 InterconnectValidationException "Unable to process request: not-an-arn is not the ARN of an Interconnect connection" (src/ops/tag_resource.rs:161-168).
4. Call CreateConnection on that environment with attachPoint={"arn": "not-an-arn"}: observe HTTP 404 ResourceNotFoundException "Unable to process request: not-an-arn is not an attach point valid for environment <id>" — the same malformed value classified as a missing resource instead of a request fault (create_connection.rs:308-320 via src/state.rs:442-447 and 816-821).
5. Call CreateConnection with attachPoint={"directConnectGateway": "abc"} (outside the member's @length(36,36)/UUID @pattern): observe the same 404, produced only because the cross-service gateway walk did not find "abc", rather than a refusal of the value itself.
6. Repeat step 4 against AcceptConnectionProposal (accept_connection_proposal.rs:278-292) with a live activationKey: the same 404 comes back from the copied branch.
