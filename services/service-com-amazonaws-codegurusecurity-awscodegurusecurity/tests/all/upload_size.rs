// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SIZE of the code archive uploaded to the target
//! `CreateUploadUrl` publishes.
//!
//! `CreateUploadUrlResponse$s3Url` is "A pre-signed S3 URL. You can upload the code file
//! you want to scan with the required `requestHeaders` using any HTTP client"
//! (`smithy_query(query="doc",shape="CreateUploadUrlResponse",member="s3Url")`), and
//! neither that member nor any other shape of this service carries a size constraint. The
//! rule this suite asserts is therefore:
//!
//! * an upload to a published target is accepted, stored against the code artifact the
//!   URL names and accounted for in full, for EVERY archive size a code archive reaches —
//!   so a scan started on it has a resource to analyse and settles into the modelled
//!   successful state;
//! * how much of an archive this emulator keeps a copy of
//!   (`UPLOAD_RETAINED_PREFIX_MAX_BYTES`) is a retention bound, not a ceiling on the
//!   upload: a larger archive is stored, not refused, and what every target retains
//!   together stays bounded whatever was uploaded, with nothing the account already
//!   uploaded limiting the next upload;
//! * the sizes an upload IS refused at are the published maximum of one `PUT`
//!   (`S3_SINGLE_PUT_MAX_BYTES`) and what this process takes in one upload
//!   (`UPLOAD_ACCEPTED_MAX_BYTES`), both judged on the declared length before the body is
//!   read, and checked here on each side of each bound.
//!
//! Every HTTP request goes through the real router (`MultiServiceRouter::dispatch`), the
//! same path a client takes.

use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    exceeds_single_put_maximum, upload_size_bound_exceeded, SCAN_STATE_SUCCESSFUL,
    S3_SINGLE_PUT_MAX_BYTES, UPLOAD_ACCEPTED_MAX_BYTES, UPLOAD_RETAINED_PREFIX_MAX_BYTES,
};

use serde_json::json;

use crate::scan_lifecycle::{create_scan, create_upload_url, harness, store_of};
use crate::upload_target::{
    archive_of, describe, is_record_of, required_headers, settled_run, stored_upload, upload,
    wait_for_settle,
};

/// A mebibyte, so a case's size reads as the archive size it stands for.
const MIB: usize = 1024 * 1024;

#[tokio::test]
async fn an_archive_of_any_size_is_uploaded_stored_and_scanned() {
    let (engine, router) = harness();

    // Rows span the SIZE of the archive, which is what the rule generalises over: nothing
    // at all, one byte, each side of how much of an archive this emulator retains a copy
    // of, and sizes a realistic zipped source tree reaches — including sizes that are
    // multiples of a mebibyte, which is the scale at which a code archive is refused by
    // no AWS limit.
    let cases: Vec<(&str, &str, usize)> = vec![
        ("an empty archive", "size-empty", 0),
        ("a single byte", "size-one-byte", 1),
        (
            "one byte short of what is retained",
            "size-under-retained",
            UPLOAD_RETAINED_PREFIX_MAX_BYTES - 1,
        ),
        (
            "exactly what is retained",
            "size-at-retained",
            UPLOAD_RETAINED_PREFIX_MAX_BYTES,
        ),
        (
            "one byte past what is retained",
            "size-over-retained",
            UPLOAD_RETAINED_PREFIX_MAX_BYTES + 1,
        ),
        ("a one-mebibyte archive", "size-1-mib", MIB),
        ("a nine-mebibyte archive", "size-9-mib", 9 * MIB),
        ("a sixteen-mebibyte archive", "size-16-mib", 16 * MIB),
    ];

    let mut uploaded: Vec<(&str, &str, String, usize)> = Vec::new();
    for (case, scan_name, size) in &cases {
        let size = *size;
        let minted = create_upload_url(&router, scan_name).await;
        assert_eq!(
            minted.status, 200,
            "case {case:?}: CreateUploadUrl expected 200, got {} {}",
            minted.status, minted.json
        );
        let artifact = minted.str_of("codeArtifactId");
        let bytes = archive_of(size, 101);

        let answer = upload(
            &router,
            &minted.str_of("s3Url"),
            &required_headers(&minted),
            bytes.clone(),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: an upload of {size} bytes must be accepted, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            String::from_utf8_lossy(&answer.body)
        );

        // The whole archive is accounted for: its length and a digest over every byte
        // that arrived, with the leading content this emulator retains.
        let held = stored_upload(&engine, &artifact)
            .unwrap_or_else(|| panic!("case {case:?}: the target holds no resource"));
        assert!(
            is_record_of(&held, &bytes),
            "case {case:?}: the record of a {size}-byte archive must account for all of it, \
             got {}",
            describe(&held)
        );
        assert_eq!(
            held.retained_prefix.len(),
            size.min(UPLOAD_RETAINED_PREFIX_MAX_BYTES),
            "case {case:?}: expected {} bytes retained of {size}, got {}",
            size.min(UPLOAD_RETAINED_PREFIX_MAX_BYTES),
            describe(&held)
        );

        uploaded.push((case, scan_name, artifact, size));
    }

    // The documented workflow continues on every one of them: a scan started on an
    // uploaded resource of any size HAD a resource to analyse.
    for (case, scan_name, artifact, size) in &uploaded {
        let created = create_scan(&router, json!({ "scanName": scan_name }), artifact).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateScan on a {size}-byte archive expected 200, got {} {}",
            created.status, created.json
        );
    }
    wait_for_settle().await;
    for (case, scan_name, _artifact, size) in &uploaded {
        let run = settled_run(&router, scan_name).await;
        assert_eq!(
            run.scan_state, SCAN_STATE_SUCCESSFUL,
            "case {case:?}: a run on a {size}-byte archive must complete, got {run:?}"
        );
        assert_eq!(
            run.error_message, "",
            "case {case:?}: a run that did not fail carries no errorMessage, got {run:?}"
        );
    }
}

