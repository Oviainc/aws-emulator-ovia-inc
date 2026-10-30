Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: bdfeee3e6512ca319f6cb10a0fb27e2039f01e12

# An empty tagKeys (and an empty tags) list is accepted and quietly changes nothing, the one thing this crate's own empty-value-list rule refuses

_Filed: 2026-10-29T15:33:15Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `bdfeee3e6512ca319f6cb10a0fb27e2039f01e12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource$tagKeys` is `@required` but targets `TagKeyList`, which carries NO list-level `@length` and whose member targets a plain `String` (`smithy_query(query="shape",shape="TagKeyList")` -> `{"type": "list", "member": "String"}`). An empty array satisfies `@required`, so `{"resourceArn": ..., "ruleRevision": 1, "tagKeys": []}` passes the generated boundary walk and reaches the handler.

The handler judges each KEY it was sent (`crate::state::tag_keys_violation`) but never the list itself, then runs `rule.tags.retain(|tag| !input.tag_keys.contains(&tag.key))` — which removes nothing for an empty list. The call answers 200 with an empty body, records the client token, and leaves the rule exactly as it was: a request that named nothing to remove is answered as though it had removed something.

That is exactly the outcome this crate refuses everywhere else it reads a caller-supplied value list, as its own single rule for such a list: an empty `accountIds` "is refused rather than acted on for a prefix of the list" (`src/state.rs:456-464`), an empty `recommendedActionTypes` "selects no action at all" and is refused (`src/state.rs:2808-2818`), and a criteria condition with no values "[cannot] select resources, so each is refused rather than quietly matching nothing" (`src/state.rs:2632-2636`, implemented at `src/state.rs:2768-2773`). `tagKeys` is the one REQUIRED caller-supplied value list in the crate that gets neither a boundary bound nor a handler check — and the handler's own comment is explicit that the boundary checks nothing for it, which is why its other two rules were brought into the handler.

`TagResource` has the identical shape and the same root cause: `TagList` carries `@length(max: 200)` only, so `"tags": []` reaches `src/ops/tag_resource.rs`, passes the duplicate-key and key-namespace checks, passes the revision check, and then rewrites the stored list with an identical copy (`src/ops/tag_resource.rs:262-273`) and answers 200. `UntagResource` is named here because its member exists solely to name what to remove, so an empty list is unambiguously a request that states nothing.

A second consequence is on the idempotency ledger: the no-op consumes the caller's `clientToken` (`src/ops/untag_resource.rs:258-263`), so the real removal, retried under that same token, is then classified `ParameterMismatch` and refused 409 `IdempotentParameterMismatchException` (`src/state.rs` `classify_token`) — a token spent on a call that did nothing.

## Evidence

The handler judges each key but never the list, and the comment states that the boundary judged NOTHING for this member — `src/ops/untag_resource.rs:175-186`:
```
        // `TagKeyList`'s member targets a plain `String`, so the boundary
        // validator admitted keys the service's own `TagKey` shape would refuse,
        // and no modelled constraint can express the reserved namespace either.
        // Both are judged here, BEFORE the store is touched, so a rejected
        // request removes nothing.
        if let Some(message) =
            crate::state::tag_keys_violation(input.tag_keys.iter().map(String::as_str))
        {
            // The requirement is the modelled code and its 400 status; the
            // message is the service's own and echoes no caller-supplied key.
            return Err(invalid_parameter(message));
        }
```
and what it judges — per KEY, never the list's size — `src/state.rs:429-442`:
```
pub fn tag_keys_violation<'a, I>(keys: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    for key in keys {
        if !tag_key_is_valid(key) {
            return Some(TAG_VALIDATION_FAILED_MESSAGE.to_string());
        }
        if tag_key_is_reserved(key) {
            return Some(reserved_tag_key_message());
        }
    }
    None
}
```
The removal itself, a no-op for an empty list — `src/ops/untag_resource.rs:252-256`:
```
        // Remove exactly the keys named; a key the rule does not carry leaves
        // the rule as it is, so the same request may be repeated (the operation
        // is modelled `@idempotent`). For the same reason the removal does not
        // advance `ruleRevision`: a repeat would then fail the check above.
        rule.tags.retain(|tag| !input.tag_keys.contains(&tag.key));
```
followed by the token spend and the 200 — `src/ops/untag_resource.rs:258-265`:
```
        guard.record_token(
            client_token,
            Self::NAME,
            &request_fingerprint,
            Some(crate::state::TokenSubject::Rule(input.resource_arn.clone())),
        );

        Ok(UntagResourceOutput {})
```

