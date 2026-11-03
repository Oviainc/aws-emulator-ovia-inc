// SPDX-License-Identifier: CC0-1.0

//! The resource tagging contract, over the wire: the tags a resource carries live ON that
//! resource, so `TagResource` merges the request's key-value pairs into the stored set,
//! `UntagResource` removes the keys it names, and `ListTagsForResource` serves exactly what
//! those writes and the create that made the resource left behind.
//!
//! The rule is asserted over EVERY resource kind a GameLift ARN names and this service
//! tags, not over the one kind a recording happened to carry, and over tag sets that differ
//! in size, in content and in whether a key is new, re-assigned or absent.
//!
//! The harness (`GameLift`, `gamelift_arn`, `ec2_fleet`, `error_message`) is the one
//! `hosting_loop` already drives the production dispatch path with.

use serde_json::{json, Value};

use super::hosting_loop::{ec2_fleet, error_message, gamelift_arn, GameLift, REGION};
use super::scripts::{zip_bytes, zip_file_member};

/// The modelled bounds of `Tag$Key` and `Tag$Value` (`smithy_query shape=Tag`: `TagKey`
/// len 1..128, `TagValue` len 0..256), so a row can carry a key and a value at the longest
/// the model allows.
const MAX_TAG_KEY_LENGTH: usize = 128;
/// See [`MAX_TAG_KEY_LENGTH`].
const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The most tags a resource holds, which is the modelled `TagList` length
/// (`smithy_query shape=TagList`, len 0..200) the service keeps as the per-resource bound.
const MAX_TAGS_PER_RESOURCE: usize =
    service_com_amazonaws_gamelift_gamelift::state::MAX_TAGS_PER_RESOURCE;

/// The rule set every matchmaking configuration below matches with: a body with the one
/// team `CreateMatchmakingRuleSet` needs to accept it.
fn rule_set_body() -> String {
    json!({
        "name": "solo-duel",
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "minPlayers": 1, "maxPlayers": 2}],
    })
    .to_string()
}

