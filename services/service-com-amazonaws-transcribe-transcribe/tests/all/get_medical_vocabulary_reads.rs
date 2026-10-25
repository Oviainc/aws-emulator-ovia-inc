// SPDX-License-Identifier: CC0-1.0

//! What `GetMedicalVocabulary` REPORTS, over the wire, beyond the create ->
//! read -> list walk `crate::medical_vocabularies` already drives.
//!
//! THE RULES THESE ROWS ASSERT:
//!
//! 1. A custom medical vocabulary's NAME identifies it exactly: "Custom medical
//!    vocabulary names are case sensitive"
//!    (`smithy_query(query="doc", shape="GetMedicalVocabularyRequest",
//!    member="VocabularyName")`). So two names differing only in case are two
//!    resources, each read back with its OWN record, and a spelling no record of the
//!    Region carries is this family's not-found refusal rather than a near match.
//! 2. The operation only REPORTS: reading a medical vocabulary never changes what a
//!    later caller sees. The single state change across a sequence of reads is the
//!    transition the clock already owed the record, materialised at the instant it
//!    fell due — so repeated reads neither bring it forward nor move the
//!    `LastModifiedTime` it stamped, and a read (found or refused) adds nothing to
//!    and removes nothing from the family's listing.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these rows exercise the awsJson1_1 envelope and the handler's own
//! refusals. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`); the medical-vocabulary facts
//! (`MEDICAL_LANGUAGE`, [`crate::medical_vocabularies::is_absent`],
//! `listed_names`, the wait past the transition) are the family suite's, reused rather
//! than spelled a second time.

use serde_json::{json, Value};

use crate::medical_vocabularies::{
    is_absent, listed_names, MEDICAL_LANGUAGE, PAST_THE_TRANSITION,
};
use crate::transcription_jobs::{Reply, Transcribe};

/// The code this family answers for a medical vocabulary the Region does not hold:
/// the shape whose own documentation covers an entity that "doesn't exist".
const NOT_FOUND_CODE: &str = "BadRequestException";

/// Pairs of names that differ ONLY in case, one row per form the `VocabularyName`
/// `@pattern` (`^[0-9a-zA-Z._-]+$`) admits a case distinction in: a whole word, a
/// leading letter, a dotted name and an underscored, digit-bearing one. The third
/// column is a THIRD casing of the same letters that neither row stores, so the
/// refusal side of the rule is asserted on a spelling that is a near match of both.
fn case_variant_rows() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "a whole word lowercased and uppercased",
            "clinicalterms",
            "CLINICALTERMS",
            "ClinicalTerms",
        ),
        (
            "a leading letter in each case",
            "cardiology",
            "Cardiology",
            "cardiologY",
        ),
        (
            "a dotted name",
            "oncology.v2",
            "Oncology.V2",
            "oncology.V2",
        ),
        (
            "an underscored name carrying digits",
            "icd_10_codes",
            "ICD_10_CODES",
            "Icd_10_Codes",
        ),
    ]
}

/// The table file a row of [`case_variant_rows`] names, distinct per name so the
/// record a read reports can be told apart from its case twin's by the file its
/// `FailureReason` names.
fn file_uri_for(name: &str) -> String {
    format!("s3://DOC-EXAMPLE-BUCKET/{name}.txt")
}

/// Create the medical vocabulary `name` from its own table file, and answer the reply.
async fn create(transcribe: &Transcribe, name: &str) -> Reply {
    transcribe
        .call(
            "CreateMedicalVocabulary",
            json!({
                "VocabularyName": name,
                "LanguageCode": MEDICAL_LANGUAGE,
                "VocabularyFileUri": file_uri_for(name)
            }),
        )
        .await
}

/// Read the medical vocabulary `name` back.
async fn get(transcribe: &Transcribe, name: &str) -> Reply {
    transcribe
        .call("GetMedicalVocabulary", json!({"VocabularyName": name}))
        .await
}

/// RULE 1: the name identifies the resource exactly, so case twins are two resources
/// and each reads back with its own record, while a casing nothing stores is refused.
#[tokio::test]
async fn a_medical_vocabulary_is_read_by_its_exact_case_sensitive_name() {
    let transcribe = Transcribe::new();
    let rows = case_variant_rows();

    // Both casings of each row are accepted: a name is taken only by itself, so the
    // second create is not the conflict a re-create of one name would be.
    for (case, lower, upper, _) in &rows {
        for name in [lower, upper] {
            let created = create(&transcribe, name).await;
            created.ok(&format!("{case}: creating {name}"));
            assert_eq!(
                created.member(case, "VocabularyName"),
                *name,
                "case {case:?}: a create must answer the name it stored, got {:?}",
                created.body
            );
        }
    }

    // Every casing created is in the store, and nothing else is.
    let mut expected: Vec<String> = rows
        .iter()
        .flat_map(|(_, lower, upper, _)| [(*lower).to_string(), (*upper).to_string()])
        .collect();
    let listed = transcribe
        .call("ListMedicalVocabularies", json!({"MaxResults": 100}))
        .await;
    let mut names = listed_names(&listed, "every casing created");
    names.sort();
    expected.sort();
    assert_eq!(
        names, expected,
        "the store must hold each casing as a resource of its own, got {:?}",
        listed.body
    );

    // A casing neither create stored is not a near match of either: it is not found.
    for (case, _, _, absent) in &rows {
        get(&transcribe, absent)
            .await
            .refused(
                &format!("{case}: a casing no create stored ({absent})"),
                400,
                NOT_FOUND_CODE,
            );
    }

    // The transition the clock owes every one of these records: the build that would
    // have had to READ the table file ends in the modelled failure state, whose reason
    // names that record's OWN file — which is what tells the two case twins apart.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, lower, upper, _) in &rows {
        for name in [lower, upper] {
            let case = format!("{case}: reading {name}");
            let got = get(&transcribe, name).await;
            got.ok(&case);
            assert_eq!(
                got.member(&case, "VocabularyName"),
                *name,
                "case {case:?}: the reader must report the name it was asked for, got {:?}",
                got.body
            );
            let reason = got.member(&case, "FailureReason");
            let own_file = file_uri_for(name);
            assert!(
                reason.contains(&own_file),
                "case {case:?}: the record read back must be the one stored under this \
                 exact name, whose file is {own_file:?}, got {reason:?}"
            );
        }
    }
}

