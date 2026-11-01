// SPDX-License-Identifier: CC0-1.0

//! Which SCOPE a minted CHILD resource is addressed in, over the wire (the harness
//! `suite_lifecycle` owns).
//!
//! The rule under test: a resource a call mints under an existing parent is addressed in
//! the scope the PARENT's stored ARN records — its partition, region and account — and not
//! in the scope of the request that happened to mint it, so every regional value the child
//! reports (its ARN and the endpoint it connects through) names the one scope its parent is
//! in, and the one ARN form that addresses the parent's scope addresses the child too.
//!
//! The only parent/child pair this service mints is a suite run of a suite definition (a
//! run's resource portion is `suiterun/<suiteDefinitionId>/<suiteRunId>`, so it is not
//! addressable apart from its definition — source: the two resource forms
//! `state::parse_resource_arn` accepts), so the table below is the CROSS PRODUCT of the
//! region a definition is created in with the region its run is started from: the same
//! region, the service's fallback region on either side, and regions of the other published
//! shapes.

use serde_json::{json, Value};

use super::endpoint::percent_encode;
use super::suite_lifecycle::{thing_arn, Answer, Emulator, ROLE_ARN};

/// The regions every case is built from: `us-east-1` (the region an unsigned request falls
/// back to), plus one of each other published region shape — a four-label region, a
/// two-letter prefix with a single middle label, and a US GovCloud region.
const REGIONS: &[&str] = &[
    "us-east-1",
    "eu-central-1",
    "ap-southeast-4",
    "il-central-1",
    "us-gov-west-1",
];

/// The status a successful tag mutation answers with: 204 No Content, as AWS answers an
/// empty-output tag mutation.
const TAG_MUTATION_STATUS: u16 = 204;

/// The five scope fields an ARN opens with — `arn:<partition>:<service>:<region>:<account>:`
/// — the separator that closes them included, so an ARN in the same scope is this followed
/// by a resource portion.
fn scope_of(arn: &str) -> &str {
    let (account_end, _) = arn
        .match_indices(':')
        .nth(4)
        .unwrap_or_else(|| panic!("{arn:?} does not carry the five ARN scope fields"));
    &arn[..=account_end]
}

/// The region field of `arn`.
fn region_of(arn: &str) -> &str {
    arn.split(':')
        .nth(3)
        .unwrap_or_else(|| panic!("{arn:?} does not carry an ARN region field"))
}

/// `arn` with its region field replaced by `region`: the same resource id named in ANOTHER
/// scope, which is what a client deriving the ARN from the wrong region would send.
fn in_region(arn: &str, region: &str) -> String {
    let mut fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(fields.len(), 6, "{arn:?} is not an ARN");
    fields[3] = region;
    fields.join(":")
}

/// A suite definition configuration named `name`.
fn configuration(name: &str) -> Value {
    json!({
        "suiteDefinitionName": name,
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
    })
}

/// `StartSuiteRun` of `definition_id`, signed for `region`, carrying `tags` when any.
async fn start_run(
    emulator: &Emulator,
    definition_id: &str,
    region: &str,
    tags: &Value,
) -> Answer {
    let mut body = json!({
        "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing_arn(3) } }
    });
    if !tags.as_object().is_some_and(serde_json::Map::is_empty) {
        body["tags"] = tags.clone();
    }
    emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            region,
            Some(body),
        )
        .await
}

/// `ListTagsForResource` for `arn`, signed for `region`.
async fn list_tags(emulator: &Emulator, arn: &str, region: &str) -> Answer {
    emulator
        .call("GET", &format!("/tags/{}", percent_encode(arn)), region, None)
        .await
}

