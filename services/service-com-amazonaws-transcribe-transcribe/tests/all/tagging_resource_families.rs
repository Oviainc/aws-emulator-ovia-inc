// SPDX-License-Identifier: CC0-1.0

//! The three tagging operations over EVERY resource family this service stores
//! records of, end to end over the wire.
//!
//! The rule these tests pin: an ARN of the form
//! `arn:<partition>:transcribe:<region>:<account>:<type>/<name>` resolves, through its
//! `<type>` segment, to the family whose records this service holds and, through its
//! `<name>`, to one stored record of that family — so `ListTagsForResource` answers
//! that record's tag set, `TagResource` merges into it (one value per key) and
//! `UntagResource` removes the named keys, whatever the family; an ARN naming no stored
//! record is refused instead.
//!
//! The rows differ along what the rule generalises over: the resource FAMILY (and so
//! the ARN `<type>` segment, the create that establishes a record and whether an
//! operation of that family reports `Tags` at all), whether the tags were supplied at
//! create time or added afterwards, and how many keys a call carries.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) rather than standing up a second emulator, so these
//! tests exercise the awsJson1_1 envelope and the boundary constraint validation too.

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::service::medical_scribe_availability::MEDICAL_SCRIBE_REGIONS;
use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_REGION;

use crate::transcription_jobs::Transcribe;

/// The modelled client error this service answers for an ARN that names no stored
/// resource, and the status its awsJson1_1 wire protocol puts on every client error
/// (both pinned by `crate::client_error_wire_status`).
const ABSENT_CODE: &str = "BadRequestException";
/// See [`ABSENT_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// A valid `DataAccessRoleArn`, whose `@pattern` pins it to an IAM role ARN
/// (`smithy_query(query="operation", operation="CreateLanguageModel")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/TranscribeAccess";

/// The read that reports a family's own view of its tag set: the operation, the member
/// that names the resource, and the member of the answer carrying the record.
struct Reader {
    operation: &'static str,
    name_member: &'static str,
    record: &'static str,
}

/// One resource family: the ARN `<type>` segment that names it, the create that
/// establishes a record of it, and the read that reports its tags (when the model gives
/// that family's output shape a `Tags` member at all).
///
/// `pub(crate)`, with the members a table of families needs: `crate::tagging_arn_qualifiers`
/// drives the SAME families through the same creates rather than writing a second copy of
/// this table.
pub(crate) struct Family {
    pub(crate) case: &'static str,
    /// The ARN resource type, as the `TranscribeArn` pattern's `<type>/<name>` segment
    /// spells it for this family.
    pub(crate) resource_type: &'static str,
    pub(crate) create: &'static str,
    /// This family's create request for a resource named `name`, without `Tags`.
    base_request: fn(&str) -> Value,
    /// `None` for a family no operation of which reports `Tags`: `VocabularyInfo`,
    /// `VocabularyFilterInfo` and `LanguageModel` have no such member
    /// (`smithy_query(query="shape", shape="VocabularyInfo,LanguageModel")`), so the
    /// standard tagging API is the only reader of their tag sets.
    reader: Option<Reader>,
    /// The Regions this family is SERVED in, or `None` for a family served wherever this
    /// service has an endpoint.
    ///
    /// Only the Medical Scribe family names a set: its operations belong to AWS
    /// HealthScribe, which is offered in a subset of this service's Regions
    /// (`service::medical_scribe_availability`), so a row driving a family through
    /// several Regions asks [`Family::is_served_in`] first.
    served_in: Option<&'static [&'static str]>,
}

