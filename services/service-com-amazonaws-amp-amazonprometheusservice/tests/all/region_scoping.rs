// SPDX-License-Identifier: CC0-1.0

//! Region scoping, end to end over HTTP.
//!
//! The rule under test: Amazon Managed Service for Prometheus is a REGIONAL service —
//! `CreateWorkspace` states "You can have one or more workspaces in each Region in your
//! account" — so a resource exists only in the Region whose endpoint created it. Every
//! listing and every lookup answers from the store of the Region the request is signed
//! for, an id or an ARN minted in another Region resolves to nothing there, and each
//! per-Region bound counts only that Region's own records.
//!
//! The rows span what the rule generalises over: the Regions (five of them, each holding
//! a workspace of its own, so every pair is exercised in both directions), the
//! workspace-scoped operation making the cross-Region call (every verb the model binds
//! under `/workspaces/{workspaceId}`), the resource kind (a workspace, a scraper), how
//! the resource is addressed (by id and by ARN), the idempotency token and the pagination
//! cursor, and the per-Region bounds (the workspace quota, and how many Regions the store
//! holds records for at once).
//!
//! The harness (`Amp`, `Reply`), the scraper fixtures and the request-body builders are
//! the ones the other suites in this target already build.

use crate::logging_configuration::log_group_arn;
use crate::rule_groups_namespace_lifecycle::create_body;
use crate::scraper_listing::{dataset_destination, eks_source, listed, scraper, seed};
use crate::workspace_lifecycle::{
    announced_prometheus_endpoint, tags_of, text, wait_for_due_transition, Amp, Reply,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    region_ceiling_message, workspace_arn, IngestedMetrics, Workspace, WorkspaceConfiguration,
    DESCRIBE_WORKSPACE_NOT_FOUND_MESSAGE, MAX_REGIONS, RESOURCE_NOT_FOUND_CODE,
    RESOURCE_NOT_FOUND_MESSAGE, STATUS_ACTIVE, WORKSPACE_NOT_FOUND_MESSAGE, WORKSPACE_QUOTA,
};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

/// The Regions this suite spans: enough of them, of different naming shapes, that no
/// assertion can hold by coincidence of there being only two.
const REGIONS: &[&str] = &[
    "us-west-2",
    "eu-north-1",
    "ap-southeast-2",
    "us-east-1",
    "me-central-1",
];

/// One stored workspace of `region`, settled with nothing owed by the clock — the record
/// a create leaves behind once its transition has come due.
///
/// Used where the rule under test is a COUNT the store holds, so the records are seeded
/// through the same engine the handlers read rather than created one request at a time.
fn workspace_record(region: &str, workspace_id: &str, alias: &str) -> Workspace {
    Workspace {
        workspace_id: workspace_id.to_string(),
        arn: workspace_arn(region, workspace_id),
        alias: Some(alias.to_string()),
        pending_alias: None,
        status_code: STATUS_ACTIVE.to_string(),
        created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        tags: BTreeMap::new(),
        kms_key_arn: None,
        anomaly_detectors: HashMap::new(),
        anomaly_detector_tokens: HashMap::new(),
        transition_due: None,
        delete_token: None,
        update_token: None,
        alert_manager_definition: None,
        configuration: WorkspaceConfiguration::default(),
        logging_configuration: None,
        policy: None,
        rule_groups_namespaces: BTreeMap::new(),
        metrics: IngestedMetrics::default(),
    }
}

/// The `workspaceId`s a `ListWorkspaces` page reported, in the order it reported them.
fn listed_workspaces(body: &serde_json::Value) -> Vec<String> {
    body["workspaces"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| text(&summary["workspaceId"]))
                .collect()
        })
        .unwrap_or_default()
}

/// Every assertion a cross-Region call on a workspace must satisfy: the modelled 404 for
/// a workspace this endpoint does not hold, naming the id that was asked for. `message` is
/// the sentence the OPERATION under test is recorded answering, which for
/// `DescribeWorkspace` is `DESCRIBE_WORKSPACE_NOT_FOUND_MESSAGE`.
fn assert_workspace_not_found(case: &str, reply: &Reply, workspace_id: &str, message: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: a workspace of another Region must not be found, got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["message"]),
        message,
        "case {case:?}: expected the workspace not-found sentence, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["resourceId"]),
        workspace_id,
        "case {case:?}: the refusal must name the workspace asked for, got {reply:?}"
    );
}

