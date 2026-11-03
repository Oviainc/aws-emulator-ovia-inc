// SPDX-License-Identifier: CC0-1.0

//! `TagResource`, end to end over HTTP.
//!
//! The rule under test is the one FIS's tagging surface states: a call that applies tags
//! names its resource by ARN, and that ARN is resolved in this service's OWN namespace — a
//! value that is not an ARN this service could have ISSUED for a resource carrying caller
//! tags (not an ARN at all, another vendor's, a resource type this service holds no caller
//! tags for, or a resource id outside the form this service mints) addresses nothing here
//! and is refused as a bad request (400 `BadRequestException`), an ARN naming a resource of
//! this service the store holds no record of is a miss (404 `ResourceNotFoundException`),
//! and otherwise the request's tags are MERGED onto that resource's own tag map — a key it
//! already carries takes the new value, a key the request does not name keeps the value it
//! had, and nothing else about the record changes. A successful application answers 204 No
//! Content. The tags a call leaves are the tags that resource's own reader reports, which
//! is what makes the write persisted rather than acknowledged.
//!
//! The rows below differ along everything that rule generalises over: WHICH kind of
//! resource is tagged (an experiment template and an experiment, the two records of this
//! service that carry caller tags), how many tags a call carries (none, one, three, the
//! most a resource holds), whether a key is new or already present, how many calls the
//! tags arrive in, and the shape of a key and a value (plain, empty, whitespace,
//! non-ASCII, and each at the ceiling `TagKey` and `TagValue` publish).
//!
//! The refusal tables assert the other half of the rule, each with the stored tags read
//! back afterwards: a refused call writes NOTHING.
//!
//! The harness below — the resource builders, the tag-map helpers, the modelled ceilings
//! and the table of ARNs this service cannot address — is `pub(crate)` because the tag
//! READ suite (`list_tags_for_resource`) asserts its half of the same rule over the same
//! resources, and the two suites must not drift on either.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{create_template, emulator, send, Answer, FALLBACK_REGION};

/// `TagKey`'s published ceiling (`@length(min: 0, max: 128)` on
/// `smithy_query(query="shape", shape="TagKey", …)`), spelled out here rather than
/// imported from the crate so a wrong bound in `src/state.rs` fails this suite.
pub(crate) const TAG_KEY_CEILING: usize = 128;

/// `TagValue`'s published ceiling (`@length(min: 0, max: 256)`), spelled out for the same
/// reason.
pub(crate) const TAG_VALUE_CEILING: usize = 256;

/// The most tags one resource carries: `TagMap` is modelled `@length(min: 0, max: 50)`, so
/// a resource holding more could not be reported in one.
pub(crate) const TAGS_PER_RESOURCE: usize = 50;

/// A well-formed IAM role ARN for the templates this suite creates. The emulator's IAM
/// store holds no role for it, which is what lets `StartExperiment` accept it.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-tagging-suite";

/// The account id every ARN this emulator mints carries.
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// How many characters follow the `EXT`/`EXP` prefix of an id this service mints (FIS's
/// own `ID_SUFFIX_LEN`), spelled out here rather than imported so a changed id form fails
/// this suite.
pub(crate) const ID_SUFFIX_LEN: usize = 12;

/// A minted-SHAPE experiment-template id: the `EXT` prefix plus [`ID_SUFFIX_LEN`]
/// characters of the upper-case base-36 alphabet `format_id` writes.
pub(crate) fn template_id_shaped(suffix: &str) -> String {
    format!("EXT{suffix}")
}

/// A wait inside the 1..720 minutes FIS documents for `aws:fis:wait`'s `duration`.
const WAIT_DURATION: &str = "PT5M";

/// The `@httpLabel` form of an ARN: `resourceArn` is a NON-greedy label, so a client
/// percent-encodes the `:` and `/` it carries before putting it in the path.
pub(crate) fn label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// A `clientToken` (`pattern ^[\S]+$`) derived from a case name.
pub(crate) fn client_token(prefix: &str, case: &str) -> String {
    let slug: String = case
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("{prefix}-{slug}")
}

/// `TagResource`: apply `tags` to the resource `resource_arn` names.
pub(crate) async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &Value,
) -> Answer {
    send(
        router,
        "POST",
        &format!("/tags/{}", label(resource_arn)),
        Some(json!({ "tags": tags })),
        FALLBACK_REGION,
    )
    .await
}

