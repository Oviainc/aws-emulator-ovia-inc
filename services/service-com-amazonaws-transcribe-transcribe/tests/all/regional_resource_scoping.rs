// SPDX-License-Identifier: CC0-1.0

//! Amazon Transcribe resources are REGIONAL, end to end over the wire.
//!
//! The rule these tests pin: a resource of any family belongs to the one Region whose
//! endpoint created it, so a request served in another Region can neither read, list,
//! update, delete nor reference it, its name is free to be created again there, and the
//! per-account-per-Region ceilings count only the Region the request is served in. The
//! Region is caller-supplied and unauthenticated, so a family's store is bounded by that
//! ceiling TOGETHER WITH `state::MAX_REGIONS`, the cap on how many Regions hold records
//! of it at all.
//!
//! The rows differ along what the rule generalises over: the resource FAMILY (all nine
//! this emulator stores records of, each with its own create, read, listing and delete)
//! and the Region a call is signed for (four, deliberately including Regions no recorded
//! request used). The resource NAME is held constant across Regions on purpose — that is
//! what makes "the same name in two Regions is two resources" observable.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator, so these
//! tests exercise the awsJson1_1 envelope and the boundary constraint validation too.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::state::{
    DEFAULT_REGION, MAX_REGIONS, MAX_VOCABULARIES,
};

use crate::transcription_jobs::Transcribe;

/// Long enough for every clock-owed transition of a resource created during the test to
/// have fallen due: a queued job's `QUEUED -> IN_PROGRESS -> COMPLETED` is two spans of
/// `crate::state::JOB_TRANSITION_DELAY` (500 ms each), and a vocabulary's
/// `PENDING -> READY` is one of `crate::state::VOCABULARY_TRANSITION_DELAY`.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_EVERY_TRANSITION: Duration = Duration::from_millis(1_300);

/// The modelled client error this service answers for a resource the Region a request is
/// served in does not hold, and the status its awsJson1_1 wire protocol puts on every
/// client error (both pinned by `crate::client_error_wire_status`).
const ABSENT_CODE: &str = "BadRequestException";
/// See [`ABSENT_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// The Region a row's resource is created in: the one an unsigned request is served in
/// too, so a reported URI is predictable.
const HOME: &str = DEFAULT_REGION;

/// The Regions no row of a table creates its resource in. Four hyphen-separated forms
/// the Region-identifier shape allows, including a three-part one and a Region outside
/// the commercial partition, so no row depends on the shape of one Region string.
const ELSEWHERE: &[&str] = &["eu-west-1", "ap-southeast-3", "us-gov-west-1", "sa-east-1"];

/// A valid `DataAccessRoleArn`, whose `@pattern` pins it to an IAM role ARN
/// (`smithy_query(query="operation", operation="CreateLanguageModel")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/TranscribeAccess";

/// One resource family: the create that establishes a record, the read and the listing
/// that report it, the member its name travels in, and the delete that drops it where
/// this emulator implements one.
struct Family {
    case: &'static str,
    create: &'static str,
    /// This family's create request for a resource named `name`.
    request: fn(&str) -> Value,
    read: &'static str,
    /// The member both the read and the delete name the resource in.
    name_member: &'static str,
    list: &'static str,
    /// The member each entry of the listing carries the resource's name under.
    name_in_list: &'static str,
    /// `None` for a family whose delete is still a stub (`DeleteMedicalVocabulary`,
    /// `DeleteMedicalTranscriptionJob`).
    delete: Option<&'static str>,
}

