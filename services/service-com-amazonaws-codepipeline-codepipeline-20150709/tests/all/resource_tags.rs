// SPDX-License-Identifier: CC0-1.0

//! Resource tagging for CodePipeline: `TagResource` and the `ListTagsForResource`
//! read that reports what it stored, end to end over the wire the service speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test is `TagResource`'s own documented one — "adds to or modifies
//! the tags of the given resource" — so every table row varies what the rule
//! generalises over (which keys a call names, whether they are new or already held,
//! how many, how long, what they contain) rather than only the resource's name, and
//! every refusal row varies the ARN the call addresses.
//!
//! The harness (`Emulator`, `declaration`) is the one `pipeline_lifecycle.rs` already
//! defines: one shared engine, the service mounted on it, the router in front.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX,
};

use crate::pipeline_lifecycle::{declaration, Emulator, ACCOUNT_ID};

/// `TagKey`'s modelled maximum (`@length(min: 1, max: 128)`).
const MAX_TAG_KEY_CHARS: usize = 128;

/// `TagValue`'s modelled maximum (`@length(min: 0, max: 256)`).
const MAX_TAG_VALUE_CHARS: usize = 256;

/// The ARN the service mints for a pipeline: `emulator_arn("codepipeline", region,
/// name)`, which carries the pipeline name bare, with no resource-type prefix.
fn pipeline_arn(region: &str, name: &str) -> String {
    format!("arn:aws:codepipeline:{region}:{ACCOUNT_ID}:{name}")
}

/// A tag set as a table row writes it: `(key, value)` pairs.
type Tags = Vec<(String, String)>;