/// Every family this service holds records of, each with a create this emulator
/// implements. `TagList` is accepted by each of these creates
/// (`smithy_query(query="operation", operation="StartTranscriptionJob,CreateVocabulary,\
/// CreateMedicalVocabulary,CreateVocabularyFilter,CreateLanguageModel,\
/// CreateCallAnalyticsCategory,StartMedicalScribeJob,StartMedicalTranscriptionJob,\
/// StartCallAnalyticsJob")`), so
/// each family's records carry a tag set the three tagging operations own.
pub(crate) fn families() -> Vec<Family> {
    vec![
        Family {
            case: "transcription job",
            resource_type: "transcription-job",
            create: "StartTranscriptionJob",
            base_request: |name| {
                json!({
                    "TranscriptionJobName": name,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
                })
            },
            reader: Some(Reader {
                operation: "GetTranscriptionJob",
                name_member: "TranscriptionJobName",
                record: "TranscriptionJob",
            }),
            served_in: None,
        },
        Family {
            case: "medical transcription job",
            resource_type: "medical-transcription-job",
            create: "StartMedicalTranscriptionJob",
            base_request: |name| {
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
            reader: Some(Reader {
                operation: "GetMedicalTranscriptionJob",
                name_member: "MedicalTranscriptionJobName",
                record: "MedicalTranscriptionJob",
            }),
            served_in: None,
        },
        Family {
            case: "custom vocabulary",
            resource_type: "vocabulary",
            create: "CreateVocabulary",
            base_request: |name| {
                json!({
                    "VocabularyName": name,
                    "LanguageCode": "en-US",
                    "Phrases": ["Aardvark", "Transcribe"]
                })
            },
            reader: None,
            served_in: None,
        },
        Family {
            case: "custom medical vocabulary",
            resource_type: "medical-vocabulary",
            create: "CreateMedicalVocabulary",
            base_request: |name| {
                json!({
                    "VocabularyName": name,
                    // "US English (en-US) is the only language supported with Amazon
                    // Transcribe Medical" (model doc for the medical language code).
                    "LanguageCode": "en-US",
                    "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/clinical-terms.txt"
                })
            },
            reader: None,
            served_in: None,
        },
        Family {
            case: "custom vocabulary filter",
            resource_type: "vocabulary-filter",
            create: "CreateVocabularyFilter",
            base_request: |name| {
                json!({
                    "VocabularyFilterName": name,
                    "LanguageCode": "en-US",
                    "Words": ["crud", "bogus"]
                })
            },
            reader: None,
            served_in: None,
        },
        Family {
            case: "custom language model",
            resource_type: "language-model",
            create: "CreateLanguageModel",
            base_request: |name| {
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
            reader: None,
            served_in: None,
        },
        Family {
            case: "Call Analytics category",
            resource_type: "analytics-category",
            create: "CreateCallAnalyticsCategory",
            base_request: |name| {
                json!({
                    "CategoryName": name,
                    "Rules": [{"NonTalkTimeFilter": {"Threshold": 10_000}}]
                })
            },
            reader: Some(Reader {
                operation: "GetCallAnalyticsCategory",
                name_member: "CategoryName",
                record: "CategoryProperties",
            }),
            served_in: None,
        },
        Family {
            case: "Medical Scribe job",
            resource_type: "medical-scribe-job",
            create: "StartMedicalScribeJob",
            base_request: |name| {
                json!({
                    "MedicalScribeJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
                })
            },
            reader: Some(Reader {
                operation: "GetMedicalScribeJob",
                name_member: "MedicalScribeJobName",
                record: "MedicalScribeJob",
            }),
            // The one family of this service that is not served Region-wide.
            served_in: Some(MEDICAL_SCRIBE_REGIONS),
        },
        Family {
            case: "Call Analytics job",
            // The published `<type>/${JobName}` segment of this family's ARN
            // (`arn:${Partition}:transcribe:${Region}:${Account}:analytics-job/${JobName}`),
            // the same hyphenation family as the Call Analytics CATEGORY type above.
            resource_type: "analytics-job",
            create: "StartCallAnalyticsJob",
            base_request: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/call.wav"},
                    "DataAccessRoleArn": ROLE_ARN
                })
            },
            reader: Some(Reader {
                operation: "GetCallAnalyticsJob",
                name_member: "CallAnalyticsJobName",
                record: "CallAnalyticsJob",
            }),
            served_in: None,
        },
    ]
}

impl Family {
    /// This family's create request for `name`, carrying `tags` when any are given.
    pub(crate) fn create_request(&self, name: &str, tags: &[(&str, &str)]) -> Value {
        let mut request = (self.base_request)(name);
        if !tags.is_empty() {
            request["Tags"] = tag_list(tags);
        }
        request
    }

    /// The ARN of this family's resource named `name`, as the three tagging operations
    /// take it: the ARN this service hands out for a request served in the emulator's
    /// default Region.
    fn arn(&self, name: &str) -> String {
        self.arn_in(DEFAULT_REGION, name)
    }

    /// The same ARN for a request served in `region` — built through the framework's own
    /// ARN helper, so no test names a partition or an account literal.
    pub(crate) fn arn_in(&self, region: &str, name: &str) -> String {
        engine::emulator_arn(
            "transcribe",
            region,
            &format!("{}/{name}", self.resource_type),
        )
    }

