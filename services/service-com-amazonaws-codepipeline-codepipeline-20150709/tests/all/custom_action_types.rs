// SPDX-License-Identifier: CC0-1.0

//! `CreateCustomActionType`, read back through `ListActionTypes`, over the wire the
//! service actually speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: a created custom action type is persisted under its
//! `(category, Custom, provider, version)` `ActionTypeId` in the Region the request
//! was signed for, and every later read reports the members the create request
//! carried — so these cases always assert the LISTING, never only the create answer.
//! Each table row differs along what the rule generalises over (category, version,
//! how many configuration properties and with which flags, which settings URLs, the
//! artifact-count bounds, how many tags), and each refusal is asserted at the
//! published bound and one past it.
//!
//! The listing's own rules — the catalog the service provides itself, the two
//! filters, the page order and the cursor — are pinned by `action_type_catalog.rs`;
//! these cases are about what a CREATE puts in that catalog.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::{
    ActionTypeSettings, ArtifactDetails,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    ActionTypeKey, ActionTypeRecord, MAX_ACTION_TYPES_PER_ACCOUNT, MAX_TAGS_PER_RESOURCE,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::ComAmazonawsCodepipelineCodepipeline20150709State as ServiceState;

use crate::pipeline_lifecycle::Emulator;

/// Every `ActionCategory` the model declares
/// (`smithy_query(query="shape",shape="ActionCategory")` → these seven values).
const ACTION_CATEGORIES: [&str; 7] = [
    "Source", "Build", "Deploy", "Test", "Invoke", "Approval", "Compute",
];

/// Every `ActionConfigurationPropertyType` the model declares
/// (`smithy_query(query="shape",shape="ActionConfigurationPropertyType")`).
const PROPERTY_TYPES: [&str; 3] = ["String", "Number", "Boolean"];

/// The owner a custom action type carries: `ActionTypeId$owner` is an `ActionOwner`
/// (`AWS`, `ThirdParty`, `Custom`) and `CreateCustomActionType` "creates a new custom
/// action".
pub(crate) const CUSTOM: &str = "Custom";

/// The Region `Emulator::call` signs for, so a listing in it sees what those creates
/// registered.
const DEFAULT_REGION: &str = "us-east-1";

/// `CreateCustomActionTypeInput$configurationProperties` is
/// `@length(min: 0, max: 10)`.
const MAX_CONFIGURATION_PROPERTIES: usize = 10;

/// `MinimumArtifactCount` and `MaximumArtifactCount` are `@range(min: 0, max: 5)`.
const MAX_ARTIFACT_COUNT: u64 = 5;

/// One configuration property with every member stated, so a response that echoes it
/// can be compared member for member.
fn property(name: &str, index: usize, required: bool, secret: bool, queryable: bool) -> Value {
    json!({
        "name": name,
        "required": required,
        "key": index.is_multiple_of(2),
        "secret": secret,
        "queryable": queryable,
        "description": format!("property {name}"),
        "type": PROPERTY_TYPES[index % PROPERTY_TYPES.len()],
    })
}

/// `count` configuration properties, none queryable.
fn properties(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                property(
                    &format!("p{index}"),
                    index,
                    index.is_multiple_of(2),
                    false,
                    false,
                )
            })
            .collect(),
    )
}

/// `count` tags, each with a distinct key.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"key": format!("k{index}"), "value": format!("v{index}")}))
            .collect(),
    )
}

/// The `ActionType` a create request must be answered — and later listed — with:
/// the identity it named, owned by `Custom`, plus the members it carried.
fn expected_action_type(request: &Value) -> Value {
    let mut expected = json!({
        "id": {
            "category": request["category"],
            "owner": CUSTOM,
            "provider": request["provider"],
            "version": request["version"],
        },
        "inputArtifactDetails": request["inputArtifactDetails"],
        "outputArtifactDetails": request["outputArtifactDetails"],
    });
    if let Some(settings) = request.get("settings") {
        expected["settings"] = settings.clone();
    }
    match request
        .get("configurationProperties")
        .and_then(Value::as_array)
    {
        Some(declared) if !declared.is_empty() => {
            expected["actionConfigurationProperties"] = Value::Array(declared.clone());
        }
        _ => {}
    }
    expected
}

