// SPDX-License-Identifier: CC0-1.0

//! Region scoping, end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so the route, the status code and the
//! serialized body are all exercised, not just a handler body.
//!
//! RULE: this service is regional. A request is served out of the store of the
//! Region its own credential scope names, so a resource exists only in the
//! Region whose endpoint created it: another Region's endpoint reports it in no
//! listing and refuses every read of it, the same id creates an independent
//! resource in each Region, and the ARN a Region mints carries that Region.
//!
//! The tests assert that rule over TABLES: pairs of Regions for the workspace
//! itself, and then every kind of resource the service holds — the contents of
//! a workspace (entity, component type, scene, sync job), the Region-level
//! resources (metadata transfer job, pricing plan) and the ARN-addressed
//! tagging operations.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    component_type_arn, entity_arn, metadata_transfer_job_arn, region_ceiling_message, scene_arn,
    sync_job_arn, workspace_arn, workspace_not_found_message, DEFAULT_REGION, MAX_REGIONS,
    PRICING_MODE_BASIC,
};

use crate::harness::{encode_label, encode_query, Emulator, Resp, SYNC_ROLE};

/// The Region PAIRS every case runs over: `(home, other)`, where `home` is the
/// Region a resource is created in and `other` the Region that must not see it.
///
/// The rows differ along what the rule generalises over — which Regions are
/// involved and how far apart they are: two Regions of one continent, two of
/// different continents, two whose names share a prefix (so a store keyed by a
/// prefix of the Region would be caught), and a Region whose name carries a
/// digit in its middle segment. Not along a mere rename: every row runs the
/// same resource ids.
const REGION_PAIRS: [(&str, &str); 4] = [
    ("us-east-1", "eu-west-1"),
    ("ap-southeast-2", "ca-central-1"),
    ("us-east-1", "us-east-2"),
    ("eu-central-1", "ap-northeast-3"),
];

/// The workspace id every case creates: ONE id, used in both Regions of a row,
/// because "the same id names an independent resource in each Region" is half
/// the rule under test.
const WORKSPACE: &str = "wsRegionProbe";

/// The Region segment of an ARN — its fourth colon-separated field.
fn arn_region(case: &str, arn: &str) -> String {
    arn.split(':')
        .nth(3)
        .unwrap_or_else(|| panic!("case {case:?}: {arn:?} is not an ARN with a Region segment"))
        .to_string()
}

/// The `arn` a create answered with.
fn arn_of(case: &str, resp: &Resp) -> String {
    resp.member("arn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an arn member, got {resp:?}"))
        .to_string()
}

/// The values of `id_field` in the `member` list of a listing's response, in the
/// order the page served them.
fn listed_ids(case: &str, resp: &Resp, member: &str, id_field: &str) -> Vec<String> {
    resp.member(member)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be a present array, got {resp:?}"))
        .iter()
        .map(|summary| {
            summary[id_field]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary carries {id_field}, got {summary:?}")
                })
                .to_string()
        })
        .collect()
}

/// `CreateWorkspace` signed for `region`.
async fn create_workspace_in(emulator: &Emulator, region: &str, workspace_id: &str) -> Resp {
    emulator
        .call_signed_for(
            Some(region),
            "POST",
            &format!("/workspaces/{workspace_id}"),
            Some(json!({})),
        )
        .await
}

/// `ListWorkspaces` signed for `region`, or unsigned when `region` is `None`.
async fn list_workspaces_in(emulator: &Emulator, region: Option<&str>) -> Resp {
    emulator
        .call_signed_for(region, "POST", "/workspaces-list", Some(json!({})))
        .await
}

/// `GetWorkspace` signed for `region`.
async fn get_workspace_in(emulator: &Emulator, region: &str, id_or_arn: &str) -> Resp {
    emulator
        .call_signed_for(
            Some(region),
            "GET",
            &format!("/workspaces/{}", encode_label(id_or_arn)),
            None,
        )
        .await
}