/// The tags a response reports, as key-value pairs sorted by key: tags are a key-value MAP,
/// so what a read must agree on is the SET of pairs rather than any order over them.
fn tag_pairs(tags: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// The tags `ListTagsForResource` reports for `resource_arn`, as sorted pairs.
async fn listed_tags(gamelift: &GameLift, resource_arn: &str) -> Vec<(String, String)> {
    let listed = gamelift
        .ok(
            "ListTagsForResource",
            json!({"ResourceARN": resource_arn}),
        )
        .await;
    assert!(
        listed["Tags"].is_array(),
        "ListTagsForResource must report the Tags collection, got {listed}"
    );
    tag_pairs(&listed["Tags"])
}

/// One taggable resource kind: what creates a resource of it, and where that create reports
/// the resource's ARN. The kinds are the resource segments GameLift ARNs carry for the
/// resources whose creates accept a `TagList` (`smithy_query operation=Create…`).
struct Kind {
    /// The resource kind, as its ARN names it.
    label: &'static str,
    /// The create that makes one, and the two members its response nests the ARN under.
    operation: &'static str,
    record: &'static str,
    arn_member: &'static str,
    /// The create request, without its `Tags`.
    request: Value,
}

/// A tag scenario run against one resource: what the create assigned, what two tag writes
/// then assign, which keys are removed, and what the resource must hold after each step.
struct Scenario {
    created_with: Value,
    first_write: Value,
    after_first: Value,
    second_write: Value,
    after_second: Value,
    removed_keys: Value,
    after_removal: Value,
}

/// Every taggable resource kind, each with its own tag scenario, so the rows differ both in
/// the kind of resource tagged and in the tag sets written to it.
fn rows() -> Vec<(Kind, Scenario)> {
    let long_key = "k".repeat(MAX_TAG_KEY_LENGTH);
    let long_value = "v".repeat(MAX_TAG_VALUE_LENGTH);
    vec![
        (
            Kind {
                label: "fleet",
                operation: "CreateFleet",
                record: "FleetAttributes",
                arn_member: "FleetArn",
                request: ec2_fleet("tagged-fleet"),
            },
            Scenario {
                created_with: json!([]),
                first_write: json!([{"Key": "env", "Value": "prod"}]),
                after_first: json!([{"Key": "env", "Value": "prod"}]),
                second_write: json!([
                    {"Key": "env", "Value": "staging"},
                    {"Key": "owner", "Value": "liveops"},
                ]),
                after_second: json!([
                    {"Key": "env", "Value": "staging"},
                    {"Key": "owner", "Value": "liveops"},
                ]),
                removed_keys: json!(["owner", "a-key-the-resource-never-carried"]),
                after_removal: json!([{"Key": "env", "Value": "staging"}]),
            },
        ),
        (
            Kind {
                label: "alias",
                operation: "CreateAlias",
                record: "Alias",
                arn_member: "AliasArn",
                request: json!({
                    "Name": "tagged-alias",
                    "RoutingStrategy": {"Type": "SIMPLE", "FleetId": "fleet-tag-target"},
                }),
            },
            Scenario {
                created_with: json!([{"Key": "owner", "Value": "liveops"}]),
                // A key the resource already carries takes the request's value.
                first_write: json!([{"Key": "owner", "Value": "platform"}]),
                after_first: json!([{"Key": "owner", "Value": "platform"}]),
                second_write: json!([
                    {"Key": "team", "Value": "red"},
                    {"Key": "cost-centre", "Value": "games"},
                ]),
                after_second: json!([
                    {"Key": "owner", "Value": "platform"},
                    {"Key": "team", "Value": "red"},
                    {"Key": "cost-centre", "Value": "games"},
                ]),
                removed_keys: json!(["owner"]),
                after_removal: json!([
                    {"Key": "team", "Value": "red"},
                    {"Key": "cost-centre", "Value": "games"},
                ]),
            },
        ),
        (
            Kind {
                label: "build",
                operation: "CreateBuild",
                record: "Build",
                arn_member: "BuildArn",
                request: json!({
                    "Name": "tagged-build",
                    "Version": "1.0.0",
                    "OperatingSystem": "AMAZON_LINUX_2023",
                }),
            },
            Scenario {
                created_with: json!([
                    {"Key": "a", "Value": "1"},
                    {"Key": "b", "Value": "2"},
                    {"Key": "c", "Value": "3"},
                ]),
                // The modelled `TagList` length starts at 0, so a write that assigns no
                // tag at all is a valid request that changes nothing.
                first_write: json!([]),
                after_first: json!([
                    {"Key": "a", "Value": "1"},
                    {"Key": "b", "Value": "2"},
                    {"Key": "c", "Value": "3"},
                ]),
                // `TagValue`'s length starts at 0, so the empty value is a value.
                second_write: json!([{"Key": "b", "Value": ""}]),
                after_second: json!([
                    {"Key": "a", "Value": "1"},
                    {"Key": "b", "Value": ""},
                    {"Key": "c", "Value": "3"},
                ]),
                removed_keys: json!(["a", "c"]),
                after_removal: json!([{"Key": "b", "Value": ""}]),
            },
        ),
        (
            Kind {
                label: "script",
                operation: "CreateScript",
                record: "Script",
                arn_member: "ScriptArn",
                request: json!({
                    "Name": "tagged-script",
                    "Version": "1.0.0",
                    "ZipFile": zip_file_member(&zip_bytes(256)),
                }),
            },
            Scenario {
                created_with: json!([]),
                // A key and a value at the longest the model allows.
                first_write: json!([{"Key": &long_key, "Value": &long_value}]),
                after_first: json!([{"Key": &long_key, "Value": &long_value}]),
                second_write: json!([{"Key": &long_key, "Value": "shortened"}]),
                after_second: json!([{"Key": &long_key, "Value": "shortened"}]),
                // The modelled `TagKeyList` length starts at 0: a removal that names no
                // key removes nothing.
                removed_keys: json!([]),
                after_removal: json!([{"Key": &long_key, "Value": "shortened"}]),
            },
        ),
        (
            Kind {
                label: "gamesessionqueue",
                operation: "CreateGameSessionQueue",
                record: "GameSessionQueue",
                arn_member: "GameSessionQueueArn",
                request: json!({"Name": "tagged-queue"}),
            },
            Scenario {
                created_with: json!([{"Key": "env", "Value": "prod"}]),
                // "The operation succeeds even if the list includes tags that are already
                // assigned to the resource."
                first_write: json!([{"Key": "env", "Value": "prod"}]),
                after_first: json!([{"Key": "env", "Value": "prod"}]),
                second_write: json!([
                    {"Key": "env", "Value": "prod"},
                    {"Key": "pool", "Value": "spot"},
                ]),
                after_second: json!([
                    {"Key": "env", "Value": "prod"},
                    {"Key": "pool", "Value": "spot"},
                ]),
                removed_keys: json!(["env"]),
                after_removal: json!([{"Key": "pool", "Value": "spot"}]),
            },
        ),
        (
            Kind {
                label: "matchmakingruleset",
                operation: "CreateMatchmakingRuleSet",
                record: "RuleSet",
                arn_member: "RuleSetArn",
                request: json!({"Name": "tagged-rules", "RuleSetBody": rule_set_body()}),
            },
            Scenario {
                created_with: json!([]),
                first_write: json!([
                    {"Key": "one", "Value": "1"},
                    {"Key": "two", "Value": "2"},
                    {"Key": "three", "Value": "3"},
                    {"Key": "four", "Value": "4"},
                ]),
                after_first: json!([
                    {"Key": "one", "Value": "1"},
                    {"Key": "two", "Value": "2"},
                    {"Key": "three", "Value": "3"},
                    {"Key": "four", "Value": "4"},
                ]),
                second_write: json!([{"Key": "two", "Value": "II"}]),
                after_second: json!([
                    {"Key": "one", "Value": "1"},
                    {"Key": "two", "Value": "II"},
                    {"Key": "three", "Value": "3"},
                    {"Key": "four", "Value": "4"},
                ]),
                removed_keys: json!(["one", "three"]),
                after_removal: json!([
                    {"Key": "two", "Value": "II"},
                    {"Key": "four", "Value": "4"},
                ]),
            },
        ),
        (
            Kind {
                label: "matchmakingconfiguration",
                operation: "CreateMatchmakingConfiguration",
                record: "Configuration",
                arn_member: "ConfigurationArn",
                request: json!({
                    "Name": "tagged-config",
                    "RuleSetName": "shared-rules",
                    "RequestTimeoutSeconds": 60,
                    "AcceptanceRequired": false,
                    "FlexMatchMode": "STANDALONE",
                }),
            },
            Scenario {
                created_with: json!([
                    {"Key": "owner", "Value": "liveops"},
                    {"Key": "env", "Value": "dev"},
                ]),
                first_write: json!([{"Key": "env", "Value": "prod"}]),
                after_first: json!([
                    {"Key": "owner", "Value": "liveops"},
                    {"Key": "env", "Value": "prod"},
                ]),
                second_write: json!([{"Key": "owner", "Value": "liveops"}]),
                after_second: json!([
                    {"Key": "owner", "Value": "liveops"},
                    {"Key": "env", "Value": "prod"},
                ]),
                removed_keys: json!(["owner", "env"]),
                after_removal: json!([]),
            },
        ),
        (
            Kind {
                label: "location",
                operation: "CreateLocation",
                record: "Location",
                arn_member: "LocationArn",
                request: json!({"LocationName": "custom-tagged-location"}),
            },
            Scenario {
                created_with: json!([]),
                first_write: json!([{"Key": "pool", "Value": "on-premises"}]),
                after_first: json!([{"Key": "pool", "Value": "on-premises"}]),
                second_write: json!([
                    {"Key": "pool", "Value": "on-premises"},
                    {"Key": "spare", "Value": ""},
                ]),
                after_second: json!([
                    {"Key": "pool", "Value": "on-premises"},
                    {"Key": "spare", "Value": ""},
                ]),
                removed_keys: json!(["spare"]),
                after_removal: json!([{"Key": "pool", "Value": "on-premises"}]),
            },
        ),
    ]
}

/// The rule set the matchmaking configuration rows match with, created before them.
async fn create_shared_rule_set(gamelift: &GameLift) {
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": "shared-rules", "RuleSetBody": rule_set_body()}),
        )
        .await;
}