/// Every member of `expected` appears in `actual` with the same value, recursively.
///
/// Not plain equality: a response may also carry a member the request left to its
/// modelled default (`queryable`, say), which is not a difference these cases are
/// about. A list, though, must match element for element and in order.
pub(crate) fn contains(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => expected
            .iter()
            .all(|(key, value)| actual.get(key).is_some_and(|found| contains(value, found))),
        (Value::Array(expected), Value::Array(actual)) => {
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual)
                    .all(|(expected, actual)| contains(expected, actual))
        }
        _ => expected == actual,
    }
}

/// The `(category, provider, version)` identity of a listed action type, which is
/// also its key beside the owner.
///
/// `pub(crate)` so the other suites of this target name a listed action type the same
/// way; see `tests/all/delete_custom_action_type.rs`.
pub(crate) fn identity(action_type: &Value) -> String {
    let id = &action_type["id"];
    let member = |name: &str| {
        id.get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("every id member must be a string, got {action_type}"))
            .to_string()
    };
    format!(
        "{}/{}/{}",
        member("category"),
        member("provider"),
        member("version")
    )
}

/// Every action type the listing reports for `owner` in `region`, paged to the end.
///
/// The owner filter is what separates the account's own registrations from the
/// catalog the service provides itself (pinned in `action_type_catalog.rs`), so a
/// caller picks which of the two it is asking about.
pub(crate) async fn listed_in(emulator: &Emulator, owner: &str, region: &str) -> Vec<Value> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({"actionOwnerFilter": owner, "regionFilter": region});
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let answer = emulator.ok("ListActionTypes", &body).await;
        let page = answer
            .get("actionTypes")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("ListActionTypes must answer actionTypes, got {answer}"))
            .clone();
        for action_type in &page {
            assert_eq!(
                action_type["id"]["owner"], json!(owner),
                "an owner-filtered listing reports only that owner, got {action_type}"
            );
        }
        all.extend(page);
        match answer.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    all
}

/// Every action type this account registered in the Region the creates were signed
/// for.
async fn registered(emulator: &Emulator) -> Vec<Value> {
    listed_in(emulator, CUSTOM, DEFAULT_REGION).await
}

/// One create request, and the member absences its answer must also show.
struct CreateCase {
    case: &'static str,
    request: Value,
    /// `actionType` members the request carried nothing for, which the answer reports as
    /// the EMPTY form of their modelled type (`settings` as `{}`,
    /// `actionConfigurationProperties` as `[]`) rather than leaving out.
    empty: &'static [&'static str],
    /// Whether the create answer must echo a `tags` list.
    tags_echoed: bool,
}

/// Whether `value` is the empty form of the type it is: an object or a list with nothing
/// in it.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Object(members) => members.is_empty(),
        Value::Array(elements) => elements.is_empty(),
        _ => false,
    }
}

