Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 772fb5ab5b7496e2df886721dbd75d9882bc460e

# TagResource writes tags onto a connector whose teardown has already been accepted

_Filed: 2026-10-28T21:30:32Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `772fb5ab5b7496e2df886721dbd75d9882bc460e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` resolves the target through `store.tags_of_mut(&region, &resource_arn)` and inserts the request's entries without ever looking at the connector's `ConnectorStatus`. A connector whose `DeleteConnector` has been ACCEPTED keeps its record in the store and reports `DELETING` for the whole teardown window (`TRANSITION_SPAN`), so the tagging call is answered 204 and the new tag is observable through `ListTagsForResource` — until the teardown drops the record and the tag with it. The same holds for a challenge whose owning connector is `DELETING`, because `tags_of_mut` reaches it through `connector_mut`, which applies no status filter either.

Everywhere else this crate treats a `DELETING` connector as a record that may only be REPORTED, never changed: `StoredConnector::is_deleting`'s own doc says the connector "is still reported, but nothing can be built on it and it is about to disappear", `DeleteConnector`'s handler says "The record stays readable, reporting `DELETING`, until the clock makes its removal due", and the sibling `CreateChallenge` refuses that very connector with HTTP 400 `ValidationException` carrying `Reason` `INVALID_STATE` (the state module's `REASON_INVALID_STATE`). `TagResource` already has the helper it needs (`StoredConnector::is_deleting`, used by `CreateConnector`) and its own comment shows only the COMPLETED teardown was considered — "so a connector whose teardown has come due cannot be tagged back into existence" — leaving the accepted-but-not-yet-due teardown as a write door the crate's own rule closes everywhere else.

This is a different defect from the open `TagResource` wrong-behavior issue about a stale `UpdatedAt`: that one is about a write that happens but is not stamped, this one is about a write that should not be accepted at all.

## Evidence

The write with no status check — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/tag_resource.rs:186-189 and :204-232:
```
        let mut store = state.lock();
        // The transition the clock already owes, materialised before the lookup, so a
        // connector whose teardown has come due cannot be tagged back into existence.
        store.settle(now);
...
        let Some(tags) = store.tags_of_mut(&region, &resource_arn) else {
            return Err(not_found(&resource_arn));
        };
...
        for (key, value) in input.tags {
            tags.insert(key, value);
        }
```
(the only teardown this handler reasons about is the one that has "come due"; nothing consults `is_deleting`)

The crate's own rule for a connector under teardown — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs:348-352:
```
    /// True while the connector's teardown is running: it is still reported, but nothing can
    /// be built on it and it is about to disappear.
    pub fn is_deleting(&self) -> bool {
        self.status == STATUS_DELETING
    }
```
and services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/delete_connector.rs:189-192:
```
        // A delete is accepted whatever non-terminal status the connector is in: the call
        // only schedules the teardown, so a still-CREATING connector does not have to settle
        // first. The record stays readable, reporting `DELETING`, until the clock makes its
        // removal due
```

The sibling handler that refuses the same connector — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/create_challenge.rs:269-281:
```
        // A connector that is not yet serving cannot authenticate a certificate request
        // with a challenge password, so it cannot hand one out either.
        if !connector.is_active() {
            return Err(CreateChallengeErrorModeled::ValidationException(ValidationException {
                message: format!(
                    "Connector {connector_arn} has status {}; it must be {} to create a challenge",
                    connector.status,
                    crate::state::STATUS_ACTIVE
                ),
                reason: Some(crate::state::REASON_INVALID_STATE.to_string()),
```

Test coverage: the DELETING window is pinned for the sibling but not for tagging. tests/all/connector_deletion.rs:318-334 pins `CreateChallenge` on a held-open `DELETING` connector as `(400, "ValidationException")` with `Reason` `INVALID_STATE` ("A connector being torn down is not serving, so nothing new can be built on it"). tests/all/tag_resource.rs:578-638 walks the 404 rows and its only teardown row is `"connector whose teardown has come due"` (tests/all/tag_resource.rs:600-605), i.e. the record already removed; no row tags a connector that is still reporting `DELETING`, so nothing currently asserted changes if the in-flight teardown is refused too.

## Steps to Reproduce

1. CreateConnector with a valid `CertificateAuthorityArn`, then let the clock settle it so GetConnector reports Status ACTIVE.
2. DeleteConnector on that ARN -> 202 (the teardown is only accepted; hold the window open, e.g. tests/all/harness.rs `hold_pending`).
3. GetConnector -> 200 with `Connector.Status` == "DELETING".
4. CreateChallenge with that same `ConnectorArn` -> 400 ValidationException, Reason INVALID_STATE (the crate refuses a call against a connector under teardown).
5. TagResource on the same ARN with `{"Tags": {"owner": "platform"}}` -> 204, and ListTagsForResource on that ARN reports the new tag: the same connector that refuses a challenge accepts a tag write.
6. Let the teardown come due: the connector, and the tag just written, are gone — the 204 reported success for a mutation that was discarded.