/// Every family this service holds records of, each with the create, read and listing
/// this emulator implements
/// (`smithy_query(query="digest")` lists all four job families and all five
/// custom-resource families bound to the service).
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "transcription job",
            create: "StartTranscriptionJob",
            request: |name| {
                json!({
                    "TranscriptionJobName": name,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
                })
            },
            read: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
            list: "ListTranscriptionJobs",
            name_in_list: "TranscriptionJobName",
            delete: Some("DeleteTranscriptionJob"),
        },
        Family {
            case: "medical transcription job",
            create: "StartMedicalTranscriptionJob",
            request: |name| {
                json!({
                    "MedicalTranscriptionJobName": name,
                    // "US English (en-US) is the only valid value for medical
                    // transcription jobs" (model doc for this request's LanguageCode).
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                    "OutputBucketName": "example-medical-output",
                    "Specialty": "PRIMARYCARE",
                    "Type": "DICTATION"
                })
            },
            read: "GetMedicalTranscriptionJob",
            name_member: "MedicalTranscriptionJobName",
            list: "ListMedicalTranscriptionJobs",
            name_in_list: "MedicalTranscriptionJobName",
            delete: None,
        },
        Family {
            case: "Call Analytics job",
            create: "StartCallAnalyticsJob",
            request: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"}
                })
            },
            read: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
            list: "ListCallAnalyticsJobs",
            name_in_list: "CallAnalyticsJobName",
            delete: Some("DeleteCallAnalyticsJob"),
        },
        Family {
            case: "Medical Scribe job",
            create: "StartMedicalScribeJob",
            request: |name| {
                json!({
                    "MedicalScribeJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
                })
            },
            read: "GetMedicalScribeJob",
            name_member: "MedicalScribeJobName",
            list: "ListMedicalScribeJobs",
            name_in_list: "MedicalScribeJobName",
            delete: Some("DeleteMedicalScribeJob"),
        },
        Family {
            case: "custom vocabulary",
            create: "CreateVocabulary",
            request: |name| {
                json!({
                    "VocabularyName": name,
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark", "Transcribe"]
                })
            },
            read: "GetVocabulary",
            name_member: "VocabularyName",
            list: "ListVocabularies",
            name_in_list: "VocabularyName",
            delete: Some("DeleteVocabulary"),
        },
        Family {
            case: "custom medical vocabulary",
            create: "CreateMedicalVocabulary",
            request: |name| {
                json!({
                    "VocabularyName": name,
                    // "US English (en-US) is the only language supported with Amazon
                    // Transcribe Medical" (model doc for the medical language code).
                    "LanguageCode": "en-US",
                    "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/clinical-terms.txt"
                })
            },
            read: "GetMedicalVocabulary",
            name_member: "VocabularyName",
            list: "ListMedicalVocabularies",
            name_in_list: "VocabularyName",
            delete: None,
        },
        Family {
            case: "custom vocabulary filter",
            create: "CreateVocabularyFilter",
            request: |name| {
                json!({
                    "VocabularyFilterName": name,
                    "LanguageCode": "en-US",
                    "Words": ["crud", "bogus"]
                })
            },
            read: "GetVocabularyFilter",
            name_member: "VocabularyFilterName",
            list: "ListVocabularyFilters",
            name_in_list: "VocabularyFilterName",
            delete: Some("DeleteVocabularyFilter"),
        },
        Family {
            case: "custom language model",
            create: "CreateLanguageModel",
            request: |name| {
                json!({
                    "ModelName": name,
                    "LanguageCode": "en-US",
                    "BaseModelName": "WideBand",
                    "InputDataConfig": {
                        "S3Uri": "s3://DOC-EXAMPLE-BUCKET/training/",
                        "DataAccessRoleArn": ROLE_ARN
                    }
                })
            },
            read: "DescribeLanguageModel",
            name_member: "ModelName",
            list: "ListLanguageModels",
            name_in_list: "ModelName",
            delete: Some("DeleteLanguageModel"),
        },
        Family {
            case: "Call Analytics category",
            create: "CreateCallAnalyticsCategory",
            request: |name| {
                json!({
                    "CategoryName": name,
                    "Rules": [{"NonTalkTimeFilter": {"Threshold": 10_000}}]
                })
            },
            read: "GetCallAnalyticsCategory",
            name_member: "CategoryName",
            list: "ListCallAnalyticsCategories",
            name_in_list: "CategoryName",
            delete: Some("DeleteCallAnalyticsCategory"),
        },
    ]
}