/// RULE: a created custom action type is stored under its
/// `(category, Custom, provider, version)` id, and both the create answer and every
/// later `ListActionTypes` report exactly the members the request carried.
#[tokio::test]
async fn every_created_custom_action_type_round_trips_through_list_action_types() {
    let emulator = Emulator::new();

    let mut cases = vec![
        CreateCase {
            case: "the smallest create: required members only",
            request: json!({
                "category": "Build",
                "provider": "Minimal",
                "version": "1",
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 0},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 0},
            }),
            empty: &["settings", "actionConfigurationProperties"],
            tags_echoed: false,
        },
        CreateCase {
            case: "artifact counts at the modelled maximum of 5",
            request: json!({
                "category": "Deploy",
                "provider": "ArtifactBounds",
                "version": "9chars_v9",
                "inputArtifactDetails": {
                    "minimumCount": 0,
                    "maximumCount": MAX_ARTIFACT_COUNT,
                },
                "outputArtifactDetails": {
                    "minimumCount": MAX_ARTIFACT_COUNT,
                    "maximumCount": MAX_ARTIFACT_COUNT,
                },
            }),
            empty: &["settings", "actionConfigurationProperties"],
            tags_echoed: false,
        },
        CreateCase {
            case: "one settings URL only",
            request: json!({
                "category": "Test",
                "provider": "OneSetting",
                "version": "1",
                "settings": {"entityUrlTemplate": "https://example.test/{Config:p0}"},
                "inputArtifactDetails": {"minimumCount": 1, "maximumCount": 2},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            }),
            empty: &["actionConfigurationProperties"],
            tags_echoed: false,
        },
        CreateCase {
            case: "every settings URL, every property flag, and tags",
            request: json!({
                "category": "Invoke",
                "provider": "Full-Provider_1",
                "version": "v2",
                "settings": {
                    "thirdPartyConfigurationUrl": "https://example.test/signup",
                    "entityUrlTemplate": "https://example.test/entity/{Config:needle}",
                    "executionUrlTemplate": "https://example.test/execution",
                    "revisionUrlTemplate": "https://example.test/revision",
                },
                "configurationProperties": [
                    property("needle", 0, true, false, true),
                    property("secretly", 1, false, true, false),
                    property("plain", 2, false, false, false),
                ],
                "inputArtifactDetails": {"minimumCount": 1, "maximumCount": 5},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 3},
                "tags": tags(3),
            }),
            empty: &[],
            tags_echoed: true,
        },
        CreateCase {
            case: "configuration properties at the modelled maximum of 10",
            request: json!({
                "category": "Approval",
                "provider": "TenProperties",
                "version": "1",
                "configurationProperties": properties(MAX_CONFIGURATION_PROPERTIES),
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 0},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 0},
                "tags": tags(MAX_TAGS_PER_RESOURCE),
            }),
            empty: &["settings"],
            tags_echoed: true,
        },
    ];
    // The rule holds for every category the model declares, not only the ones the
    // rows above spell out.
    cases.extend(ACTION_CATEGORIES.iter().map(|category| CreateCase {
        case: "one action type per declared category",
        request: json!({
            "category": category,
            "provider": format!("Category{category}"),
            "version": "1",
            "configurationProperties": properties(1),
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
        }),
        empty: &["settings"],
        tags_echoed: false,
    }));

    let mut expected_identities = Vec::new();
    for CreateCase {
        case,
        request,
        empty,
        tags_echoed,
    } in &cases
    {
        let answer = emulator.ok("CreateCustomActionType", request).await;
        let created = answer
            .get("actionType")
            .unwrap_or_else(|| panic!("case {case:?}: no actionType in {answer}"));
        let expected = expected_action_type(request);
        assert!(
            contains(&expected, created),
            "case {case:?}: expected the create answer to carry {expected}, got {created}"
        );
        for member in *empty {
            let reported = created.get(member).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: every modelled member is reported, so {member} must be \
                     there even when the request carried nothing for it, got {created}"
                )
            });
            assert!(
                is_empty(reported),
                "case {case:?}: the request carried no {member}, so the answer reports it \
                 empty, got {created}"
            );
        }
        match (tags_echoed, answer.get("tags")) {
            (true, Some(echoed)) => assert!(
                contains(&request["tags"], echoed),
                "case {case:?}: expected tags {}, got {echoed}",
                request["tags"]
            ),
            (true, None) => panic!("case {case:?}: the request carried tags, none were echoed"),
            (false, echoed) => assert!(
                echoed.is_none(),
                "case {case:?}: the request carried no tags, got {answer}"
            ),
        }
        expected_identities.push((*case, identity(created), expected.clone()));
    }

    // The read-back: every identity is in the listing exactly once, reported with
    // the same members the create answered.
    let all = registered(&emulator).await;
    for (case, identity_wanted, expected) in &expected_identities {
        let found: Vec<&Value> = all
            .iter()
            .filter(|action_type| &identity(action_type) == identity_wanted)
            .collect();
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: expected {identity_wanted} listed exactly once, got {} entries",
            found.len()
        );
        assert!(
            contains(expected, found[0]),
            "case {case:?}: expected the listing to carry {expected}, got {}",
            found[0]
        );
    }
    assert_eq!(
        all.len(),
        cases.len(),
        "the listing reports exactly the created action types, got {all:?}"
    );

    // And a second listing answers the same: a read changes nothing.
    let again = registered(&emulator).await;
    assert_eq!(
        again, all,
        "ListActionTypes must answer from state, unchanged by the read"
    );
}