/// Which taggable resource a case tags. These are the two records of this service that
/// carry caller tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    ExperimentTemplate,
    Experiment,
}

impl Kind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ExperimentTemplate => "experiment template",
            Self::Experiment => "experiment",
        }
    }
}

/// One live resource: the ARN a tag call names it by, and where its own reader reports it.
pub(crate) struct Resource {
    pub(crate) arn: String,
    pub(crate) read_uri: String,
    pub(crate) member: &'static str,
}

/// The `aws:fis:wait` action every template in this suite declares.
pub(crate) fn wait_action() -> Value {
    json!({ "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } })
}

/// Create a template carrying `tags` and report its id and ARN.
pub(crate) async fn new_template(
    router: &MultiServiceRouter,
    case: &str,
    tags: &Value,
) -> (String, String) {
    let answer = create_template(
        router,
        json!({
            "clientToken": client_token("tpl", case),
            "description": "the tagging suite's template",
            "roleArn": ROLE_ARN,
            "stopConditions": [{ "source": "none" }],
            "actions": wait_action(),
            "tags": tags,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    let template = answer.member("experimentTemplate");
    let id = template["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {:?}", answer.body))
        .to_string();
    let arn = template["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.arn in {:?}", answer.body))
        .to_string();
    (id, arn)
}

/// One resource of kind `kind`, created carrying `tags`.
pub(crate) async fn new_resource(
    router: &MultiServiceRouter,
    kind: Kind,
    case: &str,
    tags: &Value,
) -> Resource {
    match kind {
        Kind::ExperimentTemplate => {
            let (id, arn) = new_template(router, case, tags).await;
            Resource {
                arn,
                read_uri: format!("/experimentTemplates/{id}"),
                member: "experimentTemplate",
            }
        }
        Kind::Experiment => {
            // The experiment's own tags are the ones the start request carries, so the
            // template it runs is created without any.
            let (template_id, _) = new_template(router, case, &json!({})).await;
            let answer = send(
                router,
                "POST",
                "/experiments",
                Some(json!({
                    "clientToken": client_token("exp", case),
                    "experimentTemplateId": template_id,
                    "tags": tags,
                })),
                FALLBACK_REGION,
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
                answer.status, answer.body
            );
            let experiment = answer.member("experiment");
            let id = experiment["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no experiment.id in {:?}", answer.body))
                .to_string();
            let arn = experiment["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no experiment.arn in {:?}", answer.body))
                .to_string();
            Resource {
                arn,
                read_uri: format!("/experiments/{id}"),
                member: "experiment",
            }
        }
    }
}

/// The resource as its own reader reports it.
pub(crate) async fn read_resource(
    router: &MultiServiceRouter,
    resource: &Resource,
    case: &str,
) -> Value {
    let answer = send(router, "GET", &resource.read_uri, None, FALLBACK_REGION).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 reading {}, got {} with {:?}",
        resource.read_uri, answer.status, answer.body
    );
    answer.member(resource.member).clone()
}