/// The ARN of a resource created from `kind` with `tags` assigned.
async fn create_tagged(gamelift: &GameLift, kind: &Kind, tags: &Value) -> String {
    let mut request = kind.request.clone();
    request["Tags"] = tags.clone();
    let created = gamelift.ok(kind.operation, request).await;
    created[kind.record][kind.arn_member]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: {} must report the resource's ARN, got {created}",
                kind.label, kind.operation
            )
        })
        .to_string()
}

/// A tag write merges the request's pairs into the tags the resource holds and nothing
/// else: a key the resource does not carry is added, a key it does carry takes the new
/// value, and a key the request does not name is left alone — for every resource kind this
/// service tags. `UntagResource` then removes exactly the keys it names, and
/// `ListTagsForResource` serves the result of all of it.
#[tokio::test]
async fn tag_writes_merge_into_the_stored_tags_of_every_taggable_resource_kind() {
    let gamelift = GameLift::new();
    create_shared_rule_set(&gamelift).await;

    for (kind, scenario) in rows() {
        let label = kind.label;
        let resource_arn = create_tagged(&gamelift, &kind, &scenario.created_with).await;

        // The tags the create persisted are what the read serves before any tag write.
        assert_eq!(
            listed_tags(&gamelift, &resource_arn).await,
            tag_pairs(&scenario.created_with),
            "case {label:?}: the tags the create assigned must read back, for {resource_arn}"
        );

        for (step, write, expected) in [
            ("the first write", &scenario.first_write, &scenario.after_first),
            (
                "the second write",
                &scenario.second_write,
                &scenario.after_second,
            ),
        ] {
            let tagged = gamelift
                .ok(
                    "TagResource",
                    json!({"ResourceARN": &resource_arn, "Tags": write.clone()}),
                )
                .await;
            assert_eq!(
                tagged,
                json!({}),
                "case {label:?}: {step} must answer the modelled empty response, got {tagged}"
            );
            let listed = listed_tags(&gamelift, &resource_arn).await;
            assert_eq!(
                listed,
                tag_pairs(expected),
                "case {label:?}: after {step} of {write} the resource must hold {expected}, got {listed:?}"
            );
        }

        let untagged = gamelift
            .ok(
                "UntagResource",
                json!({
                    "ResourceARN": &resource_arn,
                    "TagKeys": scenario.removed_keys.clone(),
                }),
            )
            .await;
        assert_eq!(
            untagged,
            json!({}),
            "case {label:?}: UntagResource must answer the modelled empty response, got {untagged}"
        );
        let listed = listed_tags(&gamelift, &resource_arn).await;
        assert_eq!(
            listed,
            tag_pairs(&scenario.after_removal),
            "case {label:?}: removing {} must leave {}, got {listed:?}",
            scenario.removed_keys,
            scenario.after_removal
        );

        // Reading the tags changed nothing: the same read, run again, reports the same set.
        assert_eq!(
            listed_tags(&gamelift, &resource_arn).await,
            listed,
            "case {label:?}: listing a resource's tags must not change what a later list reports"
        );
    }
}