/// A suite run is minted in the scope its DEFINITION's ARN records, whatever region the
/// start call is signed for: its ARN, the endpoint it reports, and the ARN it is addressable
/// by all name that one scope.
#[tokio::test]
async fn a_suite_run_is_minted_in_its_definitions_scope() {
    let emulator = Emulator::new();

    for definition_region in REGIONS {
        for start_region in REGIONS {
            let case = format!("definition in {definition_region}, started from {start_region}");

            let created = emulator
                .create_definition(
                    &configuration(&format!("scope-suite-{definition_region}-{start_region}")),
                    definition_region,
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
                created.status, created.body
            );
            let definition_id = created.string("suiteDefinitionId");
            let definition_arn = created.string("suiteDefinitionArn");
            assert_eq!(
                region_of(&definition_arn),
                *definition_region,
                "case {case:?}: the definition's own ARN must name the region it was created \
                 in, got {definition_arn:?}"
            );

            let started = start_run(&emulator, &definition_id, start_region, &json!({})).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected HTTP 200 from StartSuiteRun, got {} {}",
                started.status, started.body
            );
            let run_id = started.string("suiteRunId");
            let run_arn = started.string("suiteRunArn");
            assert_eq!(
                run_arn,
                format!("{}suiterun/{definition_id}/{run_id}", scope_of(&definition_arn)),
                "case {case:?}: the run must be minted in its definition's scope, got \
                 {run_arn:?} beside definition {definition_arn:?}"
            );

            // The endpoint the run reports is a host of the run's OWN region, which is its
            // definition's: a run never announces a host in a region its ARN says it does
            // not live in.
            let endpoint = started.string("endpoint");
            let suffix = format!(".deviceadvisor.iot.{definition_region}.amazonaws.com");
            assert!(
                endpoint.ends_with(&suffix),
                "case {case:?}: the run's endpoint must name its own region, expected a host \
                 ending in {suffix:?}, got {endpoint:?}"
            );

            // Round trip: the reader answers the stored ARN, read from either region.
            let run_path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}");
            for reading_region in [*start_region, *definition_region] {
                let fetched = emulator.call("GET", &run_path, reading_region, None).await;
                assert_eq!(
                    fetched.status, 200,
                    "case {case:?}: expected HTTP 200 from GetSuiteRun read in \
                     {reading_region:?}, got {} {}",
                    fetched.status, fetched.body
                );
                assert_eq!(
                    fetched.string("suiteRunArn"),
                    run_arn,
                    "case {case:?}: GetSuiteRun read in {reading_region:?} must answer the \
                     stored ARN, got {}",
                    fetched.string("suiteRunArn")
                );
            }

            // The run is addressable by the ARN form its definition's scope gives it …
            let tagged = emulator
                .call(
                    "POST",
                    &format!("/tags/{}", percent_encode(&run_arn)),
                    start_region,
                    Some(json!({ "tags": { "scope": "inherited" } })),
                )
                .await;
            assert_eq!(
                tagged.status, TAG_MUTATION_STATUS,
                "case {case:?}: tagging the run under its own ARN must answer \
                 {TAG_MUTATION_STATUS}, got {} {}",
                tagged.status, tagged.body
            );
            let listed = list_tags(&emulator, &run_arn, start_region).await;
            assert_eq!(
                listed.member("tags"),
                &json!({ "scope": "inherited" }),
                "case {case:?}: the run's tags must read back under its own ARN, got {}",
                listed.body
            );

            // … and by NO other: the same ids in another region name a resource of that
            // other scope, which this store does not hold.
            for other_region in REGIONS.iter().filter(|region| **region != *definition_region) {
                let other_arn = in_region(&run_arn, other_region);
                let refused = list_tags(&emulator, &other_arn, start_region).await;
                assert_eq!(
                    (refused.status, refused.error_code.as_deref()),
                    (404, Some("ResourceNotFoundException")),
                    "case {case:?}: the run must not be addressable in {other_region:?}, got \
                     {} {}",
                    refused.status,
                    refused.body
                );
            }
        }
    }
}

/// The tags a start call carries are keyed by the run's ARN in its DEFINITION's scope, so
/// the run reports them and they are readable under that one ARN — including when the start
/// was signed for another region.
#[tokio::test]
async fn tags_written_by_a_start_follow_the_runs_inherited_scope() {
    let emulator = Emulator::new();

    // Rows differing in where the parent lives relative to the caller, and in the size of
    // the tag map the start carries (none, one, several).
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        ("same region, no tags", "us-east-1", "us-east-1", json!({})),
        (
            "same region, one tag",
            "eu-central-1",
            "eu-central-1",
            json!({ "team": "devices" }),
        ),
        (
            "parent in the fallback region, start elsewhere, three tags",
            "us-east-1",
            "ap-southeast-4",
            json!({ "team": "devices", "stage": "beta", "owner": "qa" }),
        ),
        (
            "parent elsewhere, start in the fallback region, one tag",
            "il-central-1",
            "us-east-1",
            json!({ "team": "devices" }),
        ),
    ];

    for (case, definition_region, start_region, tags) in rows {
        let created = emulator
            .create_definition(&configuration(&format!("tag-scope-{case}")), definition_region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
            created.status, created.body
        );
        let definition_id = created.string("suiteDefinitionId");
        let definition_arn = created.string("suiteDefinitionArn");

        let started = start_run(&emulator, &definition_id, start_region, &tags).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartSuiteRun, got {} {}",
            started.status, started.body
        );
        let run_id = started.string("suiteRunId");
        let run_arn = started.string("suiteRunArn");
        assert_eq!(
            region_of(&run_arn),
            region_of(&definition_arn),
            "case {case:?}: the run's region must be its definition's, got {run_arn:?} beside \
             {definition_arn:?}"
        );

        let expected = if tags.as_object().is_some_and(serde_json::Map::is_empty) {
            json!({})
        } else {
            tags.clone()
        };
        let listed = list_tags(&emulator, &run_arn, start_region).await;
        assert_eq!(
            listed.member("tags"),
            &expected,
            "case {case:?}: the tags the start carried must read back under the run's own \
             ARN, got {}",
            listed.body
        );

        let fetched = emulator
            .call(
                "GET",
                &format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}"),
                start_region,
                None,
            )
            .await;
        assert_eq!(
            fetched.member("tags"),
            &expected,
            "case {case:?}: GetSuiteRun must report the tags held under the run's own ARN, \
             got {}",
            fetched.body
        );

        if start_region != definition_region {
            let other_arn = in_region(&run_arn, start_region);
            let refused = list_tags(&emulator, &other_arn, start_region).await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: the run's tags must not be addressable in the caller's own \
                 region, got {} {}",
                refused.status,
                refused.body
            );
        }
    }
}