/// A workspace is reported only by the Region that created it, and the summary that
/// Region serves names that Region in its `arn` and its `prometheusEndpoint`.
#[tokio::test]
async fn a_workspace_is_reported_only_by_the_region_that_created_it() {
    let amp = Amp::new();

    // (the Region, the workspace it holds)
    let mut held: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let workspace_id = amp.active_workspace(region, &format!("home-{region}")).await;
        held.push((region, workspace_id));
    }

    for (region, workspace_id) in &held {
        let listing = amp.list(region, "").await;
        assert_eq!(
            listing.status, 200,
            "case {region:?}: the listing must answer 200, got {listing:?}"
        );
        assert_eq!(
            listed_workspaces(&listing.body),
            vec![workspace_id.clone()],
            "case {region:?}: a Region reports its OWN workspace and no other, got {listing:?}"
        );
        assert_eq!(
            text(&listing.body["workspaces"][0]["arn"]),
            workspace_arn(region, workspace_id),
            "case {region:?}: the summary's arn must name the Region serving it, got {listing:?}"
        );

        let described = amp.describe(region, workspace_id).await;
        assert_eq!(
            described.status, 200,
            "case {region:?}: the workspace must be described where it was created, got \
             {described:?}"
        );
        assert_eq!(
            text(&described.body["workspace"]["prometheusEndpoint"]),
            announced_prometheus_endpoint(region, workspace_id),
            "case {region:?}: the endpoint must name the host serving it, got {described:?}"
        );

        for (other, _) in &held {
            if other == region {
                continue;
            }
            let case = format!("the workspace of {region} read from {other}");
            assert_workspace_not_found(
                &case,
                &amp.describe(other, workspace_id).await,
                workspace_id,
                DESCRIBE_WORKSPACE_NOT_FOUND_MESSAGE,
            );
            let foreign_listing = amp.list(other, "").await;
            assert!(
                !listed_workspaces(&foreign_listing.body).contains(workspace_id),
                "case {case:?}: {other} must not report it, got {foreign_listing:?}"
            );
        }
    }
}