    /// The name this suite gives a resource of this family in `scenario`.
    pub(crate) fn resource_name(&self, scenario: &str) -> String {
        format!("{scenario}-{}", self.resource_type)
    }

    /// Whether a request served in `region` reaches this family at all: every family but
    /// the Medical Scribe one is served wherever this service has an endpoint, and that
    /// one only where AWS offers AWS HealthScribe (see [`Family::served_in`]).
    ///
    /// A row that drives a family through a Region of its own asks this first; a Region
    /// outside the set answers every operation of the family with its authorization
    /// refusal, which `crate::medical_scribe_region_availability` is the suite for.
    pub(crate) fn is_served_in(&self, region: &str) -> bool {
        self.served_in
            .is_none_or(|regions| regions.contains(&region))
    }
}

/// `tags` as the modelled `TagList` reaches the wire.
pub(crate) fn tag_list(tags: &[(&str, &str)]) -> Value {
    tags.iter()
        .map(|(key, value)| json!({"Key": key, "Value": value}))
        .collect()
}

/// The tag pairs a `Tags` list carries, sorted by key so a comparison is about the SET
/// a resource holds rather than the order a merge happened to leave it in.
pub(crate) fn pairs(tags: &Value, case: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = tags
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags list, got {tags:?}"))
        .iter()
        .map(|tag| {
            let key = tag["Key"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected a Key in {tag:?}")
            });
            let value = tag["Value"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected a Value in {tag:?}")
            });
            (key.to_string(), value.to_string())
        })
        .collect();
    found.sort();
    found
}

/// The same pairs, from a `(key, value)` table.
pub(crate) fn expected_pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = tags
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    expected.sort();
    expected
}

/// `ListTagsForResource` for `arn`, asserting it answered and echoed the ARN asked
/// about, and returning the tag set it reported.
async fn listed(
    transcribe: &Transcribe,
    arn: &str,
    case: &str,
) -> Vec<(String, String)> {
    let reply = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": arn}))
        .await;
    let body = reply.ok(case);
    assert_eq!(
        body.get("ResourceArn").and_then(Value::as_str),
        Some(arn),
        "case {case:?}: the answer must name the resource asked about, got {body:?}"
    );
    let tags = body
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {body:?}"));
    pairs(tags, case)
}

/// The tag set the family's OWN read reports for `name`, or `None` when that read
/// leaves `Tags` out (which is how a record carrying none reaches the wire: `TagList`
/// is modelled `@length(min: 1, max: 200)`, so an empty list is not a value of it).
async fn read_tags(
    transcribe: &Transcribe,
    reader: &Reader,
    name: &str,
    case: &str,
) -> Option<Vec<(String, String)>> {
    let reply = transcribe
        .call(reader.operation, json!({reader.name_member: name}))
        .await;
    let body = reply.ok(case);
    let record = body
        .get(reader.record)
        .unwrap_or_else(|| panic!("case {case:?}: expected {}, got {body:?}", reader.record));
    record
        .get("Tags")
        .filter(|tags| !tags.is_null())
        .map(|tags| pairs(tags, case))
}

/// Assert the family's own read agrees with `expected` about the resource's tag set.
async fn read_reports(
    transcribe: &Transcribe,
    family: &Family,
    name: &str,
    expected: &[(&str, &str)],
    case: &str,
) {
    let Some(reader) = &family.reader else {
        return;
    };
    let found = read_tags(transcribe, reader, name, case).await;
    let expected = (!expected.is_empty()).then(|| expected_pairs(expected));
    assert_eq!(
        found, expected,
        "case {case:?}: {} must report the stored tag set, got {found:?}",
        reader.operation
    );
}

/// The tags a create carries, and the tags the resource must then report through
/// `ListTagsForResource`: whatever the create stored.
///
/// One row per axis the rule generalises over: no tags at all, one tag, several tags,
/// and a tag whose value is the empty string the modelled `TagValue`
/// (`@length(min: 0, max: 256)`) allows.
fn create_time_tags() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        ("no tags at create time", vec![]),
        ("one tag at create time", vec![("team", "speech")]),
        (
            "three tags at create time",
            vec![("team", "speech"), ("env", "test"), ("cost", "7")],
        ),
        ("a tag whose value is empty", vec![("env", "")]),
    ]
}

