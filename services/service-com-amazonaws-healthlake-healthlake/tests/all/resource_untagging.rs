// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `UntagResource` over EVERY resource form the
//! modelled `AmazonResourceName` admits: a FHIR data store ARN
//! (`datastore/fhir/.{32}`) and a data transformation profile ARN
//! (`dataTransformationProfile/[a-f0-9]{32}`).
//!
//! The rule under test: `UntagResource` removes from the resource its
//! `ResourceARN` names exactly the tags whose keys the request's `TagKeys`
//! lists, leaving every other tag and every other resource as they were; a key
//! the resource does not carry removes nothing, so the call is idempotent and
//! succeeds, and an ARN whose resource the service does not hold is refused with
//! `ResourceNotFoundException` and removes nothing. `ListTagsForResource` on the
//! same ARN reports the remaining set.
//!
//! The rows differ along what the rule generalises over — how many keys a
//! request names, whether each key is stored or absent, whether it repeats, and
//! the characters and lengths the key holds — with `TagResource` and
//! `ListTagsForResource` (`tests/all/resource_tagging.rs`,
//! `tests/all/list_tags_for_resource.rs`) used to seed and to read back.
//!
//! Every call below goes through [`crate::fhir_datastore_lifecycle::HealthLake`],
//! i.e. `MultiServiceRouter::dispatch` — the same entry point `core/router`
//! serves — so routing, deserialization, the modelled constraints and the error
//! envelope are all exercised, not bypassed.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    DATASTORE_NOT_FOUND_MESSAGE, profile_not_found_message,
};

use crate::fhir_datastore_lifecycle::{error_code, HealthLake};
use crate::resource_tagging::{create_datastore, create_profile, profile_arn, tag_list};

/// A key at the modelled `TagKey` maximum (len 1..128).
fn longest_key() -> String {
    "k".repeat(128)
}

/// The tag set every row starts from, rewritten before each row so the rows are
/// independent of each other.
///
/// The keys span the modelled `TagKey` alphabet (pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) and its published maximum length, and one
/// value is the empty string the modelled `TagValue` (len 0..256) admits.
fn seed_pairs() -> Vec<(String, String)> {
    vec![
        ("env".to_string(), "test".to_string()),
        ("owner".to_string(), "platform".to_string()),
        ("team".to_string(), "clinical".to_string()),
        ("cost.center".to_string(), "42".to_string()),
        ("lab:unit/x=1+y-2@z".to_string(), "ward 3".to_string()),
        ("клиника".to_string(), "ünïcode".to_string()),
        ("empty.value".to_string(), String::new()),
        (longest_key(), "v".repeat(256)),
    ]
}

/// One removal request: the label it reports itself under and the `TagKeys` it
/// names.
struct Removal {
    case: &'static str,
    keys: Vec<String>,
}

/// The removal requests under test, each applied to the SAME seed set.
///
/// `TagKeys` is the modelled `TagKeyList` (len 0..200), so the rows run from the
/// empty list the model admits up to a request at that published maximum.
fn removals() -> Vec<Removal> {
    let seed: Vec<String> = seed_pairs().into_iter().map(|(key, _)| key).collect();
    // 200 keys, the modelled `TagKeyList` maximum: two keys the resource
    // carries and 198 it does not.
    let at_list_maximum: Vec<String> = seed[..2]
        .iter()
        .cloned()
        .chain((0..198).map(|index| format!("absent-{index:03}")))
        .collect();
    vec![
        Removal {
            case: "an empty key list removes nothing",
            keys: vec![],
        },
        Removal {
            case: "one stored key",
            keys: vec!["env".to_string()],
        },
        Removal {
            case: "one key the resource does not carry",
            keys: vec!["never.stored".to_string()],
        },
        Removal {
            case: "several stored keys in one call",
            keys: vec!["env".to_string(), "team".to_string(), "owner".to_string()],
        },
        Removal {
            case: "stored and absent keys mixed in one call",
            keys: vec![
                "cost.center".to_string(),
                "never.stored".to_string(),
                "owner".to_string(),
            ],
        },
        Removal {
            case: "the same stored key twice in one call",
            keys: vec!["env".to_string(), "env".to_string()],
        },
        Removal {
            case: "keys across the modelled TagKey alphabet, including a non-Latin one",
            keys: vec![
                "lab:unit/x=1+y-2@z".to_string(),
                "клиника".to_string(),
                "empty.value".to_string(),
            ],
        },
        Removal {
            case: "a key at the modelled 128-character maximum",
            keys: vec![longest_key()],
        },
        Removal {
            case: "every stored key, emptying the set",
            keys: seed_pairs().into_iter().map(|(key, _)| key).collect(),
        },
        Removal {
            case: "a key list at the modelled 200-key maximum",
            keys: at_list_maximum,
        },
    ]
}