/// Every operation the model binds under `/workspaces/{workspaceId}` answers the modelled
/// not-found when it is signed for a Region other than the one holding the workspace,
/// while the same request in the Region that does hold it is NOT that refusal. One row per
/// bound verb and path, so the rule is asserted over the whole workspace-scoped surface
/// rather than over one reader.
#[tokio::test]
async fn every_workspace_scoped_operation_is_not_found_from_another_region() {
    let home = "us-west-2";
    let other = "eu-north-1";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(home, "home-only").await;
    let log_group = log_group_arn("aws", home, "/aws/prometheus/region-scoping");
    let policy_document =
        "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"Scoped\",\"Effect\":\"Allow\",\
         \"Principal\":{\"AWS\":\"*\"},\"Action\":[\"aps:QueryMetrics\"],\"Resource\":\"*\"}]}";

    // (case, method, the path under the workspace, the request body) — the bindings the
    // model declares, from `smithy_query(query="digest")`.
    let rows: Vec<(&str, &str, String, String)> = vec![
        ("DescribeWorkspace", "GET", String::new(), String::new()),
        (
            "DescribeWorkspaceConfiguration",
            "GET",
            "/configuration".to_string(),
            String::new(),
        ),
        (
            "UpdateWorkspaceConfiguration",
            "PATCH",
            "/configuration".to_string(),
            serde_json::json!({ "retentionPeriodInDays": 30 }).to_string(),
        ),
        (
            "UpdateWorkspaceAlias",
            "POST",
            "/alias".to_string(),
            serde_json::json!({ "alias": "renamed-from-elsewhere" }).to_string(),
        ),
        (
            "DescribeResourcePolicy",
            "GET",
            "/policy".to_string(),
            String::new(),
        ),
        (
            "PutResourcePolicy",
            "PUT",
            "/policy".to_string(),
            serde_json::json!({ "policyDocument": policy_document }).to_string(),
        ),
        (
            "DeleteResourcePolicy",
            "DELETE",
            "/policy".to_string(),
            String::new(),
        ),
        (
            "ListRuleGroupsNamespaces",
            "GET",
            "/rulegroupsnamespaces".to_string(),
            String::new(),
        ),
        (
            "CreateRuleGroupsNamespace",
            "POST",
            "/rulegroupsnamespaces".to_string(),
            create_body("from-elsewhere", b"groups: []", &[]).to_string(),
        ),
        (
            "DescribeRuleGroupsNamespace",
            "GET",
            "/rulegroupsnamespaces/from-elsewhere".to_string(),
            String::new(),
        ),
        (
            "PutRuleGroupsNamespace",
            "PUT",
            "/rulegroupsnamespaces/from-elsewhere".to_string(),
            serde_json::json!({ "data": aws_smithy_types::base64::encode(b"groups: []") })
                .to_string(),
        ),
        (
            "DeleteRuleGroupsNamespace",
            "DELETE",
            "/rulegroupsnamespaces/from-elsewhere".to_string(),
            String::new(),
        ),
        (
            "DescribeAlertManagerDefinition",
            "GET",
            "/alertmanager/definition".to_string(),
            String::new(),
        ),
        (
            "CreateAlertManagerDefinition",
            "POST",
            "/alertmanager/definition".to_string(),
            serde_json::json!({
                "data": aws_smithy_types::base64::encode(b"alertmanager_config: {}\n"),
            })
            .to_string(),
        ),
        (
            "DeleteAlertManagerDefinition",
            "DELETE",
            "/alertmanager/definition".to_string(),
            String::new(),
        ),
        (
            "ListAnomalyDetectors",
            "GET",
            "/anomalydetectors".to_string(),
            String::new(),
        ),
        (
            "DescribeLoggingConfiguration",
            "GET",
            "/logging".to_string(),
            String::new(),
        ),
        (
            "CreateLoggingConfiguration",
            "POST",
            "/logging".to_string(),
            serde_json::json!({ "logGroupArn": log_group.clone() }).to_string(),
        ),
        (
            "DescribeQueryLoggingConfiguration",
            "GET",
            "/logging/query".to_string(),
            String::new(),
        ),
        (
            "CreateQueryLoggingConfiguration",
            "POST",
            "/logging/query".to_string(),
            serde_json::json!({
                "destinations": [{
                    "cloudWatchLogs": { "logGroupArn": log_group.clone() },
                    "filters": { "qspThreshold": 100 },
                }],
            })
            .to_string(),
        ),
        // Last, because the request that IS served here starts the workspace's deletion.
        ("DeleteWorkspace", "DELETE", String::new(), String::new()),
    ];

    for (case, method, path, body) in &rows {
        let uri = format!("/workspaces/{workspace_id}{path}");
        let refused = amp.call(method, &uri, other, body).await;
        // `DescribeWorkspace` answers its own recorded spelling of the sentence; every
        // other workspace-addressed operation shares the one constant.
        let expected = if *case == "DescribeWorkspace" {
            DESCRIBE_WORKSPACE_NOT_FOUND_MESSAGE
        } else {
            WORKSPACE_NOT_FOUND_MESSAGE
        };
        assert_workspace_not_found(case, &refused, &workspace_id, expected);

        // The very same request, signed for the Region that holds the workspace, is not
        // the missing-workspace refusal — whatever it answers about the child resource it
        // names or the status that resource is in.
        let served = amp.call(method, &uri, home, body).await;
        assert_ne!(
            (served.status, text(&served.body["message"])),
            (404, expected.to_string()),
            "case {case:?}: the Region holding the workspace must not answer the \
             missing-workspace refusal, got {served:?}"
        );
    }
}