#[tokio::test]
async fn what_the_account_already_uploaded_does_not_limit_the_next_upload() {
    let (engine, router) = harness();

    // Rows: successive uploads to DIFFERENT targets of the one account, whose total
    // (eight times nine mebibytes, 72 MiB) passes any plausible store-shaped ceiling on
    // what one account's targets hold together, so the only thing that could refuse a
    // later one is an account-wide ceiling — and there is none.
    let archive = archive_of(9 * MIB, 103);
    let mut artifacts: Vec<String> = Vec::new();
    for index in 0..8usize {
        let scan_name = format!("account-total-{index}");
        let minted = create_upload_url(&router, &scan_name).await;
        assert_eq!(
            minted.status, 200,
            "upload {index}: CreateUploadUrl expected 200, got {} {}",
            minted.status, minted.json
        );
        let artifact = minted.str_of("codeArtifactId");
        let answer = upload(
            &router,
            &minted.str_of("s3Url"),
            &required_headers(&minted),
            archive.clone(),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "upload {index}: expected 200 after {} bytes already uploaded, got {} {:?}",
            index * archive.len(),
            answer.status,
            answer.error_code
        );
        let held = stored_upload(&engine, &artifact)
            .unwrap_or_else(|| panic!("upload {index}: the target holds no resource"));
        assert!(
            is_record_of(&held, &archive),
            "upload {index}: the target must account for the whole archive, got {}",
            describe(&held)
        );
        artifacts.push(artifact);
    }

    // And the store stayed bounded while accepting them: what every target retains
    // together is the retention bound times the number of targets, not the bytes uploaded.
    let retained = store_of(&engine).lock().upload_bytes_held;
    assert_eq!(
        retained,
        artifacts.len() * UPLOAD_RETAINED_PREFIX_MAX_BYTES,
        "expected {} retained bytes for {} targets, got {retained}",
        artifacts.len() * UPLOAD_RETAINED_PREFIX_MAX_BYTES,
        artifacts.len()
    );
}