/// The harness: the shared router plus the tagging calls each row drives.
struct Untagging {
    service: HealthLake,
}

impl Untagging {
    fn new() -> Self {
        Self {
            service: HealthLake::new(),
        }
    }

    /// `TagResource` with `pairs`, which must succeed.
    async fn tag(&self, case: &str, arn: &str, pairs: &[(String, String)]) {
        let tags = tag_list(
            &pairs
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<(&str, &str)>>(),
        );
        let (status, payload) = self
            .service
            .call("TagResource", &json!({ "ResourceARN": arn, "Tags": tags }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource on {arn:?} must answer 200, got {status}: {payload}"
        );
    }

    /// `UntagResource` with `keys`; returns `(status, payload)`.
    async fn untag(&self, arn: &str, keys: &[String]) -> (u16, Value) {
        self.service
            .call(
                "UntagResource",
                &json!({ "ResourceARN": arn, "TagKeys": keys }),
            )
            .await
    }

    /// `UntagResource` with `keys`, which must succeed with an empty response.
    async fn untag_ok(&self, case: &str, arn: &str, keys: &[String]) {
        let (status, payload) = self.untag(arn, keys).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource on {arn:?} must answer 200, got {status}: {payload}"
        );
        assert_eq!(
            payload,
            json!({}),
            "case {case:?}: UntagResource's response carries no members, got {payload}"
        );
    }

    /// The tags `ListTagsForResource` reports for `arn`, as a comparable map.
    async fn listed_tags(&self, case: &str, arn: &str) -> BTreeMap<String, String> {
        let payload = self
            .service
            .ok("ListTagsForResource", &json!({ "ResourceARN": arn }))
            .await;
        crate::list_tags_for_resource::reported_tags(case, &payload)
    }
}

/// The two ARN forms the modelled `AmazonResourceName` admits, each addressing a
/// live resource of this emulator's own minting.
async fn live_resources(harness: &Untagging) -> Vec<(&'static str, String)> {
    let datastore_arn = create_datastore(&harness.service, "untagged store", None).await;
    let profile_id = create_profile(&harness.service, "untagged profile", None).await;
    let profile = profile_arn(&datastore_arn, &profile_id);
    vec![
        ("a FHIR data store ARN", datastore_arn),
        ("a data transformation profile ARN", profile),
    ]
}