/// A `(key, value)` pair list as the modelled `TagList`.
fn tag_list(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// A response's `tags` member as a key -> value map, so an assertion compares tag
/// SETS rather than wire order.
fn tag_map(tags: &Value) -> BTreeMap<String, String> {
    tags.as_array()
        .unwrap_or_else(|| panic!("tags must be a list, got {tags}"))
        .iter()
        .map(|tag| {
            (
                tag["key"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a tag carries a key, got {tag}"))
                    .to_string(),
                tag["value"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a tag carries a value, got {tag}"))
                    .to_string(),
            )
        })
        .collect()
}

fn expected_map(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// `count` distinct tags, keys zero-padded so their byte order is their numeric one.
fn generated_tags(count: usize) -> Tags {
    (0..count)
        .map(|index| (format!("key-{index:03}"), format!("value-{index}")))
        .collect()
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Create a pipeline named `name` and answer the ARN the service minted for it.
async fn create_pipeline(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration(name)}))
        .await;
    assert_eq!(
        created["pipeline"]["name"], name,
        "the pipeline the tagging cases tag must exist, got {created}"
    );
    pipeline_arn("us-east-1", name)
}

/// The tags the store holds for `arn`, read back through `ListTagsForResource`.
async fn stored_tags(emulator: &Emulator, arn: &str) -> BTreeMap<String, String> {
    let listed = emulator
        .ok("ListTagsForResource", &json!({"resourceArn": arn}))
        .await;
    assert!(
        listed.get("nextToken").is_none_or(Value::is_null),
        "an unpaged ListTagsForResource lists every tag in one call, got {listed}"
    );
    tag_map(&listed["tags"])
}

/// Apply `tags` to `arn`, expecting the call to be accepted.
async fn tag(emulator: &Emulator, arn: &str, tags: &[(String, String)], case: &str) {
    let applied = emulator
        .ok(
            "TagResource",
            &json!({"resourceArn": arn, "tags": tag_list(tags)}),
        )
        .await;
    assert_eq!(
        applied,
        json!({}),
        "case {case:?}: TagResource answers an empty 200 body, got {applied}"
    );
}

/// One row of the merge table: the calls to make, and the tag set they must leave.
struct MergeCase {
    case: &'static str,
    calls: Vec<Tags>,
    expected: Tags,
}

/// `TagResource` merges the tags it is given into the resource's tag set, keyed by
/// tag key: a key the resource already carries takes the new value, every other key
/// it holds is left alone, and the result is what a later read reports.
#[tokio::test]
async fn tag_resource_merges_the_request_into_the_stored_tag_set() {
    let long_key = "k".repeat(MAX_TAG_KEY_CHARS);
    let long_value = "v".repeat(MAX_TAG_VALUE_CHARS);
    let cases = vec![
        MergeCase {
            case: "one tag onto an untagged pipeline",
            calls: vec![vec![pair("team", "build")]],
            expected: vec![pair("team", "build")],
        },
        MergeCase {
            case: "a second call naming a disjoint key keeps the first",
            calls: vec![vec![pair("team", "build")], vec![pair("env", "prod")]],
            expected: vec![pair("env", "prod"), pair("team", "build")],
        },
        MergeCase {
            case: "a second call naming a held key modifies it",
            calls: vec![vec![pair("env", "dev")], vec![pair("env", "prod")]],
            expected: vec![pair("env", "prod")],
        },
        MergeCase {
            case: "one call carrying both a new key and a held one",
            calls: vec![
                vec![pair("a", "1"), pair("b", "2")],
                vec![pair("b", "3"), pair("c", "4")],
            ],
            expected: vec![pair("a", "1"), pair("b", "3"), pair("c", "4")],
        },
        MergeCase {
            case: "repeating the same call changes nothing",
            calls: vec![vec![pair("team", "build")], vec![pair("team", "build")]],
            expected: vec![pair("team", "build")],
        },
        MergeCase {
            // `TagValue` is `@length(min: 0, …)`, so the empty value is a legal one
            // and is a value, not an absence.
            case: "an empty tag value",
            calls: vec![vec![pair("empty", "")]],
            expected: vec![pair("empty", "")],
        },
        MergeCase {
            case: "a key modified from a value to the empty one",
            calls: vec![vec![pair("env", "prod")], vec![pair("env", "")]],
            expected: vec![pair("env", "")],
        },
        MergeCase {
            // The modelled maxima: `TagKey` 128 characters, `TagValue` 256.
            case: "a key and a value at their modelled maximum length",
            calls: vec![vec![(long_key.clone(), long_value.clone())]],
            expected: vec![(long_key.clone(), long_value.clone())],
        },
        MergeCase {
            // `TagKey` carries no `@pattern`, so punctuation and non-ASCII keys are
            // legal; only the reserved `aws:` PREFIX is refused (in any letter case,
            // the rule `state::validate_tags` applies).
            case: "keys carrying punctuation, a colon and non-ASCII characters",
            calls: vec![vec![
                pair("cost:center", "r&d"),
                pair("not-aws:owned", "mine"),
                pair("tête-à-tête", "oui"),
            ]],
            expected: vec![
                pair("cost:center", "r&d"),
                pair("not-aws:owned", "mine"),
                pair("tête-à-tête", "oui"),
            ],
        },
        MergeCase {
            case: "a batch at the per-resource bound",
            calls: vec![generated_tags(MAX_TAGS_PER_RESOURCE)],
            expected: generated_tags(MAX_TAGS_PER_RESOURCE),
        },
        MergeCase {
            case: "the bound reached one tag at a time",
            calls: generated_tags(MAX_TAGS_PER_RESOURCE)
                .into_iter()
                .map(|tag| vec![tag])
                .collect(),
            expected: generated_tags(MAX_TAGS_PER_RESOURCE),
        },
    ];

    let emulator = Emulator::new();
    // A sibling resource, tagged once: tags belong to the resource their ARN names,
    // so nothing any row does may reach it.
    let sibling = create_pipeline(&emulator, "sibling").await;
    tag(&emulator, &sibling, &[pair("owner", "sibling")], "sibling").await;

    for (index, MergeCase { case, calls, expected }) in cases.into_iter().enumerate() {
        let arn = create_pipeline(&emulator, &format!("tagged-{index}")).await;
        assert_eq!(
            stored_tags(&emulator, &arn).await,
            BTreeMap::new(),
            "case {case:?}: a pipeline created with no tags holds none"
        );

        for call in &calls {
            tag(&emulator, &arn, call, case).await;
        }

        let actual = stored_tags(&emulator, &arn).await;
        assert_eq!(
            actual,
            expected_map(&expected),
            "case {case:?}: expected the merged tag set {:?}, got {actual:?}",
            expected_map(&expected)
        );
        let keys: Vec<&String> = actual.keys().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            keys, sorted,
            "case {case:?}: ListTagsForResource reports tags in key order, got {keys:?}"
        );
    }

    let sibling_tags = stored_tags(&emulator, &sibling).await;
    assert_eq!(
        sibling_tags,
        expected_map(&[pair("owner", "sibling")]),
        "tagging one resource must not reach another, got {sibling_tags:?}"
    );
}

/// `CreatePipeline`'s tags land in the same store, so the create's own answer and a
/// later `ListTagsForResource` report one set: deduplicated by key, in key order.
#[tokio::test]
async fn tags_applied_at_create_are_the_tags_that_are_listed() {
    // (case, the TagList to create with, the tag set it must leave)
    let cases: Vec<(&str, Tags, Tags)> = vec![
        ("no tags at all", vec![], vec![]),
        (
            "one tag",
            vec![pair("team", "build")],
            vec![pair("team", "build")],
        ),
        (
            "several tags in no particular order",
            vec![pair("z", "last"), pair("a", "first"), pair("m", "middle")],
            vec![pair("a", "first"), pair("m", "middle"), pair("z", "last")],
        ),
        (
            "tags at the per-resource bound",
            generated_tags(MAX_TAGS_PER_RESOURCE),
            generated_tags(MAX_TAGS_PER_RESOURCE),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, created_with, expected)) in cases.into_iter().enumerate() {
        let name = format!("created-{index}");
        let created = emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": declaration(&name), "tags": tag_list(&created_with)}),
            )
            .await;
        let reported = if expected.is_empty() {
            // An absent member and an empty list state the same fact, and AWS omits
            // the member.
            assert!(
                created.get("tags").is_none_or(Value::is_null),
                "case {case:?}: a create with no tags reports none, got {created}"
            );
            BTreeMap::new()
        } else {
            tag_map(&created["tags"])
        };
        assert_eq!(
            reported,
            expected_map(&expected),
            "case {case:?}: CreatePipeline reports the tags it stored, got {reported:?}"
        );

        let listed = stored_tags(&emulator, &pipeline_arn("us-east-1", &name)).await;
        assert_eq!(
            listed,
            expected_map(&expected),
            "case {case:?}: ListTagsForResource reports the same set the create did, got {listed:?}"
        );
    }
}

