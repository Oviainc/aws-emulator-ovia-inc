Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c5da63dff272cd076ef27e20b0335aa7f1d0d3e8

# An amendment that points an attachment at a different file keeps the replaced file's CreatedBy/CreatedAt

_Filed: 2026-11-05T19:24:42Z_
_Operation: AmendBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c5da63dff272cd076ef27e20b0335aa7f1d0d3e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AmendBenefitApplication` can write `FileDetails.<i>.FileURI`. When the amendment moves that element to a URI the application did not carry, the handler re-derives `FileName` and `FileType` from the new URI but leaves `CreatedBy` and `CreatedAt` exactly as they were — so the stored entry describes a file (new `FileURI`, new `FileName`, new `FileType`) while reporting the uploading principal and upload instant recorded for the file that is no longer attached. A later `GetBenefitApplication` therefore attributes a file to a caller that never supplied it, at an instant before it existed on the record.

The crate already owns one rule for this, and the sibling write path applies it: `crate::state::merged_file_details` carries the upload facts forward for an attachment whose `FileURI` the application already carries, and records the calling principal and the current instant for a file it does not already carry. `UpdateBenefitApplication` calls that helper with `caller_principal(ctx)` and `now`, so replacing attachment 0's file through the update re-attributes the entry; replacing the same attachment's file through the amendment does not. The amend handler cannot even apply the rule as written: it discards the request context (`let _ = ctx;`) and `apply_amendment` receives neither a principal nor an instant.

The amend comment justifies itself with the very premise `merged_file_details` uses ("`CreatedBy` and `CreatedAt` record WHO attached the file and WHEN rather than what was attached") and then draws the opposite conclusion for the case where what is attached CHANGED, which is exactly the case the helper's rule covers.

## Evidence

The crate's one rule for the two upload-fact members — src/state.rs:2795-2805 (doc of `merged_file_details`):
    /// `CreatedBy` and `CreatedAt` record WHO uploaded a file and WHEN ("The identifier of the
    /// user who uploaded the file", "The timestamp when the file was uploaded" —
    /// `smithy_query(query="shape",shape="FileDetail")`), so they belong to the FILE rather
    /// than to the request that last wrote the list or to the position the file holds in it: a
    /// supplied attachment naming a `FileURI` the application already carries keeps the
    /// principal and instant the call that attached it recorded, and only a file the
    /// application does not already carry records `created_by` and `now_millis`.

The sibling write path applying it — src/ops/update_benefit_application.rs:274 and :464-472:
        let principal = crate::state::caller_principal(ctx);
...
            if !crate::state::same_file_details(&application.file_details, &files) {
                application.file_details = crate::state::merged_file_details(
                    &application.file_details,
                    &files,
                    &principal,
                    now,
                );

The amend path skipping it — src/ops/amend_benefit_application.rs:618-629:
            // `FileName` and `FileType` follow the URI, exactly as they do for an
            // attachment the create or the update stored
            // ([`crate::state::file_detail`]), so a write that moved the URI derives them
            // again — and a write to anything else leaves every member the URI governs
            // exactly as it was. `CreatedBy` and `CreatedAt` record WHO attached the file
            // and WHEN rather than what was attached, so no amendment touches them.
            if supplied.file_uri != file.file_uri {
                file.file_name = crate::state::file_name_of(&supplied.file_uri);
                file.file_type = crate::state::file_type_of(&supplied.file_uri);
                file.file_uri = supplied.file_uri;
            }
            file.business_use_case = supplied.business_use_case;

and src/ops/amend_benefit_application.rs:709, which is why the principal is not even available to that branch:
        let _ = ctx;

Test coverage: tests/all/file_upload_facts.rs drives the rule on the create and update paths only. Its row at :239-244 is exactly this change on the update path —
        (
            "the attached file replaced by another",
            vec![PLAN],
            vec![FRESH],
            vec![false],
        ),
and :338-350 asserts that a file the application did not already carry reports the CALLING principal and an instant later than every earlier upload:
                assert_eq!(
                    created_by, updater,
...
                assert!(
                    created_at > last_upload,
No row of that file drives `AmendBenefitApplication`, so the amend path's attribution is pinned by nothing; the amendable-member set that includes `FileDetails` is declared at src/ops/amend_benefit_application.rs:164-177 as "the members a caller supplies and `UpdateBenefitApplication` also writes".

## Steps to Reproduce

1. Grant the entitlement and create an application in catalog `AWS`, signed with access key `AKIDCREATOR`, carrying `FileDetails: [{"FileURI": "s3://partner-bucket/proposals/plan.pdf", "BusinessUseCase": "funding request"}]`.
2. `GetBenefitApplication` it and note `FileDetails[0].CreatedBy` (the creator's principal) and `FileDetails[0].CreatedAt`.
3. `SubmitBenefitApplication` it, then call `AmendBenefitApplication` signed with a DIFFERENT access key (`AKIDAMENDER`) with `Revision` = the current one, `AmendmentReason` = "replace the attachment" and `Amendments: [{"FieldPath": "FileDetails.0.FileURI", "NewValue": "s3://partner-bucket/proposals/budget.xlsx"}]`.
4. `GetBenefitApplication` again: `FileDetails[0]` reports the new `FileURI`/`FileName`/`FileType`, but `CreatedBy` is still the creator's principal and `CreatedAt` is still the create's instant — upload facts for a file that is no longer attached.
5. Compare with the same replacement on the update path (tests/all/file_upload_facts.rs row "the attached file replaced by another"), where `merged_file_details` records the calling principal and the current instant for the file the application did not already carry.