/// RULE: the tag set a create stored is what `ListTagsForResource` reports for that
/// resource's ARN — for every family, and however many tags the create carried.
#[tokio::test]
async fn create_time_tags_are_reported_for_every_family() {
    let transcribe = Transcribe::new();

    for family in families() {
        for (index, (tag_case, tags)) in create_time_tags().into_iter().enumerate() {
            let case = format!("{} / {tag_case}", family.case);
            let name = family.resource_name(&format!("created{index}"));
            transcribe
                .call(family.create, family.create_request(&name, &tags))
                .await
                .ok(&case);

            let arn = family.arn(&name);
            assert_eq!(
                listed(&transcribe, &arn, &case).await,
                expected_pairs(&tags),
                "case {case:?}: the tagging API must report the tags the create stored"
            );
            read_reports(&transcribe, &family, &name, &tags, &case).await;
        }
    }
}

/// RULE: `TagResource` merges into the stored set — a key the resource does not carry
/// is added, a key it does carry takes the new value — and the merge is observable
/// both through `ListTagsForResource` and through the family's own read.
#[tokio::test]
async fn tag_resource_merges_into_every_family_s_stored_set() {
    let transcribe = Transcribe::new();

    for family in families() {
        let case = format!("{} / TagResource merge", family.case);
        let name = family.resource_name("merged");
        let created = [("team", "speech"), ("env", "test")];
        transcribe
            .call(family.create, family.create_request(&name, &created))
            .await
            .ok(&case);
        let arn = family.arn(&name);

        // One added key and one key already on the resource, whose value the merge
        // replaces rather than storing twice.
        transcribe
            .call(
                "TagResource",
                json!({
                    "ResourceArn": arn,
                    "Tags": tag_list(&[("env", "prod"), ("cost", "7")])
                }),
            )
            .await
            .ok(&case);

        let merged = [("team", "speech"), ("env", "prod"), ("cost", "7")];
        assert_eq!(
            listed(&transcribe, &arn, &case).await,
            expected_pairs(&merged),
            "case {case:?}: a merge must add the new key and replace the known key's value"
        );
        read_reports(&transcribe, &family, &name, &merged, &case).await;

        // A second merge of the same keys and values is idempotent: one value per key,
        // so nothing is added beside what is already there.
        transcribe
            .call(
                "TagResource",
                json!({"ResourceArn": arn, "Tags": tag_list(&merged)}),
            )
            .await
            .ok(&case);
        assert_eq!(
            listed(&transcribe, &arn, &case).await,
            expected_pairs(&merged),
            "case {case:?}: re-tagging with the stored pairs must change nothing"
        );
    }
}

/// RULE: `UntagResource` removes exactly the named keys from the stored set — a key the
/// resource does not carry removes nothing — for every family, and the removal is
/// observable through the family's own read too.
#[tokio::test]
async fn untag_resource_removes_the_named_keys_for_every_family() {
    let transcribe = Transcribe::new();

    for family in families() {
        let case = format!("{} / UntagResource", family.case);
        let name = family.resource_name("untagged");
        let created = [("team", "speech"), ("env", "test"), ("cost", "7")];
        transcribe
            .call(family.create, family.create_request(&name, &created))
            .await
            .ok(&case);
        let arn = family.arn(&name);

        // One key the resource carries and two it does not: the outcome is "these keys
        // are not on the resource", already true for the absent ones.
        transcribe
            .call(
                "UntagResource",
                json!({
                    "ResourceArn": arn,
                    "TagKeys": ["env", "not-a-key", "another-absent-key"]
                }),
            )
            .await
            .ok_status(&case, 200);

        let kept = [("team", "speech"), ("cost", "7")];
        assert_eq!(
            listed(&transcribe, &arn, &case).await,
            expected_pairs(&kept),
            "case {case:?}: an untag must remove only the keys it named"
        );
        read_reports(&transcribe, &family, &name, &kept, &case).await;

        // Removing the rest empties the set: the tagging API reports an empty list for
        // a resource it holds and carries no tags for.
        transcribe
            .call(
                "UntagResource",
                json!({"ResourceArn": arn, "TagKeys": ["team", "cost"]}),
            )
            .await
            .ok_status(&case, 200);
        assert_eq!(
            listed(&transcribe, &arn, &case).await,
            Vec::<(String, String)>::new(),
            "case {case:?}: a resource whose last tag was removed must report an empty set"
        );
        read_reports(&transcribe, &family, &name, &[], &case).await;

        // And the emptied set is still the resource's CURRENT set, not a constant: a
        // tag added afterwards is reported again.
        transcribe
            .call(
                "TagResource",
                json!({"ResourceArn": arn, "Tags": tag_list(&[("re-tagged", "yes")])}),
            )
            .await
            .ok(&case);
        assert_eq!(
            listed(&transcribe, &arn, &case).await,
            expected_pairs(&[("re-tagged", "yes")]),
            "case {case:?}: a tag added after the set emptied must be reported"
        );
        read_reports(&transcribe, &family, &name, &[("re-tagged", "yes")], &case).await;
    }
}

