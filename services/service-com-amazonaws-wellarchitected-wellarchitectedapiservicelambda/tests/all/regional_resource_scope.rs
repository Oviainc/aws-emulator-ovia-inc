// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Region a Well-Architected Tool resource belongs to.
//!
//! The rule under test: every resource of this service belongs to the Region whose
//! endpoint created it — its ARN carries that Region, and a request reaches only the
//! resources of the Region its SigV4 credential scope names. So a read signed for
//! another Region reports an empty page or answers 404 `ResourceNotFoundException`, and a
//! uniqueness rule scoped "within an account within an Amazon Web Services Region"
//! (`smithy_query(query="shape", shape="WorkloadName")`) leaves the same name free in
//! every other Region.
//!
//! Rows differ along the Region PAIR a case runs over and along the KIND of resource
//! created (workload, review template, profile, custom lens), because the rule is about
//! the service's whole resource namespace rather than one operation's.
//!
//! Every request goes through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run exactly as
//! they do in production.

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::{
    DEFAULT_REGION, MAX_REGIONS,
};

use super::custom_lens_import::{lens_document, wait_for_the_import_to_land};
use super::workload_harness::{
    created_id, encode_path_segment, member, minimal_create, minimal_profile, minimal_template,
    Answer, Client,
};

/// The Region pairs every rule below is run over: the Region a resource is created in,
/// and a Region that must not see it.
///
/// The pairs differ along which Region creates and which reads (so neither position is
/// privileged) and along the partition's geographies; none of the six is the Region
/// `workload_harness::SIGNED_REGION` names, so a case here is never served by the store
/// another suite seeded.
const REGION_PAIRS: &[(&str, &str)] = &[
    ("us-east-1", "eu-west-1"),
    ("eu-central-1", "us-east-2"),
    ("ap-southeast-2", "sa-east-1"),
];

/// `CreateAgentProfile` / `ListAgentProfiles`: `POST` and `GET /api/v1/agent-profiles`
/// (`OPERATIONS.md`).
const AGENT_PROFILES_PATH: &str = "/api/v1/agent-profiles";

/// A minimal valid `CreateAgentProfile` body: the modelled `@required` members only
/// (`smithy_query(query="operation", operation="CreateAgentProfile")`), with the
/// execution and access roles in this emulator's one account.
fn minimal_agent_profile(name: &str) -> Value {
    let role = format!(
        "arn:aws:iam::{}:role/AgentProfileRegionScope",
        engine::emulator_account_id()
    );
    json!({
        "name": name,
        "pillars": ["SECURITY"],
        "executionRoleArn": role,
        "aggregationConfiguration": [{
            "accountId": engine::emulator_account_id(),
            "regions": ["us-east-1"],
            "accessRoleArn": role,
        }],
    })
}

/// One member of every entry of a summary list a paged read answered.
fn summarized(answer: &Answer, list: &str, name: &str) -> Vec<String> {
    answer
        .body
        .get(list)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry
                        .get(name)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A read of a resource of another Region: 404 with the modelled
/// `ResourceNotFoundException`.
fn assert_not_found(answer: &Answer, case: &str, call: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: {call} in another Region must answer 404, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ResourceNotFoundException",
        "case {case:?}: {call} in another Region must answer ResourceNotFoundException, \
         got {answer:?}"
    );
}

/// A read of a resource of this Region: 200.
fn assert_served(answer: &Answer, case: &str, call: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: {call} in the resource's own Region must answer 200, got {answer:?}"
    );
}

/// Whether an ARN's Region field (its fourth, `arn:<partition>:<service>:<region>:…`) is
/// `region`.
fn arn_region_is(arn: &str, region: &str) -> bool {
    arn.split(':').nth(3) == Some(region)
}

