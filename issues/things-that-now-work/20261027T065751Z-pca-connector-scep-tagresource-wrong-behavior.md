Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# TagResource mutates the resource but leaves its UpdatedAt stale

_Filed: 2026-10-27T06:57:51Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` writes into the stored connector's (or challenge's) own `tags` map, but it never touches that record's `updated_at`. Every other write path in this crate's state module maintains `updated_at` when it changes the record, so after a successful `TagResource` the record has changed while `GetConnector` / `ListConnectors` / `GetChallengeMetadata` / `ListChallengeMetadata` keep reporting the pre-tagging `UpdatedAt` — a timestamp the model documents as "the date and time that the connector was updated".

The handler cannot even reach the field: it takes `store.tags_of_mut(..)`, which hands back only `&mut BTreeMap<String, String>` (state.rs:492-507), so the surrounding `StoredConnector` / `StoredChallenge` is never touched. The two sibling mutators (`StoredConnector::begin_deletion` and `StoredConnector::settle`) both set `updated_at` for the change they make, and `CreateConnector` / `CreateChallenge` set it at creation, which is the rule this one path skips.

No existing test pins the post-tagging value (the only `UpdatedAt` assertions in the suite are "is a number" and a member-name list), so maintaining it changes nothing that is currently asserted.

## Evidence

Model (smithy_query shape=Connector, member UpdatedAt): "<p>The date and time that the connector was updated.</p>"; shape=Challenge member UpdatedAt: "<p>The date and time that the challenge was updated.</p>".

The write that skips it — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/ops/tag_resource.rs:207-232:
```
        let Some(tags) = store.tags_of_mut(&region, &resource_arn) else {
            return Err(not_found(&resource_arn));
        };
...
        // AWS tagging semantics: a tag is a key with ONE value, so a key the resource already
        // carries is overwritten rather than duplicated, and every other tag it carries stays.
        for (key, value) in input.tags {
            tags.insert(key, value);
        }
```
(no `updated_at` assignment anywhere in the file — `grep updated_at src/ops/tag_resource.rs` matches nothing)

The sibling write paths that DO maintain it — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs:286-292 (`settle`):
```
            PendingOutcome::Status(status) => {
                self.status = status;
                // The change happened when the clock made it due, not when someone looked.
                self.updated_at = pending.due_at.max(self.updated_at);
```
and state.rs:310-313 (`begin_deletion`):
```
        self.status = STATUS_DELETING.to_string();
        // `StatusReason` says why a connector FAILED; one being torn down carries none.
        self.status_reason = None;
        self.updated_at = now;
```

Test coverage: no test asserts `UpdatedAt` after tagging — the only matches for `UpdatedAt` under tests/ are tests/all/connector_lifecycle.rs:558 (`for member in ["CreatedAt", "UpdatedAt"]` → `is_number()`) and tests/all/challenge_metadata.rs:44 (`const METADATA_MEMBERS`). tests/all/tag_resource.rs asserts only the merged `Tags` map and the 204/404/400 statuses.

## Steps to Reproduce

1. CreateConnector with a valid `CertificateAuthorityArn`; release the pending transition so the connector is ACTIVE.
2. GetConnector and record `Connector.UpdatedAt` (call it T0).
3. Wait a moment, then call TagResource on that connector ARN with `{"Tags": {"owner": "team-a"}}` → 204.
4. ListTagsForResource shows the new tag, so the stored record did change.
5. GetConnector again: `Connector.UpdatedAt` is still exactly T0, even though the record was modified — while a status change through the same record (DeleteConnector, or the CREATING→ACTIVE settle) does move `UpdatedAt`.
6. Same with a challenge: CreateChallenge, TagResource on the challenge ARN, then GetChallengeMetadata → `ChallengeMetadata.UpdatedAt` is unchanged.