/// RULE: the identity is `(category, provider, version)` together — two action types
/// differing in any one of the three are different resources, and creating the
/// identity an account already holds replaces it rather than adding a second entry.
#[tokio::test]
async fn the_identity_is_the_category_provider_and_version_together() {
    let emulator = Emulator::new();

    let base = |category: &str, provider: &str, version: &str| {
        json!({
            "category": category,
            "provider": provider,
            "version": version,
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
        })
    };

    // Four distinct identities: one base, and one differing in each component.
    let distinct = [
        ("the base identity", base("Build", "Prov", "1")),
        ("a different category", base("Test", "Prov", "1")),
        ("a different provider", base("Build", "Other", "1")),
        ("a different version", base("Build", "Prov", "2")),
    ];
    for (case, request) in &distinct {
        let answer = emulator.ok("CreateCustomActionType", request).await;
        assert_eq!(
            answer["actionType"]["id"]["owner"], CUSTOM,
            "case {case:?}: a created action type is Custom-owned, got {answer}"
        );
    }
    let all = registered(&emulator).await;
    assert_eq!(
        all.len(),
        distinct.len(),
        "each of the {} identities is its own action type, got {all:?}",
        distinct.len()
    );

    // Re-creating one identity with different details replaces it: still one entry,
    // now carrying the new details.
    let mut replacement = base("Build", "Prov", "1");
    replacement["inputArtifactDetails"] = json!({"minimumCount": 2, "maximumCount": 4});
    replacement["settings"] = json!({"revisionUrlTemplate": "https://example.test/rev"});
    emulator.ok("CreateCustomActionType", &replacement).await;

    let all = registered(&emulator).await;
    assert_eq!(
        all.len(),
        distinct.len(),
        "re-creating a held identity replaces it rather than adding one, got {all:?}"
    );
    let replaced = all
        .iter()
        .find(|action_type| identity(action_type) == "Build/Prov/1")
        .unwrap_or_else(|| panic!("the replaced identity must still be listed, got {all:?}"));
    assert!(
        contains(&expected_action_type(&replacement), replaced),
        "case \"replacement\": expected {}, got {replaced}",
        expected_action_type(&replacement)
    );
}

/// One create request that must be refused, and the modelled error that refuses it.
struct RefusalCase {
    case: &'static str,
    request: Value,
    code: &'static str,
}

/// RULE: a request whose tag set no resource can carry, or whose configuration
/// properties break a rule the model documents, is refused with the error shape the
/// model declares for that defect — and nothing is persisted.
#[tokio::test]
async fn a_refused_create_persists_nothing() {
    let emulator = Emulator::new();

    let request = |provider: &str| {
        json!({
            "category": "Build",
            "provider": provider,
            "version": "1",
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
        })
    };
    let with = |provider: &str, member: &str, value: Value| {
        let mut body = request(provider);
        body[member] = value;
        body
    };

    let refused = [
        RefusalCase {
            case: "one tag past the 50 a resource may carry",
            request: with("TooManyTags", "tags", tags(MAX_TAGS_PER_RESOURCE + 1)),
            code: "TooManyTagsException",
        },
        RefusalCase {
            case: "far past the tag bound",
            request: with("WayTooManyTags", "tags", tags(4 * MAX_TAGS_PER_RESOURCE)),
            code: "TooManyTagsException",
        },
        RefusalCase {
            case: "one tag key named twice",
            request: with(
                "DuplicateTagKey",
                "tags",
                json!([{"key": "k", "value": "a"}, {"key": "k", "value": "b"}]),
            ),
            code: "InvalidTagsException",
        },
        RefusalCase {
            case: "two queryable configuration properties",
            request: with(
                "TwoQueryable",
                "configurationProperties",
                json!([
                    property("a", 0, true, false, true),
                    property("b", 1, true, false, true),
                ]),
            ),
            code: "ValidationException",
        },
        RefusalCase {
            case: "a queryable property that is not required",
            request: with(
                "QueryableOptional",
                "configurationProperties",
                json!([property("a", 0, false, false, true)]),
            ),
            code: "ValidationException",
        },
        RefusalCase {
            case: "a queryable property that is secret",
            request: with(
                "QueryableSecret",
                "configurationProperties",
                json!([property("a", 0, true, true, true)]),
            ),
            code: "ValidationException",
        },
        RefusalCase {
            case: "one configuration property name declared twice",
            request: with(
                "DuplicateProperty",
                "configurationProperties",
                json!([
                    property("same", 0, true, false, false),
                    property("same", 1, false, false, false),
                ]),
            ),
            code: "ValidationException",
        },
    ];

    for RefusalCase {
        case,
        request,
        code,
    } in &refused
    {
        let (status, answered) = emulator
            .expect_error("CreateCustomActionType", request)
            .await;
        assert_eq!(
            (status, answered.as_str()),
            (400, *code),
            "case {case:?}: expected a 400 {code}, got {status} {answered}"
        );
    }

    // Each accepted bound, to show the refusals above are the bound and not the
    // rule: no tags, one tag, exactly 50 tags, and exactly one queryable property
    // that is required and not secret.
    for (case, body) in [
        ("no tags at all", request("NoTags")),
        ("one tag", with("OneTag", "tags", tags(1))),
        (
            "tags at the bound of 50",
            with("TagsAtBound", "tags", tags(MAX_TAGS_PER_RESOURCE)),
        ),
        (
            "exactly one queryable property, required and not secret",
            with(
                "OneQueryable",
                "configurationProperties",
                json!([
                    property("a", 0, true, false, true),
                    property("b", 1, false, true, false),
                ]),
            ),
        ),
    ] {
        emulator.ok("CreateCustomActionType", &body).await;
        let provider = body["provider"].as_str().unwrap_or_default();
        let all = registered(&emulator).await;
        assert!(
            all.iter()
                .any(|action_type| identity(action_type) == format!("Build/{provider}/1")),
            "case {case:?}: the accepted create must be listed, got {all:?}"
        );
    }

    // Nothing a refusal named reached the store.
    let all = registered(&emulator).await;
    for RefusalCase { case, request, .. } in &refused {
        let provider = request["provider"].as_str().unwrap_or_default();
        assert!(
            !all.iter()
                .any(|action_type| identity(action_type) == format!("Build/{provider}/1")),
            "case {case:?}: a refused create must persist nothing, got {all:?}"
        );
    }
}