/// A workload is reported by `ListWorkloads`, resolved by `GetWorkload` and reviewed by
/// `ListLensReviews` / `ListMilestones` only in the Region that created it, and the
/// `WorkloadArn` it hands back carries that Region.
#[tokio::test]
async fn a_workload_belongs_to_the_region_that_created_it() {
    let client = Client::new();

    for (home, other) in REGION_PAIRS {
        let case = format!("workload-{home}-vs-{other}");
        let created = client
            .send_in(
                home,
                "POST",
                "/workloads",
                Some(minimal_create(&case, &format!("tok-{case}"))),
            )
            .await;
        let workload_id = created_id(&created, &case);
        let workload_arn = member(&created, "WorkloadArn").to_string();
        assert!(
            arn_region_is(&workload_arn, home),
            "case {case:?}: the WorkloadArn must carry the creating Region {home:?}, \
             got {workload_arn:?}"
        );

        let home_page = client
            .send_in(home, "POST", "/workloadsSummaries", Some(json!({})))
            .await;
        assert_served(&home_page, &case, "ListWorkloads");
        assert_eq!(
            summarized(&home_page, "WorkloadSummaries", "WorkloadId"),
            vec![workload_id.clone()],
            "case {case:?}: the creating Region must report its own workload, got {home_page:?}"
        );
        assert_eq!(
            summarized(&home_page, "WorkloadSummaries", "WorkloadArn"),
            vec![workload_arn.clone()],
            "case {case:?}: the summary must carry the stored ARN, got {home_page:?}"
        );

        let other_page = client
            .send_in(other, "POST", "/workloadsSummaries", Some(json!({})))
            .await;
        assert_served(&other_page, &case, "ListWorkloads");
        assert_eq!(
            summarized(&other_page, "WorkloadSummaries", "WorkloadId"),
            Vec::<String>::new(),
            "case {case:?}: {other:?} holds no workload, so its page is empty, got {other_page:?}"
        );

        // Every read that resolves the workload by id: served in its Region, 404 in the
        // other, where no such workload exists.
        let reads: [(&str, &str, String, Option<Value>); 3] = [
            ("GetWorkload", "GET", format!("/workloads/{workload_id}"), None),
            (
                "ListLensReviews",
                "GET",
                format!("/workloads/{workload_id}/lensReviews"),
                None,
            ),
            (
                "ListMilestones",
                "POST",
                format!("/workloads/{workload_id}/milestonesSummaries"),
                Some(json!({})),
            ),
        ];
        for (call, method, uri, body) in reads {
            let served = client.send_in(home, method, &uri, body.clone()).await;
            assert_served(&served, &case, call);
            let refused = client.send_in(other, method, &uri, body).await;
            assert_not_found(&refused, &case, call);
        }
    }
}