/// Tags are held on the resource their ARN names, so a write reaches that resource and no
/// other — including two resources of DIFFERENT kinds that share a name, whose ARNs differ
/// only in the kind they carry — and the tags go away with the resource.
#[tokio::test]
async fn tag_writes_reach_only_the_resource_their_arn_names() {
    let gamelift = GameLift::new();

    let first = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": "first",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "retired"},
            }),
        )
        .await;
    let second = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": "second",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "retired"},
            }),
        )
        .await;
    let first_arn = first["Alias"]["AliasArn"].as_str().expect("an alias ARN");
    let second_arn = second["Alias"]["AliasArn"].as_str().expect("an alias ARN");

    gamelift
        .ok(
            "TagResource",
            json!({
                "ResourceARN": first_arn,
                "Tags": [{"Key": "owner", "Value": "liveops"}],
            }),
        )
        .await;
    assert_eq!(
        listed_tags(&gamelift, first_arn).await,
        vec![("owner".to_string(), "liveops".to_string())],
        "case \"the tagged alias\": the write must reach the alias its ARN names"
    );
    assert_eq!(
        listed_tags(&gamelift, second_arn).await,
        Vec::<(String, String)>::new(),
        "case \"the untagged alias\": a write to another alias must leave this one untagged"
    );

    // A queue and a rule set under the SAME name are two resources: their ARNs differ in
    // the kind they name, so each carries its own tags.
    let shared_name = "shared-name";
    let queue = gamelift
        .ok("CreateGameSessionQueue", json!({"Name": shared_name}))
        .await;
    let rule_set = gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": shared_name, "RuleSetBody": rule_set_body()}),
        )
        .await;
    let queue_arn = queue["GameSessionQueue"]["GameSessionQueueArn"]
        .as_str()
        .expect("a queue ARN");
    let rule_set_arn = rule_set["RuleSet"]["RuleSetArn"]
        .as_str()
        .expect("a rule set ARN");
    gamelift
        .ok(
            "TagResource",
            json!({"ResourceARN": queue_arn, "Tags": [{"Key": "on", "Value": "queue"}]}),
        )
        .await;
    assert_eq!(
        listed_tags(&gamelift, queue_arn).await,
        vec![("on".to_string(), "queue".to_string())],
        "case \"the queue of the shared name\": the write must reach the queue"
    );
    assert_eq!(
        listed_tags(&gamelift, rule_set_arn).await,
        Vec::<(String, String)>::new(),
        "case \"the rule set of the shared name\": a write to the queue must not reach it"
    );

    // The tags live ON the resource, so deleting it takes them with it: its ARN then names
    // no resource at all.
    gamelift
        .ok("DeleteAlias", json!({"AliasId": first_arn}))
        .await;
    let (status, parsed) = gamelift
        .call("ListTagsForResource", &json!({"ResourceARN": first_arn}))
        .await;
    assert_eq!(
        (status, super::hosting_loop::error_code(&parsed).as_str()),
        (400, "NotFoundException"),
        "case \"the deleted alias\": its tags must not outlive it, got {status} with {parsed}"
    );
}