/// A scraper is addressed by id with no workspace in its path, and it too is reached only
/// from the Region whose store holds it: each Region lists its own scrapers, and every
/// id-addressed operation on another Region's scraper is a not-found.
#[tokio::test]
async fn a_scraper_is_reached_only_from_the_region_that_holds_it() {
    let home = "us-west-2";
    let other = "ap-southeast-2";
    let amp = Amp::new();

    let home_scraper = scraper(
        "s-11111111-1111-1111-1111-111111111111",
        Some("home"),
        STATUS_ACTIVE,
        eks_source("home-cluster"),
        dataset_destination("home-metrics"),
    );
    let other_scraper = scraper(
        "s-22222222-2222-2222-2222-222222222222",
        Some("other"),
        STATUS_ACTIVE,
        eks_source("other-cluster"),
        dataset_destination("other-metrics"),
    );
    seed(&amp, home, &home_scraper);
    seed(&amp, other, &other_scraper);

    // (the Region, the scraper it holds, the scraper only the other Region holds)
    let rows = [
        (home, &home_scraper, &other_scraper),
        (other, &other_scraper, &home_scraper),
    ];
    for (region, held, foreign) in rows {
        let listing = amp.call("GET", "/scrapers", region, "").await;
        assert_eq!(
            listing.status, 200,
            "case {region:?}: the listing must answer 200, got {listing:?}"
        );
        assert_eq!(
            listed(&listing.body),
            vec![held.scraper_id.clone()],
            "case {region:?}: a Region lists its OWN scrapers only, got {listing:?}"
        );

        let described = amp
            .call("GET", &format!("/scrapers/{}", held.scraper_id), region, "")
            .await;
        assert_eq!(
            described.status, 200,
            "case {region:?}: its own scraper must be describable, got {described:?}"
        );

        // (case, method, the path of the scraper the OTHER Region holds, body)
        let foreign_calls: Vec<(&str, &str, String, String)> = vec![
            (
                "DescribeScraper",
                "GET",
                format!("/scrapers/{}", foreign.scraper_id),
                String::new(),
            ),
            (
                "UpdateScraper",
                "PUT",
                format!("/scrapers/{}", foreign.scraper_id),
                serde_json::json!({ "alias": "renamed-from-elsewhere" }).to_string(),
            ),
            (
                "DeleteScraperLoggingConfiguration",
                "DELETE",
                format!("/scrapers/{}/logging-configuration", foreign.scraper_id),
                String::new(),
            ),
            (
                "DescribeScraperLoggingConfiguration",
                "GET",
                format!("/scrapers/{}/logging-configuration", foreign.scraper_id),
                String::new(),
            ),
            (
                "DeleteScraper",
                "DELETE",
                format!("/scrapers/{}", foreign.scraper_id),
                String::new(),
            ),
        ];
        for (operation, method, uri, body) in &foreign_calls {
            let case = format!("{operation} in {region} on the scraper of the other Region");
            let reply = amp.call(method, uri, region, body).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: another Region's scraper must not be found, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["resourceId"]),
                foreign.scraper_id,
                "case {case:?}: the refusal must name the scraper asked for, got {reply:?}"
            );
        }

        // The refusals changed nothing: the Region that holds the foreign scraper still
        // reports it.
        let foreign_region = if region == home { other } else { home };
        assert_eq!(
            listed(&amp.call("GET", "/scrapers", foreign_region, "").await.body),
            vec![foreign.scraper_id.clone()],
            "case {region:?}: the refused calls must leave the other Region's scraper alone"
        );
    }
}

/// `clientToken` makes a create idempotent within the Region it was sent to: the same
/// token sent to several Regions is several requests, each creating that Region's own
/// workspace, while a replay inside one Region answers from the record that Region holds.
#[tokio::test]
async fn an_idempotency_token_is_scoped_to_the_region_it_was_sent_to() {
    let amp = Amp::new();
    let body = serde_json::json!({
        "alias": "token-scoped",
        "clientToken": "one-token-several-regions",
    })
    .to_string();

    // (the Region, the workspace its create minted)
    let mut created: Vec<(&str, String)> = Vec::new();
    for region in &REGIONS[..3] {
        let reply = amp.call("POST", "/workspaces", region, &body).await;
        assert_eq!(
            reply.status, 202,
            "case {region:?}: the create must answer its modelled 202, got {reply:?}"
        );
        created.push((region, text(&reply.body["workspaceId"])));
    }

    for (region, workspace_id) in &created {
        let replayed = amp.call("POST", "/workspaces", region, &body).await;
        assert_eq!(
            text(&replayed.body["workspaceId"]),
            *workspace_id,
            "case {region:?}: a replay in the same Region must answer from that Region's \
             record, got {replayed:?}"
        );
        for (other, other_id) in &created {
            if other == region {
                continue;
            }
            assert_ne!(
                workspace_id, other_id,
                "case {region:?}: the token must not hand {other} this Region's workspace, \
                 got {workspace_id:?} and {other_id:?}"
            );
        }
    }
}