/// A review template and a profile belong to their creating Region exactly as a workload
/// does: listed and resolved there, absent and 404 everywhere else, each ARN carrying the
/// Region that minted it.
#[tokio::test]
async fn a_review_template_and_a_profile_belong_to_the_region_that_created_them() {
    let client = Client::new();

    for (home, other) in REGION_PAIRS {
        let case = format!("template-{home}-vs-{other}");
        let template = client
            .send_in(
                home,
                "POST",
                "/reviewTemplates",
                Some(minimal_template(&case, &format!("tok-{case}"))),
            )
            .await;
        assert_served(&template, &case, "CreateReviewTemplate");
        let template_arn = member(&template, "TemplateArn").to_string();
        assert!(
            arn_region_is(&template_arn, home),
            "case {case:?}: the TemplateArn must carry the creating Region {home:?}, \
             got {template_arn:?}"
        );

        let listed_home = client.send_in(home, "GET", "/reviewTemplates", None).await;
        assert_eq!(
            summarized(&listed_home, "ReviewTemplates", "TemplateArn"),
            vec![template_arn.clone()],
            "case {case:?}: the creating Region must report its own template, got {listed_home:?}"
        );
        let listed_other = client.send_in(other, "GET", "/reviewTemplates", None).await;
        assert_eq!(
            summarized(&listed_other, "ReviewTemplates", "TemplateArn"),
            Vec::<String>::new(),
            "case {case:?}: {other:?} holds no review template, got {listed_other:?}"
        );

        let template_uri = format!("/reviewTemplates/{}", encode_path_segment(&template_arn));
        assert_served(
            &client.send_in(home, "GET", &template_uri, None).await,
            &case,
            "GetReviewTemplate",
        );
        assert_not_found(
            &client.send_in(other, "GET", &template_uri, None).await,
            &case,
            "GetReviewTemplate",
        );

        let case = format!("profile-{home}-vs-{other}");
        let profile = client
            .send_in(home, "POST", "/profiles", Some(minimal_profile(&case)))
            .await;
        assert_served(&profile, &case, "CreateProfile");
        let profile_arn = member(&profile, "ProfileArn").to_string();
        assert!(
            arn_region_is(&profile_arn, home),
            "case {case:?}: the ProfileArn must carry the creating Region {home:?}, \
             got {profile_arn:?}"
        );

        let listed_home = client.send_in(home, "GET", "/profileSummaries", None).await;
        assert_eq!(
            summarized(&listed_home, "ProfileSummaries", "ProfileArn"),
            vec![profile_arn.clone()],
            "case {case:?}: the creating Region must report its own profile, got {listed_home:?}"
        );
        let listed_other = client.send_in(other, "GET", "/profileSummaries", None).await;
        assert_eq!(
            summarized(&listed_other, "ProfileSummaries", "ProfileArn"),
            Vec::<String>::new(),
            "case {case:?}: {other:?} holds no profile, got {listed_other:?}"
        );

        let profile_uri = format!("/profiles/{}", encode_path_segment(&profile_arn));
        assert_served(
            &client.send_in(home, "GET", &profile_uri, None).await,
            &case,
            "GetProfile",
        );
        assert_not_found(
            &client.send_in(other, "GET", &profile_uri, None).await,
            &case,
            "GetProfile",
        );

        // An agent profile, which the API names by its own ARN and lists through its own
        // route (`GET /api/v1/agent-profiles`), belongs to its Region the same way.
        let case = format!("agent-profile-{home}-vs-{other}");
        let agent = client
            .send_in(
                home,
                "POST",
                AGENT_PROFILES_PATH,
                Some(minimal_agent_profile(&case)),
            )
            .await;
        assert_served(&agent, &case, "CreateAgentProfile");
        let agent_arn = member(&agent, "arn").to_string();
        assert!(
            arn_region_is(&agent_arn, home),
            "case {case:?}: the AgentProfileArn must carry the creating Region {home:?}, \
             got {agent_arn:?}"
        );

        let listed_home = client.send_in(home, "GET", AGENT_PROFILES_PATH, None).await;
        assert_eq!(
            summarized(&listed_home, "items", "arn"),
            vec![agent_arn.clone()],
            "case {case:?}: the creating Region must list its own agent profile, \
             got {listed_home:?}"
        );
        let listed_other = client
            .send_in(other, "GET", AGENT_PROFILES_PATH, None)
            .await;
        assert_eq!(
            summarized(&listed_other, "items", "arn"),
            Vec::<String>::new(),
            "case {case:?}: {other:?} holds no agent profile, got {listed_other:?}"
        );

        let agent_uri = format!("{AGENT_PROFILES_PATH}/{}", encode_path_segment(&agent_arn));
        assert_served(
            &client.send_in(home, "GET", &agent_uri, None).await,
            &case,
            "GetAgentProfile",
        );
        assert_not_found(
            &client.send_in(other, "GET", &agent_uri, None).await,
            &case,
            "GetAgentProfile",
        );
    }
}