/// RULE: the ARN's `<name>` segment names ONE record of its family — a tagging call
/// reaches that record and no other, so same-named resources of two families carry
/// independent tag sets.
#[tokio::test]
async fn one_arn_reaches_one_record() {
    let transcribe = Transcribe::new();
    let families = families();
    let shared = "shared.name";
    let case = "one name across every family";

    for family in &families {
        transcribe
            .call(family.create, family.create_request(shared, &[]))
            .await
            .ok(case);
    }

    // Each family's ARN is tagged with a key naming that family, so a leak between two
    // of them cannot pass unnoticed.
    for family in &families {
        transcribe
            .call(
                "TagResource",
                json!({
                    "ResourceArn": family.arn(shared),
                    "Tags": tag_list(&[("family", family.resource_type)])
                }),
            )
            .await
            .ok(case);
    }

    for family in &families {
        let case = format!("{case} / {}", family.case);
        assert_eq!(
            listed(&transcribe, &family.arn(shared), &case).await,
            expected_pairs(&[("family", family.resource_type)]),
            "case {case:?}: a tagging call must reach only its own family's record"
        );
    }

    // Untagging one family's record leaves every other family's set alone.
    transcribe
        .call(
            "UntagResource",
            json!({"ResourceArn": families[0].arn(shared), "TagKeys": ["family"]}),
        )
        .await
        .ok_status(case, 200);
    for (index, family) in families.iter().enumerate() {
        let case = format!("{case} / after untagging {}", families[0].case);
        let found = listed(&transcribe, &family.arn(shared), &case).await;
        let expected = if index == 0 {
            Vec::new()
        } else {
            expected_pairs(&[("family", family.resource_type)])
        };
        assert_eq!(
            found, expected,
            "case {case:?}: untagging one record must not touch another family's set"
        );
    }
}

/// RULE, the other side: an ARN whose `<name>` names no stored record of its family —
/// and an ARN whose `<type>` names no resource family of this service at all —
/// is REFUSED by all three tagging operations, rather than answered with an empty tag
/// set or silently tagging nothing.
#[tokio::test]
async fn an_arn_naming_no_stored_record_is_refused() {
    let transcribe = Transcribe::new();
    let families = families();

    // Every family's own type segment, plus one the `TranscribeArn` pattern's
    // `<type>` segment admits that the model's own `ResourceArn` doc does not list and
    // no resource family of this service answers to.
    let mut resource_types: Vec<&str> = families
        .iter()
        .map(|family| family.resource_type)
        .collect();
    resource_types.push("call-analytics-job");

    for resource_type in resource_types {
        let arn = format!(
            "arn:aws:transcribe:us-east-1:{account}:{resource_type}/never-created",
            account = engine::emulator_account_id(),
        );
        let calls: Vec<(&str, Value)> = vec![
            ("ListTagsForResource", json!({"ResourceArn": arn})),
            (
                "TagResource",
                json!({"ResourceArn": arn, "Tags": [{"Key": "team", "Value": "speech"}]}),
            ),
            (
                "UntagResource",
                json!({"ResourceArn": arn, "TagKeys": ["team"]}),
            ),
        ];
        for (operation, request) in calls {
            let case = format!("{operation} / {resource_type} naming nothing");
            transcribe
                .call(operation, request)
                .await
                .refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
        }
    }

    // A refused tag call stores nothing: the resource is still absent to the tagging
    // API after the create that would have made it findable is run and the ARN of a
    // DIFFERENT name is asked about.
    for family in &families {
        let case = format!("{} / a refused tag call minted nothing", family.case);
        let name = family.resource_name("refused");
        transcribe
            .call(family.create, family.create_request(&name, &[]))
            .await
            .ok(&case);
        assert_eq!(
            listed(&transcribe, &family.arn(&name), &case).await,
            Vec::<(String, String)>::new(),
            "case {case:?}: the created resource must carry the tags it was created with"
        );
        transcribe
            .call(
                "ListTagsForResource",
                json!({"ResourceArn": family.arn("a.different.name")}),
            )
            .await
            .refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    }
}