/// Each tag operation names ONE resource, so a `ResourceARN` that names no taggable
/// resource this store holds is the modelled `NotFoundException` carrying the ARN the
/// caller supplied — never an empty success.
///
/// The rows span the class: an unused id of a taggable kind, a stored resource's id under
/// another account and under another Region, a kind that carries no tags, an identifier
/// that is not an ARN, and a value that is no ARN at all.
#[tokio::test]
async fn an_arn_that_names_no_taggable_resource_is_refused_by_every_tag_operation() {
    let gamelift = GameLift::new();
    let created = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": "the-one-stored-alias",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "retired"},
            }),
        )
        .await;
    let stored_arn = created["Alias"]["AliasArn"]
        .as_str()
        .expect("an alias ARN")
        .to_string();
    let stored_id = created["Alias"]["AliasId"]
        .as_str()
        .expect("an alias id")
        .to_string();

    let cases = vec![
        (
            "an ARN of a taggable kind that no resource carries",
            gamelift_arn("fleet/fleet-never-created"),
        ),
        (
            "a stored resource's id in another account",
            format!("arn:aws:gamelift:{REGION}:999999999999:alias/{stored_id}"),
        ),
        (
            "a stored resource's id in another Region",
            format!(
                "arn:aws:gamelift:eu-central-1:{}:alias/{stored_id}",
                engine::emulator_account_id()
            ),
        ),
        (
            "an ARN of a kind that carries no tags",
            gamelift_arn("gamesession/fleet-abc/session-abc"),
        ),
        ("an identifier that is not an ARN", stored_id.clone()),
        ("a value that is no ARN at all", "not-an-arn".to_string()),
    ];

    for (label, resource_arn) in &cases {
        for request in [
            json!({"ResourceARN": resource_arn, "Tags": [{"Key": "k", "Value": "v"}]}),
            json!({"ResourceARN": resource_arn, "TagKeys": ["k"]}),
            json!({"ResourceARN": resource_arn}),
        ] {
            let operation = if request.get("Tags").is_some() {
                "TagResource"
            } else if request.get("TagKeys").is_some() {
                "UntagResource"
            } else {
                "ListTagsForResource"
            };
            let (status, parsed) = gamelift.call(operation, &request).await;
            assert_eq!(
                (
                    status,
                    super::hosting_loop::error_code(&parsed).as_str(),
                    error_message(&parsed)
                ),
                (
                    400,
                    "NotFoundException",
                    format!("Failed to find resource: {resource_arn}")
                ),
                "case {label:?} on {operation}: an ARN naming no taggable resource must be refused, got {status} with {parsed}"
            );
        }
    }

    // None of those refusals touched the resource this store does hold.
    assert_eq!(
        listed_tags(&gamelift, &stored_arn).await,
        Vec::<(String, String)>::new(),
        "the stored alias must still carry the tags it was created with"
    );
}