/// The documented uniqueness rules are scoped to one Region: a name another Region's
/// resource already holds is still free, while the SAME Region refuses it 409
/// `ConflictException`.
#[tokio::test]
async fn a_name_is_unique_within_one_region_only() {
    let client = Client::new();

    for (home, other) in REGION_PAIRS {
        let case = format!("name-{home}-vs-{other}");

        let home_workload = client
            .send_in(
                home,
                "POST",
                "/workloads",
                Some(minimal_create(&case, &format!("tok-{case}-home"))),
            )
            .await;
        let home_id = created_id(&home_workload, &case);

        let taken = client
            .send_in(
                home,
                "POST",
                "/workloads",
                Some(minimal_create(&case, &format!("tok-{case}-again"))),
            )
            .await;
        assert_eq!(
            taken.status, 409,
            "case {case:?}: the name is taken in {home:?}, got {taken:?}"
        );
        assert_eq!(
            taken.error_code, "ConflictException",
            "case {case:?}: a name collision is a ConflictException, got {taken:?}"
        );

        let elsewhere = client
            .send_in(
                other,
                "POST",
                "/workloads",
                Some(minimal_create(&case, &format!("tok-{case}-other"))),
            )
            .await;
        let other_id = created_id(&elsewhere, &case);
        assert_ne!(
            other_id, home_id,
            "case {case:?}: each Region's workload is its own resource, got {elsewhere:?}"
        );
        let other_arn = member(&elsewhere, "WorkloadArn").to_string();
        assert!(
            arn_region_is(&other_arn, other),
            "case {case:?}: the second workload's ARN must carry {other:?}, got {other_arn:?}"
        );

        // The same rule for a review template's name.
        let template_case = format!("{case}-template");
        let home_template = client
            .send_in(
                home,
                "POST",
                "/reviewTemplates",
                Some(minimal_template(&template_case, &format!("tok-{case}-t-home"))),
            )
            .await;
        assert_served(&home_template, &template_case, "CreateReviewTemplate");
        let taken = client
            .send_in(
                home,
                "POST",
                "/reviewTemplates",
                Some(minimal_template(
                    &template_case,
                    &format!("tok-{case}-t-again"),
                )),
            )
            .await;
        assert_eq!(
            (taken.status, taken.error_code.as_str()),
            (409, "ConflictException"),
            "case {template_case:?}: the template name is taken in {home:?}, got {taken:?}"
        );
        let elsewhere = client
            .send_in(
                other,
                "POST",
                "/reviewTemplates",
                Some(minimal_template(
                    &template_case,
                    &format!("tok-{case}-t-other"),
                )),
            )
            .await;
        assert_served(&elsewhere, &template_case, "CreateReviewTemplate");
        let elsewhere_arn = member(&elsewhere, "TemplateArn").to_string();
        assert!(
            arn_region_is(&elsewhere_arn, other),
            "case {template_case:?}: the second template's ARN must carry {other:?}, \
             got {elsewhere_arn:?}"
        );
    }
}

/// A request's Region is its caller's own credential scope, so the Regions a caller can
/// REACH are the published set and not the strings it can sign for: a scope naming no
/// published Region is attributed to the default Region, exactly as a request carrying no
/// scope at all is, while a published Region keeps its own store. That is what bounds the
/// store's Region dimension at `MAX_REGIONS` — one partition per published Region.
#[tokio::test]
async fn a_scope_naming_no_published_region_is_served_by_the_default_region() {
    let client = Client::new();

    // Rows differ along the FORM of the unpublished Region string: a plausible-looking
    // id, a word, a mis-cased spelling of a real Region, a real Region with a suffix, and
    // one far longer than any Region name.
    let overlong = "a".repeat(300);
    let unpublished = [
        "zz-test-1",
        "banana",
        "US-EAST-1",
        "us-east-1-extra",
        overlong.as_str(),
    ];

    let mut attributed = Vec::new();
    for (index, region) in unpublished.iter().enumerate() {
        let case = format!("unpublished-{index}");
        let created = client
            .send_in(
                region,
                "POST",
                "/workloads",
                Some(minimal_create(&case, &format!("tok-{case}"))),
            )
            .await;
        let workload_id = created_id(&created, &case);
        let workload_arn = member(&created, "WorkloadArn").to_string();
        assert!(
            arn_region_is(&workload_arn, DEFAULT_REGION),
            "case {case:?}: a scope naming no published Region is attributed to \
             {DEFAULT_REGION:?}, got {workload_arn:?}"
        );
        attributed.push((case, workload_id));
    }

    // The default Region's own endpoint reports every one of them: they are its
    // workloads, not a Region of their own.
    let page = client
        .send_in(
            DEFAULT_REGION,
            "POST",
            "/workloadsSummaries",
            Some(json!({ "MaxResults": 50 })),
        )
        .await;
    assert_served(&page, DEFAULT_REGION, "ListWorkloads");
    let listed = summarized(&page, "WorkloadSummaries", "WorkloadId");
    for (case, workload_id) in &attributed {
        assert!(
            listed.contains(workload_id),
            "case {case:?}: {DEFAULT_REGION:?} must report the workload it was attributed, \
             got {listed:?}"
        );
    }

    // A PUBLISHED Region still gets its own store, so the attribution above is not the
    // store collapsing: this workload is reported there and nowhere else.
    let published = "eu-west-3";
    let created = client
        .send_in(
            published,
            "POST",
            "/workloads",
            Some(minimal_create("published-region", "tok-published-region")),
        )
        .await;
    let published_id = created_id(&created, published);
    assert_eq!(
        summarized(
            &client
                .send_in(published, "POST", "/workloadsSummaries", Some(json!({})))
                .await,
            "WorkloadSummaries",
            "WorkloadId"
        ),
        vec![published_id.clone()],
        "case {published:?}: a published Region reports its own workload alone"
    );
    assert!(
        !listed.contains(&published_id),
        "case {published:?}: its workload is not {DEFAULT_REGION:?}'s, got {listed:?}"
    );

    // However many Region strings were signed for, the store holds one partition per
    // published Region it was asked for — here the two Regions above.
    let held = client.store().lock().regions_held();
    assert_eq!(
        held, 2,
        "the {} Region strings signed for must key only the {DEFAULT_REGION:?} and \
         {published:?} stores (at most {MAX_REGIONS}), got {held}",
        unpublished.len() + 1
    );
}