/// The tag map a reported resource carries.
pub(crate) fn tags_of(record: &Value, case: &str) -> BTreeMap<String, String> {
    record["tags"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: no tags map in {record:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The reported resource with its tag map removed: what a tag call must leave untouched.
pub(crate) fn without_tags(record: &Value) -> Value {
    let mut record = record.clone();
    if let Some(object) = record.as_object_mut() {
        object.remove("tags");
    }
    record
}

/// A JSON tag map from pairs.
pub(crate) fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// `count` distinct tags, for the rows about how many a resource carries.
pub(crate) fn generated(count: usize, value: &str) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index:02}"), format!("{value}-{index:02}")))
        .collect()
}

/// One row: the tags the resource is created with, the tag maps applied to it in order,
/// and the tags its reader must report afterwards.
pub(crate) struct Row {
    pub(crate) case: String,
    pub(crate) initial: Vec<(String, String)>,
    pub(crate) applies: Vec<Vec<(String, String)>>,
    pub(crate) expected: Vec<(String, String)>,
}

pub(crate) fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The rows the merge rule is asserted over. The recorded case — one tag applied to a
/// resource that carries none — is the first row.
fn merge_rows() -> Vec<Row> {
    let key_at_ceiling = "k".repeat(TAG_KEY_CEILING);
    let value_at_ceiling = "v".repeat(TAG_VALUE_CEILING);
    vec![
        Row {
            case: "one tag onto a resource carrying none".to_string(),
            initial: Vec::new(),
            applies: vec![pairs(&[("Env", "test")])],
            expected: pairs(&[("Env", "test")]),
        },
        Row {
            case: "three tags in one call".to_string(),
            initial: Vec::new(),
            applies: vec![pairs(&[("Env", "test"), ("Owner", "fis"), ("Team", "chaos")])],
            expected: pairs(&[("Env", "test"), ("Owner", "fis"), ("Team", "chaos")]),
        },
        Row {
            case: "a key already carried takes the new value, the others are kept".to_string(),
            initial: pairs(&[("Env", "test"), ("Suite", "tagging")]),
            applies: vec![pairs(&[("Env", "prod"), ("Owner", "fis")])],
            expected: pairs(&[("Env", "prod"), ("Owner", "fis"), ("Suite", "tagging")]),
        },
        Row {
            case: "an empty tag map leaves the tags exactly as they were".to_string(),
            initial: pairs(&[("Env", "test")]),
            applies: vec![Vec::new()],
            expected: pairs(&[("Env", "test")]),
        },
        Row {
            case: "the same map applied twice is the same as applying it once".to_string(),
            initial: Vec::new(),
            applies: vec![pairs(&[("Env", "test")]), pairs(&[("Env", "test")])],
            expected: pairs(&[("Env", "test")]),
        },
        Row {
            case: "tags arriving in three calls accumulate".to_string(),
            initial: pairs(&[("Suite", "tagging")]),
            applies: vec![
                pairs(&[("Env", "test")]),
                pairs(&[("Owner", "fis")]),
                pairs(&[("Env", "staging"), ("Team", "chaos")]),
            ],
            expected: pairs(&[
                ("Env", "staging"),
                ("Owner", "fis"),
                ("Suite", "tagging"),
                ("Team", "chaos"),
            ]),
        },
        Row {
            case: "an empty value, which TagValue's pattern admits".to_string(),
            initial: Vec::new(),
            applies: vec![pairs(&[("Env", "")])],
            expected: pairs(&[("Env", "")]),
        },
        Row {
            case: "a key and a value carrying whitespace".to_string(),
            initial: Vec::new(),
            applies: vec![pairs(&[("two words", "a b"), (" ", "just a space")])],
            expected: pairs(&[("two words", "a b"), (" ", "just a space")]),
        },
        Row {
            case: "a non-ASCII key and value".to_string(),
            initial: Vec::new(),
            applies: vec![pairs(&[("環境", "本番"), ("naïve", "café")])],
            expected: pairs(&[("環境", "本番"), ("naïve", "café")]),
        },
        Row {
            case: "a value at TagValue's published ceiling".to_string(),
            initial: Vec::new(),
            applies: vec![vec![("Long".to_string(), value_at_ceiling.clone())]],
            expected: vec![("Long".to_string(), value_at_ceiling)],
        },
        Row {
            case: "a key at TagKey's published ceiling".to_string(),
            initial: Vec::new(),
            applies: vec![vec![(key_at_ceiling.clone(), "x".to_string())]],
            expected: vec![(key_at_ceiling, "x".to_string())],
        },
        Row {
            case: "the most tags a resource carries, in one call".to_string(),
            initial: Vec::new(),
            applies: vec![generated(TAGS_PER_RESOURCE, "once")],
            expected: generated(TAGS_PER_RESOURCE, "once"),
        },
        Row {
            case: "the most tags a resource carries, reached across two calls".to_string(),
            initial: generated(TAGS_PER_RESOURCE - 10, "first"),
            applies: vec![generated(TAGS_PER_RESOURCE, "second")],
            expected: generated(TAGS_PER_RESOURCE, "second"),
        },
    ]
}

/// The merge rule, over both kinds of taggable resource.
#[tokio::test]
async fn applied_tags_are_merged_onto_the_resource_its_reader_reports() {
    for kind in [Kind::ExperimentTemplate, Kind::Experiment] {
        for row in merge_rows() {
            let case = format!("{} / {}", kind.as_str(), row.case);
            // One emulator per row, so each row judges a resource of its own.
            let router = emulator();
            let resource = new_resource(&router, kind, &case, &tag_map(&row.initial)).await;
            let before = without_tags(&read_resource(&router, &resource, &case).await);

            for (index, apply) in row.applies.iter().enumerate() {
                let answer = tag_resource(&router, &resource.arn, &tag_map(apply)).await;
                assert_eq!(
                    answer.status, 204,
                    "case {case:?} call {index}: expected HTTP 204 from TagResource, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
            }

            let record = read_resource(&router, &resource, &case).await;
            let expected: BTreeMap<String, String> = row.expected.iter().cloned().collect();
            assert_eq!(
                tags_of(&record, &case),
                expected,
                "case {case:?}: expected the merged tags, got {:?}",
                record["tags"]
            );
            assert_eq!(
                without_tags(&record),
                before,
                "case {case:?}: a tag call must change only the tag map, got {:?}",
                without_tags(&record)
            );
        }
    }
}

/// An ARN that addresses nothing in this service's namespace, with the reason it does not.
///
/// The last rows are about the resource ID: a tag call accepts only an ARN this service
/// could have ISSUED, so an id outside the form it mints (the type's `EXT`/`EXP` prefix
/// plus [`ID_SUFFIX_LEN`] upper-case base-36 characters) names nothing here, exactly as a
/// resource type it holds no tags for does.
pub(crate) fn unaddressable_arns() -> Vec<(&'static str, String)> {
    let digits = "0".repeat(ID_SUFFIX_LEN);
    vec![
        (
            "a value that is not an ARN at all",
            "this-is-not-an-arn-at-all".to_string(),
        ),
        (
            "an ARN missing its resource field",
            format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}"),
        ),
        (
            "an ARN of another vendor",
            format!("arn:aws:ec2:{FALLBACK_REGION}:{ACCOUNT_ID}:instance/i-0123456789abcdef0"),
        ),
        (
            "an ARN whose vendor differs only in case",
            format!("arn:aws:FIS:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/EXT000000000001"),
        ),
        (
            // An action is service-owned: no API creates one and it carries no caller tag.
            "a fis action ARN",
            format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:action/aws:fis:wait"),
        ),
        (
            // Nor does the account's safety lever: no API creates, lists or deletes one.
            "a fis safety-lever ARN",
            format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:safety-lever/{ACCOUNT_ID}"),
        ),
        (
            "a fis ARN of an unknown resource type",
            format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-report/REP0000000001"),
        ),
        (
            "a fis experiment-template ARN naming no id",
            format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/"),
        ),
        (
            "a fis experiment ARN naming no id",
            format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment/"),
        ),
        (
            "a fis experiment-template id of a shape this service never mints",
            format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/not-an-id"),
        ),
        (
            "a fis experiment-template ARN carrying an experiment's id prefix",
            format!(
                "arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/EXP{digits}"
            ),
        ),
        (
            "a fis experiment ARN carrying a template's id prefix",
            format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment/EXT{digits}"),
        ),
        (
            "an id whose suffix is one character short of the minted width",
            format!(
                "arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/{}",
                template_id_shaped(&"0".repeat(ID_SUFFIX_LEN - 1))
            ),
        ),
        (
            "an id whose suffix is one character past the minted width",
            format!(
                "arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/{}",
                template_id_shaped(&"0".repeat(ID_SUFFIX_LEN + 1))
            ),
        ),
        (
            "an id whose suffix is lower case, which the minted alphabet is not",
            format!(
                "arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/{}",
                template_id_shaped(&"z".repeat(ID_SUFFIX_LEN))
            ),
        ),
        (
            "an id whose suffix carries a character outside the minted alphabet",
            format!(
                "arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/{}",
                template_id_shaped(&format!("{}-", "0".repeat(ID_SUFFIX_LEN - 1)))
            ),
        ),
    ]
}

/// An ARN this service CAN address whose record the store does not hold: an id of the form
/// it mints that no call ever minted, for each kind of taggable resource.
pub(crate) fn unminted_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "an experiment-template id no call minted",
            format!(
                "arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment-template/{}",
                template_id_shaped(&"Z".repeat(ID_SUFFIX_LEN))
            ),
        ),
        (
            "an experiment id no call minted",
            format!(
                "arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:experiment/EXP{}",
                "Z".repeat(ID_SUFFIX_LEN)
            ),
        ),
    ]
}