/// RULE: a created action type belongs to the Region its request was signed for, so
/// it is reported to a caller in that Region (or filtering on it) and to no other.
#[tokio::test]
async fn a_created_action_type_belongs_to_the_region_its_request_was_signed_for() {
    let emulator = Emulator::new();

    let request = |provider: &str| {
        json!({
            "category": "Build",
            "provider": provider,
            "version": "1",
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
        })
    };
    // Rows differ in the Region the create is signed for, including two in one
    // Region and one in a Region named nowhere else.
    let created = [
        ("us-east-1", "EastOne"),
        ("us-east-1", "EastTwo"),
        ("eu-west-2", "WestOne"),
        ("ap-southeast-4", "SouthOne"),
    ];
    for (region, provider) in created {
        let (status, answer) = emulator
            .call_in(region, "CreateCustomActionType", &request(provider))
            .await;
        assert_eq!(
            status, 200,
            "creating {provider} in {region} must succeed, got {answer}"
        );
    }

    for (case, region, expected) in [
        ("the Region two were created in", "us-east-1", 2),
        ("a Region one was created in", "eu-west-2", 1),
        ("another Region one was created in", "ap-southeast-4", 1),
        ("a Region nothing was created in", "me-central-1", 0),
    ] {
        let listed = listed_in(&emulator, CUSTOM, region).await;
        assert_eq!(
            listed.len(),
            expected,
            "case {case:?}: expected {expected} registrations in {region}, got {listed:?}"
        );
        for action_type in &listed {
            let provider = action_type["id"]["provider"].as_str().unwrap_or_default();
            assert!(
                created
                    .iter()
                    .any(|(own, name)| *own == region && *name == provider),
                "case {case:?}: {provider} was not created in {region}, got {action_type}"
            );
        }
    }
}