/// A workspace created through one Region's endpoint exists only there: the
/// other Region lists nothing, refuses the read with the modelled 404, and
/// accepts the same id as an independent workspace of its own.
#[tokio::test]
async fn a_workspace_exists_only_in_the_region_it_was_created_in() {
    for (home, other) in REGION_PAIRS {
        let case = format!("{home} then {other}");
        let emulator = Emulator::new();

        let created = create_workspace_in(&emulator, home, WORKSPACE).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from the create, got {created:?}"
        );
        let home_arn = arn_of(&case, &created);
        assert_eq!(
            arn_region(&case, &home_arn),
            home,
            "case {case:?}: expected the created ARN to carry the Region it was created in, \
             got {home_arn:?}"
        );

        let listed = list_workspaces_in(&emulator, Some(other)).await;
        assert_eq!(
            listed_ids(&case, &listed, "workspaceSummaries", "workspaceId"),
            Vec::<String>::new(),
            "case {case:?}: expected another Region to list no workspace, got {listed:?}"
        );

        let read = get_workspace_in(&emulator, other, WORKSPACE).await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected a 404 ResourceNotFoundException from another Region, \
             got {read:?}"
        );
        assert_eq!(
            read.body["message"].as_str(),
            Some(workspace_not_found_message(WORKSPACE).as_str()),
            "case {case:?}: expected the refusal to name the workspace looked for, got {read:?}"
        );

        // The ARN is the other form of the same label, and it resolves only in
        // the Region that minted it.
        let by_arn = get_workspace_in(&emulator, other, &home_arn).await;
        assert_eq!(
            (by_arn.status, by_arn.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected another Region to resolve no ARN of this one, got {by_arn:?}"
        );

        // The same id is free in the other Region, and the workspace it creates
        // there is a second, independent one.
        let twin = create_workspace_in(&emulator, other, WORKSPACE).await;
        assert_eq!(
            twin.status, 200,
            "case {case:?}: expected the same id to be creatable in another Region, got {twin:?}"
        );
        let other_arn = arn_of(&case, &twin);
        assert_eq!(
            other_arn,
            workspace_arn(other, WORKSPACE),
            "case {case:?}: expected the second workspace's ARN to carry its own Region, \
             got {other_arn:?}"
        );

        // Each Region now reports exactly its own workspace, with its own ARN.
        for region in [home, other] {
            let listed = list_workspaces_in(&emulator, Some(region)).await;
            assert_eq!(
                listed_ids(&case, &listed, "workspaceSummaries", "workspaceId"),
                vec![WORKSPACE.to_string()],
                "case {case:?}: expected {region} to list only its own workspace, got {listed:?}"
            );
            let summary_arn = listed.member("workspaceSummaries")[0]["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every summary carries an arn"))
                .to_string();
            assert_eq!(
                summary_arn,
                workspace_arn(region, WORKSPACE),
                "case {case:?}: expected {region} to report its own ARN, got {summary_arn:?}"
            );
            let read = get_workspace_in(&emulator, region, WORKSPACE).await;
            assert_eq!(
                (read.status, arn_of(&case, &read)),
                (200, workspace_arn(region, WORKSPACE)),
                "case {case:?}: expected {region} to read back its own workspace, got {read:?}"
            );
        }
    }
}

