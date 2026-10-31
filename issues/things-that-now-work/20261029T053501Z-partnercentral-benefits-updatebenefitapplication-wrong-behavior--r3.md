Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0727557aa604deafa81f0cee75b9e46b63ce964

# An update that adds or reorders one attachment rewrites CreatedBy/CreatedAt on every file already attached

_Filed: 2026-10-29T05:35:01Z_
_Operation: UpdateBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0727557aa604deafa81f0cee75b9e46b63ce964`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateBenefitApplication` treats `FileDetails` as an all-or-nothing replacement: as soon as the supplied list differs from the stored one in ANY way, the handler rebuilds EVERY stored `FileDetail` through `crate::state::file_detail(file, &principal, now)`, which stamps `CreatedBy` with the CURRENT caller's principal and `CreatedAt` with the CURRENT time for all of them.

So adding a second attachment (or re-sending the same two files in the other order) silently rewrites the upload identity and upload time of the file that was already there, losing state an earlier call recorded. The modelled members then report something that is not true of that file: `FileDetail$CreatedAt` is documented as "The timestamp when the file was uploaded" and `FileDetail$CreatedBy` as "The identifier of the user who uploaded the file", yet after the second update the first file reports the instant and the principal of the LATER update, which uploaded a different file.

The crate states the opposite rule twice, in the comment sitting directly above the offending code and in the doc of the helper that comment points at: those two members "record WHO attached that file and WHEN rather than what was attached, so they are not part of what the caller can change". The crate also already has the per-file identity comparison the correct behaviour needs — `same_file_details` compares the supplied `FileURI`/`BusinessUseCase` against the stored ones pairwise — but the write path throws that per-file information away and re-derives every entry from scratch.

Symptoms of this one defect: (a) adding a file resets the existing files' `CreatedBy`/`CreatedAt`; (b) removing one file resets the survivors'; (c) re-sending exactly the same files in a different order resets both, because `same_file_details` is order-sensitive while the stored metadata it protects is not.

## Evidence

The handler rebuilds every file whenever the lists differ, under a comment stating the opposite — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/update_benefit_application.rs:428-440:
```
        if let Some(files) = input.file_details {
            // An attachment is the file the caller named, so a request re-sending the
            // files the application already carries attaches nothing a second time and
            // leaves the `CreatedBy`/`CreatedAt` the first attachment recorded — see
            // [`crate::state::same_file_details`].
            if !crate::state::same_file_details(&application.file_details, &files) {
                application.file_details = files
                    .iter()
                    .map(|file| crate::state::file_detail(file, &principal, now))
                    .collect();
                applied_members.push(MEMBER_FILE_DETAILS.to_string());
            }
        }
```

The helper that comment points at states the rule the code breaks — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/state.rs:2577-2593:
```
/// Whether `stored` already holds exactly the files `supplied` names, in the same order.
///
/// A file's caller-supplied content is its `FileURI` and `BusinessUseCase`, and the
/// `FileName`/`FileType` beside them follow the URI (see [`file_detail`]), so two
/// attachments naming the same URI and use case are the same attachment; `CreatedBy` and
/// `CreatedAt` record WHO attached that file and WHEN rather than what was attached, so
/// they are not part of what the caller can change. A request re-sending the files an
/// application already carries therefore changes nothing.
pub fn same_file_details(stored: &[FileDetail], supplied: &[FileInput]) -> bool {
    stored.len() == supplied.len()
        && stored
            .iter()
            .zip(supplied)
            .all(|(stored, supplied)| {
                stored.file_uri == supplied.file_uri
                    && stored.business_use_case == supplied.business_use_case
            })
}
```

What the rebuild stamps — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/state.rs:2557-2575:
```
/// The stored `FileDetail` for one supplied `FileInput`.
///
/// `FileURI` and `BusinessUseCase` are the caller's own values; ... `CreatedBy` and
/// `CreatedAt` record who attached the file and when.
pub fn file_detail(input: &FileInput, created_by: &str, now_millis: i64) -> FileDetail {
    FileDetail {
        ...
        created_by: Some(created_by.to_string()),
        created_at: Some(timestamp(now_millis)),
    }
}
```

The model on what those two members mean — `smithy_query(query="shape",shape="FileDetail")`:
```
"CreatedBy": {"target": "String", "doc": "<p>The identifier of the user who uploaded the file.</p>"}
"CreatedAt": {"target": "Timestamp", "doc": "<p>The timestamp when the file was uploaded.</p>"}
```

Test coverage: no test pins the current behaviour. `tests/all/write_that_changes_nothing.rs:181-188` only drives a SINGLE-file list changed in its `BusinessUseCase`, and its assertion at lines 248-267 deliberately compares "the caller's own values" only (`FileURI`/`BusinessUseCase`), skipping `CreatedBy`/`CreatedAt`; `tests/all/benefit_application_lifecycle.rs:842` only asserts `CreatedBy` is non-empty. So nothing states a reason for the reset, and the multi-file case is untested.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as the lifecycle tests do
2. CreateBenefitApplication in catalog `AWS` for an ACTIVE benefit with FileDetails=[{"FileURI":"s3://b/plan.pdf","BusinessUseCase":"funding request"}]; note Id and Revision
3. GetBenefitApplication and record FileDetails[0].CreatedAt and FileDetails[0].CreatedBy
4. Wait a moment (or sign the next request with a different access key id so the principal differs), then call UpdateBenefitApplication with Catalog=AWS, Identifier=<id>, Revision=<revision>, ClientToken="tok-add", FileDetails=[{"FileURI":"s3://b/plan.pdf","BusinessUseCase":"funding request"},{"FileURI":"s3://b/extra.pdf"}]
5. GetBenefitApplication again: the entry for s3://b/plan.pdf now reports the UPDATE's CreatedAt (and the update caller's CreatedBy), not the create's — the attachment facts the first call recorded are gone
6. Repeat with FileDetails listing the same two files in the opposite order and observe both entries' CreatedAt/CreatedBy rewritten again, although no file was attached or removed
