// SPDX-License-Identifier: CC0-1.0

//! A tag map carrying an inadmissible key is refused WHOLE, and the refusal names
//! every inadmissible key in it — end to end over the wire.
//!
//! The rule under test: a caller-supplied tag map is admitted only when EVERY key in
//! it satisfies the key shape's modelled bounds, so a map carrying any key outside
//! them is refused with HTTP 400 `ValidationException` and nothing of that map is
//! written — not even the admissible keys sitting beside the inadmissible one. The
//! refusal carries one clause per inadmissible key, naming the key and the
//! requirement it failed, in the validation envelope every constraint refusal in this
//! emulator is worded with; the clauses are ordered by key, so one request is refused
//! with one sentence however the map reached the handler.
//!
//! `tag_key_rules.rs` spans the key shape one key at a time. These cases span what a
//! SINGLE request can carry: how many inadmissible keys it holds, which of the key
//! shape's bounds each one fails, and how many admissible keys share the map with
//! them — asserted on both operations that accept a tag map (`CreateCampaign` and
//! `TagResource`), whose messages must agree because the member is the same.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ONBOARDING_JOB_DURATION, TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH, TAG_KEY_PATTERN,
};
use smithy_server_runtime::MultiServiceRouter;

// One harness per crate: the routing header, the router under test and the
// request/response plumbing live in `tests/all/common.rs`.
use super::common::{call, error_code, router};

/// The region every case here signs for, and the one its ARNs are rooted in.
const REGION: &str = "ap-southeast-2";

/// The Connect instance every campaign below belongs to.
const INSTANCE: &str = "instance-refusal";

/// The member a tag map arrives under, which is what a refusal clause names.
const MEMBER: &str = "tags";

/// Onboard [`INSTANCE`] and wait out the span the clock owes its onboarding job, so
/// the instance is a campaign target by the time this returns.
async fn onboard(router: &MultiServiceRouter) {
    let (status, body) = call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{INSTANCE}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"onboard {INSTANCE}\": expected HTTP 200, got {status} {body}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;
}

/// The `{arn}` path label of `/v2/tags/{arn}`, percent-encoded as a single segment.
fn encoded(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// A JSON tag map carrying `keys`, each with a value the value shape admits.
fn tag_map(keys: &[String]) -> Value {
    let mut map = Map::new();
    for key in keys {
        map.insert(key.clone(), Value::String("value".to_string()));
    }
    Value::Object(map)
}

/// The tag map a response carries, as an ordered map so a mismatch prints
/// deterministically.
fn tags_of(response: &Value) -> BTreeMap<String, String> {
    response
        .get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or("<not a string>").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The message a refusal carries.
fn message_of(response: &Value) -> &str {
    response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// `ListTagsForResource` for `arn`, asserting it answered, and returning the stored
/// tag map — the one every reader of this resource sees.
async fn stored_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        REGION,
        "GET",
        &format!("/v2/tags/{}", encoded(arn)),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected HTTP 200, got {status} {body}"
    );
    tags_of(&body)
}

/// Every campaign name the store holds, walked page by page through `ListCampaigns` —
/// the reading a refused create must leave exactly as it was.
async fn campaign_names(router: &MultiServiceRouter) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut next_token: Option<String> = None;
    loop {
        let request = match &next_token {
            Some(token) => json!({"maxResults": 50, "nextToken": token}),
            None => json!({"maxResults": 50}),
        };
        let (status, body) = call(router, REGION, "POST", "/v2/campaigns-summary", &request).await;
        assert_eq!(
            status, 200,
            "ListCampaigns {request}: expected HTTP 200, got {status} {body}"
        );
        for summary in body
            .get("campaignSummaryList")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            names.insert(
                summary
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("<no name>")
                    .to_string(),
            );
        }
        next_token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return names;
        }
    }
}