/// A request that carries no credential scope has no Region of its own, so it is
/// served out of the service's default Region — and is invisible from any other.
#[tokio::test]
async fn an_unsigned_request_is_served_out_of_the_default_region() {
    let emulator = Emulator::new();
    let created = emulator
        .call_signed_for(
            None,
            "POST",
            &format!("/workspaces/{WORKSPACE}"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from an unsigned create, got {created:?}"
    );
    assert_eq!(
        arn_of("unsigned", &created),
        workspace_arn(DEFAULT_REGION, WORKSPACE),
        "expected an unsigned create to mint an ARN in the default Region, got {created:?}"
    );

    // The default Region sees it, whether the caller signs for it or not.
    for (case, region) in [
        ("unsigned", None),
        ("signed for the default Region", Some(DEFAULT_REGION)),
    ] {
        let listed = list_workspaces_in(&emulator, region).await;
        assert_eq!(
            listed_ids(case, &listed, "workspaceSummaries", "workspaceId"),
            vec![WORKSPACE.to_string()],
            "case {case:?}: expected the default Region's workspace to be listed, got {listed:?}"
        );
    }

    // Every other Region does not.
    for region in REGION_PAIRS
        .iter()
        .flat_map(|(home, other)| [*home, *other])
        .filter(|region| *region != DEFAULT_REGION)
    {
        let listed = list_workspaces_in(&emulator, Some(region)).await;
        assert_eq!(
            listed_ids(region, &listed, "workspaceSummaries", "workspaceId"),
            Vec::<String>::new(),
            "case {region:?}: expected no other Region to see the default Region's workspace, \
             got {listed:?}"
        );
    }
}

/// The Region a request names is the caller's own credential scope, so the
/// store bounds how many Regions it holds resources for: past the ceiling a
/// create that would give a further Region its first resource is refused and
/// stores nothing, while the Regions already in the store go on working.
#[tokio::test]
async fn the_number_of_regions_the_store_holds_is_bounded() {
    let emulator = Emulator::new();
    // Fill the store: one workspace in each of as many Regions as it holds.
    // The names are of the shape the service accepts for a Region segment.
    let filled: Vec<String> = (0..MAX_REGIONS)
        .map(|index| format!("emulator-region-{index}"))
        .collect();
    for region in &filled {
        assert_eq!(
            create_workspace_in(&emulator, region, WORKSPACE).await.status,
            200,
            "case {region:?}: the workspace filling the store must be created"
        );
    }

    // The rows differ in which call arrives at the full store: a create that
    // needs room for a further Region, a create in a Region the store already
    // holds, and a read of a Region it does not.
    let overflow = create_workspace_in(&emulator, "emulator-region-overflow", WORKSPACE).await;
    assert_eq!(
        (overflow.status, overflow.error_type.as_deref()),
        (402, Some("ServiceQuotaExceededException")),
        "expected a create in a further Region to be refused for the quota, got {overflow:?}"
    );
    assert_eq!(
        overflow.body["message"].as_str(),
        Some(region_ceiling_message().as_str()),
        "expected the refusal to name the ceiling it reached, got {overflow:?}"
    );
    let listed = list_workspaces_in(&emulator, Some("emulator-region-overflow")).await;
    assert_eq!(
        listed_ids("overflow", &listed, "workspaceSummaries", "workspaceId"),
        Vec::<String>::new(),
        "expected the refused create to store nothing, got {listed:?}"
    );

    let held = &filled[0];
    let second = create_workspace_in(&emulator, held, "wsSecondProbe").await;
    assert_eq!(
        second.status, 200,
        "expected a Region the full store already holds to keep taking creates, got {second:?}"
    );
    let still_there = list_workspaces_in(&emulator, Some(held)).await;
    assert_eq!(
        listed_ids(held, &still_there, "workspaceSummaries", "workspaceId"),
        vec![WORKSPACE.to_string(), "wsSecondProbe".to_string()],
        "expected the filled Region to report both its workspaces, got {still_there:?}"
    );
}

/// A Region a caller only READ, or wrote nothing in, leaves nothing behind: the
/// store holds resources for the Regions that have them, so a refused call in a
/// fresh Region neither occupies a slot under the ceiling nor changes what the
/// next caller sees.
#[tokio::test]
async fn a_region_nothing_was_created_in_occupies_no_slot() {
    let emulator = Emulator::new();
    for region in REGION_PAIRS.iter().flat_map(|(home, other)| [*home, *other]) {
        // A delete and a tagging call in a Region holding nothing: both refuse,
        // and neither may leave the Region a resource of its own.
        let deleted = emulator
            .call_signed_for(
                Some(region),
                "DELETE",
                &format!("/workspaces/{WORKSPACE}"),
                None,
            )
            .await;
        assert_eq!(
            (deleted.status, deleted.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {region:?}: expected a delete in an empty Region to be a 404, got {deleted:?}"
        );
        let tagged = emulator
            .call_signed_for(
                Some(region),
                "POST",
                "/tags",
                Some(json!({
                    "resourceARN": workspace_arn(region, WORKSPACE),
                    "tags": {"team": "plant"},
                })),
            )
            .await;
        assert_eq!(
            (tagged.status, tagged.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {region:?}: expected tagging in an empty Region to be a 404, got {tagged:?}"
        );
    }

    // The store still has room for every Region it may hold, which it would not
    // if the calls above had each taken a slot.
    for index in 0..MAX_REGIONS {
        let region = format!("emulator-region-{index}");
        assert_eq!(
            create_workspace_in(&emulator, &region, WORKSPACE).await.status,
            200,
            "case {region:?}: expected the store to still have room for this Region"
        );
    }
}

/// One resource created INSIDE a workspace, as the requests that create, read
/// and list it: the rows differ in which kind of contained resource it is, which
/// is what the Region rule generalises over.
struct Contained {
    kind: &'static str,
    /// The resource's own id, as the listing reports it.
    id: &'static str,
    create_method: &'static str,
    create_path: String,
    create_body: Value,
    /// The method the single-resource read uses; it carries no body.
    read_method: &'static str,
    read_path: String,
    list_path: String,
    list_member: &'static str,
    list_id_field: &'static str,
    /// The ARN the resource must carry when it is created in the given Region,
    /// read off the service's own ARN builders rather than restated here.
    expected_arn: Box<dyn Fn(&str) -> String>,
}

/// The contained resources every Region pair is run over: one row per kind of
/// resource a workspace holds.
fn contained_rows() -> Vec<Contained> {
    vec![
        Contained {
            kind: "entity",
            id: "probeEntity",
            create_method: "POST",
            create_path: format!("/workspaces/{WORKSPACE}/entities"),
            create_body: json!({"entityId": "probeEntity", "entityName": "Probe"}),
            read_method: "GET",
            read_path: format!("/workspaces/{WORKSPACE}/entities/probeEntity"),
            list_path: format!("/workspaces/{WORKSPACE}/entities-list"),
            list_member: "entitySummaries",
            list_id_field: "entityId",
            expected_arn: Box::new(|region| entity_arn(region, WORKSPACE, "probeEntity")),
        },
        Contained {
            kind: "component type",
            id: "com.example.probe",
            create_method: "POST",
            create_path: format!("/workspaces/{WORKSPACE}/component-types/com.example.probe"),
            create_body: json!({}),
            read_method: "GET",
            read_path: format!("/workspaces/{WORKSPACE}/component-types/com.example.probe"),
            list_path: format!("/workspaces/{WORKSPACE}/component-types-list"),
            list_member: "componentTypeSummaries",
            list_id_field: "componentTypeId",
            expected_arn: Box::new(|region| {
                component_type_arn(region, WORKSPACE, "com.example.probe")
            }),
        },
        Contained {
            kind: "scene",
            id: "probeScene",
            create_method: "POST",
            create_path: format!("/workspaces/{WORKSPACE}/scenes"),
            create_body: json!({
                "sceneId": "probeScene",
                "contentLocation": "s3://twinmaker-probe/scene.json",
            }),
            read_method: "GET",
            read_path: format!("/workspaces/{WORKSPACE}/scenes/probeScene"),
            list_path: format!("/workspaces/{WORKSPACE}/scenes-list"),
            list_member: "sceneSummaries",
            list_id_field: "sceneId",
            expected_arn: Box::new(|region| scene_arn(region, WORKSPACE, "probeScene")),
        },
        Contained {
            kind: "sync job",
            // A sync job has no id of its own: the source is its identity
            // inside the workspace, and the only one the service supports.
            id: "SITEWISE",
            create_method: "POST",
            create_path: format!("/workspaces/{WORKSPACE}/sync-jobs/SITEWISE"),
            create_body: json!({"syncRole": SYNC_ROLE}),
            read_method: "GET",
            read_path: format!("/sync-jobs/SITEWISE?workspace={WORKSPACE}"),
            list_path: format!("/workspaces/{WORKSPACE}/sync-jobs-list"),
            list_member: "syncJobSummaries",
            list_id_field: "syncSource",
            expected_arn: Box::new(|region| sync_job_arn(region, WORKSPACE, "SITEWISE")),
        },
    ]
}

/// Everything a workspace CONTAINS is scoped with it: with the same workspace id
/// created in both Regions, a resource created in one is read and listed only
/// there, and its ARN carries that Region.
#[tokio::test]
async fn a_workspaces_contents_are_scoped_to_the_region_they_were_created_in() {
    for (home, other) in REGION_PAIRS {
        for row in contained_rows() {
            let case = format!("{} in {home} then {other}", row.kind);
            let emulator = Emulator::new();
            // The SAME workspace id in both Regions, so the only thing that can
            // hide the resource from the other Region is the Region itself.
            for region in [home, other] {
                assert_eq!(
                    create_workspace_in(&emulator, region, WORKSPACE).await.status,
                    200,
                    "case {case:?}: the workspace must exist in {region}"
                );
            }

            let created = emulator
                .call_signed_for(
                    Some(home),
                    row.create_method,
                    &row.create_path,
                    Some(row.create_body.clone()),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected 200 from the create, got {created:?}"
            );
            assert_eq!(
                arn_of(&case, &created),
                (row.expected_arn)(home),
                "case {case:?}: expected the created ARN to carry the Region it was created in, \
                 got {created:?}"
            );

            // The home Region reads it back and lists it.
            let read = emulator
                .call_signed_for(Some(home), row.read_method, &row.read_path, None)
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected the home Region to read it back, got {read:?}"
            );
            let listed = emulator
                .call_signed_for(Some(home), "POST", &row.list_path, Some(json!({})))
                .await;
            assert!(
                listed_ids(&case, &listed, row.list_member, row.list_id_field)
                    .iter()
                    .any(|id| id == row.id),
                "case {case:?}: expected the home Region to list it, got {listed:?}"
            );

            // The other Region holds the workspace but not its contents.
            let missed = emulator
                .call_signed_for(Some(other), row.read_method, &row.read_path, None)
                .await;
            assert_eq!(
                (missed.status, missed.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected a 404 ResourceNotFoundException from the other Region, \
                 got {missed:?}"
            );
            let elsewhere = emulator
                .call_signed_for(Some(other), "POST", &row.list_path, Some(json!({})))
                .await;
            let elsewhere_ids = listed_ids(&case, &elsewhere, row.list_member, row.list_id_field);
            assert!(
                !elsewhere_ids.iter().any(|id| id == row.id),
                "case {case:?}: expected the other Region to list none of it, got {elsewhere_ids:?}"
            );
        }
    }
}

/// A metadata transfer job hangs off no workspace, and is scoped to its Region
/// all the same: it is read and listed only through the endpoint that accepted
/// it, and its ARN carries that Region.
#[tokio::test]
async fn a_metadata_transfer_job_is_scoped_to_the_region_that_accepted_it() {
    const JOB: &str = "probeTransferJob";
    for (home, other) in REGION_PAIRS {
        let case = format!("{home} then {other}");
        let emulator = Emulator::new();

        let created = emulator
            .call_signed_for(
                Some(home),
                "POST",
                "/metadata-transfer-jobs",
                Some(json!({
                    "metadataTransferJobId": JOB,
                    "sources": [{"type": "iotsitewise"}],
                    "destination": {"type": "iotsitewise"},
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from the create, got {created:?}"
        );
        assert_eq!(
            arn_of(&case, &created),
            metadata_transfer_job_arn(home, JOB),
            "case {case:?}: expected the job's ARN to carry the Region that accepted it, \
             got {created:?}"
        );

        let read_path = format!("/metadata-transfer-jobs/{JOB}");
        let read = emulator
            .call_signed_for(Some(home), "GET", &read_path, None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the home Region to read the job back, got {read:?}"
        );
        let missed = emulator
            .call_signed_for(Some(other), "GET", &read_path, None)
            .await;
        assert_eq!(
            (missed.status, missed.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected a 404 ResourceNotFoundException from the other Region, \
             got {missed:?}"
        );

        let list_body = json!({"sourceType": "iotsitewise", "destinationType": "iotsitewise"});
        for (region, expected) in [(home, vec![JOB.to_string()]), (other, Vec::new())] {
            let listed = emulator
                .call_signed_for(
                    Some(region),
                    "POST",
                    "/metadata-transfer-jobs-list",
                    Some(list_body.clone()),
                )
                .await;
            assert_eq!(
                listed_ids(
                    &case,
                    &listed,
                    "metadataTransferJobSummaries",
                    "metadataTransferJobId"
                ),
                expected,
                "case {case:?}: expected {region} to list {expected:?}, got {listed:?}"
            );
        }
    }
}

/// The pricing plan is per account AND Region: a plan set through one endpoint
/// leaves every other Region on the plan it was already on, and the billable
/// entity count a Region reports counts only its own entities.
#[tokio::test]
async fn the_pricing_plan_and_its_entity_count_are_scoped_to_their_region() {
    // The bundle a case buys, and the mode it buys it in: a bundled plan is the
    // only kind that reports `bundleInformation`, so it is visibly not the
    // default plan an untouched Region is on.
    const BUNDLED_MODE: &str = "TIERED_BUNDLE";
    const BUNDLE: &str = "TwinMaker-Basic";

    for (home, other) in REGION_PAIRS {
        let case = format!("{home} then {other}");
        let emulator = Emulator::new();

        let updated = emulator
            .call_signed_for(
                Some(home),
                "POST",
                "/pricingplan",
                Some(json!({"pricingMode": BUNDLED_MODE, "bundleNames": [BUNDLE]})),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from the plan change, got {updated:?}"
        );

        // An entity in the home Region, so the two Regions disagree on the
        // billable count as well as on the plan.
        assert_eq!(
            create_workspace_in(&emulator, home, WORKSPACE).await.status,
            200,
            "case {case:?}: the workspace the entity is created in must exist"
        );
        assert_eq!(
            emulator
                .call_signed_for(
                    Some(home),
                    "POST",
                    &format!("/workspaces/{WORKSPACE}/entities"),
                    Some(json!({"entityId": "probeEntity", "entityName": "Probe"})),
                )
                .await
                .status,
            200,
            "case {case:?}: the entity the count is taken over must exist"
        );

        let home_plan = emulator
            .call_signed_for(Some(home), "GET", "/pricingplan", None)
            .await;
        assert_eq!(
            (
                home_plan.member("currentPricingPlan")["pricingMode"].as_str(),
                home_plan.member("currentPricingPlan")["billableEntityCount"].as_i64(),
            ),
            (Some(BUNDLED_MODE), Some(1)),
            "case {case:?}: expected the home Region to report its own plan and count, \
             got {home_plan:?}"
        );

        let other_plan = emulator
            .call_signed_for(Some(other), "GET", "/pricingplan", None)
            .await;
        assert_eq!(
            (
                other_plan.member("currentPricingPlan")["pricingMode"].as_str(),
                other_plan.member("currentPricingPlan")["billableEntityCount"].as_i64(),
            ),
            (Some(PRICING_MODE_BASIC), Some(0)),
            "case {case:?}: expected another Region to stay on the default plan with no \
             billable entity, got {other_plan:?}"
        );
        assert!(
            other_plan.member("currentPricingPlan")["bundleInformation"].is_null(),
            "case {case:?}: expected another Region to report no bundle, got {other_plan:?}"
        );
    }
}

/// The tagging operations address a resource by ARN, and an ARN resolves only in
/// the Region that minted it: tagging, listing and untagging through another
/// Region's endpoint is the modelled 404, and the tags a resource carries are
/// unchanged by it.
#[tokio::test]
async fn tagging_resolves_an_arn_only_in_the_region_that_minted_it() {
    for (home, other) in REGION_PAIRS {
        let case = format!("{home} then {other}");
        let emulator = Emulator::new();
        assert_eq!(
            create_workspace_in(&emulator, home, WORKSPACE).await.status,
            200,
            "case {case:?}: the workspace the ARN names must exist"
        );
        let arn = workspace_arn(home, WORKSPACE);

        let tagged = emulator
            .call_signed_for(
                Some(home),
                "POST",
                "/tags",
                Some(json!({"resourceARN": arn, "tags": {"team": "plant"}})),
            )
            .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected 200 from tagging in the home Region, got {tagged:?}"
        );

        // Every tagging operation, through the other Region's endpoint, is the
        // modelled 404 — there is no such resource there.
        let rows: Vec<(&str, &str, &str, Option<Value>)> = vec![
            (
                "TagResource",
                "POST",
                "/tags",
                Some(json!({"resourceARN": arn, "tags": {"team": "other"}})),
            ),
            (
                "ListTagsForResource",
                "POST",
                "/tags-list",
                Some(json!({"resourceARN": arn})),
            ),
            (
                "UntagResource",
                "DELETE",
                "/tags?resourceARN=&tagKeys=team",
                None,
            ),
        ];
        for (operation, method, path, body) in rows {
            // The ARN goes in the query string for the operation that binds it
            // there, built with the same encoding an SDK uses.
            let path = path.replace("resourceARN=", &format!("resourceARN={}", encode_query(&arn)));
            let refused = emulator
                .call_signed_for(Some(other), method, &path, body)
                .await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected {operation} from the other Region to be a 404 \
                 ResourceNotFoundException, got {refused:?}"
            );
        }

        let listed = emulator
            .call_signed_for(
                Some(home),
                "POST",
                "/tags-list",
                Some(json!({"resourceARN": arn})),
            )
            .await;
        assert_eq!(
            listed.member("tags"),
            &json!({"team": "plant"}),
            "case {case:?}: expected the home Region's tags to be untouched, got {listed:?}"
        );
    }
}