/// A custom lens is listed by `ListLenses` and read back by `ExportLens` only in the
/// Region that imported it; every Region still publishes the AWS-official lenses, whose
/// ARNs name the reading Region.
#[tokio::test]
async fn an_imported_lens_belongs_to_the_region_that_imported_it() {
    let client = Client::new();

    // Import in every home Region first, then wait ONCE for the imports to land.
    let mut imported: Vec<(String, String, String, String)> = Vec::new();
    for (home, other) in REGION_PAIRS {
        let case = format!("lens-{home}-vs-{other}");
        let document =
            lens_document(&format!("Lens of {home}"), None, &[("governance", 1, 2)]).to_string();
        let answer = client
            .send_in(
                home,
                "PUT",
                "/importLens",
                Some(json!({ "JSONString": document, "ClientRequestToken": format!("tok-{case}") })),
            )
            .await;
        assert_served(&answer, &case, "ImportLens");
        let lens_arn = member(&answer, "LensArn").to_string();
        assert!(
            arn_region_is(&lens_arn, home),
            "case {case:?}: the LensArn must carry the importing Region {home:?}, got {lens_arn:?}"
        );
        imported.push((case, (*home).to_string(), (*other).to_string(), lens_arn));
    }
    wait_for_the_import_to_land();

    for (case, home, other, lens_arn) in imported {
        let listed_home = client
            .send_in(&home, "GET", "/lenses?LensType=CUSTOM_SELF", None)
            .await;
        assert_eq!(
            summarized(&listed_home, "LensSummaries", "LensArn"),
            vec![lens_arn.clone()],
            "case {case:?}: the importing Region must list its own lens, got {listed_home:?}"
        );
        let listed_other = client
            .send_in(&other, "GET", "/lenses?LensType=CUSTOM_SELF", None)
            .await;
        assert_eq!(
            summarized(&listed_other, "LensSummaries", "LensArn"),
            Vec::<String>::new(),
            "case {case:?}: {other:?} imported no lens, got {listed_other:?}"
        );

        // The official catalog is published by every Region, under that Region's ARNs.
        let official = client
            .send_in(&other, "GET", "/lenses?LensType=AWS_OFFICIAL", None)
            .await;
        let official_arns = summarized(&official, "LensSummaries", "LensArn");
        assert!(
            !official_arns.is_empty()
                && official_arns
                    .iter()
                    .all(|arn| arn_region_is(arn, other.as_str())),
            "case {case:?}: {other:?} must publish the official lenses under its own Region, \
             got {official:?}"
        );

        let export_uri = format!("/lenses/{}/export", encode_path_segment(&lens_arn));
        assert_served(
            &client.send_in(&home, "GET", &export_uri, None).await,
            &case,
            "ExportLens",
        );
        assert_not_found(
            &client.send_in(&other, "GET", &export_uri, None).await,
            &case,
            "ExportLens",
        );
    }
}