/// `CreateCustomActionType`'s tags land in the same store and are reached by the same
/// ARN rule, so the action-type form of a resource ARN resolves exactly like a
/// pipeline's: its tags are listed, merged into and reported per Region.
#[tokio::test]
async fn a_custom_action_types_tags_are_reached_by_its_arn() {
    let emulator = Emulator::new();
    // A custom action type registered in the default Region, with tags.
    let created = emulator
        .ok(
            "CreateCustomActionType",
            &json!({
                "category": "Build",
                "provider": "My-Tool",
                "version": "1",
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
                "tags": [{"key": "team", "value": "build"}],
            }),
        )
        .await;
    assert_eq!(
        created["tags"],
        json!([{"key": "team", "value": "build"}]),
        "the create echoes the tags it stored, got {created}"
    );

    // The ARN form AWS documents for an action type: the four `ActionTypeId` members,
    // OWNER first.
    let arn =
        format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:actiontype:Custom/Build/My-Tool/1");

    let listed = stored_tags(&emulator, &arn).await;
    assert_eq!(
        listed,
        expected_map(&[pair("team", "build")]),
        "ListTagsForResource reports the tags the create stored, got {listed:?}"
    );

    // And the merge rule holds for this resource kind too.
    tag(
        &emulator,
        &arn,
        &[pair("env", "prod"), pair("team", "platform")],
        "an action type's tags are merged",
    )
    .await;
    let merged = stored_tags(&emulator, &arn).await;
    assert_eq!(
        merged,
        expected_map(&[pair("env", "prod"), pair("team", "platform")]),
        "tagging an action type modifies the key it already carried and adds the new one, \
         got {merged:?}"
    );

    // A registration belongs to the scope its ARN names, exactly as a pipeline does,
    // so the same identity in another Region, account or partition names no resource;
    // and an identity nothing registered, or one the SERVICE provides rather than the
    // account, is not a resource either.
    for (case, arn, id, account) in [
        (
            "the same action type in another Region",
            format!(
                "arn:aws:codepipeline:eu-west-1:{ACCOUNT_ID}:actiontype:Custom/Build/My-Tool/1"
            ),
            "Custom/Build/My-Tool/1",
            ACCOUNT_ID,
        ),
        (
            // The refusal names the account the ARN asked for, not the one this
            // emulator serves.
            "the same action type in another account",
            "arn:aws:codepipeline:us-east-1:123456789012:actiontype:Custom/Build/My-Tool/1"
                .to_string(),
            "Custom/Build/My-Tool/1",
            "123456789012",
        ),
        (
            "the same action type in another partition",
            format!(
                "arn:aws-cn:codepipeline:cn-north-1:{ACCOUNT_ID}:actiontype:Custom/Build/My-Tool/1"
            ),
            "Custom/Build/My-Tool/1",
            ACCOUNT_ID,
        ),
        (
            "an identity nothing registered",
            format!(
                "arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:actiontype:Custom/Build/Other-Tool/1"
            ),
            "Custom/Build/Other-Tool/1",
            ACCOUNT_ID,
        ),
        (
            // `ListActionTypes` reports the action types the service itself provides,
            // but they belong to no account, so they are not taggable resources.
            "an action type the service provides",
            format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:actiontype:AWS/Source/S3/1"),
            "AWS/Source/S3/1",
            ACCOUNT_ID,
        ),
    ] {
        for operation in ["TagResource", "ListTagsForResource"] {
            let body = json!({"resourceArn": arn, "tags": [{"key": "probe", "value": "1"}]});
            let (status, code, message) = expect_refusal(&emulator, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: {operation} on {arn} must be refused as 400 \
                 ResourceNotFoundException, got {status} {code} ({message})"
            );
            assert_eq!(
                message,
                format!("ActionType with id {id} does not exist in account {account}."),
                "case {case:?}: {operation} must report which action type in which account was \
                 asked for, got {message:?}"
            );
        }
    }

    // None of those refusals touched the registered action type's tags.
    let unchanged = stored_tags(&emulator, &arn).await;
    assert_eq!(
        unchanged,
        expected_map(&[pair("env", "prod"), pair("team", "platform")]),
        "a refused call must write nothing, got {unchanged:?}"
    );
}