/// RULE: the account's action-type store is bounded — once it holds
/// `MAX_ACTION_TYPES_PER_ACCOUNT` registrations a create of a NEW identity is
/// refused with the limit error the operation models, while re-creating an identity
/// the account already holds still succeeds, because replacing one adds nothing.
#[tokio::test]
async fn a_new_identity_past_the_account_limit_is_refused() {
    let emulator = Emulator::new();

    let request = |provider: &str, maximum: u64| {
        json!({
            "category": "Build",
            "provider": provider,
            "version": "1",
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": maximum},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
        })
    };

    // Fill the store to the cap through `register_action_type`, the one gate a
    // handler writes through, rather than making that many wire calls; every
    // assertion below still goes through the router.
    {
        let state = emulator.engine().state::<ServiceState>();
        let mut state = state.lock();
        for index in 0..MAX_ACTION_TYPES_PER_ACCOUNT {
            let accepted = state.register_action_type(
                ActionTypeKey {
                    category: "Build".to_string(),
                    owner: CUSTOM.to_string(),
                    provider: format!("Filler{index}"),
                    version: "1".to_string(),
                },
                ActionTypeRecord {
                    settings: None::<ActionTypeSettings>,
                    configuration_properties: Vec::new(),
                    input_artifact_details: ArtifactDetails {
                        minimum_count: 0,
                        maximum_count: 1,
                    },
                    output_artifact_details: ArtifactDetails {
                        minimum_count: 0,
                        maximum_count: 1,
                    },
                    region: Some(DEFAULT_REGION.to_string()),
                    tags: Vec::new(),
                },
            );
            assert!(
                accepted,
                "filling the store must be accepted below the bound, failed at {index}"
            );
        }
        assert_eq!(
            state.registered_action_type_count(),
            MAX_ACTION_TYPES_PER_ACCOUNT,
            "the store must start this case full"
        );
    }

    // One past the cap: refused with the only limit error the operation models.
    let (status, code) = emulator
        .expect_error("CreateCustomActionType", &request("OnePastTheCap", 1))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "case \"a new identity at the cap\": expected a 400 LimitExceededException, \
         got {status} {code}"
    );
    let refused = registered(&emulator).await;
    assert!(
        !refused
            .iter()
            .any(|action_type| identity(action_type) == "Build/OnePastTheCap/1"),
        "case \"a new identity at the cap\": the refused create must persist nothing"
    );

    // An identity the account already holds is still accepted at the cap, and the
    // listing reports the replacement's details.
    let answer = emulator
        .ok("CreateCustomActionType", &request("Filler0", 4))
        .await;
    assert_eq!(
        answer["actionType"]["inputArtifactDetails"]["maximumCount"],
        json!(4),
        "case \"a held identity at the cap\": expected the replacement's details, got {answer}"
    );
    let all = registered(&emulator).await;
    assert_eq!(
        all.len(),
        MAX_ACTION_TYPES_PER_ACCOUNT,
        "a replacement adds nothing to a full store, got {} entries",
        all.len()
    );
    let replaced = all
        .iter()
        .find(|action_type| identity(action_type) == "Build/Filler0/1")
        .unwrap_or_else(|| panic!("the replaced identity must still be listed"));
    assert_eq!(
        replaced["inputArtifactDetails"]["maximumCount"],
        json!(4),
        "case \"a held identity at the cap\": the listing must report the replacement, \
         got {replaced}"
    );
}

/// RULE: a modelled constraint on the input is refused at the boundary, before any
/// state is touched — the handler never sees a request that breaks one.
#[tokio::test]
async fn a_request_the_model_rejects_never_reaches_the_store() {
    let emulator = Emulator::new();

    let request = |overrides: Value| {
        let mut body = json!({
            "category": "Build",
            "provider": "Prov",
            "version": "1",
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
        });
        for (member, value) in overrides.as_object().expect("an object of overrides") {
            body[member] = value.clone();
        }
        body
    };

    for (case, body) in [
        (
            "a category outside the ActionCategory enum",
            request(json!({"category": "Nonsense"})),
        ),
        (
            "a provider breaking ActionProvider's pattern",
            request(json!({"provider": "not a provider"})),
        ),
        (
            "a version past Version's 9-character maximum",
            request(json!({"version": "0123456789"})),
        ),
        (
            "an artifact count past the modelled maximum of 5",
            request(json!({"inputArtifactDetails": {
                "minimumCount": 0,
                "maximumCount": MAX_ARTIFACT_COUNT + 1,
            }})),
        ),
        (
            "more configuration properties than the modelled maximum of 10",
            request(json!({
                "configurationProperties": properties(MAX_CONFIGURATION_PROPERTIES + 1),
            })),
        ),
    ] {
        let (status, code) = emulator
            .expect_error("CreateCustomActionType", &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {code}"
        );
        let all = registered(&emulator).await;
        assert!(
            all.is_empty(),
            "case {case:?}: a refused request must persist nothing, got {all:?}"
        );
    }
}
