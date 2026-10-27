// SPDX-License-Identifier: CC0-1.0

//! A scraper's `destination` is a REFERENCE to another resource, so every operation that
//! STORES one resolves it before it is persisted and answers the modelled
//! `ResourceNotFoundException` (404) for a reference that does not resolve — "The request
//! references a resources that doesn't exist."
//!
//! The reference this service can resolve is a workspace of its own in the account the
//! emulator serves: an `ampConfiguration` naming a workspace ARN of this account that this
//! store holds no workspace for does not resolve, while a destination in another account
//! (the cross-account case `roleConfiguration` exists to enable) or in another service
//! names a resource no local state speaks for and is taken as given.
//!
//! The rule is asserted over the TWO operations that store a destination —
//! `smithy_query(query="operation",operation="CreateScraper,UpdateScraper")` gives both the
//! member and the error — so the same reference earns the same answer whether it arrives at
//! the create or at the update, and a refusal leaves the stored scraper exactly as it was.
//! Every assertion reads the scraper back through `DescribeScraper`, so what is checked is
//! what the service PERSISTED.

use crate::scraper_creation::{dataset_destination_json, eks_source_json, workspace_destination_json};
use crate::scraper_update::scrape_configuration_json;
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    STATUS_ACTIVE, WORKSPACE_NOT_FOUND_MESSAGE, WORKSPACE_RESOURCE_TYPE,
};

/// `POST /scrapers` with a destination and the two other `@required` members.
async fn create_scraper(amp: &Amp, region: &str, destination: &serde_json::Value) -> Reply {
    let body = serde_json::json!({
        "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
        "source": eks_source_json(region, "resolution-cluster", &["subnet-0123456789abcdef0"], &[]),
        "destination": destination,
    });
    amp.call("POST", "/scrapers", region, &body.to_string()).await
}

/// `PUT /scrapers/{scraperId}` carrying the destination alone.
async fn update_scraper(
    amp: &Amp,
    region: &str,
    scraper_id: &str,
    destination: &serde_json::Value,
) -> Reply {
    let body = serde_json::json!({ "destination": destination });
    amp.call(
        "PUT",
        &format!("/scrapers/{scraper_id}"),
        region,
        &body.to_string(),
    )
    .await
}

/// `GET /scrapers/{scraperId}` — the reader every assertion goes through.
async fn describe_scraper(amp: &Amp, region: &str, scraper_id: &str) -> Reply {
    amp.call("GET", &format!("/scrapers/{scraper_id}"), region, "")
        .await
}

/// One scraper, created through `CreateScraper` with a destination that resolves and
/// settled to `ACTIVE` by the clock — the state an update is allowed in.
async fn active_scraper(amp: &Amp, region: &str, destination: &serde_json::Value) -> String {
    let created = create_scraper(amp, region, destination).await;
    assert_eq!(
        created.status, 202,
        "the scraper to update must be creatable, got {created:?}"
    );
    wait_for_due_transition();
    text(&created.body["scraperId"])
}