/// Every ARN this service mints resolves again through the tagging operations,
/// whatever Region the request was signed for: the ARN's Region segment comes from
/// the SigV4 credential scope, and the rule that reads an ARN back is the rule that
/// minted it.
#[tokio::test]
async fn an_arn_this_service_minted_always_resolves() {
    let emulator = Emulator::new();
    // (case, the Region the request is signed for, the Region the ARN must carry)
    let cases = [
        ("the default Region", "us-east-1", "us-east-1"),
        ("another Region", "eu-west-1", "eu-west-1"),
        ("a Region with several hyphens", "ap-southeast-3", "ap-southeast-3"),
        // Not a Region token, so the resource is minted in the default Region rather
        // than in an ARN no operation could resolve again.
        ("a scope Region that is not a Region token", "US-EAST-1", "us-east-1"),
    ];

    for (index, (case, signed_for, expected_region)) in cases.iter().enumerate() {
        let name = format!("minted-{index}");
        let (status, created) = emulator
            .call_in(
                signed_for,
                "CreatePipeline",
                &json!({"pipeline": declaration(&name), "tags": [{"key": "team", "value": "build"}]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePipeline must accept the request, got {status} with {created}"
        );

        let fetched = emulator
            .call_in(signed_for, "GetPipeline", &json!({"name": name}))
            .await
            .1;
        let arn = fetched["metadata"]["pipelineArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the metadata carries an ARN, got {fetched}"))
            .to_string();
        assert_eq!(
            arn,
            pipeline_arn(expected_region, &name),
            "case {case:?}: expected the ARN minted in {expected_region}, got {arn:?}"
        );

        // The minted ARN resolves through both tagging operations.
        let listed = stored_tags(&emulator, &arn).await;
        assert_eq!(
            listed,
            expected_map(&[pair("team", "build")]),
            "case {case:?}: the minted ARN must resolve for ListTagsForResource, got {listed:?}"
        );
        tag(&emulator, &arn, &[pair("env", "prod")], case).await;
        let merged = stored_tags(&emulator, &arn).await;
        assert_eq!(
            merged,
            expected_map(&[pair("env", "prod"), pair("team", "build")]),
            "case {case:?}: the minted ARN must resolve for TagResource, got {merged:?}"
        );
    }
}

/// A webhook's tags are reached by its ARN too: `PutWebhook` stores them, the tag
/// operations resolve the `webhook:<name>` ARN form against the webhook store, and a
/// webhook in another scope names no resource.
#[tokio::test]
async fn a_webhooks_tags_are_reached_by_its_arn() {
    let emulator = Emulator::new();
    create_pipeline(&emulator, "webhook-target").await;
    let put = emulator
        .ok(
            "PutWebhook",
            &json!({
                "webhook": {
                    "name": "hook",
                    "targetPipeline": "webhook-target",
                    "targetAction": "Src",
                    "filters": [{"jsonPath": "$.ref", "matchEquals": "refs/heads/main"}],
                    "authentication": "UNAUTHENTICATED",
                    "authenticationConfiguration": {},
                },
                "tags": [{"key": "team", "value": "build"}],
            }),
        )
        .await;
    let arn = put["webhook"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutWebhook reports the webhook's ARN, got {put}"))
        .to_string();
    assert_eq!(
        arn,
        format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:webhook:hook"),
        "the webhook ARN carries the webhook resource form, got {arn:?}"
    );

    let listed = stored_tags(&emulator, &arn).await;
    assert_eq!(
        listed,
        expected_map(&[pair("team", "build")]),
        "ListTagsForResource reports the tags PutWebhook stored, got {listed:?}"
    );

    tag(
        &emulator,
        &arn,
        &[pair("env", "prod"), pair("team", "platform")],
        "a webhook's tags are merged",
    )
    .await;
    let merged = stored_tags(&emulator, &arn).await;
    assert_eq!(
        merged,
        expected_map(&[pair("env", "prod"), pair("team", "platform")]),
        "tagging a webhook modifies the key it carried and adds the new one, got {merged:?}"
    );

    // Another Region, another account and a name nothing defined name no webhook.
    for (case, arn, name, account) in [
        (
            "the same webhook in another Region",
            format!("arn:aws:codepipeline:eu-west-1:{ACCOUNT_ID}:webhook:hook"),
            "hook",
            ACCOUNT_ID,
        ),
        (
            "the same webhook in another account",
            "arn:aws:codepipeline:us-east-1:123456789012:webhook:hook".to_string(),
            "hook",
            "123456789012",
        ),
        (
            "a webhook name nothing defined",
            format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:webhook:other-hook"),
            "other-hook",
            ACCOUNT_ID,
        ),
    ] {
        for operation in ["TagResource", "ListTagsForResource"] {
            let body = json!({"resourceArn": arn, "tags": [{"key": "probe", "value": "1"}]});
            let (status, code, message) = expect_refusal(&emulator, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: {operation} on {arn} must be refused as 400 \
                 ResourceNotFoundException, got {status} {code} ({message})"
            );
            assert_eq!(
                message,
                format!("Webhook with name {name} does not exist in account {account}."),
                "case {case:?}: {operation} must report which webhook in which account was \
                 asked for, got {message:?}"
            );
        }
    }

    let unchanged = stored_tags(&emulator, &arn).await;
    assert_eq!(
        unchanged,
        expected_map(&[pair("env", "prod"), pair("team", "platform")]),
        "a refused call must write nothing, got {unchanged:?}"
    );
}

/// One row of the ARN table: the ARN a tagging call addresses, the error code it
/// must be refused with (`None` for an ARN that resolves), and — where the message is
/// a rule rather than an example, because the service builds it from the request —
/// the message the refusal must carry.
struct ArnCase {
    case: &'static str,
    arn: String,
    expected: Option<&'static str>,
    message: Option<String>,
}

/// The code and the message a refusal carries. `awsJson1_1` renders the error code in
/// `__type`; the modelled `Message` member serialises under its own member name.
async fn expect_refusal(
    emulator: &Emulator,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, response) = emulator.call(operation, body).await;
    let code = response["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
        .to_string();
    let message = response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{operation}: a refusal carries a message, got {response}"))
        .to_string();
    (status, code, message)
}

/// A tagging call resolves the resource its ARN names before it does anything else:
/// a string that is not a `codepipeline` resource ARN is `InvalidArnException`, an
/// ARN naming no resource this service holds is `ResourceNotFoundException` — naming
/// the resource and the account the ARN asked for, which is a message the service
/// builds from the request — and both operations that address a resource by ARN
/// answer the same way.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_refused() {
    let emulator = Emulator::new();
    let held = create_pipeline(&emulator, "held").await;
    tag(&emulator, &held, &[pair("team", "build")], "held").await;
    // A pipeline that existed and was deleted: its ARN names nothing afterwards.
    let deleted = create_pipeline(&emulator, "deleted").await;
    emulator
        .ok("DeletePipeline", &json!({"name": "deleted"}))
        .await;

    let cases = vec![
        ArnCase {
            case: "the ARN of a stored pipeline",
            arn: held.clone(),
            expected: None,
            message: None,
        },
        ArnCase {
            case: "a pipeline name no pipeline uses",
            arn: pipeline_arn("us-east-1", "absent"),
            expected: Some("ResourceNotFoundException"),
            message: Some(format!(
                "Pipeline with name absent does not exist in account {ACCOUNT_ID}."
            )),
        },
        ArnCase {
            case: "the ARN of a deleted pipeline",
            arn: deleted,
            expected: Some("ResourceNotFoundException"),
            message: Some(format!(
                "Pipeline with name deleted does not exist in account {ACCOUNT_ID}."
            )),
        },
        ArnCase {
            case: "a held pipeline's name in another Region",
            arn: pipeline_arn("eu-west-1", "held"),
            expected: Some("ResourceNotFoundException"),
            message: Some(format!(
                "Pipeline with name held does not exist in account {ACCOUNT_ID}."
            )),
        },
        ArnCase {
            // The account the refusal names comes from the ARN the caller sent, not
            // from the account this emulator serves.
            case: "a held pipeline's name in another account",
            arn: "arn:aws:codepipeline:us-east-1:123456789012:held".to_string(),
            expected: Some("ResourceNotFoundException"),
            message: Some(
                "Pipeline with name held does not exist in account 123456789012.".to_string(),
            ),
        },
        ArnCase {
            case: "a held pipeline's name in another partition",
            arn: format!("arn:aws-cn:codepipeline:cn-north-1:{ACCOUNT_ID}:held"),
            expected: Some("ResourceNotFoundException"),
            message: Some(format!(
                "Pipeline with name held does not exist in account {ACCOUNT_ID}."
            )),
        },
        ArnCase {
            // `actiontype:<owner>/<category>/<provider>/<version>` is a well-formed
            // CodePipeline resource ARN; nothing registered this identity in THIS
            // test, so it names nothing. The resolving case — an action type this
            // account did register — is
            // `a_custom_action_types_tags_are_reached_by_its_arn` above.
            case: "a well-formed custom action type ARN",
            arn: format!(
                "arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:actiontype:Custom/Build/My-Tool/1"
            ),
            expected: Some("ResourceNotFoundException"),
            message: Some(format!(
                "ActionType with id Custom/Build/My-Tool/1 does not exist in account {ACCOUNT_ID}."
            )),
        },
        ArnCase {
            case: "a well-formed webhook ARN",
            arn: format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:webhook:my-hook"),
            expected: Some("ResourceNotFoundException"),
            message: Some(format!(
                "Webhook with name my-hook does not exist in account {ACCOUNT_ID}."
            )),
        },
        ArnCase {
            // The message an invalid ARN is refused with is not a rule AWS states, so
            // only the code and status are asserted for these rows.
            case: "a region segment that is not a Region token",
            arn: format!("arn:aws:codepipeline:US-EAST-1:{ACCOUNT_ID}:held"),
            expected: Some("InvalidArnException"),
            message: None,
        },
        ArnCase {
            case: "an extra segment where the account id belongs",
            arn: format!("arn:aws:codepipeline:us-east-1:00000000000:{ACCOUNT_ID}:held"),
            expected: Some("InvalidArnException"),
            message: None,
        },
        ArnCase {
            case: "an action type ARN missing its version part",
            arn: format!(
                "arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:actiontype:Custom/Build/My-Tool"
            ),
            expected: Some("InvalidArnException"),
            message: None,
        },
        ArnCase {
            case: "an action type ARN with an empty part",
            arn: format!(
                "arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:actiontype:Custom//My-Tool/1"
            ),
            expected: Some("InvalidArnException"),
            message: None,
        },
        ArnCase {
            case: "a webhook ARN with no name",
            arn: format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:webhook:"),
            expected: Some("InvalidArnException"),
            message: None,
        },
    ];

    for ArnCase {
        case,
        arn,
        expected,
        message,
    } in cases
    {
        let tagging = json!({"resourceArn": arn, "tags": [{"key": "probe", "value": "1"}]});
        let listing = json!({"resourceArn": arn});
        for (operation, body) in [("TagResource", &tagging), ("ListTagsForResource", &listing)] {
            match expected {
                None => {
                    let (status, response) = emulator.call(operation, body).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: {operation} must accept a stored resource's ARN, \
                         got {status} with {response}"
                    );
                }
                Some(code) => {
                    let (status, actual, actual_message) =
                        expect_refusal(&emulator, operation, body).await;
                    assert_eq!(
                        (status, actual.as_str()),
                        (400, code),
                        "case {case:?}: {operation} on {arn} must be refused as {code}, \
                         got {status} {actual} ({actual_message})"
                    );
                    if let Some(expected_message) = &message {
                        assert_eq!(
                            &actual_message, expected_message,
                            "case {case:?}: {operation} on {arn} must report which resource \
                             in which account was asked for, got {actual_message:?}"
                        );
                    }
                }
            }
        }
    }

    // A call that is wrong in two ways — an ARN that cannot be resolved AND a tag key
    // in the reserved namespace — is refused for the RESOURCE, which is what makes
    // the two ARN-addressed operations answer one request the same way (the listing
    // carries no tags to judge).
    for (case, arn, code) in [
        (
            "an absent pipeline named with a reserved tag key",
            pipeline_arn("us-east-1", "absent"),
            "ResourceNotFoundException",
        ),
        (
            "a malformed ARN named with a reserved tag key",
            format!("arn:aws:codepipeline:US-EAST-1:{ACCOUNT_ID}:held"),
            "InvalidArnException",
        ),
    ] {
        let (status, actual, message) = expect_refusal(
            &emulator,
            "TagResource",
            &json!({"resourceArn": arn, "tags": [{"key": "aws:internal", "value": "1"}]}),
        )
        .await;
        assert_eq!(
            (status, actual.as_str()),
            (400, code),
            "case {case:?}: the resource is judged before the tags, so expected 400 {code}, \
             got {status} {actual} ({message})"
        );
    }

    // Only the accepted row above wrote: every other row addressed no resource (or
    // addressed the held one with a refused ARN), so the held pipeline carries what
    // it was tagged with plus the probe tag the accepted row applied.
    let held_tags = stored_tags(&emulator, &held).await;
    assert_eq!(
        held_tags,
        expected_map(&[pair("probe", "1"), pair("team", "build")]),
        "a refused tagging call must write nothing, got {held_tags:?}"
    );
}