/// A request whose only member is the one a family names its resource in.
///
/// Built from a map rather than written per family: the member NAME differs by family,
/// which is exactly what the table generalises over.
fn by_name(member: &str, name: &str) -> Value {
    let mut request = serde_json::Map::new();
    request.insert(member.to_string(), Value::String(name.to_string()));
    Value::Object(request)
}

/// Every name the body of a listing reports under `field`, wherever in the answer's
/// shape that member sits — flat on a job summary, nested inside `CategoryProperties`
/// or `LanguageModel` — so one helper reads every family's listing.
fn listed_names(body: &Value, field: &str) -> Vec<String> {
    let mut found = Vec::new();
    collect(body, field, &mut found);
    found.sort();
    found
}

/// The recursive half of [`listed_names`].
fn collect(value: &Value, field: &str, found: &mut Vec<String>) {
    match value {
        Value::Object(members) => {
            for (key, nested) in members {
                if key == field {
                    if let Some(name) = nested.as_str() {
                        found.push(name.to_string());
                    }
                }
                collect(nested, field, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect(item, field, found);
            }
        }
        _ => {}
    }
}

/// A resource created through one Region's endpoint is readable and listable through
/// that Region and through no other.
#[tokio::test]
async fn a_resource_is_reachable_only_through_the_region_that_created_it() {
    let transcribe = Transcribe::new();
    let name = "shared-name";

    for family in families() {
        let case = family.case;
        transcribe
            .call_in_region(family.create, (family.request)(name), Some(HOME))
            .await
            .ok(&format!("{case}: create in {HOME}"));

        let read_in_home = transcribe
            .call_in_region(family.read, by_name(family.name_member, name), Some(HOME))
            .await;
        read_in_home.ok(&format!("{case}: read in {HOME}"));

        let listed_in_home = transcribe
            .call_in_region(family.list, json!({}), Some(HOME))
            .await;
        let home_names = listed_names(
            listed_in_home.ok(&format!("{case}: list in {HOME}")),
            family.name_in_list,
        );
        assert!(
            home_names.iter().any(|found| found == name),
            "case {case:?}: {HOME} created the resource, so {} must report it, got {home_names:?}",
            family.list
        );

        for region in ELSEWHERE {
            let read = transcribe
                .call_in_region(family.read, by_name(family.name_member, name), Some(region))
                .await;
            read.refused(
                &format!("{case}: read in {region}"),
                CLIENT_ERROR_STATUS,
                ABSENT_CODE,
            );

            let listing = transcribe
                .call_in_region(family.list, json!({}), Some(region))
                .await;
            let names = listed_names(
                listing.ok(&format!("{case}: list in {region}")),
                family.name_in_list,
            );
            assert!(
                names.is_empty(),
                "case {case:?}: {region} holds nothing, so {} must report no resource, \
                 got {names:?}",
                family.list
            );
        }
    }
}

/// A name is unique per Region: the same create succeeds once in every Region and is
/// refused the second time only in the Region that already holds it.
#[tokio::test]
async fn one_name_is_taken_only_in_the_region_that_holds_it() {
    let transcribe = Transcribe::new();
    let name = "name.held_once-per-region";

    for family in families() {
        let case = family.case;
        let mut regions = vec![HOME];
        regions.extend_from_slice(ELSEWHERE);

        for region in &regions {
            transcribe
                .call_in_region(family.create, (family.request)(name), Some(region))
                .await
                .ok(&format!("{case}: first create in {region}"));
        }

        for region in &regions {
            let again = transcribe
                .call_in_region(family.create, (family.request)(name), Some(region))
                .await;
            again.refused(
                &format!("{case}: second create in {region}"),
                CLIENT_ERROR_STATUS,
                "ConflictException",
            );

            let read = transcribe
                .call_in_region(family.read, by_name(family.name_member, name), Some(region))
                .await;
            read.ok(&format!("{case}: read in {region}"));

            let listing = transcribe
                .call_in_region(family.list, json!({}), Some(region))
                .await;
            let names = listed_names(
                listing.ok(&format!("{case}: list in {region}")),
                family.name_in_list,
            );
            assert_eq!(
                names,
                vec![name.to_string()],
                "case {case:?}: {region} holds exactly its own resource of that name, \
                 got {names:?}"
            );
        }
    }
}

/// A delete reaches only its own Region's resource: the record of the same name in
/// every other Region survives it.
#[tokio::test]
async fn a_delete_reaches_only_its_own_regions_resource() {
    let transcribe = Transcribe::new();
    let name = "deleted-in-one-region";

    for family in families() {
        if family.delete.is_none() {
            continue;
        }
        let case = family.case;
        for region in [HOME, ELSEWHERE[0]] {
            transcribe
                .call_in_region(family.create, (family.request)(name), Some(region))
                .await
                .ok(&format!("{case}: create in {region}"));
        }
    }

    // One wait for every family, not one per family: a job and a vocabulary are only
    // deletable once the service has finished with them.
    std::thread::sleep(PAST_EVERY_TRANSITION);

    for family in families() {
        let Some(delete) = family.delete else {
            continue;
        };
        let case = family.case;
        let deleted_in = ELSEWHERE[0];

        transcribe
            .call_in_region(
                delete,
                by_name(family.name_member, name),
                Some(deleted_in),
            )
            .await
            .ok(&format!("{case}: delete in {deleted_in}"));

        let gone = transcribe
            .call_in_region(
                family.read,
                by_name(family.name_member, name),
                Some(deleted_in),
            )
            .await;
        gone.refused(
            &format!("{case}: read after the delete in {deleted_in}"),
            CLIENT_ERROR_STATUS,
            ABSENT_CODE,
        );

        let survivor = transcribe
            .call_in_region(family.read, by_name(family.name_member, name), Some(HOME))
            .await;
        survivor.ok(&format!(
            "{case}: the {HOME} resource survives a delete made in {deleted_in}"
        ));
    }
}

/// An update made through another Region's endpoint reaches nothing: it is refused, and
/// the resource keeps the Region it was created in — so the URI it reports does not
/// move.
#[tokio::test]
async fn an_update_reaches_only_its_own_regions_resource() {
    let transcribe = Transcribe::new();
    let elsewhere = ELSEWHERE[0];

    // A custom vocabulary: the family whose read reports a Region-dependent
    // `DownloadUri`, so a migration would be visible in the answer.
    transcribe
        .call_in_region(
            "CreateVocabulary",
            json!({
                "VocabularyName": "stays-put",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            Some(HOME),
        )
        .await
        .ok("vocabulary: create");
    std::thread::sleep(PAST_EVERY_TRANSITION);

    let before = transcribe
        .call_in_region(
            "GetVocabulary",
            json!({"VocabularyName": "stays-put"}),
            Some(HOME),
        )
        .await;
    let download_uri = before.member("vocabulary: before the update", "DownloadUri");
    assert!(
        download_uri.contains(HOME),
        "case {:?}: a vocabulary created in {HOME} reports its DownloadUri there, got \
         {download_uri:?}",
        "vocabulary: before the update"
    );

    let refused = transcribe
        .call_in_region(
            "UpdateVocabulary",
            json!({
                "VocabularyName": "stays-put",
                "LanguageCode": "en-US",
                "Phrases": ["Zebra"]
            }),
            Some(elsewhere),
        )
        .await;
    refused.refused(
        &format!("vocabulary: update in {elsewhere}"),
        CLIENT_ERROR_STATUS,
        ABSENT_CODE,
    );

    let after = transcribe
        .call_in_region(
            "GetVocabulary",
            json!({"VocabularyName": "stays-put"}),
            Some(HOME),
        )
        .await;
    let case = format!("vocabulary: after the refused update in {elsewhere}");
    assert_eq!(
        after.member(&case, "DownloadUri"),
        download_uri,
        "case {case:?}: the refused update must leave the reported DownloadUri where it was"
    );
    assert_eq!(
        after.member(&case, "VocabularyState"),
        "READY",
        "case {case:?}: the refused update must not restart the build"
    );

    // A Call Analytics category: the family whose read reports the rules an update
    // REPLACES, so a cross-Region update would be visible as a changed rule count.
    transcribe
        .call_in_region(
            "CreateCallAnalyticsCategory",
            json!({
                "CategoryName": "stays-put",
                "Rules": [{"NonTalkTimeFilter": {"Threshold": 10_000}}]
            }),
            Some(HOME),
        )
        .await
        .ok("category: create");

    let elsewhere_update = transcribe
        .call_in_region(
            "UpdateCallAnalyticsCategory",
            json!({
                "CategoryName": "stays-put",
                "Rules": [
                    {"NonTalkTimeFilter": {"Threshold": 20_000}},
                    {"InterruptionFilter": {"Threshold": 30_000}}
                ]
            }),
            Some(elsewhere),
        )
        .await;
    elsewhere_update.refused(
        &format!("category: update in {elsewhere}"),
        CLIENT_ERROR_STATUS,
        ABSENT_CODE,
    );

    let kept = transcribe
        .call_in_region(
            "GetCallAnalyticsCategory",
            json!({"CategoryName": "stays-put"}),
            Some(HOME),
        )
        .await;
    let case = format!("category: after the refused update in {elsewhere}");
    let rules = kept.ok(&case)["CategoryProperties"]["Rules"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Rules list, got {:?}", kept.body))
        .len();
    assert_eq!(
        rules, 1,
        "case {case:?}: the refused update must leave the stored rules as they were, \
         got {rules} rule(s)"
    );

    // The same update, through the Region that holds the resource, is served.
    let served = transcribe
        .call_in_region(
            "UpdateVocabulary",
            json!({
                "VocabularyName": "stays-put",
                "LanguageCode": "en-US",
                "Phrases": ["Zebra"]
            }),
            Some(HOME),
        )
        .await;
    served.ok(&format!("vocabulary: update in {HOME}"));
    std::thread::sleep(PAST_EVERY_TRANSITION);
    let case = format!("vocabulary: after the served update in {HOME}");
    let reread = transcribe
        .call_in_region(
            "GetVocabulary",
            json!({"VocabularyName": "stays-put"}),
            Some(HOME),
        )
        .await;
    assert_eq!(
        reread.member(&case, "DownloadUri"),
        download_uri,
        "case {case:?}: an update does not move the Region a resource lives in"
    );
}

/// A job may only name a custom resource of its OWN Region: the settings member is
/// resolved in the Region the start is served in.
#[tokio::test]
async fn a_job_references_only_a_custom_resource_of_its_own_region() {
    let transcribe = Transcribe::new();
    let elsewhere = ELSEWHERE[0];
    let vocabulary = "referenced-vocabulary";

    transcribe
        .call_in_region(
            "CreateVocabulary",
            json!({
                "VocabularyName": vocabulary,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            Some(HOME),
        )
        .await
        .ok("create the referenced vocabulary");

    let refused = transcribe
        .call_in_region(
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "cross-region-reference",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"},
                "Settings": {"VocabularyName": vocabulary}
            }),
            Some(elsewhere),
        )
        .await;
    refused.refused(
        &format!("a start in {elsewhere} naming a vocabulary only {HOME} holds"),
        CLIENT_ERROR_STATUS,
        ABSENT_CODE,
    );

    transcribe
        .call_in_region(
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "same-region-reference",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"},
                "Settings": {"VocabularyName": vocabulary}
            }),
            Some(HOME),
        )
        .await
        .ok(&format!("a start in {HOME} naming a vocabulary {HOME} holds"));

    // Creating the same name in the other Region makes the reference resolvable THERE
    // too — a second, independent resource.
    transcribe
        .call_in_region(
            "CreateVocabulary",
            json!({
                "VocabularyName": vocabulary,
                "LanguageCode": "en-US",
                "Phrases": ["Zebra"]
            }),
            Some(elsewhere),
        )
        .await
        .ok(&format!("create the same vocabulary name in {elsewhere}"));
    transcribe
        .call_in_region(
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "cross-region-reference",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"},
                "Settings": {"VocabularyName": vocabulary}
            }),
            Some(elsewhere),
        )
        .await
        .ok(&format!(
            "a start in {elsewhere} naming the vocabulary {elsewhere} now holds"
        ));
}