#[tokio::test]
async fn named_keys_are_removed_from_whichever_resource_the_resource_arn_names() {
    let harness = Untagging::new();
    let resources = live_resources(&harness).await;

    for (resource_case, arn) in &resources {
        for removal in removals() {
            let case = format!("{resource_case} / {}", removal.case);
            // Rewrite the whole seed set, so each row starts from the same
            // known state whatever the row before it removed.
            let seed = seed_pairs();
            harness.tag(&case, arn, &seed).await;
            let before = harness.listed_tags(&case, arn).await;
            let seeded: BTreeMap<String, String> = seed.iter().cloned().collect();
            assert_eq!(
                before, seeded,
                "case {case:?}: the seed set must be stored whole, got {before:?}"
            );

            let mut expected = seeded;
            for key in &removal.keys {
                expected.remove(key);
            }

            harness.untag_ok(&case, arn, &removal.keys).await;
            let actual = harness.listed_tags(&case, arn).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: removing {:?} must leave the rest, got {actual:?}",
                removal.keys
            );

            // Removal is idempotent: the same request again answers the same
            // way and leaves the same set.
            harness
                .untag_ok(&format!("{case} / repeated"), arn, &removal.keys)
                .await;
            let repeated = harness.listed_tags(&case, arn).await;
            assert_eq!(
                repeated, expected,
                "case {case:?}: a repeated removal must change nothing, got {repeated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_removal_touches_only_the_resource_its_arn_names() {
    let harness = Untagging::new();
    let resources = live_resources(&harness).await;
    let shared = vec![
        ("env".to_string(), "test".to_string()),
        ("owner".to_string(), "platform".to_string()),
    ];
    for (resource_case, arn) in &resources {
        harness.tag(resource_case, arn, &shared).await;
    }

    // Remove one key through the FIRST ARN form only.
    harness
        .untag_ok(
            "remove env from the data store",
            &resources[0].1,
            &["env".to_string()],
        )
        .await;

    let datastore_tags = harness.listed_tags("the data store", &resources[0].1).await;
    assert_eq!(
        datastore_tags,
        [("owner".to_string(), "platform".to_string())].into(),
        "the data store must have lost exactly the removed key, got {datastore_tags:?}"
    );
    let profile_tags = harness.listed_tags("the profile", &resources[1].1).await;
    assert_eq!(
        profile_tags,
        shared.iter().cloned().collect::<BTreeMap<_, _>>(),
        "the profile must keep every tag of its own, got {profile_tags:?}"
    );

    // A removed key is writable again, and comes back with its new value: the
    // removal took the tag, not the resource's ability to carry that key.
    harness
        .tag(
            "re-tag the removed key",
            &resources[0].1,
            &[("env".to_string(), "prod".to_string())],
        )
        .await;
    let retagged = harness.listed_tags("the data store", &resources[0].1).await;
    assert_eq!(
        retagged.get("env").map(String::as_str),
        Some("prod"),
        "a removed key must be writable again, got {retagged:?}"
    );
}

#[tokio::test]
async fn untagging_an_arn_whose_resource_is_not_held_is_refused_and_removes_nothing() {
    let harness = Untagging::new();
    let datastore_arn = create_datastore(
        &harness.service,
        "live store",
        Some(&tag_list(&[("owner", "platform")])),
    )
    .await;
    let profile_id = create_profile(
        &harness.service,
        "live profile",
        Some(&json!({ "owner": "platform" })),
    )
    .await;
    let live_profile_arn = profile_arn(&datastore_arn, &profile_id);

    // Every row satisfies the modelled `AmazonResourceName` pattern, so the
    // boundary validator passes it and the handler decides. Rows span the ways
    // an ARN can name a resource this service does not hold, in EITHER resource
    // form, each paired with the sentence of the resource it names.
    let fields: Vec<&str> = datastore_arn.split(':').collect();
    let (region, account, datastore_resource) = (fields[3], fields[4], fields[5]);
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a profile id nothing was minted under",
            profile_arn(&datastore_arn, "0123456789abcdef0123456789abcdef"),
            profile_not_found_message("0123456789abcdef0123456789abcdef"),
        ),
        (
            "the live profile id in another region",
            format!(
                "arn:aws:healthlake:eu-west-1:{account}:dataTransformationProfile/{profile_id}"
            ),
            profile_not_found_message(&profile_id),
        ),
        (
            "the live profile id in another account",
            format!(
                "arn:aws:healthlake:{region}:210987654321:dataTransformationProfile/{profile_id}"
            ),
            profile_not_found_message(&profile_id),
        ),
        (
            "a data store id nothing was minted under",
            format!(
                "arn:aws:healthlake:{region}:{account}:datastore/fhir/\
                 00000000000000000000000000000000"
            ),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "the live data store in another partition",
            format!("arn:aws-us-gov:healthlake:{region}:{account}:{datastore_resource}"),
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    for (case, arn, expected_message) in &rows {
        // The keys of the live records, so a refusal that reached state would
        // be visible as a lost tag below.
        let (status, payload) = harness.untag(arn, &["owner".to_string()]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: UntagResource must answer 400, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: UntagResource must answer ResourceNotFoundException, got {payload}"
        );
        let message = payload.get("Message").and_then(Value::as_str);
        assert_eq!(
            message,
            Some(expected_message.as_str()),
            "case {case:?}: UntagResource must state {expected_message:?}, got {message:?}"
        );
        assert!(
            !payload.to_string().contains(arn.as_str()),
            "case {case:?}: UntagResource must quote no ARN from the request, got {payload}"
        );
    }

    // The live records kept every tag through every refusal above.
    let expected: BTreeMap<String, String> =
        [("owner".to_string(), "platform".to_string())].into();
    let profile_tags = harness
        .listed_tags("the live profile", &live_profile_arn)
        .await;
    assert_eq!(
        profile_tags, expected,
        "the live profile must still report its own tags, got {profile_tags:?}"
    );
    let datastore_tags = harness
        .listed_tags("the live data store", &datastore_arn)
        .await;
    assert_eq!(
        datastore_tags, expected,
        "the live data store must still report its own tags, got {datastore_tags:?}"
    );
}