/// Tag content and the per-resource bound: a key in AWS's reserved `aws:` namespace
/// is `InvalidTagsException`, a call that would leave a resource holding more than
/// `MAX_TAGS_PER_RESOURCE` tags is `TooManyTagsException`, and a refused call writes
/// nothing — judged on the MERGED set, so rewriting a held key at the bound is fine.
#[tokio::test]
async fn reserved_keys_and_the_tag_bound_are_refused() {
    let emulator = Emulator::new();
    let arn = create_pipeline(&emulator, "bounded").await;

    // (case, the TagList, the error code it is refused with)
    let reserved_cases: Vec<(&str, Tags, &str)> = vec![
        (
            "a key in the reserved namespace",
            vec![pair(&format!("{RESERVED_TAG_KEY_PREFIX}cost-center"), "1")],
            "InvalidTagsException",
        ),
        (
            "a key whose whole name is in the reserved namespace",
            vec![pair("aws:cloudformation:stack-name", "demo")],
            "InvalidTagsException",
        ),
        (
            // The reserved namespace is matched in any letter case.
            "the reserved prefix in upper case",
            vec![pair("AWS:cost-center", "1")],
            "InvalidTagsException",
        ),
        (
            "a reserved key beside a legal one refuses the whole call",
            vec![pair("team", "build"), pair("aws:internal", "1")],
            "InvalidTagsException",
        ),
        (
            "the reserved prefix alone",
            vec![pair(RESERVED_TAG_KEY_PREFIX, "1")],
            "InvalidTagsException",
        ),
        (
            "a key named twice in one call",
            vec![pair("dup", "first"), pair("dup", "last")],
            "InvalidTagsException",
        ),
        (
            "more tags in one call than a resource may hold",
            generated_tags(MAX_TAGS_PER_RESOURCE + 1),
            "TooManyTagsException",
        ),
    ];

    for (case, tags, code) in reserved_cases {
        let (status, actual) = emulator
            .expect_error(
                "TagResource",
                &json!({"resourceArn": arn, "tags": tag_list(&tags)}),
            )
            .await;
        assert_eq!(
            (status, actual.as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {status} {actual}"
        );
        let stored = stored_tags(&emulator, &arn).await;
        assert_eq!(
            stored,
            BTreeMap::new(),
            "case {case:?}: a refused call must write nothing, got {stored:?}"
        );
    }

    // The bound itself: exactly MAX_TAGS_PER_RESOURCE tags are accepted.
    let full = generated_tags(MAX_TAGS_PER_RESOURCE);
    tag(&emulator, &arn, &full, "at the bound").await;
    assert_eq!(
        stored_tags(&emulator, &arn).await.len(),
        MAX_TAGS_PER_RESOURCE,
        "a resource holds {MAX_TAGS_PER_RESOURCE} tags"
    );

    // One key past it is refused, and the resource keeps the set it had.
    let (status, actual) = emulator
        .expect_error(
            "TagResource",
            &json!({"resourceArn": arn, "tags": [{"key": "one-too-many", "value": "1"}]}),
        )
        .await;
    assert_eq!(
        (status, actual.as_str()),
        (400, "TooManyTagsException"),
        "case {:?}: expected 400 TooManyTagsException, got {status} {actual}",
        "one key past the bound"
    );
    let at_bound = stored_tags(&emulator, &arn).await;
    assert_eq!(
        at_bound,
        expected_map(&full),
        "a refused call at the bound must write nothing, got {at_bound:?}"
    );

    // Rewriting a key the resource already holds does not grow the set, so it is
    // accepted even at the bound: the bound is judged on the merged set.
    let rewritten = vec![pair("key-000", "rewritten")];
    tag(&emulator, &arn, &rewritten, "rewriting a held key at the bound").await;
    let after = stored_tags(&emulator, &arn).await;
    assert_eq!(
        after.len(),
        MAX_TAGS_PER_RESOURCE,
        "rewriting a held key keeps the tag count at the bound, got {after:?}"
    );
    assert_eq!(
        after.get("key-000").map(String::as_str),
        Some("rewritten"),
        "rewriting a held key stores the new value, got {after:?}"
    );
}

/// The same two rules at the create: `CreatePipeline`'s tags are refused the same way
/// a later `TagResource` call's are, and a refused create stores no pipeline.
#[tokio::test]
async fn create_pipeline_refuses_the_same_tags_tag_resource_does() {
    // (case, pipeline name, the TagList, the error code)
    let cases: Vec<(&str, &str, Tags, &str)> = vec![
        (
            "a key in the reserved namespace",
            "create-reserved",
            vec![pair("aws:internal", "1")],
            "InvalidTagsException",
        ),
        (
            "more tags than a resource may hold",
            "create-too-many",
            generated_tags(MAX_TAGS_PER_RESOURCE + 1),
            "TooManyTagsException",
        ),
        (
            "a key named twice",
            "create-duplicate-key",
            vec![pair("env", "dev"), pair("env", "prod")],
            "InvalidTagsException",
        ),
    ];

    let emulator = Emulator::new();
    for (case, name, tags, code) in cases {
        let (status, actual) = emulator
            .expect_error(
                "CreatePipeline",
                &json!({"pipeline": declaration(name), "tags": tag_list(&tags)}),
            )
            .await;
        assert_eq!(
            (status, actual.as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {status} {actual}"
        );
        let (status, actual) = emulator
            .expect_error("GetPipeline", &json!({"name": name}))
            .await;
        assert_eq!(
            (status, actual.as_str()),
            (400, "PipelineNotFoundException"),
            "case {case:?}: a refused create stores no pipeline, got {status} {actual}"
        );
    }
}

/// `ListTagsForResource` "lists all available tags in one call and does not use
/// pagination" — so a call naming no `maxResults` returns the whole set — and a
/// caller that asks for a smaller page is answered with that page plus the opaque
/// cursor the next one resumes at. A token this listing did not issue is refused.
#[tokio::test]
async fn listing_tags_pages_in_key_order_on_its_own_cursors() {
    let emulator = Emulator::new();
    let arn = create_pipeline(&emulator, "paged").await;
    let all = generated_tags(7);
    tag(&emulator, &arn, &all, "seven tags").await;

    // No maxResults: every tag, no cursor.
    let unpaged = stored_tags(&emulator, &arn).await;
    assert_eq!(
        unpaged,
        expected_map(&all),
        "an unpaged call lists every tag, got {unpaged:?}"
    );

    // Every page size from 1 to one past the tag count: the pages concatenate to the
    // whole set, in key order, and only the last page carries no cursor.
    for page_size in 1..=all.len() + 1 {
        let case = format!("maxResults={page_size}");
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"resourceArn": arn, "maxResults": page_size});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let page = emulator.ok("ListTagsForResource", &body).await;
            let page_pairs: Vec<(String, String)> = page["tags"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: tags must be a list, got {page}"))
                .iter()
                .map(|tag| {
                    (
                        tag["key"].as_str().unwrap_or_default().to_string(),
                        tag["value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            pages += 1;
            assert!(
                !page_pairs.is_empty() && page_pairs.len() <= page_size,
                "case {case:?}: page {pages} must carry 1..={page_size} tags, got {}",
                page_pairs.len()
            );
            collected.extend(page_pairs);
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= all.len(),
                "case {case:?}: paging must terminate within {} pages",
                all.len()
            );
        }
        let expected_pages = all.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
        assert_eq!(
            collected,
            all.clone(),
            "case {case:?}: the pages must concatenate to the whole set in key order, \
             got {collected:?}"
        );
    }

    // A cursor this listing never issued is refused, whatever it is: a string that is
    // no cursor at all, and a cursor another listing of this service issued.
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("other")}))
        .await;
    let pipelines = emulator
        .ok("ListPipelines", &json!({"maxResults": 1}))
        .await;
    let foreign = pipelines["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListPipelines must issue a cursor, got {pipelines}"))
        .to_string();
    for (case, token) in [
        ("a string that is not a cursor", "not-a-cursor".to_string()),
        ("base64 of a payload with no cursor kind", "dGFn".to_string()),
        ("a cursor ListPipelines issued", foreign),
    ] {
        let (status, actual) = emulator
            .expect_error(
                "ListTagsForResource",
                &json!({"resourceArn": arn, "nextToken": token}),
            )
            .await;
        assert_eq!(
            (status, actual.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected 400 InvalidNextTokenException, got {status} {actual}"
        );
    }
}