/// A tagging ARN resolves only in the Region that holds the resource it names: the
/// Region segment of the ARN and the Region the request is served in have to be the one
/// the resource lives in.
#[tokio::test]
async fn a_tagging_arn_resolves_only_where_the_resource_lives() {
    let transcribe = Transcribe::new();
    let elsewhere = ELSEWHERE[0];
    let name = "tagged-elsewhere";
    let arn_in = |region: &str| engine::emulator_arn("transcribe", region, &format!("vocabulary/{name}"));

    transcribe
        .call_in_region(
            "CreateVocabulary",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"],
                "Tags": [{"Key": "team", "Value": "speech"}]
            }),
            Some(elsewhere),
        )
        .await
        .ok(&format!("create in {elsewhere}"));

    let served = transcribe
        .call_in_region(
            "ListTagsForResource",
            json!({"ResourceArn": arn_in(elsewhere)}),
            Some(elsewhere),
        )
        .await;
    let case = format!("the {elsewhere} ARN read in {elsewhere}");
    let tags = served.ok(&case)["Tags"].clone();
    assert_eq!(
        tags,
        json!([{"Key": "team", "Value": "speech"}]),
        "case {case:?}: the Region that holds the resource reports its tags, got {tags:?}"
    );

    // The LOCAL ARN of the same name reaches nothing in a Region that holds no such
    // resource: the record another Region holds under that name is a different resource.
    let refused = transcribe
        .call_in_region(
            "ListTagsForResource",
            json!({"ResourceArn": arn_in(HOME)}),
            Some(HOME),
        )
        .await;
    refused.refused(
        &format!("the {HOME} ARN read in {HOME}"),
        CLIENT_ERROR_STATUS,
        ABSENT_CODE,
    );

    let write_refused = transcribe
        .call_in_region(
            "TagResource",
            json!({
                "ResourceArn": arn_in(HOME),
                "Tags": [{"Key": "team", "Value": "other"}]
            }),
            Some(HOME),
        )
        .await;
    write_refused.refused(
        &format!("a tag write through the {HOME} ARN"),
        CLIENT_ERROR_STATUS,
        ABSENT_CODE,
    );

    let write_served = transcribe
        .call_in_region(
            "TagResource",
            json!({
                "ResourceArn": arn_in(elsewhere),
                "Tags": [{"Key": "stage", "Value": "test"}]
            }),
            Some(elsewhere),
        )
        .await;
    write_served.ok(&format!("a tag write through the {elsewhere} ARN"));

    let case = format!("the {elsewhere} tag set after the write");
    let reread = transcribe
        .call_in_region(
            "ListTagsForResource",
            json!({"ResourceArn": arn_in(elsewhere)}),
            Some(elsewhere),
        )
        .await;
    let mut keys: Vec<String> = reread.ok(&case)["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags list, got {:?}", reread.body))
        .iter()
        .map(|tag| tag["Key"].as_str().unwrap_or_default().to_string())
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["stage".to_string(), "team".to_string()],
        "case {case:?}: the write merged into the tag set of the resource it named"
    );
}