/// A listing's `nextToken` is a cursor over the Region that issued it: presented to
/// another Region it can only narrow that Region's own page, never reach into the Region
/// the token came from.
#[tokio::test]
async fn a_cursor_one_region_issued_pages_only_that_regions_records() {
    let home = "us-west-2";
    let other = "eu-north-1";
    let amp = Amp::new();

    let mut home_ids: Vec<String> = Vec::new();
    for index in 0..3 {
        home_ids.push(amp.active_workspace(home, &format!("home-{index}")).await);
    }
    let mut other_ids: Vec<String> = Vec::new();
    for index in 0..2 {
        other_ids.push(amp.active_workspace(other, &format!("other-{index}")).await);
    }

    let first_page = amp.list(home, "maxResults=1").await;
    let cursor = text(&first_page.body["nextToken"]);
    assert!(
        !cursor.is_empty(),
        "a listing with more to serve must carry a nextToken, got {first_page:?}"
    );

    let borrowed = amp.list(other, &format!("nextToken={cursor}")).await;
    assert_eq!(
        borrowed.status, 200,
        "a cursor from another Region is a value the member accepts, got {borrowed:?}"
    );
    let served = listed_workspaces(&borrowed.body);
    assert!(
        served
            .iter()
            .all(|workspace_id| other_ids.contains(workspace_id)),
        "a borrowed cursor must serve only the Region it was presented to, expected a \
         subset of {other_ids:?}, got {served:?}"
    );
    assert!(
        home_ids
            .iter()
            .all(|workspace_id| !served.contains(workspace_id)),
        "a borrowed cursor must not reach the Region that issued it, got {served:?}"
    );
}

/// An ARN is resolved against the records of the Region serving the request, so the
/// ARN-addressed tagging operations answer not-found for a resource of another Region —
/// and those refusals leave the resource's own tags where they were.
#[tokio::test]
async fn an_arn_from_another_region_resolves_to_nothing() {
    let home = "us-west-2";
    let other = "me-central-1";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(home, "tagged-at-home").await;
    let arn = workspace_arn(home, &workspace_id);
    let tagged = amp
        .tag(home, &arn, &serde_json::json!({ "team": "metrics" }))
        .await;
    assert_eq!(
        tagged.status, 200,
        "tagging in the Region that holds the workspace must be served, got {tagged:?}"
    );

    // (case, the reply to that call signed for the other Region)
    let refusals = [
        ("ListTagsForResource", amp.list_tags(other, &arn).await),
        (
            "TagResource",
            amp.tag(other, &arn, &serde_json::json!({ "team": "elsewhere" }))
                .await,
        ),
        ("UntagResource", amp.untag(other, &arn, &["team"]).await),
        (
            "ListTagsForResource with the ARN rebased on the serving Region",
            amp.list_tags(other, &workspace_arn(other, &workspace_id))
                .await,
        ),
    ];
    for (case, reply) in &refusals {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an ARN of another Region must resolve to nothing, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            RESOURCE_NOT_FOUND_CODE,
            "case {case:?}: expected the code the tagging path answers, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the ARN not-found sentence, got {reply:?}"
        );
    }

    let listed_tags = amp.list_tags(home, &arn).await;
    assert_eq!(
        tags_of(&listed_tags.body["tags"]),
        BTreeMap::from([("team".to_string(), "metrics".to_string())]),
        "the refused calls must leave the workspace's own tags where they were, got \
         {listed_tags:?}"
    );
}

/// Each per-Region bound counts only that Region's own records: a Region holding
/// `WORKSPACE_QUOTA` workspaces refuses a further create with the modelled
/// `ServiceQuotaExceededException`, while every other Region still has all its room.
#[tokio::test]
async fn the_workspace_quota_counts_only_the_records_of_its_own_region() {
    let full = "us-west-2";
    let empty = "eu-north-1";
    let amp = Amp::new();
    {
        let store = amp.store();
        let mut guard = store.lock();
        let records = guard.region_mut_for_create(full);
        for index in 0..WORKSPACE_QUOTA {
            let workspace_id = format!("ws-quota-{index:04}");
            records.workspaces.insert(
                workspace_id.clone(),
                workspace_record(full, &workspace_id, &format!("bulk-{index:04}")),
            );
        }
    }

    let refused = amp
        .call(
            "POST",
            "/workspaces",
            full,
            &serde_json::json!({ "alias": "one-too-many" }).to_string(),
        )
        .await;
    assert_eq!(
        refused.status, 402,
        "case {full:?}: a create past the Region's quota must be refused, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "case {full:?}: expected the modelled quota refusal, got {refused:?}"
    );

    let accepted = amp
        .call(
            "POST",
            "/workspaces",
            empty,
            &serde_json::json!({ "alias": "room-of-its-own" }).to_string(),
        )
        .await;
    assert_eq!(
        accepted.status, 202,
        "case {empty:?}: a full Region must not spend another Region's quota, got {accepted:?}"
    );

    let listing = amp.list(empty, "").await;
    assert_eq!(
        listed_workspaces(&listing.body),
        vec![text(&accepted.body["workspaceId"])],
        "case {empty:?}: the Region must report only the workspace created in it, got {listing:?}"
    );
}