/// An ARN this service CAN address, naming a resource the store holds no record of.
#[tokio::test]
async fn an_arn_naming_no_record_is_a_miss_and_writes_nothing() {
    let router = emulator();
    let case = "the suite's own template";
    let resource = new_resource(
        &router,
        Kind::ExperimentTemplate,
        case,
        &tag_map(&pairs(&[("Suite", "tagging")])),
    )
    .await;

    // A template this suite creates and then deletes: the ARN stays well formed, and the
    // record behind it is gone.
    let (deleted_id, deleted_arn) = new_template(&router, "deleted", &json!({})).await;
    let deleted = send(
        &router,
        "DELETE",
        &format!("/experimentTemplates/{deleted_id}"),
        None,
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeleteExperimentTemplate, got {} with {:?}",
        deleted.status, deleted.body
    );

    let mut misses = unminted_arns();
    misses.push(("a template that has since been deleted", deleted_arn));

    for (case, arn) in misses {
        let answer = tag_resource(&router, &arn, &tag_map(&pairs(&[("Env", "test")]))).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for {arn:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for {arn:?}, got {:?}",
            answer.body
        );
    }

    // Nothing any of those calls carried reached a record.
    let record = read_resource(&router, &resource, case).await;
    assert_eq!(
        tags_of(&record, case),
        pairs(&[("Suite", "tagging")]).into_iter().collect(),
        "case {case:?}: a refused tag call must write nothing, got {:?}",
        record["tags"]
    );
}