The crate's OWN rule for a caller-supplied value list that acts on nothing — `src/state.rs:2629-2636`:
```
/// The model types `comparison` and `values` as OPTIONAL members of every
/// condition and types `comparison` as the whole `ComparisonOperator` enum, so
/// the protocol boundary admits a condition with no operator, with no values, or
/// with an operator from the wrong family (a `Numeric*` operator on a string
/// criterion). None of those can select resources, so each is refused rather
/// than quietly matching nothing. The documented size caps
```
implemented at `src/state.rs:2768-2773`:
```
    if value_count == 0 {
        return Some(constraint_report(
            &format!("{path}.member.values"),
            "Member must have length greater than or equal to 1",
        ));
    }
```
the same rule for `accountIds` — `src/state.rs:453-464`:
```
/// the member reads "You can specify up to 50 account IDs"
/// ([`MAX_ACCOUNT_IDS_PER_REQUEST`]), so a request naming none, and one naming
/// more, is refused rather than acted on for a prefix of the list.
pub fn account_ids_violation(account_ids: &[String]) -> Option<String> {
    if account_ids.is_empty() {
        return Some(constraint_report(
            "accountIds",
            "Member must have length greater than or equal to 1",
        ));
    }
```
and for `recommendedActionTypes` — `src/state.rs:2808-2818`:
```
/// The list names the action types the rule may execute, so an empty one selects
/// no action at all and leaves a rule that automates nothing; the model carries
/// no `@length` on `RecommendedActionTypeList`, so the check lives here. The
/// preview operations refuse their own (required) list the same way.
pub fn recommended_action_types_violation(action_types: &[String]) -> Option<String> {
    if action_types.is_empty() {
        return Some(constraint_report(
            "recommendedActionTypes",
            "Member must have length greater than or equal to 1",
        ));
    }
```

The sibling tag-write path with the same hole — `src/ops/tag_resource.rs:180-196` checks only duplicates and key namespaces:
```
        // A tag list states one value per key, so a request naming the same key
        // twice states two values for one tag. Refused ahead of the token ledger
        // and of the store, so it consumes nothing and changes nothing. The same
        // check the create path makes of the `tags` it stores.
        if let Some(report) = crate::state::duplicate_tag_key_violation(&input.tags) {
            return Err(invalid_parameter(report));
        }
```
and then writes an identical list — `src/ops/tag_resource.rs:260-273`:
```
        // Computed off the STORED list before anything is written, so a request
        // the per-resource bound refuses leaves the rule exactly as it was.
        let merged = merge_tags(&rule.tags, &input.tags);
...
        rule.tags = merged;
```

Model facts: `smithy_query(query="shape",shape="TagKeyList")` -> `{"shape_id": "...#TagKeyList", "type": "list", "member": "String"}` (no list `@length`), and `TagList` -> `{"type": "list", "length": {"max": 200}, "member": "Tag"}` (no minimum), while `UntagResourceRequest$tagKeys` and `TagResourceRequest$tags` are both `@required`.

Test coverage: NONE. A search across `tests/` for `"tagKeys": []` and `"tags": []` returns no matches, so no test sends either empty list; `tests/all/reserved_tag_keys.rs` and `tests/all/resource_tagging.rs` only exercise non-empty lists, and `tests/all/not_found_wording.rs:67-71` sends `"tagKeys": ["owner"]`. The sibling refusal IS pinned for the criteria path (`tests/all/automation_rule_preview.rs` sends `"values": []` and asserts 400 `InvalidParameterValueException`), so the behaviour reported here is pinned by nothing.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"} so the caller is past the enrollment gate.
2. Call CreateAutomationRule with a minimal valid rule ({"name": "r1", "ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"], "schedule": {"scheduleExpression": "0 3 * * ? *"}, "status": "Active"}) and keep its ruleArn and ruleRevision (1).
3. Call TagResource with {"resourceArn": <ruleArn>, "ruleRevision": 1, "tags": [{"key": "owner", "value": "team"}]} — 200; ListTagsForResource reports the tag.
4. Call UntagResource with {"resourceArn": <ruleArn>, "ruleRevision": 1, "tagKeys": []} — 200 with an empty body, because src/ops/untag_resource.rs:180-182 judges only the keys it was sent and src/ops/untag_resource.rs:256 retains every tag whose key is not in an EMPTY list. ListTagsForResource still reports {"key": "owner", "value": "team"}: the request named nothing to remove and was answered as though it had.
5. Call TagResource with {"resourceArn": <ruleArn>, "ruleRevision": 1, "tags": []} — 200, and src/ops/tag_resource.rs:262-273 rewrites the stored list with an identical copy.
6. Compare with every other caller-supplied value list of this crate: AssociateAccounts with {"accountIds": []} answers 400 InvalidParameterValueException "1 validation error detected: Value at 'accountIds' failed to satisfy constraint: Member must have length greater than or equal to 1" (src/state.rs:456-464); CreateAutomationRule with "recommendedActionTypes": [] answers the same shape (src/state.rs:2813-2818); ListAutomationRulePreview with {"ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"], "criteria": {"region": [{"comparison": "StringEquals", "values": []}]}} answers 400 for 'criteria.region.member.values' (src/state.rs:2768-2773).
7. Observe the token consequence: call UntagResource with {"resourceArn": <ruleArn>, "ruleRevision": 1, "tagKeys": [], "clientToken": "t1"} — 200, and the token is recorded (src/ops/untag_resource.rs:258-263). Now send the removal the caller meant, {"resourceArn": <ruleArn>, "ruleRevision": 1, "tagKeys": ["owner"], "clientToken": "t1"} — 409 IdempotentParameterMismatchException "The request tied to this token is different!", so the token was spent on a call that did nothing.