/// A Region a caller only READ, or wrote nothing in, leaves nothing behind: the store
/// holds records for the Regions that have them, so a refused call under a fresh Region
/// name neither occupies a slot under the ceiling nor changes what the next caller sees.
#[tokio::test]
async fn a_region_nothing_was_created_in_occupies_no_slot() {
    let amp = Amp::new();
    let workspace_id = "ws-00000000-0000-0000-0000-000000000000";

    // Far more Region names than the store holds slots for, each carrying calls that
    // write nothing: a delete and a tagging call on a resource no Region holds.
    for index in 0..(MAX_REGIONS * 4) {
        let region = format!("emulator-empty-region-{index}");
        let deleted = amp.delete(&region, workspace_id, None).await;
        assert_eq!(
            deleted.status, 404,
            "case {region:?}: a delete in a Region holding nothing must be a 404, got \
             {deleted:?}"
        );
        let tagged = amp
            .tag(
                &region,
                &workspace_arn(&region, workspace_id),
                &serde_json::json!({ "team": "metrics" }),
            )
            .await;
        assert_eq!(
            tagged.status, 404,
            "case {region:?}: tagging in a Region holding nothing must be a 404, got \
             {tagged:?}"
        );
    }

    // The store still has room for every Region it may hold, which it would not if the
    // calls above had each taken a slot.
    for index in 0..MAX_REGIONS {
        let region = format!("emulator-region-{index}");
        let created = amp
            .call(
                "POST",
                "/workspaces",
                &region,
                &serde_json::json!({ "alias": "after-the-empty-regions" }).to_string(),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {region:?}: the store must still have room for this Region, got {created:?}"
        );
    }
}

/// The Region of a request is the caller's own credential scope, so the store bounds how
/// many Regions it holds records for: past the ceiling a create that would give a further
/// Region its first record is refused and writes nothing, while the Regions already held
/// go on taking creates.
#[tokio::test]
async fn a_create_past_the_region_ceiling_is_refused_and_writes_nothing() {
    let amp = Amp::new();
    // Region-shaped names, one per slot the store holds: `WorkspaceArn`'s published
    // pattern holds the Region segment to `[-a-z0-9]+`, which these satisfy.
    let filled: Vec<String> = (0..MAX_REGIONS)
        .map(|index| format!("emulator-region-{index}"))
        .collect();
    for region in &filled {
        let created = amp
            .call(
                "POST",
                "/workspaces",
                region,
                &serde_json::json!({ "alias": "filling" }).to_string(),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {region:?}: filling the store must answer 202, got {created:?}"
        );
    }

    let overflow_region = "emulator-region-overflow";
    let overflow = amp
        .call(
            "POST",
            "/workspaces",
            overflow_region,
            &serde_json::json!({ "alias": "one-region-too-many" }).to_string(),
        )
        .await;
    assert_eq!(
        (overflow.status, overflow.error_code()),
        (402, "ServiceQuotaExceededException".to_string()),
        "case {overflow_region:?}: a create in a further Region must be refused, got {overflow:?}"
    );
    assert_eq!(
        text(&overflow.body["message"]),
        region_ceiling_message(),
        "case {overflow_region:?}: the refusal must name the ceiling it reached, got {overflow:?}"
    );
    assert_eq!(
        listed_workspaces(&amp.list(overflow_region, "").await.body),
        Vec::<String>::new(),
        "case {overflow_region:?}: the refused create must write nothing"
    );

    let held = &filled[0];
    let second = amp
        .call(
            "POST",
            "/workspaces",
            held,
            &serde_json::json!({ "alias": "second-of-its-region" }).to_string(),
        )
        .await;
    assert_eq!(
        second.status, 202,
        "case {held:?}: a Region the full store already holds must keep taking creates, \
         got {second:?}"
    );
    wait_for_due_transition();
    assert_eq!(
        listed_workspaces(&amp.list(held, "").await.body).len(),
        2,
        "case {held:?}: the Region must report both of its workspaces"
    );
}