/// One refused request: the ARN it names (`None` for the suite's own live resource), the
/// tags it carries and the error code the refusal answers with.
struct Refusal {
    case: String,
    arn: Option<String>,
    tags: Vec<(String, String)>,
    code: &'static str,
}

/// A request the service refuses: an ARN it cannot address, a tag key outside `TagKey`'s
/// bounds, and a request that would leave the resource with more tags than a `TagMap`
/// holds. Every one answers HTTP 400 and leaves the stored tags alone — under the ARN
/// layer's `BadRequestException` for a `resourceArn` this service could not have issued,
/// and under the modelled `ValidationException` for a request whose TAGS it will not take.
#[tokio::test]
async fn a_refused_tag_call_answers_its_own_code_and_writes_nothing() {
    let stored = pairs(&[("Suite", "tagging"), ("Env", "test")]);
    let key_past_ceiling = "k".repeat(TAG_KEY_CEILING + 1);

    let mut cases: Vec<Refusal> = unaddressable_arns()
        .into_iter()
        .map(|(case, arn)| Refusal {
            case: case.to_string(),
            arn: Some(arn),
            tags: pairs(&[("Env", "prod")]),
            code: "BadRequestException",
        })
        .collect();
    cases.push(Refusal {
        case: "a tag key one character past TagKey's ceiling".to_string(),
        arn: None,
        tags: vec![(key_past_ceiling, "x".to_string())],
        code: "ValidationException",
    });
    cases.push(Refusal {
        case: "an empty tag key, which TagKey's pattern does not admit".to_string(),
        arn: None,
        tags: vec![(String::new(), "x".to_string())],
        code: "ValidationException",
    });
    cases.push(Refusal {
        case: "one tag past the most a resource carries".to_string(),
        arn: None,
        tags: generated(TAGS_PER_RESOURCE, "over"),
        code: "ValidationException",
    });
    cases.push(Refusal {
        case: "a request mixing an admissible key with an over-long one".to_string(),
        arn: None,
        tags: vec![
            ("Env".to_string(), "prod".to_string()),
            ("k".repeat(TAG_KEY_CEILING + 1), "x".to_string()),
        ],
        code: "ValidationException",
    });

    for Refusal {
        case,
        arn,
        tags,
        code,
    } in cases
    {
        // One emulator per case, so the "nothing was written" assertion is about this
        // case's call alone.
        let router = emulator();
        let resource = new_resource(
            &router,
            Kind::ExperimentTemplate,
            &case,
            &tag_map(&stored),
        )
        .await;
        let target = arn.unwrap_or_else(|| resource.arn.clone());

        let answer = tag_resource(&router, &target, &tag_map(&tags)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {target:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code} for {target:?}, got {:?}",
            answer.body
        );

        let record = read_resource(&router, &resource, &case).await;
        assert_eq!(
            tags_of(&record, &case),
            stored.iter().cloned().collect(),
            "case {case:?}: a refused tag call must leave the stored tags alone, got {:?}",
            record["tags"]
        );
    }
}