/// A tag set is a key-value MAP, so a request that assigns one key twice is refused with the
/// modelled `TaggingFailedException` and writes nothing — the same refusal the creates that
/// take a `TagList` make.
#[tokio::test]
async fn a_tag_set_that_assigns_one_key_twice_is_refused_and_writes_nothing() {
    let gamelift = GameLift::new();
    let created = gamelift
        .ok("CreateGameSessionQueue", json!({"Name": "duplicate-keys"}))
        .await;
    let queue_arn = created["GameSessionQueue"]["GameSessionQueueArn"]
        .as_str()
        .expect("a queue ARN")
        .to_string();
    gamelift
        .ok(
            "TagResource",
            json!({"ResourceARN": &queue_arn, "Tags": [{"Key": "owner", "Value": "liveops"}]}),
        )
        .await;

    let cases = vec![
        (
            "one key assigned twice",
            json!([
                {"Key": "env", "Value": "prod"},
                {"Key": "env", "Value": "staging"},
            ]),
            "env",
        ),
        (
            "one key assigned twice with the same value",
            json!([
                {"Key": "team", "Value": "red"},
                {"Key": "team", "Value": "red"},
            ]),
            "team",
        ),
        (
            "a repeated key among distinct ones",
            json!([
                {"Key": "a", "Value": "1"},
                {"Key": "b", "Value": "2"},
                {"Key": "a", "Value": "3"},
            ]),
            "a",
        ),
    ];

    for (label, tags, repeated) in &cases {
        let (status, parsed) = gamelift
            .call(
                "TagResource",
                &json!({"ResourceARN": &queue_arn, "Tags": tags.clone()}),
            )
            .await;
        assert_eq!(
            (
                status,
                super::hosting_loop::error_code(&parsed).as_str(),
                error_message(&parsed)
            ),
            (
                400,
                "TaggingFailedException",
                format!(
                    "Tags assigns the key {repeated} more than once; a resource's tags carry \
                     each key at most once."
                )
            ),
            "case {label:?}: a tag set assigning one key twice must be refused, got {status} with {parsed}"
        );
        assert_eq!(
            listed_tags(&gamelift, &queue_arn).await,
            vec![("owner".to_string(), "liveops".to_string())],
            "case {label:?}: the refused write must have changed nothing"
        );
    }
}

/// The tag count is bounded: a resource holds at most the modelled `TagList` length, so a
/// write that would take it past that is refused while one that only re-assigns keys it
/// already holds is served. A single request longer than the modelled list is refused by the
/// boundary walk before the handler runs.
#[tokio::test]
async fn the_tag_count_a_resource_holds_is_bounded_by_the_modelled_tag_list() {
    let gamelift = GameLift::new();
    let created = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": "the-bounded-alias",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "retired"},
            }),
        )
        .await;
    let alias_arn = created["Alias"]["AliasArn"]
        .as_str()
        .expect("an alias ARN")
        .to_string();

    let full: Vec<Value> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
        .collect();
    gamelift
        .ok(
            "TagResource",
            json!({"ResourceARN": &alias_arn, "Tags": full.clone()}),
        )
        .await;
    assert_eq!(
        listed_tags(&gamelift, &alias_arn).await.len(),
        MAX_TAGS_PER_RESOURCE,
        "a resource must hold the whole bound of {MAX_TAGS_PER_RESOURCE} tags"
    );

    // A write that only re-assigns keys the resource already holds does not grow it.
    gamelift
        .ok(
            "TagResource",
            json!({
                "ResourceARN": &alias_arn,
                "Tags": [{"Key": "key-0", "Value": "re-assigned"}],
            }),
        )
        .await;
    let listed = listed_tags(&gamelift, &alias_arn).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS_PER_RESOURCE,
        "re-assigning a key the resource holds must not grow its tag count, got {}",
        listed.len()
    );
    assert!(
        listed.contains(&("key-0".to_string(), "re-assigned".to_string())),
        "re-assigning a key must store the new value, got {listed:?}"
    );

    // One key past the bound is refused, and the resource keeps what it held.
    let (status, parsed) = gamelift
        .call(
            "TagResource",
            &json!({
                "ResourceARN": &alias_arn,
                "Tags": [{"Key": "one-key-too-many", "Value": "v"}],
            }),
        )
        .await;
    assert_eq!(
        (status, super::hosting_loop::error_code(&parsed).as_str()),
        (400, "TaggingFailedException"),
        "a write past the per-resource tag bound must be refused, got {status} with {parsed}"
    );
    assert_eq!(
        listed_tags(&gamelift, &alias_arn).await.len(),
        MAX_TAGS_PER_RESOURCE,
        "the refused write must have changed nothing"
    );

    // A single request longer than the modelled `TagList` never reaches the handler.
    let mut too_long = full;
    too_long.push(json!({"Key": "past-the-modelled-list", "Value": "v"}));
    let (status, code) = gamelift
        .err(
            "TagResource",
            json!({"ResourceARN": &alias_arn, "Tags": too_long}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "a Tags list past the modelled maximum must be refused by the boundary walk, got {status} {code}"
    );
}
