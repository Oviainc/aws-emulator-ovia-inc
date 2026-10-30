Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.delete-collection-group-nonempty-rejected opensearchserverless.tag-unicode-value-round-trip opensearchserverless.update-account-settings-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless.delete-collection-group-nonempty-rejected|run#24|AssertionError|assert dead_code == "ConflictException", f"expected ConflictException for the deleted group, got {dead_code!r}"
Issue-Key: opensearchserverless.tag-unicode-value-round-trip|_error#13|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: opensearchserverless.update-account-settings-out-of-range-rejected|run#7|AssertionError|assert message == "The requested total indexing capacity of 1701.0 OCUs exceeds your maximum indexing capacity service quota limit of 1700 OCUs. Current setting
Issue-Commit: 67ed68e040d396203b765d4dc2b0964aebb19c3c

# opensearchserverless: 3 net-new causes in the refusal/validation plane — UpdateCollectionGroup's dead-id refusal uses ValidationException where AWS uses ConflictException, TagResource accepts characters outside AWS's tag charset, and the OCU quota message renders the collection-group figure as an integer

> _Note: this issue was identified against the code base at commit `67ed68e040d396203b765d4dc2b0964aebb19c3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCollectionGroup

## Description
THREE net-new root causes grounded in failing behavioral checks of the opensearchserverless service token. Each is independent and must be closed on its own.

=== Cause 1 of 3: UpdateCollectionGroup against a group id that no longer exists is refused ValidationException (400) where AWS refuses ConflictException (409) ===
AWS: Smithy: UpdateCollectionGroup requires `id` (CollectionGroupId, pattern ^[a-z0-9]{3,40}$) and models ConflictException (409), InternalServerException (500), ServiceQuotaExceededException (402) and ValidationException (400) — it models NO ResourceNotFoundException. Recorded behavior: a mutation addressed at a group that was deleted is refused ConflictException with the message `The CollectionGroup withId <id>, does not exist` — all fixed text (including the missing space in "withId" and the comma after the id) except `<id>`, which is the `id` from the request.
Emulator: ValidationException (400) with `id <id> does not identify a collection group in this account`.
Why: both the HTTP status (400 vs 409) and the error code differ, so a client's error handling takes the wrong branch — a dead-id mutation is classified as malformed input rather than the resource-state conflict AWS reports, and a matcher keyed on AWS's message sees nothing it recognises.
Fix: the UpdateCollectionGroup handler's missing-group refusal in the opensearchserverless crate (the `store.collection_groups.get_mut(&input.id)` else-branch) — classify it as ConflictException with AWS's message. NOTE: the resolved issue 20261029T191222Z-opensearchserverless-10-net-new-causes-in-the-refusal.md touched UpdateCollectionGroup for its capacity-range MESSAGE (its Cause 10) and DeleteCollectionGroup's non-empty class (its Cause 6); the non-empty refusal now matches and this dead-id classification is a distinct, net-new divergence reached only after it.
Traces: opensearchserverless.delete-collection-group-nonempty-rejected (the earlier non-empty-group assertions now pass; the check fails on the dead-id mutation probe).

=== Cause 2 of 3: TagResource accepts a tag key/value containing characters outside AWS's permitted tag character set ===
AWS: Smithy: TagResource requires resourceArn + tags (Tag.key @length 1..128, Tag.value @length 0..256; the model states no @pattern, so the charset is recorded behavior only). Recorded behavior: a tag whose key or value contains a character outside letters, numbers, spaces and `_ . : / = + - @` (an emoji) is refused ValidationException (400) with the message `Tags can only contain letters, numbers, spaces, and the following special characters: _ . : / = + - @` — entirely fixed text, nothing taken from the request — and the rejected tag never appears in a subsequent ListTagsForResource. Tags built only from unicode LETTERS (CJK, accented latin, Greek) and that punctuation set are accepted and round-trip byte-for-byte.
Emulator: the call SUCCEEDS — HTTP 200 with an empty body (content-length 2) — and the out-of-charset tag is stored.
Why: there is no enforced boundary on the tag charset, so a tag set real AWS refuses is persisted and later reported by ListTagsForResource; a caller shipping a bad tag gets no signal, and the emulator's tag state diverges from what the account could actually hold.
Fix: the TagResource handler's tag validation in the opensearchserverless crate, and the same rule on every handler that accepts a `tags` list (CreateCollection, CreateCollectionGroup, the policy creates) so the write path and the tagging plane agree.
Traces: opensearchserverless.tag-unicode-value-round-trip (its unicode-letter positive control passes; only the out-of-charset rejection is missing).

=== Cause 3 of 3: the ServiceQuotaExceededException text for an over-quota indexing capacity renders the collection-group OCU figure as an integer instead of a decimal ===
AWS: Smithy: UpdateAccountSettings takes capacityLimits (CapacityLimits members are INTEGER, @range min 2) and models ServiceQuotaExceededException (402). Recorded behavior: a maxIndexingCapacityInOCU above the non-adjustable account ceiling of 1700 OCU is refused ServiceQuotaExceededException whose message has the fixed form `The requested total indexing capacity of <requested>.0 OCUs exceeds your maximum indexing capacity service quota limit of 1700 OCUs. Current settings: <n> OCUs for collections that are not part of any collection group, <m> OCUs for collection groups. New request of <requested> OCUs would bring total to <total>.0 OCUs.` — the 1700 limit is fixed; `<requested>` comes from the request and `<n>`, `<m>`, `<total>` from account state. The collection-group figure `<m>` and the totals are rendered as DECIMALS (collection-group capacity is a modeled float), while the non-grouped-collection figure `<n>` is an integer.
Emulator: the 402 and the rest of the sentence are right, but the collection-group figure is rendered `0 OCUs for collection groups` instead of `0.0 OCUs for collection groups`.
Why: the message is the only carrier of these figures (the code is the generic ServiceQuotaExceededException), so a client or test parsing AWS's text does not match the refusal, and the rendering silently misreports a float quantity as an integer one.
Fix: the quota-refusal message builder used by UpdateAccountSettings (`crate::state::account_capacity_quota_refusal` in the opensearchserverless crate's state module) — render every collection-group/total OCU figure with the float formatting AWS uses. NOTE: the resolved issue 20261029T191222Z-opensearchserverless-10-net-new-causes-in-the-refusal.md (Cause 9) added this refusal; the 402 now fires and only this rendering diverges.
Traces: opensearchserverless.update-account-settings-out-of-range-rejected.

## Evidence
Cause 1 — expected error code 'ConflictException' (409) with `The CollectionGroup withId <id>, does not exist`; actual: `AssertionError: expected ConflictException for the deleted group, got 'ValidationException'` (message `id <id> does not identify a collection group in this account`). Source: services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/update_collection_group.rs:197-207.
Cause 2 — expected ValidationException (400) `Tags can only contain letters, numbers, spaces, and the following special characters: _ . : / = + - @`; actual: `AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}}}` for tags=[{'key': 'launch<emoji>', 'value': '<emoji>-party-2024'}].
Cause 3 — expected `... Current settings: 10 OCUs for collections that are not part of any collection group, 0.0 OCUs for collection groups. New request of 1701 OCUs would bring total to 1701.0 OCUs.`; actual the same sentence with `0 OCUs for collection groups`. The 10/0.0/1701 figures are per-run values from request and state; the defect is the integer rendering of the collection-group figure.

## Steps to Reproduce
Cause 1: create_collection_group(name=<g>); delete_collection_group(id=<g id>) once it is empty; then update_collection_group(id=<g id>, description='dead-id-probe') -> expected 409 ConflictException `The CollectionGroup withId <g id>, does not exist`; observed 400 ValidationException.
Cause 2: create_collection_group(name=<n>); tag_resource(resourceArn=<its arn>, tags=[{'key': 'launch\U0001F680', 'value': '\U0001F389-party-2024'}]) -> expected 400 ValidationException with the charset message and no such tag on list_tags_for_resource; observed HTTP 200 and the tag stored.
Cause 3: update_account_settings(capacityLimits={'maxIndexingCapacityInOCU': 1701, 'maxSearchCapacityInOCU': 4}) -> 402 ServiceQuotaExceededException; compare its message's collection-group OCU figure (`0` vs AWS's `0.0`).