/// The published ceiling is per account PER REGION: a Region at its limit does not
/// stop another Region from holding a resource of the same name.
#[tokio::test]
async fn a_per_region_ceiling_counts_only_its_own_region() {
    let transcribe = Transcribe::new();
    let elsewhere = ELSEWHERE[0];

    for index in 0..MAX_VOCABULARIES {
        transcribe
            .call_in_region(
                "CreateVocabulary",
                json!({
                    "VocabularyName": format!("quota-{index}"),
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark"]
                }),
                Some(HOME),
            )
            .await
            .ok(&format!("create {index} in {HOME}"));
    }

    let past_the_limit = transcribe
        .call_in_region(
            "CreateVocabulary",
            json!({
                "VocabularyName": "quota-one-too-many",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            Some(HOME),
        )
        .await;
    past_the_limit.refused(
        &format!("one past the {MAX_VOCABULARIES} the {HOME} endpoint holds"),
        CLIENT_ERROR_STATUS,
        "LimitExceededException",
    );

    // The same name, in a Region holding nothing: served, because the ceiling the
    // refusal above reports is that Region's own.
    transcribe
        .call_in_region(
            "CreateVocabulary",
            json!({
                "VocabularyName": "quota-0",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            Some(elsewhere),
        )
        .await
        .ok(&format!("the first create in {elsewhere}"));
}

/// The number of Regions a family keeps records for is itself bounded: the Region a
/// record is keyed by comes from the caller's own credential scope, so each family holds
/// records for at most `MAX_REGIONS` Regions and a create signed for an
/// `MAX_REGIONS + 1`-th one is refused rather than opening a fresh per-Region bucket.
///
/// One row per family, so the cap is pinned on every store the Region dimension was
/// added to — a count-quota family and a concurrency-quota job family alike.
#[tokio::test]
async fn the_number_of_regions_a_family_holds_records_for_is_bounded() {
    let name = "region-cap";
    // Region-shaped identifiers (`crate::state::is_region_like`) that are not Regions
    // any other row uses, so the cap is reached by the Regions this row created.
    let region_of = |index: usize| format!("zz-cap-{index}");

    for family in families() {
        let case = family.case;
        // A fresh emulator per family: the cap is per family, and this row is about one
        // family reaching it.
        let transcribe = Transcribe::new();

        for index in 0..MAX_REGIONS {
            transcribe
                .call_in_region(
                    family.create,
                    (family.request)(name),
                    Some(&region_of(index)),
                )
                .await
                .ok(&format!(
                    "{case}: the record in Region {} of {MAX_REGIONS}",
                    index + 1
                ));
        }

        let over = region_of(MAX_REGIONS);
        let past_the_cap = transcribe
            .call_in_region(family.create, (family.request)(name), Some(&over))
            .await;
        let case_over = format!("{case}: one Region past the cap of {MAX_REGIONS}");
        past_the_cap.refused(&case_over, CLIENT_ERROR_STATUS, "LimitExceededException");
        let message = past_the_cap.error_message(&case_over);
        assert!(
            message.contains(&MAX_REGIONS.to_string()) && message.contains(&over),
            "case {case_over:?}: the refusal must name the cap and the Region asked for, \
             got {message:?}"
        );

        // A Region the caller signed for before is still served: the cap bounds how many
        // Regions hold records, not how many a caller may name.
        let held = region_of(0);
        transcribe
            .call_in_region(
                family.create,
                (family.request)("region-cap-second"),
                Some(&held),
            )
            .await
            .ok(&format!("{case}: a second record in a Region already held"));
    }
}

/// The Region cap bounds what is STORED: emptying a Region's records frees its slot, so
/// a Region the store no longer holds anything for stops counting against the cap.
#[tokio::test]
async fn emptying_a_region_frees_its_slot_against_the_cap() {
    let transcribe = Transcribe::new();
    let name = "region-cap";
    let region_of = |index: usize| format!("zz-free-{index}");

    for index in 0..MAX_REGIONS {
        transcribe
            .call_in_region(
                "CreateVocabulary",
                json!({
                    "VocabularyName": name,
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark"]
                }),
                Some(&region_of(index)),
            )
            .await
            .ok(&format!("the vocabulary in Region {} of {MAX_REGIONS}", index + 1));
    }

    // A vocabulary is only deletable once the service has finished building it.
    std::thread::sleep(PAST_EVERY_TRANSITION);

    let freed = region_of(0);
    transcribe
        .call_in_region(
            "DeleteVocabulary",
            json!({"VocabularyName": name}),
            Some(&freed),
        )
        .await
        .ok(&format!("empty {freed}"));

    let over = region_of(MAX_REGIONS);
    transcribe
        .call_in_region(
            "CreateVocabulary",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            Some(&over),
        )
        .await
        .ok(&format!(
            "a new Region once {freed} holds nothing, so the cap has room again"
        ));
}