/// RULE 2: the operation reports. A sequence of reads sees exactly the one change the
/// clock owed the record, stamped at the instant it fell due, and leaves the family's
/// listing as it was.
#[tokio::test]
async fn reading_a_medical_vocabulary_changes_nothing_a_later_caller_sees() {
    let transcribe = Transcribe::new();
    let name = "polled_vocabulary";
    let created = create(&transcribe, name).await;
    created.ok("the vocabulary that is polled");
    let created_time = created.timestamp("the vocabulary that is polled", "LastModifiedTime");

    // Polling inside the transition's span never brings it forward: the span is
    // non-zero, so every one of these reads still sees the transient state.
    for poll in 0..5 {
        let case = format!("poll {poll} while the build is still owed");
        let got = get(&transcribe, name).await;
        got.ok(&case);
        assert_eq!(
            (
                got.member(&case, "VocabularyState"),
                got.timestamp(&case, "LastModifiedTime"),
            ),
            ("PENDING".to_string(), created_time),
            "case {case:?}: a read must not advance the record it reports, got {:?}",
            got.body
        );
        assert!(
            is_absent(&got, "FailureReason") && is_absent(&got, "DownloadUri"),
            "case {case:?}: a PENDING vocabulary has neither failed nor a file to \
             download, got {:?}",
            got.body
        );
    }

    // A read that finds nothing creates nothing either.
    get(&transcribe, "never_created")
        .await
        .refused("a medical vocabulary nothing created", 400, NOT_FOUND_CODE);

    std::thread::sleep(PAST_THE_TRANSITION);

    // The one change across the whole sequence, and it is the clock's: the first read
    // past the due instant reports the failure, stamped at that instant rather than at
    // the time of the read.
    let first = get(&transcribe, name).await;
    first.ok("the first read past the due instant");
    let failed_at = first.timestamp("the first read past the due instant", "LastModifiedTime");
    assert_eq!(
        first.member("the first read past the due instant", "VocabularyState"),
        "FAILED",
        "a medical vocabulary whose file is never read must end FAILED, got {:?}",
        first.body
    );
    assert!(
        failed_at > created_time,
        "the modification instant must move to the transition's own: created at \
         {created_time}, reported {failed_at}"
    );
    let reason = first.member("the first read past the due instant", "FailureReason");

    // Every later read is the same answer: nothing the reads themselves did is visible.
    for poll in 0..3 {
        let case = format!("poll {poll} after the transition");
        let again = get(&transcribe, name).await;
        again.ok(&case);
        assert_eq!(
            (
                again.member(&case, "VocabularyState"),
                again.timestamp(&case, "LastModifiedTime"),
                again.member(&case, "FailureReason"),
            ),
            ("FAILED".to_string(), failed_at, reason.clone()),
            "case {case:?}: a repeated read must report the same record, got {:?}",
            again.body
        );
        assert!(
            is_absent(&again, "DownloadUri"),
            "case {case:?}: no file was built for this vocabulary, so there is nothing \
             to download, got {:?}",
            again.body
        );
    }

    // The listing agrees with the reader, and holds exactly the one record created:
    // the eight reads above stored nothing.
    let listed = transcribe
        .call("ListMedicalVocabularies", json!({}))
        .await;
    assert_eq!(
        listed_names(&listed, "after the polling"),
        vec![name.to_string()],
        "the reads must have stored nothing, got {:?}",
        listed.body
    );
    let entry = &listed.ok("after the polling")["Vocabularies"][0];
    assert_eq!(
        (
            entry["VocabularyState"].as_str(),
            entry["LastModifiedTime"].as_f64()
        ),
        (Some("FAILED"), Some(failed_at)),
        "the listing must report the record the reader reports, got {:?}",
        listed.body
    );
    assert_eq!(
        entry["LanguageCode"].as_str(),
        Some(MEDICAL_LANGUAGE),
        "the listing must report the language the create named, got {:?}",
        listed.body
    );
    assert!(
        entry.get("FailureReason").is_none_or(Value::is_null),
        "the listed VocabularyInfo shape has no FailureReason member, got {:?}",
        listed.body
    );
}