/// Which of the key shape's modelled bounds a key fails.
///
/// Source: `TagKey`, the shape a tag map's key targets
/// (`smithy_query query="shape" shape="TagKey"`) — `@length` 1..128 and `@pattern`
/// `^(?!aws:)[a-zA-Z+-=._:/]+$`, whose leading lookahead excludes the reserved `aws:`
/// namespace and whose character class admits ASCII letters, `_` and the `+`..`=`
/// range.
#[derive(Debug, Clone, Copy)]
enum Failed {
    /// Outside the key shape's `@pattern`.
    Pattern,
    /// Shorter than the key shape's `@length` minimum.
    TooShort,
    /// Longer than the key shape's `@length` maximum.
    TooLong,
}

impl Failed {
    /// The requirement a clause states for this bound, in the wording every
    /// constraint refusal this emulator answers is built from.
    fn requirement(self) -> String {
        match self {
            Self::Pattern => {
                format!("Member must satisfy regular expression pattern: {TAG_KEY_PATTERN}")
            }
            Self::TooShort => format!(
                "Member must have length greater than or equal to {TAG_KEY_MIN_LENGTH}"
            ),
            Self::TooLong => {
                format!("Member must have length less than or equal to {TAG_KEY_MAX_LENGTH}")
            }
        }
    }
}

/// The whole message a map whose `offenders` are inadmissible is refused with: the
/// validation envelope, carrying one clause per offending key, ordered by key.
fn expected_message(offenders: &[(String, Failed)]) -> String {
    let mut ordered: Vec<&(String, Failed)> = offenders.iter().collect();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let clauses: Vec<String> = ordered
        .iter()
        .map(|(key, failed)| {
            format!(
                "Value '{key}' at '{MEMBER}.{key}.member' failed to satisfy constraint: {}",
                failed.requirement()
            )
        })
        .collect();
    if clauses.len() == 1 {
        format!("1 validation error detected: {}", clauses[0])
    } else {
        format!(
            "{} validation errors detected: {}",
            clauses.len(),
            clauses.join("; ")
        )
    }
}

/// One map a request can carry: what to call it, the keys it holds, and which of
/// those keys are inadmissible and why. An empty `offenders` means the whole map is
/// admissible.
struct MapCase {
    case: String,
    keys: Vec<String>,
    offenders: Vec<(String, Failed)>,
}

impl MapCase {
    fn new(case: &str, keys: &[&str], offenders: &[(&str, Failed)]) -> Self {
        Self {
            case: case.to_string(),
            keys: keys.iter().map(|key| (*key).to_string()).collect(),
            offenders: offenders
                .iter()
                .map(|(key, failed)| ((*key).to_string(), *failed))
                .collect(),
        }
    }
}

/// The maps one request can carry.
///
/// The rows differ along the number of inadmissible keys (none, one, two, three), the
/// bound each one fails, and the number of admissible keys sharing the map with them
/// (none, one, three).
fn map_cases() -> Vec<MapCase> {
    let too_long = "a".repeat(TAG_KEY_MAX_LENGTH + 1);
    vec![
        MapCase::new("every key admissible", &["Environment", "cost-center"], &[]),
        MapCase::new(
            "the reserved namespace alone",
            &["aws:reserved"],
            &[("aws:reserved", Failed::Pattern)],
        ),
        MapCase::new(
            "the reserved namespace beside an admissible key",
            &["aws:reserved", "Keepme"],
            &[("aws:reserved", Failed::Pattern)],
        ),
        MapCase::new(
            "two keys outside the character class",
            &["has space", "bang!"],
            &[("has space", Failed::Pattern), ("bang!", Failed::Pattern)],
        ),
        MapCase::new(
            "one key per bound the key shape states",
            &["", &too_long, "aws:too", "Admissible"],
            &[
                ("", Failed::TooShort),
                (&too_long, Failed::TooLong),
                ("aws:too", Failed::Pattern),
            ],
        ),
        MapCase::new(
            "three admissible keys and one that is not",
            &["one", "two", "three", "no@good"],
            &[("no@good", Failed::Pattern)],
        ),
    ]
}

