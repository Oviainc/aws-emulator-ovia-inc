// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UntagResource` over EVERY kind of taggable resource this service
//! holds, with the `ListTagsForResource` reader that reports what the removal left behind.
//!
//! The rule under test: a tagging operation addresses a resource by the ARN that resource is
//! STORED under, whatever kind of resource of this service the ARN names — so `UntagResource`
//! detaches exactly the keys the request names from any live resource (a service level
//! objective and a dynamic instrumentation configuration alike), leaves that resource's other
//! tags and the resource itself alone, and answers HTTP 200 with the empty body its modelled
//! output (no members, `smithy_query(query="operation",operation="UntagResource")`) serialises
//! to; the modelled `ResourceNotFoundException` (404) is reserved for a `ResourceArn` that
//! names no resource this service still holds, and that refusal detaches nothing.
//!
//! Every assertion is made over a SET of resources and key lists rather than on one request:
//! the resource table is the configuration table `instrumentation_configuration_tags` already
//! drives (reused rather than re-derived, so both suites speak of one set of configurations)
//! plus an SLO, and the key table spans the shapes a `TagKeys` list takes. The SLO-only side
//! of the same rule lives in `resource_tags.rs`.

use std::collections::BTreeMap;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::{
    instrumentation_key, ComAmazonawsApplicationsignalsApplicationsignalsState,
    InstrumentationScope,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_tags::{
    cases, created, delete_configuration, get_configuration, router_with_engine, Case,
    CONFIGURATION_RESOURCE_TYPE, SNAPSHOT,
};
use crate::resource_tags::{
    expected_map, list_tags_raw, reported_tags, slo_arn, tag, tag_list, untag, SLO_RESOURCE_TYPE,
};
// The tagging plane's own not-found wording, owned by the suite that holds this service's
// refusal prose.
use crate::refusal_prose::tagged_resource_missing;
use crate::slo_lifecycle::{arn_uri_segment, call, create, sli_config, ACCOUNT, REGION};

/// A request that proves the resource a removal addressed is still served afterwards: the tags
/// were addressed, not the resource.
#[derive(Clone, Debug)]
enum Probe {
    /// `GET /slo/<name>`.
    Slo(String),
    /// `POST /get-instrumentation-configuration` with this body.
    Configuration(Value),
}

/// One taggable resource of this service, as it is stored: the ARN its writer minted, the tags
/// that writer persisted, and the read that proves it survived.
#[derive(Clone, Debug)]
struct Resource {
    label: String,
    /// The ARN the service answered for the stored resource, never one the test derived from
    /// the request.
    arn: String,
    tags: BTreeMap<String, String>,
    probe: Probe,
}

/// Read `resource` back through its own reader.
async fn read_back(router: &MultiServiceRouter, resource: &Resource) -> (u16, Value) {
    match &resource.probe {
        Probe::Slo(name) => call(router, "GET", &format!("/slo/{name}"), None).await,
        Probe::Configuration(body) => get_configuration(router, body.clone()).await,
    }
}

/// The tags the SLO row is created with. Three keys, so a row that removes "the first" or
/// "every other" key leaves something behind to assert on.
fn slo_tags() -> Vec<(String, String)> {
    vec![
        tag("team", "checkout"),
        tag("env", "prod"),
        tag("tier", "gold"),
    ]
}

/// The name of the SLO row, which is also what its ARN spells.
const SLO_NAME: &str = "untagged-slo";

/// Create one resource of EVERY taggable kind this service holds on `router`, and describe each
/// by the ARN the service minted for it.
///
/// The configuration rows are the ones `instrumentation_configuration_tags::cases` spans (both
/// instrumentation types, the whole `ProgrammingLanguage` catalogue, environments carrying the
/// `:` and `/` the ARN doc calls out, and tag sets from none through the modelled `TagList`
/// maximum); the SLO row is the service's other taggable kind, so one key list is asserted
/// against one rule over both.
async fn stage_resources(router: &MultiServiceRouter) -> Vec<Resource> {
    let mut resources = Vec::new();

    let tags = slo_tags();
    let (status, body) = create(
        router,
        json!({
            "Name": SLO_NAME,
            "SliConfig": sli_config("checkout", "GET /cart", 100.0),
            "Tags": tag_list(&tags),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a service level objective\": expected HTTP 200 from the setup create, \
         got {status} with {body}"
    );
    resources.push(Resource {
        label: "a service level objective".to_string(),
        arn: slo_arn(SLO_NAME),
        tags: expected_map(&tags),
        probe: Probe::Slo(SLO_NAME.to_string()),
    });

    for case in cases() {
        let (arn, hash) = created(router, &case).await;
        resources.push(Resource {
            label: format!("an instrumentation configuration: {}", case.label),
            arn,
            tags: case.expected(),
            probe: Probe::Configuration(case.get_body(&hash)),
        });
    }

    resources
}

/// One row of the key table: the `TagKeys` a removal names, derived from the tags the resource
/// carries so the same row applies to every resource of the table.
struct KeyCase {
    label: &'static str,
    keys: fn(&BTreeMap<String, String>) -> Vec<String>,
}

/// The key lists the rule is asserted over.
///
/// The rows differ along what the rule generalises over: how MANY keys the request names (none,
/// one, some, all), whether each named key is one the resource carries, and whether a key is
/// named more than once — the modelled `TagKeys` is a `TagKeyList` of `length 0..200`
/// (`smithy_query(query="operation",operation="UntagResource")`), so naming no key at all is a
/// request the model admits.
fn key_cases() -> Vec<KeyCase> {
    vec![
        KeyCase {
            label: "the empty TagKeys list the model admits",
            keys: |_| Vec::new(),
        },
        KeyCase {
            label: "the first key the resource carries",
            keys: |tags| tags.keys().next().cloned().into_iter().collect(),
        },
        KeyCase {
            label: "the last key the resource carries",
            keys: |tags| tags.keys().next_back().cloned().into_iter().collect(),
        },
        KeyCase {
            label: "every other key the resource carries",
            keys: |tags| {
                tags.keys()
                    .enumerate()
                    .filter(|(index, _)| index % 2 == 0)
                    .map(|(_, key)| key.clone())
                    .collect()
            },
        },
        KeyCase {
            label: "every key the resource carries",
            keys: |tags| tags.keys().cloned().collect(),
        },
        KeyCase {
            label: "only keys the resource never carried",
            keys: |_| vec!["never-attached".to_string(), "ghost".to_string()],
        },
        KeyCase {
            label: "a key it carries and one it never carried",
            keys: |tags| {
                tags.keys()
                    .next()
                    .cloned()
                    .into_iter()
                    .chain(["never-attached".to_string()])
                    .collect()
            },
        },
        KeyCase {
            label: "the same key named twice",
            keys: |tags| {
                let first = tags.keys().next().cloned();
                first.iter().cloned().chain(first.iter().cloned()).collect()
            },
        },
    ]
}

/// The tags `resource` must carry once `keys` are detached: removal is per KEY, so exactly the
/// named keys are gone and every other tag keeps its value.
fn remaining(resource: &Resource, keys: &[String]) -> BTreeMap<String, String> {
    resource
        .tags
        .iter()
        .filter(|(key, _)| !keys.contains(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// A removal detaches exactly the keys the request names from the resource its ARN names —
/// whichever kind of resource of this service that is — answers HTTP 200 with the empty body,
/// and leaves the resource itself served.
///
/// Each row is driven three times: removal is per KEY, so the result of a call is "none of these
/// keys is attached any more", which makes a repeat answer exactly as the first call did and
/// change nothing further.
#[tokio::test]
async fn untag_detaches_the_named_keys_from_every_kind_of_resource() {
    for key_case in key_cases() {
        // A fresh store per key row, so each row starts from the tags the creates persisted.
        let (router, _engine) = router_with_engine();
        for resource in stage_resources(&router).await {
            let keys = (key_case.keys)(&resource.tags);
            let expected = remaining(&resource, &keys);
            let escaped = arn_uri_segment(&resource.arn);

            for attempt in 1..=3 {
                let (status, body) = untag(&router, &resource.arn, &keys).await;
                assert_eq!(
                    status, 200,
                    "case {:?} / {:?} attempt {attempt}: expected HTTP 200 from the untag of {}, \
                     got {status} with {body}",
                    resource.label, key_case.label, resource.arn
                );
                assert_eq!(
                    body,
                    json!({}),
                    "case {:?} / {:?} attempt {attempt}: expected the untag to answer the empty \
                     body {{}}, got {body}",
                    resource.label,
                    key_case.label
                );

                let (status, after) = list_tags_raw(&router, &escaped).await;
                assert_eq!(
                    status, 200,
                    "case {:?} / {:?} attempt {attempt}: expected HTTP 200 from the post-untag \
                     read, got {status} with {after}",
                    resource.label, key_case.label
                );
                assert_eq!(
                    reported_tags(&after),
                    expected,
                    "case {:?} / {:?} attempt {attempt}: expected the remaining tags \
                     {expected:?}, got {:?}",
                    resource.label,
                    key_case.label,
                    reported_tags(&after)
                );

                let (status, served) = read_back(&router, &resource).await;
                assert_eq!(
                    status, 200,
                    "case {:?} / {:?} attempt {attempt}: expected the resource itself to survive \
                     the untag, got {status} with {served}",
                    resource.label, key_case.label
                );
            }
        }
    }
}

/// One row of the refusal table: an ARN that names no resource this service holds, and the
/// resource type the refusal must report for it (`None` where the ARN addresses neither kind of
/// this service, so the type it is reported under is not part of the rule).
struct MissingCase {
    label: String,
    resource_arn: String,
    resource_type: Option<&'static str>,
}

/// A `ResourceArn` that names no resource this service still holds is refused with the modelled
/// `ResourceNotFoundException` (404) identifying the ARN the request carried, and the refusal
/// detaches nothing from the resources that ARE stored.
///
/// The rows differ along WHY the ARN names nothing: a configuration location nothing was
/// created at, a configuration that was deleted, one whose documented lifetime the clock has
/// ended, an SLO that was never created, another account's configuration, another service's ARN
/// and a bare resource name (which is no ARN at all, and `ResourceArn` is modelled as an
/// `AmazonResourceName`).
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_refused_and_detaches_nothing() {
    let (router, engine) = router_with_engine();
    let live = Case {
        tags: Some(vec![tag("team", "checkout"), tag("env", "prod")]),
        ..cases()
            .into_iter()
            .next()
            .expect("the configuration table holds at least one row")
    };
    let (live_arn, live_hash) = created(&router, &live).await;

    // A configuration created and then deleted: its ARN named a resource once.
    let mut removed = live.clone();
    removed.label = "a deleted configuration";
    removed.location = json!({
        "Language": "Java",
        "ClassName": "CartService",
        "FilePath": "src/main/java/com/example/CartService.java",
        "LineNumber": 7,
    });
    let (removed_arn, removed_hash) = created(&router, &removed).await;
    let (status, body) = delete_configuration(&router, removed.get_body(&removed_hash)).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the setup delete, got {status} with {body}"
    );

    // A configuration whose expiry the clock has passed, staged on the store the handlers share
    // through the very method the create path writes with, since the documented lifetime floor
    // outlasts any test.
    let mut lapsing = live.clone();
    lapsing.label = "a lapsed configuration";
    lapsing.location = json!({
        "Language": "Java",
        "ClassName": "CartService",
        "FilePath": "src/main/java/com/example/CartService.java",
        "LineNumber": 4242,
    });
    let (lapsed_arn, lapsed_hash) = created(&router, &lapsing).await;
    let scope = InstrumentationScope::new(
        lapsing.instrumentation_type,
        lapsing.service,
        lapsing.environment,
    );
    let key = instrumentation_key(SNAPSHOT, &lapsed_hash);
    let lapsed_at = DateTime::from_secs_f64(
        DateTime::from(std::time::SystemTime::now())
            .as_secs_f64()
            .trunc()
            - 1.0,
    );
    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let mut store = state.lock();
        let mut record = store
            .instrumentation_configuration(&scope, &key)
            .expect("the create stored the configuration under its identity")
            .clone();
        record.expires_at = Some(lapsed_at);
        store.put_instrumentation_configuration(scope, record, lapsed_at);
    }

    let rows = vec![
        MissingCase {
            label: "a location nothing was created at".to_string(),
            resource_arn: live_arn.replace(&live_hash, &"0".repeat(16)),
            resource_type: Some(CONFIGURATION_RESOURCE_TYPE),
        },
        MissingCase {
            label: "a deleted configuration".to_string(),
            resource_arn: removed_arn,
            resource_type: Some(CONFIGURATION_RESOURCE_TYPE),
        },
        MissingCase {
            label: "a configuration whose lifetime the clock ended".to_string(),
            resource_arn: lapsed_arn,
            resource_type: Some(CONFIGURATION_RESOURCE_TYPE),
        },
        MissingCase {
            label: "another account's configuration".to_string(),
            resource_arn: live_arn.replace(ACCOUNT, "111111111111"),
            resource_type: Some(CONFIGURATION_RESOURCE_TYPE),
        },
        MissingCase {
            label: "an SLO that was never created".to_string(),
            resource_arn: slo_arn("absent-slo"),
            resource_type: Some(SLO_RESOURCE_TYPE),
        },
        MissingCase {
            label: "another service's resource".to_string(),
            resource_arn: format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:alarm/my-alarm"),
            resource_type: None,
        },
        MissingCase {
            label: "a bare resource name, not an ARN".to_string(),
            resource_arn: SLO_NAME.to_string(),
            resource_type: None,
        },
    ];

    for row in rows {
        let (status, error) = untag(&router, &row.resource_arn, &["team".to_string()]).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected HTTP 404 for {}, got {status} with {error}",
            row.label, row.resource_arn
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {:?}: expected a ResourceNotFoundException, got {error}",
            row.label
        );
        assert_eq!(
            error["ResourceId"].as_str(),
            Some(row.resource_arn.as_str()),
            "case {:?}: expected the refusal to name the ARN the request carried, got {error}",
            row.label
        );
        // `ResourceType` is `@required` on `ResourceNotFoundException`, so every refusal
        // reports one; which type an ARN addressing neither kind of this service is
        // reported under is not part of the rule, so those rows assert only that it is
        // there.
        let reported_type = error["ResourceType"].as_str().unwrap_or_default();
        assert!(
            !reported_type.is_empty(),
            "case {:?}: expected the refusal to report a ResourceType, got {error}",
            row.label
        );
        if let Some(resource_type) = row.resource_type {
            assert_eq!(
                reported_type, resource_type,
                "case {:?}: expected the type the ARN addresses, got {error}",
                row.label
            );
        }
        // The MESSAGE is the tagging plane's own: it names the resource generically,
        // whichever kind the ARN addresses, and echoes the ARN the request carried.
        assert_eq!(
            error["Message"].as_str(),
            Some(tagged_resource_missing(&row.resource_arn).as_str()),
            "case {:?}: expected the tagging plane's generic wording, got {error}",
            row.label
        );

        // The refused removal wrote nothing: the live configuration still carries its tags.
        let (status, after) = list_tags_raw(&router, &arn_uri_segment(&live_arn)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the live configuration to still answer 200, \
             got {status} with {after}",
            row.label
        );
        assert_eq!(
            reported_tags(&after),
            live.expected(),
            "case {:?}: expected a refused removal to detach nothing, got {:?}",
            row.label,
            reported_tags(&after)
        );
    }
}
