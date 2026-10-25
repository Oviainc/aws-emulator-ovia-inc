// SPDX-License-Identifier: CC0-1.0

//! Which endpoint owns a resource, end to end over the wire.
//!
//! Every case drives the real request path through the shared harness
//! (`tests/all/harness.rs`), signing each request for a chosen Region — the service
//! reads the Region out of the SigV4 credential scope, so a request signed for one
//! Region is a request that reached that Region's endpoint, and what these tests assert
//! is what a client of that endpoint sees.
//!
//! The rule under test: this service is REGIONAL, so a control-plane resource belongs to
//! the Region whose endpoint created it and to no other. Every operation resolves a
//! resource within the Region its request was signed for, so a resource is listed, read
//! and mutated through that endpoint alone, an id or an ARN presented to any other
//! Region names nothing there (the modelled `ResourceNotFoundException`, HTTP 404), and
//! each Region's service quota is its own.
//!
//! It is asserted over a TABLE of Regions rather than one pair, on BOTH resources this
//! service holds (feed and dictionary), and across EVERY operation that reaches one by
//! its id — the probes in [`feed_probes`] and [`dictionary_probes`].

use crate::harness::{
    call_in_region, listed_ids, minimal_feed, output_config, router, tags_path,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    MAX_DICTIONARIES, MAX_FEEDS, NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Regions these cases sign for.
///
/// The Region of a request is whatever its credential scope carries, so what the rule
/// generalises over is the Region VALUE: this table spans four Region codes from four
/// different geographies, only the first two of which the reported case used.
const REGIONS: [&str; 4] = ["us-west-2", "us-east-1", "eu-central-1", "ap-southeast-2"];

/// The `language` every dictionary below is created with — one value the
/// `DictionaryLanguage` enum admits (`smithy_query(query="shape",
/// shape="DictionaryLanguage")`: `eng`, `fra`, `ita`, `deu`, `spa`, `por`).
const LANGUAGE: &str = "deu";

/// The entries payload every dictionary below is created with, so a foreign-Region
/// export has something it must not be given.
const ENTRIES: &str = "Fussballstadion";

/// One request that reaches a resource by its id: what an operation is, as the wire
/// sees it.
struct Probe {
    label: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// Every operation of this service that reaches a FEED by its bare `FeedId`, with the
/// route the model binds it to (`OPERATIONS.md`, "Route").
///
/// `DeleteFeedPolicy` is left out: it is still scaffolded, so it answers 501 for every
/// id and asserts nothing about which Region holds a feed.
fn feed_probes(id: &str) -> Vec<Probe> {
    let replacement = json!({
        "name": "renamed-by-another-region",
        "outputs": [
            {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
        ]
    });
    vec![
        Probe {
            label: "GetFeed",
            method: "GET",
            path: format!("/v1/feed/{id}"),
            body: None,
        },
        Probe {
            label: "UpdateFeed",
            method: "PUT",
            path: format!("/v1/feed/{id}"),
            body: Some(replacement),
        },
        Probe {
            label: "DeleteFeed",
            method: "DELETE",
            path: format!("/v1/feed/{id}"),
            body: None,
        },
        Probe {
            label: "AssociateFeed",
            method: "POST",
            path: format!("/v1/feed/{id}/associate"),
            body: Some(json!({"associatedResourceName": "another-regions-media", "outputs": []})),
        },
        Probe {
            label: "DisassociateFeed",
            method: "POST",
            path: format!("/v1/feed/{id}/disassociate"),
            body: Some(json!({"associatedResourceName": "another-regions-media"})),
        },
        Probe {
            label: "GetFeedPolicy",
            method: "GET",
            path: format!("/v1/feed/{id}/policy"),
            body: None,
        },
        Probe {
            label: "PutFeedPolicy",
            method: "PUT",
            path: format!("/v1/feed/{id}/policy"),
            body: Some(json!({"policy": r#"{"Sid":"from-another-region"}"#})),
        },
    ]
}

/// Every operation of this service that reaches a DICTIONARY by its bare
/// `DictionaryId`, with the route the model binds it to.
fn dictionary_probes(id: &str) -> Vec<Probe> {
    vec![
        Probe {
            label: "GetDictionary",
            method: "GET",
            path: format!("/v1/dictionary/{id}"),
            body: None,
        },
        Probe {
            label: "UpdateDictionary",
            method: "PATCH",
            path: format!("/v1/dictionary/{id}"),
            body: Some(json!({"name": "renamed-by-another-region"})),
        },
        Probe {
            label: "DeleteDictionary",
            method: "DELETE",
            path: format!("/v1/dictionary/{id}"),
            body: None,
        },
        Probe {
            label: "ExportDictionaryEntries",
            method: "GET",
            path: format!("/v1/dictionary/{id}/entries/export"),
            body: None,
        },
    ]
}

/// The modelled not-found refusal, as every operation of this service answers it for an
/// id (or an ARN) that names no resource of the endpoint it was sent to.
fn assert_not_found(label: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {label:?}: expected HTTP 404, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceNotFoundException"),
        "case {label:?}: expected the modelled ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["message"],
        json!(NOT_FOUND_MESSAGE),
        "case {label:?}: expected this service's one not-found message, got {body}"
    );
}

/// Create one feed through `region`'s endpoint and return its id and ARN.
async fn create_feed_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (String, String) {
    let (status, created) =
        call_in_region(router, region, "POST", "/v1/feed", Some(minimal_feed(name))).await;
    assert_eq!(
        status, 202,
        "case {region:?}: CreateFeed({name}) is accepted, got {status} with {created}"
    );
    (
        created["id"]
            .as_str()
            .expect("a created feed carries an id")
            .to_string(),
        created["arn"]
            .as_str()
            .expect("a created feed carries an arn")
            .to_string(),
    )
}

/// Create one dictionary through `region`'s endpoint and return its id and ARN.
async fn create_dictionary_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (String, String) {
    let (status, created) = call_in_region(
        router,
        region,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": name, "language": LANGUAGE, "entries": ENTRIES})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {region:?}: CreateDictionary({name}) is accepted, got {status} with {created}"
    );
    (
        created["id"]
            .as_str()
            .expect("a created dictionary carries an id")
            .to_string(),
        created["arn"]
            .as_str()
            .expect("a created dictionary carries an arn")
            .to_string(),
    )
}

/// The ids `owner` minted among `owned`, sorted — what that Region's list must report.
fn ids_owned_by(owned: &[(&str, String, String)], owner: &str) -> Vec<String> {
    let mut ids: Vec<String> = owned
        .iter()
        .filter(|(region, _, _)| *region == owner)
        .map(|(_, id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// The Region field of an ARN (`arn:<partition>:<service>:<region>:…`).
fn arn_region(arn: &str) -> &str {
    arn.split(':').nth(3).expect("an ARN carries a region")
}

/// A feed belongs to the Region whose endpoint created it: only that endpoint lists it,
/// reports it and may change it.
#[tokio::test]
async fn a_feed_is_listed_read_and_mutated_only_through_its_own_region() {
    let router = router();

    // One feed per Region, all through the one emulator and the one shared store.
    let mut owned: Vec<(&str, String, String)> = Vec::new();
    for region in REGIONS {
        let (id, arn) = create_feed_in(&router, region, &format!("feed-{region}")).await;
        assert_eq!(
            arn_region(&arn),
            region,
            "case {region:?}: the minted ARN carries the Region of the endpoint that \
             created the feed, got {arn}"
        );
        owned.push((region, id, arn));
    }

    // Each endpoint reports its OWN feeds and no other Region's.
    for region in REGIONS {
        let listed = listed_ids(&router, region, "/v1/feeds", "feeds").await;
        let expected = ids_owned_by(&owned, region);
        assert_eq!(
            listed, expected,
            "case {region:?}: ListFeeds reports the feeds of this Region alone, expected \
             {expected:?}, got {listed:?}"
        );
    }

    for (owner, id, arn) in &owned {
        // Every id-based operation refuses this feed through every OTHER Region.
        for foreign in REGIONS.iter().filter(|region| *region != owner) {
            for probe in feed_probes(id) {
                let label = format!("{} on a {owner} feed, signed for {foreign}", probe.label);
                let (status, body) =
                    call_in_region(&router, foreign, probe.method, &probe.path, probe.body).await;
                assert_not_found(&label, status, &body);
            }
        }

        // ...and those refusals wrote nothing: the feed is still the one its own Region
        // created, with the name, the status and the (absent) policy it was created with.
        let (status, feed) =
            call_in_region(&router, owner, "GET", &format!("/v1/feed/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {owner:?}: the creating Region still reports its feed, got {status} with {feed}"
        );
        assert_eq!(
            feed["arn"],
            json!(arn),
            "case {owner:?}: the feed keeps the ARN its own Region minted, got {feed}"
        );
        assert_eq!(
            feed["name"],
            json!(format!("feed-{owner}")),
            "case {owner:?}: no other Region's update renamed it, got {feed}"
        );
        assert_eq!(
            feed["status"],
            json!("AVAILABLE"),
            "case {owner:?}: no other Region's delete or association moved its status, got {feed}"
        );
        let (status, policy) =
            call_in_region(&router, owner, "GET", &format!("/v1/feed/{id}/policy"), None).await;
        assert_eq!(
            status, 404,
            "case {owner:?}: no other Region's put attached a policy to it, got {status} with \
             {policy}"
        );
    }
}

/// A dictionary belongs to the Region whose endpoint created it, exactly as a feed does.
#[tokio::test]
async fn a_dictionary_is_listed_read_and_mutated_only_through_its_own_region() {
    let router = router();

    let mut owned: Vec<(&str, String, String)> = Vec::new();
    for region in REGIONS {
        let (id, arn) = create_dictionary_in(&router, region, &format!("dict-{region}")).await;
        assert_eq!(
            arn_region(&arn),
            region,
            "case {region:?}: the minted ARN carries the Region of the endpoint that \
             created the dictionary, got {arn}"
        );
        owned.push((region, id, arn));
    }

    for region in REGIONS {
        let listed = listed_ids(&router, region, "/v1/dictionaries", "dictionaries").await;
        let expected = ids_owned_by(&owned, region);
        assert_eq!(
            listed, expected,
            "case {region:?}: ListDictionaries reports the dictionaries of this Region alone, \
             expected {expected:?}, got {listed:?}"
        );
    }

    for (owner, id, arn) in &owned {
        for foreign in REGIONS.iter().filter(|region| *region != owner) {
            for probe in dictionary_probes(id) {
                let label =
                    format!("{} on a {owner} dictionary, signed for {foreign}", probe.label);
                let (status, body) =
                    call_in_region(&router, foreign, probe.method, &probe.path, probe.body).await;
                assert_not_found(&label, status, &body);
            }
        }

        let (status, dictionary) =
            call_in_region(&router, owner, "GET", &format!("/v1/dictionary/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {owner:?}: the creating Region still reports its dictionary, got {status} \
             with {dictionary}"
        );
        assert_eq!(
            dictionary["arn"],
            json!(arn),
            "case {owner:?}: the dictionary keeps the ARN its own Region minted, got {dictionary}"
        );
        assert_eq!(
            dictionary["name"],
            json!(format!("dict-{owner}")),
            "case {owner:?}: no other Region's update renamed it, got {dictionary}"
        );
        let (status, exported) = call_in_region(
            &router,
            owner,
            "GET",
            &format!("/v1/dictionary/{id}/entries/export"),
            None,
        )
        .await;
        assert_eq!(
            (status, &exported["entries"]),
            (200, &json!(ENTRIES)),
            "case {owner:?}: its own Region still exports the entries it was created with, got \
             {status} with {exported}"
        );
    }
}

/// A Region's feed quota is its own: the feeds of one Region neither consume nor are
/// consumed by another Region's.
#[tokio::test]
async fn a_regions_feed_quota_is_not_consumed_by_another_regions_feeds() {
    let router = router();
    let filled = REGIONS[0];

    for index in 0..MAX_FEEDS {
        let (_, _) = create_feed_in(&router, filled, &format!("quota{index}")).await;
    }
    let (status, refused) = call_in_region(
        &router,
        filled,
        "POST",
        "/v1/feed",
        Some(minimal_feed("past-the-quota")),
    )
    .await;
    assert_eq!(
        status, 402,
        "case {filled:?}: the create past this Region's quota is refused 402, got {status} \
         with {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ServiceQuotaExceededException"),
        "case {filled:?}: that 402 is the modelled ServiceQuotaExceededException, got {refused}"
    );

    // Every OTHER Region holds nothing, so its own quota is untouched.
    for region in REGIONS.iter().filter(|region| **region != filled) {
        let (id, _) = create_feed_in(&router, region, "first-of-this-region").await;
        let listed = listed_ids(&router, region, "/v1/feeds", "feeds").await;
        assert_eq!(
            listed,
            vec![id],
            "case {region:?}: this Region holds the one feed it created, got {listed:?}"
        );
    }
}

/// A Region's dictionary quota is its own, exactly as its feed quota is.
#[tokio::test]
async fn a_regions_dictionary_quota_is_not_consumed_by_another_regions_dictionaries() {
    let router = router();
    let filled = REGIONS[0];

    for index in 0..MAX_DICTIONARIES {
        let (_, _) = create_dictionary_in(&router, filled, &format!("quota{index}")).await;
    }
    let (status, refused) = call_in_region(
        &router,
        filled,
        "POST",
        "/v1/dictionary",
        Some(json!({"name": "past-the-quota", "language": LANGUAGE})),
    )
    .await;
    assert_eq!(
        status, 402,
        "case {filled:?}: the create past this Region's quota is refused 402, got {status} \
         with {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ServiceQuotaExceededException"),
        "case {filled:?}: that 402 is the modelled ServiceQuotaExceededException, got {refused}"
    );

    for region in REGIONS.iter().filter(|region| **region != filled) {
        let (id, _) = create_dictionary_in(&router, region, "first-of-this-region").await;
        let listed = listed_ids(&router, region, "/v1/dictionaries", "dictionaries").await;
        assert_eq!(
            listed,
            vec![id],
            "case {region:?}: this Region holds the one dictionary it created, got {listed:?}"
        );
    }
}

/// The ARN-based tagging path and the id-based path agree about which Region holds a
/// resource: the ARN that resolves is the one the creating Region minted, presented to
/// that Region.
#[tokio::test]
async fn tagging_resolves_a_resource_in_the_same_region_the_id_paths_do() {
    let router = router();

    for owner in REGIONS {
        let (feed_id, feed_arn) = create_feed_in(&router, owner, "tagged-feed").await;
        let (dictionary_id, dictionary_arn) =
            create_dictionary_in(&router, owner, "tagged-dict").await;

        for (kind, read_path, arn) in [
            ("feed", format!("/v1/feed/{feed_id}"), feed_arn),
            (
                "dictionary",
                format!("/v1/dictionary/{dictionary_id}"),
                dictionary_arn,
            ),
        ] {
            // Its own Region, its own ARN: the resource is there to be tagged.
            let (status, body) = call_in_region(
                &router,
                owner,
                "POST",
                &tags_path(&arn),
                Some(json!({"tags": {"owner": owner}})),
            )
            .await;
            assert_eq!(
                status, 204,
                "case {kind} in {owner:?}: its own Region tags it through its own ARN, got \
                 {status} with {body}"
            );

            // Any other Region, with that Region's ARN for the same id: nothing there.
            for foreign in REGIONS.iter().filter(|region| **region != owner) {
                let foreign_arn = arn.replacen(arn_region(&arn), foreign, 1);
                let label = format!("TagResource on a {owner} {kind}, signed for {foreign}");
                let (status, body) = call_in_region(
                    &router,
                    foreign,
                    "POST",
                    &tags_path(&foreign_arn),
                    Some(json!({"tags": {"owner": *foreign}})),
                )
                .await;
                assert_not_found(&label, status, &body);
            }

            // The resource carries exactly the tag its own Region wrote.
            let (status, stored) = call_in_region(&router, owner, "GET", &read_path, None).await;
            assert_eq!(
                (status, &stored["tags"]),
                (200, &json!({"owner": owner})),
                "case {kind} in {owner:?}: only its own Region's tag write landed, got {status} \
                 with {stored}"
            );
        }
    }
}