#[tokio::test]
async fn a_tag_map_with_an_inadmissible_key_is_refused_whole_on_every_path_that_takes_one() {
    let router = router();
    onboard(&router).await;

    // The campaign the merge path is exercised against, born with one tag so a
    // refused merge has something to leave alone.
    let (status, host) = call(
        &router,
        REGION,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": "refusal host",
            "connectInstanceId": INSTANCE,
            "tags": {"Keep": "me"},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the merge host\": expected HTTP 200, got {status} {host}"
    );
    let host_arn = host["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"the merge host\": no arn in {host}"))
        .to_string();

    // What the host resource holds, grown by each merge that is ACCEPTED: every
    // refused merge is asserted to leave exactly this.
    let mut expected_stored = BTreeMap::from([("Keep".to_string(), "me".to_string())]);

    for (index, row) in map_cases().into_iter().enumerate() {
        let MapCase {
            case,
            keys,
            offenders,
        } = row;
        let tags = tag_map(&keys);
        let admissible = offenders.is_empty();
        // A campaign name identifies a campaign within its Connect instance, so each
        // row creates under a name of its own.
        let name = format!("refusal case {index}");
        let names_before = campaign_names(&router).await;

        // ---- the create path -------------------------------------------------
        let (status, body) = call(
            &router,
            REGION,
            "PUT",
            "/v2/campaigns",
            &json!({"name": name, "connectInstanceId": INSTANCE, "tags": tags}),
        )
        .await;
        let names_after = campaign_names(&router).await;
        let created_message = message_of(&body).to_string();

        if admissible {
            assert_eq!(
                status, 200,
                "case {case:?}: CreateCampaign expected HTTP 200, got {status} {body}"
            );
            let mut expected_echo = BTreeMap::new();
            for key in &keys {
                expected_echo.insert(key.clone(), "value".to_string());
            }
            assert_eq!(
                tags_of(&body),
                expected_echo,
                "case {case:?}: CreateCampaign echoed {:?}, expected {expected_echo:?}",
                tags_of(&body)
            );
            let created_arn = body["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no arn in {body}"));
            assert_eq!(
                stored_tags(&router, created_arn, &case).await,
                expected_echo,
                "case {case:?}: ListTagsForResource disagreed with the create"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: CreateCampaign expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: CreateCampaign expected ValidationException, got {} {body}",
                error_code(&body)
            );
            assert_eq!(
                created_message,
                expected_message(&offenders),
                "case {case:?}: CreateCampaign worded the refusal {created_message:?}"
            );
            assert_eq!(
                names_after, names_before,
                "case {case:?}: a refused create stored a campaign, names went \
                 {names_before:?} -> {names_after:?}"
            );
        }

        // ---- the merge path, on the same map ---------------------------------
        let (status, body) = call(
            &router,
            REGION,
            "POST",
            &format!("/v2/tags/{}", encoded(&host_arn)),
            &json!({"tags": tags}),
        )
        .await;
        let merged_message = message_of(&body).to_string();

        if admissible {
            assert_eq!(
                status, 200,
                "case {case:?}: TagResource expected HTTP 200, got {status} {body}"
            );
            for key in &keys {
                expected_stored.insert(key.clone(), "value".to_string());
            }
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: TagResource expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: TagResource expected ValidationException, got {} {body}",
                error_code(&body)
            );
            assert_eq!(
                merged_message,
                expected_message(&offenders),
                "case {case:?}: TagResource worded the refusal {merged_message:?}"
            );
            // The two operations take the same member, so the same map is refused
            // with the same sentence on both — and the sentence is the same for two
            // independently built maps, so its clause order comes from the keys and
            // not from however the map reached the handler.
            assert_eq!(
                merged_message, created_message,
                "case {case:?}: CreateCampaign and TagResource worded one refusal \
                 differently: {created_message:?} vs {merged_message:?}"
            );
        }

        // Nothing of a refused map is written — not even the admissible keys beside
        // the inadmissible one.
        let stored = stored_tags(&router, &host_arn, &case).await;
        assert_eq!(
            stored, expected_stored,
            "case {case:?}: expected the host to hold {expected_stored:?}, got {stored:?}"
        );
    }
}
