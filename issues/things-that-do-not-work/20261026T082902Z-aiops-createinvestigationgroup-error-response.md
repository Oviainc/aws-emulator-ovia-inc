Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# ValidationException messages echo the caller's map key in the member path, contradicting the crate's single-homed refusal rule

_Filed: 2026-10-26T08:29:02Z_
_Operation: CreateInvestigationGroup | Class: error-response_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate single-homes one rule for how it words a 4xx: a `ValidationException` states the member and the requirement and copies NOTHING the request sent. `state::validation_exception_message` implements that by dropping `violation.value` — but it passes `violation.path` through verbatim, and for a violation inside a MAP the framework builds that path out of the caller's own map key (echoed exactly as sent, by the framework's explicit design, which also notes that a real service reports the map member with no key at all).

So a `CreateInvestigationGroup` whose `tags` map carries a value outside `TagValue` (@length 1..256, pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) is refused with `1 validation error detected: Value at 'tags.<the key the caller sent>' failed to satisfy constraint: Member must have length less than or equal to 256` — the caller's bytes copied into the message the crate promises never carries them. The same path construction applies to `chatbotNotificationChannel`, whose entries are keyed by a caller-supplied SNS topic ARN, on this operation and on `UpdateInvestigationGroup`.

The defect is in the one renderer every operation's refusal goes through (`service::override_refusal` -> `state::validation_exception_message`), so it is a property of the wording rule, not of one call site; every scalar member the crate's tests cover is unaffected, which is why nothing pins it today.

## Evidence

The rule, single-homed — `services/service-com-amazonaws-aiops-aiops/src/state.rs:314-319`:
```
// Neither carries anything the request sent. A `ValidationException` names the member
// and the requirement it violated and withholds the value that violated it;
// a `ResourceNotFoundException` names the RESOURCE TYPE that is absent and withholds
// the identifier that named it. Recorded 4xx responses from this service are built
// that way across every operation, and both messages are the whole machine-readable
// detail a client gets, so a caller's own bytes must not be copied into them.
```
and `src/state.rs:452-456`:
```
/// One clause of a `ValidationException` message: the member whose value violated a
/// constraint, and the requirement it violated — and NOTHING of the value itself.
pub fn constraint_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}'{CONSTRAINT_SEPARATOR}{requirement}")
```
The code that breaks it — `src/state.rs:496-507` (reached from `src/service.rs:120-124`'s `Refusal::Invalid(violations) => ... validation_exception_message(violations)`): the violation's `value` is dropped but its `path` is handed straight to the clause builders:
```
            match violation.value {
                // No value at that path: the member was absent from the request.
                None => absent_member_clause(&violation.path, &requirement),
                Some(_) => constraint_clause(&violation.path, &requirement),
            }
```
What that path contains for a map — `vendor/smithy-schema-server/src/constraints.rs:88-92`:
```
    /// A map entry names its key, then the collection step. The key is a value
    /// the CALLER chose, so it is echoed exactly as sent — never re-cased the
    /// way a model name is.
    pub fn entry(key: &str) -> String {
```
and `:1137-1148`:
```
            CollectionFrame::Map { pending_key, .. } => {
                let key = pending_key.clone().unwrap_or_default();
                match child {
                    CollectionChild::Descending => Some(path::entry(&key)),
                    // The entry's value IS what failed. A service reports this at
                    // the map member with a bundled requirement and no key at all;
                    // we report the key instead.
                    CollectionChild::Terminal => Some(key),
```
Reachable input: `smithy_query(query="shape",shape="Tags")` gives the map value as `{"target": "TagValue", "length": {"min": 1, "max": 256}, ...}`, and `tags` is a modelled member of `CreateInvestigationGroupInput`.
Test coverage: `tests/all/error_message_rules.rs:12-19` states the rule under test — "A `ValidationException` message names the member and the requirement it violated ... and carries NOTHING of the value that violated it" — and lists the members its rows span: "(`name`, `roleArn`, `retentionInDays`, `encryptionConfiguration.type`, `identifier`, `maxResults`, `policy`)". All seven are scalars; no row refuses a member inside `tags` or `chatbotNotificationChannel`, so the map case is unpinned.

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with {"name":"g","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole","tags":{"CostCentre-4711":"<a 300-character value>"}}
2. Observe the 400 ValidationException body: $.message reads "1 validation error detected: Value at 'tags.CostCentre-4711' failed to satisfy constraint: Member must have length less than or equal to 256" — the caller-supplied key `CostCentre-4711` is copied into the message
3. Compare with src/state.rs:314-319, which states the message "carries nothing the request sent" and that "a caller's own bytes must not be copied into them"
4. Repeat with {"chatbotNotificationChannel":{"arn:aws:sns:us-west-2:000000000000:my-private-topic":["a","b","c","d","e","f"]}} (the list exceeds ChatConfigurationArns' modelled 1..5): the refusal names the caller's topic ARN in the member path
