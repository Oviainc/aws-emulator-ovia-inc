Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: cc767c9c860d2029dd9667f4e637f1644e38e746

# A tag write on a compliance inquiry mutates the inquiry record but leaves its updatedAt at the previous value

_Filed: 2026-10-29T06:24:48Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cc767c9c860d2029dd9667f4e637f1644e38e746`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two tag-write handlers of this crate (`UntagResource` and, on the same lines, `TagResource`) take the stored `ComplianceInquiry` record mutably and change it — `UntagResource` removes entries from `resource.tags`, `TagResource` replaces the whole map — and neither touches that record's `updated_at`. A later `GetComplianceInquiryMetadata` therefore reports `complianceInquiryDetail.updatedAt` as an instant BEFORE a modification of the resource that really happened and that `ListTagsForResource` reports back.

This contradicts the crate's own model of the resource twice over:

* the model documents `InquiryDetail$updatedAt` as "Timestamp indicating when the resource was last modified" and `CreateComplianceInquiryRequest$tags` as "Tags to associate with the compliance inquiry resource", and this crate's state module states that the tag set is not a second map beside the inquiry but lives ON the inquiry record (src/state.rs:38-43) — `tags` is a field of the same `ComplianceInquiry` struct as `updated_at` (src/state.rs:999 and src/state.rs:1005);
* the sibling write path in the same crate maintains the field exactly as the rule requires: `ComplianceInquiry::materialise` assigns `self.updated_at = due` with the comment "that is when the resource changed" (src/state.rs:1051-1053), and the crate's own test asserts "the transition modified the resource, so updatedAt must have moved off createdAt" (tests/all/compliance_inquiry_lifecycle.rs:550-554). The tag writes are also the only two handlers in the crate that hold `&mut ComplianceInquiry` without calling `materialise` at all; every other inquiry handler calls it.

The handler comment that justifies the omission ("it reports when the inquiry's own record was last modified, which this service moves with the processing transition … and no AWS documentation ties it to a write of the resource's tag set", src/ops/tag_resource.rs:206-208) rests on the tag set NOT being part of the inquiry's own record — which is precisely what src/state.rs:38-43 and the struct layout say it is. No test pins the current behaviour: a grep for `updatedAt` over tests/all finds hits only in timestamp_wire_form.rs (wire form), compliance_inquiry_export.rs (presence in the export document) and compliance_inquiry_lifecycle.rs (the clock transition); none of the tag suites (resource_tags.rs, resource_tag_writes.rs, inquiry_tags_reporting.rs) asserts anything about it, so fixing this changes no existing assertion.

This issue is anchored on `UntagResource` because the already-open `TagResource` issue (20261028T215034Z-artifact-tagresource-wrong-behavior) is a different defect (tag writes leave no attribution/audit record); the same one-line omission exists in both handlers and the fix belongs in both.

## Evidence

Model (smithy_query shape=InquiryDetail): `updatedAt` — "<p>Timestamp indicating when the resource was last modified.</p>"; (smithy_query shape=CreateComplianceInquiryRequest): `tags` — "<p>Tags to associate with the compliance inquiry resource.</p>".

The crate says the tags ARE the inquiry record — services/service-com-amazonaws-artifact-artifact/src/state.rs:38-43:
```
//! An inquiry also carries the one TAG SET this service persists: `TagResource`,
//! `UntagResource` and `ListTagsForResource` act on the tags of the resource their
//! `resourceArn` names, and the only resource an account owns here is an inquiry (see
//! [`ComAmazonawsArtifactArtifactState::tagged_resource`]), so the tags live on the
//! inquiry record that `CreateComplianceInquiry` already accepts them with rather than in
//! a second map beside it.
```
and both fields sit on one struct — src/state.rs:998-1005:
```
    /// "Timestamp indicating when the resource was last modified."
    pub updated_at: SystemTime,
...
    pub tags: BTreeMap<String, String>,
```

The write that skips it — src/ops/untag_resource.rs:172-183:
```
        let mut store = state.lock();
        let Some(resource) = store.tagged_resource_mut(&input.resource_arn) else {
            ...
        };

        for key in &input.tag_keys {
            // A key the resource does not carry is already in the state the request asks
            // for, which is what makes the operation `@idempotent`.
            resource.tags.remove(key);
        }
```
and src/ops/tag_resource.rs:205-208:
```
        resource.tags = tags;
        // `updatedAt` is NOT moved: it reports when the inquiry's own record was last
        // modified, which this service moves with the processing transition the clock owes
        // it, and no AWS documentation ties it to a write of the resource's tag set.
```

The sibling path that does maintain it — src/state.rs:1050-1053:
```
        self.processing_due_at = None;
        // The transition was owed AT `due`, so that is when the resource changed —
        // not the instant some caller happened to look.
        self.updated_at = due;
```
and the rule as the crate's own test states it — tests/all/compliance_inquiry_lifecycle.rs:550-554:
```
        assert_ne!(
            settled_detail["updatedAt"], settled_detail["createdAt"],
            "case {label:?}: the transition modified the resource, so updatedAt must have \
             moved off createdAt; got {:?}",
```

Test coverage of the gap: none — no test in tests/all asserts `updatedAt` across a tag write (resource_tags.rs, resource_tag_writes.rs and inquiry_tags_reporting.rs assert only the tag set that is reported back), so the current behaviour is unpinned.

## Steps to Reproduce

1. POST /v1/compliance-inquiry/create with {"name":"soc2","inquiryContent":{"query":"Is S3 in scope?"},"tags":{"team":"legal"}} and note complianceInquirySummary.id and .arn.
2. Wait past the processing span (500 ms) and GET /v1/compliance-inquiry/getMetadata?complianceInquiryId=<id>; record complianceInquiryDetail.updatedAt (it has moved to the clock-owed transition instant).
3. DELETE /tags/<percent-encoded arn>?tagKeys=team — 204, and GET /tags/<percent-encoded arn> now reports an empty tag set, so the stored inquiry record changed.
4. GET /v1/compliance-inquiry/getMetadata?complianceInquiryId=<id> again: complianceInquiryDetail.updatedAt is unchanged from step 2, although the resource was modified in step 3.
5. Repeat with POST /tags/<percent-encoded arn> {"tags":{"team":"finance"}} (TagResource): ListTagsForResource reports the new value while updatedAt still reports the pre-write instant.