/// What one destination must earn: accepted, or refused naming the workspace that did not
/// resolve.
type DestinationCase = (&'static str, serde_json::Value, Option<String>);

#[tokio::test]
async fn a_scraper_destination_is_resolved_wherever_it_is_stored() {
    let region = "us-west-2";
    let account = engine::emulator_account_id();
    let amp = Amp::new();

    // Created through the service's own operations, so a destination that resolves resolves
    // to a workspace this service itself minted.
    let held_workspace_id = amp.active_workspace(region, "resolution").await;
    let described = amp.describe(region, &held_workspace_id).await;
    assert_eq!(
        described.status, 200,
        "the destination workspace must be describable, got {described:?}"
    );
    let held_workspace_arn = text(&described.body["workspace"]["arn"]);

    let scraper_id = active_scraper(
        &amp,
        region,
        &workspace_destination_json(&held_workspace_arn),
    )
    .await;

    let absent_workspace_id = "ws-00000000-0000-0000-0000-000000000000";
    // Every `ampConfiguration` row below is a WELL-FORMED workspace ARN, because
    // `AmpConfiguration$workspaceArn` is `pattern
    // ^arn:aws[-a-z]*:aps:[-a-z0-9]+:[0-9]{12}:workspace/.+$`
    // (`smithy_query(query="shape",shape="AmpConfiguration")`) and a value that fails it is
    // refused by the request walk before any reference is resolved — a rule
    // `crate::wire_fidelity` asserts. What varies here is what the ARN NAMES: the account,
    // the partition, the region and the workspace id.
    let cases: Vec<DestinationCase> = vec![
        (
            "the workspace this account holds",
            workspace_destination_json(&held_workspace_arn),
            None,
        ),
        (
            "a workspace id this account does not hold",
            workspace_destination_json(&format!(
                "arn:aws:aps:{region}:{account}:workspace/{absent_workspace_id}"
            )),
            Some(absent_workspace_id.to_string()),
        ),
        (
            "the held workspace's id in another region",
            workspace_destination_json(&format!(
                "arn:aws:aps:eu-central-1:{account}:workspace/{held_workspace_id}"
            )),
            Some(held_workspace_id.clone()),
        ),
        (
            "the held workspace's id in another partition",
            workspace_destination_json(&format!(
                "arn:aws-cn:aps:{region}:{account}:workspace/{held_workspace_id}"
            )),
            Some(held_workspace_id.clone()),
        ),
        (
            "the held workspace's id differing only in case",
            workspace_destination_json(&format!(
                "arn:aws:aps:{region}:{account}:workspace/{}",
                held_workspace_id.to_uppercase()
            )),
            Some(held_workspace_id.to_uppercase()),
        ),
        (
            "a workspace in another account",
            workspace_destination_json(&format!(
                "arn:aws:aps:{region}:123456789012:workspace/{absent_workspace_id}"
            )),
            None,
        ),
        (
            "a CloudWatch dataset",
            dataset_destination_json(region, "fleet-metrics"),
            None,
        ),
        (
            "the default CloudWatch dataset",
            dataset_destination_json(region, "default"),
            None,
        ),
    ];

    for (case, destination, unresolved) in cases {
        let before = describe_scraper(&amp, region, &scraper_id).await;
        assert_eq!(
            text(&before.body["scraper"]["status"]["statusCode"]),
            STATUS_ACTIVE,
            "case {case:?}: the scraper under update must start {STATUS_ACTIVE}, got {before:?}"
        );

        let created = create_scraper(&amp, region, &destination).await;
        let updated = update_scraper(&amp, region, &scraper_id, &destination).await;
        let after = describe_scraper(&amp, region, &scraper_id).await;

        let Some(unresolved) = unresolved else {
            assert_eq!(
                created.status, 202,
                "case {case:?}: CreateScraper must accept a reference this service cannot \
                 contradict, got {created:?}"
            );
            assert_eq!(
                updated.status, 202,
                "case {case:?}: UpdateScraper must accept the same reference CreateScraper \
                 accepts, got {updated:?}"
            );
            let minted = text(&created.body["scraperId"]);
            let persisted_by_create = describe_scraper(&amp, region, &minted).await;
            assert_eq!(
                persisted_by_create.body["scraper"]["destination"], destination,
                "case {case:?}: the created scraper must persist the destination as sent, got \
                 {persisted_by_create:?}"
            );
            assert_eq!(
                after.body["scraper"]["destination"], destination,
                "case {case:?}: the updated scraper must persist the destination as sent, got \
                 {after:?}"
            );
            // Let the settling the accepted update owes happen, so the next row starts from
            // an ACTIVE scraper.
            wait_for_due_transition();
            continue;
        };

        for (operation, reply) in [("CreateScraper", &created), ("UpdateScraper", &updated)] {
            assert_eq!(
                reply.status, 404,
                "case {case:?}: {operation} must refuse a reference that does not resolve \
                 with 404, got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: {operation} must answer the modelled \
                 ResourceNotFoundException, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["message"]),
                WORKSPACE_NOT_FOUND_MESSAGE,
                "case {case:?}: {operation} must report the service's not-found sentence, got \
                 {reply:?}"
            );
            assert_eq!(
                text(&reply.body["resourceId"]),
                unresolved,
                "case {case:?}: {operation} must name the workspace that did not resolve, got \
                 {reply:?}"
            );
            assert_eq!(
                text(&reply.body["resourceType"]),
                WORKSPACE_RESOURCE_TYPE,
                "case {case:?}: {operation} must name the resource type, got {reply:?}"
            );
        }

        assert_eq!(
            after.body["scraper"]["destination"], before.body["scraper"]["destination"],
            "case {case:?}: a refused update must leave the stored destination alone, \
             expected {:?}, got {:?}",
            before.body["scraper"]["destination"], after.body["scraper"]["destination"]
        );
        assert_eq!(
            text(&after.body["scraper"]["status"]["statusCode"]),
            STATUS_ACTIVE,
            "case {case:?}: a refused update must not move the status, got {after:?}"
        );
        assert_eq!(
            after.body["scraper"]["lastModifiedAt"], before.body["scraper"]["lastModifiedAt"],
            "case {case:?}: a refused update must not move lastModifiedAt, expected {:?}, got \
             {:?}",
            before.body["scraper"]["lastModifiedAt"], after.body["scraper"]["lastModifiedAt"]
        );
    }
}

/// A destination the update does not CARRY is not a reference this call stores, so an
/// update that names other members is accepted whatever the stored destination is — and the
/// stored one is left exactly as it was.
#[tokio::test]
async fn an_update_that_carries_no_destination_resolves_nothing() {
    let region = "us-west-2";
    let amp = Amp::new();
    let held_workspace_id = amp.active_workspace(region, "untouched").await;
    let described = amp.describe(region, &held_workspace_id).await;
    let held_workspace_arn = text(&described.body["workspace"]["arn"]);
    let destination = workspace_destination_json(&held_workspace_arn);
    let scraper_id = active_scraper(&amp, region, &destination).await;

    // The workspace the stored destination names goes away, so the stored reference no
    // longer resolves — a deletion the service's own operation performs.
    let deleted = amp
        .call(
            "DELETE",
            &format!("/workspaces/{held_workspace_id}"),
            region,
            "",
        )
        .await;
    assert_eq!(
        deleted.status, 202,
        "the destination workspace must be deletable, got {deleted:?}"
    );
    wait_for_due_transition();

    let bodies = [
        ("the alias alone", serde_json::json!({ "alias": "renamed" })),
        ("no updatable member at all", serde_json::json!({})),
        (
            "the scrape configuration alone",
            serde_json::json!({
                "scrapeConfiguration": scrape_configuration_json("scrape_configs: []\n"),
            }),
        ),
    ];

    for (case, body) in bodies {
        let reply = amp
            .call(
                "PUT",
                &format!("/scrapers/{scraper_id}"),
                region,
                &body.to_string(),
            )
            .await;
        assert_eq!(
            reply.status, 202,
            "case {case:?}: an update carrying no destination must be accepted, got {reply:?}"
        );
        let after = describe_scraper(&amp, region, &scraper_id).await;
        assert_eq!(
            after.body["scraper"]["destination"], destination,
            "case {case:?}: the stored destination must be left as it was, got {after:?}"
        );
        wait_for_due_transition();
    }
}