#[test]
fn a_size_bound_refuses_only_past_itself_and_reports_which_one() {
    // Rows: each side of both published-or-process bounds of this plane — the largest
    // object a single PUT uploads to a pre-signed S3 URL (`S3_SINGLE_PUT_MAX_BYTES`, 5 GB;
    // larger objects go up in parts) and the most this emulator takes in one upload
    // (`UPLOAD_ACCEPTED_MAX_BYTES`) — plus the sizes a code archive actually reaches, none
    // of which is past either. Checked on the predicates rather than over HTTP, so a bound
    // is asserted without transferring it.
    let cases: [(&str, u64, Option<u64>); 10] = [
        ("nothing at all", 0, None),
        ("one byte", 1, None),
        ("eight mebibytes", 8 * 1024 * 1024, None),
        ("sixty-four mebibytes", 64 * 1024 * 1024, None),
        (
            "one byte short of what this emulator takes",
            UPLOAD_ACCEPTED_MAX_BYTES - 1,
            None,
        ),
        (
            "exactly what this emulator takes",
            UPLOAD_ACCEPTED_MAX_BYTES,
            None,
        ),
        (
            "one byte past what this emulator takes",
            UPLOAD_ACCEPTED_MAX_BYTES + 1,
            Some(UPLOAD_ACCEPTED_MAX_BYTES),
        ),
        (
            "exactly the single-PUT maximum",
            S3_SINGLE_PUT_MAX_BYTES,
            Some(UPLOAD_ACCEPTED_MAX_BYTES),
        ),
        (
            "one byte past the single-PUT maximum",
            S3_SINGLE_PUT_MAX_BYTES + 1,
            Some(S3_SINGLE_PUT_MAX_BYTES),
        ),
        (
            "the largest countable upload",
            u64::MAX,
            Some(S3_SINGLE_PUT_MAX_BYTES),
        ),
    ];
    for (case, byte_count, expected) in cases {
        let actual = upload_size_bound_exceeded(byte_count);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected upload_size_bound_exceeded({byte_count}) to be \
             {expected:?}, got {actual:?}"
        );
        let past_the_published_bound = exceeds_single_put_maximum(byte_count);
        assert_eq!(
            past_the_published_bound,
            byte_count > S3_SINGLE_PUT_MAX_BYTES,
            "case {case:?}: expected exceeds_single_put_maximum({byte_count}) to be {}, got \
             {past_the_published_bound}",
            byte_count > S3_SINGLE_PUT_MAX_BYTES
        );
    }
}

#[tokio::test]
async fn an_upload_declaring_more_than_this_emulator_takes_is_refused_and_stores_nothing() {
    let (engine, router) = harness();
    let minted = create_upload_url(&router, "declared-too-large").await;
    assert_eq!(
        minted.status, 200,
        "CreateUploadUrl expected 200, got {} {}",
        minted.status, minted.json
    );
    let artifact = minted.str_of("codeArtifactId");
    let url = minted.str_of("s3Url");

    // Rows: the length the request DECLARES, which is what this plane judges before it
    // reads the body — one past each bound, and a declaration within both as the control.
    let cases: [(&str, u64, u16); 3] = [
        (
            "one byte past what this emulator takes",
            UPLOAD_ACCEPTED_MAX_BYTES + 1,
            400,
        ),
        (
            "one byte past the single-PUT maximum",
            S3_SINGLE_PUT_MAX_BYTES + 1,
            400,
        ),
        ("a declaration within both bounds", 64, 200),
    ];
    for (case, declared, status) in cases {
        let mut headers = required_headers(&minted);
        headers.insert("content-length".to_string(), declared.to_string());
        let answer = upload(&router, &url, &headers, archive_of(64, 107)).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected {status} for a declared length of {declared}, got {} {:?} \
             {:?}",
            answer.status,
            answer.error_code,
            String::from_utf8_lossy(&answer.body)
        );
        if status == 400 {
            assert_eq!(
                answer.error_code, "EntityTooLarge",
                "case {case:?}: expected EntityTooLarge, got {:?}",
                answer.error_code
            );
            assert!(
                stored_upload(&engine, &artifact).is_none(),
                "case {case:?}: a refused upload stores nothing, got {:?}",
                stored_upload(&engine, &artifact).map(|held| describe(&held))
            );
        } else {
            assert!(
                stored_upload(&engine, &artifact)
                    .is_some_and(|held| is_record_of(&held, &archive_of(64, 107))),
                "case {case:?}: an upload within both bounds is held"
            );
        }
    }
}
