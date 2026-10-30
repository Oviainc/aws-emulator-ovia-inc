// SPDX-License-Identifier: CC0-1.0

//! How an MSK configuration's STATE and its optional `Description` are reported,
//! driven end to end over the wire through the same `MultiServiceRouter` the
//! emulator serves from (the shared `kafka_harness`).
//!
//! Two rules, each asserted over a TABLE of inputs rather than one request:
//!
//! * a configuration's deletion is asynchronous, so a call that CHANGES a
//!   configuration whose delete is in flight is refused on the STATE it is in —
//!   distinct from the refusal of a reference the endpoint resolves to nothing,
//!   which is what the same call answers only once the removal has come due. The
//!   rows differ along how many revisions the configuration holds, whether it
//!   was given a description, the body it carries and the signing region: the
//!   refusal is the state's, not one request shape's.
//! * a member the model leaves OPTIONAL is reported only when the configuration
//!   HAS a value for it, and an empty description is no description: a
//!   configuration created or updated without a `Description`, and one created
//!   with an empty one, are both reported with no `Description` member at all.
//!   The rows walk the description forms a request can carry (absent, empty,
//!   blank, ASCII, non-ASCII, long) across every reader that reports one.

use serde_json::{json, Value};

// The service's own state values, so the suite and the handlers cannot drift
// apart on the spellings the `ConfigurationState` enum declares.
use service_com_amazonaws_kafka_kafka::state::{
    CONFIGURATION_STATE_ACTIVE, CONFIGURATION_STATE_DELETING,
};

use super::kafka_harness::{
    assert_configuration_arn_resolves_to_nothing, assert_configuration_state_refusal, pct,
    sleep_past_configuration_deletion, ConfigurationRequest, Emulator, Reply, REGION,
};

/// `DeleteConfiguration`, which the model binds to
/// `DELETE /v1/configurations/{Arn}`.
async fn delete_in(emulator: &Emulator, region: &str, arn: &str) -> Reply {
    emulator
        .call_in(
            region,
            "DELETE",
            &format!("/v1/configurations/{}", pct(arn)),
            None,
        )
        .await
}

/// `DescribeConfiguration`, which the model binds to
/// `GET /v1/configurations/{Arn}`.
async fn describe_in(emulator: &Emulator, region: &str, arn: &str) -> Reply {
    emulator
        .call_in(
            region,
            "GET",
            &format!("/v1/configurations/{}", pct(arn)),
            None,
        )
        .await
}

/// The `State` `ListConfigurations` reports for `arn` at `region`'s endpoint, or
/// `None` when that listing does not report the configuration at all.
async fn listed_state(emulator: &Emulator, region: &str, arn: &str) -> Option<String> {
    let listed = emulator
        .call_in(region, "GET", "/v1/configurations", None)
        .await;
    assert_eq!(
        listed.status, 200,
        "expected ListConfigurations in {region} to succeed, got {} {}",
        listed.status, listed.body
    );
    listed.body["configurations"]
        .as_array()?
        .iter()
        .find(|entry| entry.get("arn").and_then(Value::as_str) == Some(arn))
        .map(|entry| {
            entry
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
}

/// One row of the delete-window table: the configuration to create (whose own
/// members the rule must not depend on), the region whose endpoint serves it,
/// and the body a refused update submits.
#[derive(Debug, Clone)]
struct WindowRow {
    case: &'static str,
    region: &'static str,
    request: ConfigurationRequest,
    update_body: Vec<u8>,
}

/// The table: the rows differ along the number of revisions the configuration
/// already holds, whether it was created with a description, the size and byte
/// content of the `server.properties` the refused update submits, and the
/// signing region — a configuration lives in one region, and the state gate is
/// the same at every endpoint that holds one.
fn window_rows() -> Vec<WindowRow> {
    vec![
        WindowRow {
            case: "one-revision",
            region: REGION,
            request: ConfigurationRequest::named("window-one-revision"),
            update_body: b"num.partitions=9\n".to_vec(),
        },
        WindowRow {
            case: "three-revisions",
            region: REGION,
            request: ConfigurationRequest {
                description: Some("a described configuration".to_string()),
                extra_revisions: vec![
                    b"num.partitions=2\n".to_vec(),
                    b"num.partitions=4\n".to_vec(),
                ],
                ..ConfigurationRequest::named("window-three-revisions")
            },
            update_body: Vec::new(),
        },
        WindowRow {
            case: "empty-description-and-binary-update",
            region: REGION,
            request: ConfigurationRequest {
                description: Some(String::new()),
                kafka_versions: vec!["3.6.0".to_string()],
                ..ConfigurationRequest::named("window-binary")
            },
            // The body is a blob, so a refused update may carry bytes that are
            // not UTF-8 anywhere: the refusal is decided before the body is
            // looked at.
            update_body: vec![0x00, 0xff, 0xfe, 0x10, 0x80],
        },
        WindowRow {
            case: "another-region",
            region: "eu-central-1",
            request: ConfigurationRequest {
                server_properties: b"log.retention.hours=168\n".to_vec(),
                ..ConfigurationRequest::named("window-elsewhere")
            },
            update_body: b"num.partitions=1\n".repeat(64),
        },
    ]
}

/// RULE: the deletion of a configuration is asynchronous, so between the accepted
/// delete and the removal the clock owes it, the configuration is still a
/// resource of its endpoint: the readers resolve it with State `DELETING` and the
/// listing enumerates it. A call that CHANGES it in that window is refused on the
/// state it is in — naming the states a change IS accepted from — and writes
/// nothing; only once the removal has come due does the SAME call answer the
/// family's unresolvable-reference refusal.
#[tokio::test]
async fn a_change_during_the_delete_window_is_refused_on_state_not_as_a_lookup_miss() {
    let emulator = Emulator::new();

    for row in window_rows() {
        let region = row.region;
        let arn = emulator
            .create_configuration_row_ok_in(region, &row.request)
            .await;
        let before = describe_in(&emulator, region, &arn).await;
        assert_eq!(
            (before.status, before.body["state"].as_str()),
            (200, Some(CONFIGURATION_STATE_ACTIVE)),
            "case {}: the configuration must be ACTIVE before the delete, got {} {}",
            row.case,
            before.status,
            before.body
        );
        let revision_before = before.at("/latestRevision/revision").as_i64();

        // The delete is ACCEPTED, and reports the state it put the record in.
        let deleted = delete_in(&emulator, region, &arn).await;
        assert_eq!(
            (deleted.status, deleted.body["state"].as_str()),
            (200, Some(CONFIGURATION_STATE_DELETING)),
            "case {}: expected the delete to be accepted as DELETING, got {} {}",
            row.case,
            deleted.status,
            deleted.body
        );

        // In the window: the configuration is still there, in DELETING.
        let described = describe_in(&emulator, region, &arn).await;
        assert_eq!(
            (described.status, described.body["state"].as_str()),
            (200, Some(CONFIGURATION_STATE_DELETING)),
            "case {}: a configuration whose delete is in flight must still be readable, \
             in DELETING, got {} {}",
            row.case,
            described.status,
            described.body
        );
        assert_eq!(
            listed_state(&emulator, region, &arn).await.as_deref(),
            Some(CONFIGURATION_STATE_DELETING),
            "case {}: a configuration whose delete is in flight must still be listed, \
             with State DELETING",
            row.case
        );

        // A call that CHANGES it is refused on the state, whatever it submits.
        let refused = emulator
            .update_configuration_in(region, &arn, None, &row.update_body)
            .await;
        assert_configuration_state_refusal(&refused, &format!("{} update-in-window", row.case));
        let refused_again = emulator
            .update_configuration_in(region, &arn, Some("a retried update"), &row.update_body)
            .await;
        assert_configuration_state_refusal(
            &refused_again,
            &format!("{} retried-update-in-window", row.case),
        );
        let refused_delete = delete_in(&emulator, region, &arn).await;
        assert_configuration_state_refusal(
            &refused_delete,
            &format!("{} repeated-delete-in-window", row.case),
        );

        // The refused calls wrote nothing: no revision was appended.
        let after_refusals = describe_in(&emulator, region, &arn).await;
        assert_eq!(
            after_refusals.at("/latestRevision/revision").as_i64(),
            revision_before,
            "case {}: a refused change must append no revision, got {}",
            row.case,
            after_refusals.body
        );

        sleep_past_configuration_deletion();

        // The removal has come due, so the very same update is now the family's
        // unresolvable-reference refusal — a different message for a different
        // condition — and the configuration is gone from the listing.
        let gone = emulator
            .update_configuration_in(region, &arn, None, &row.update_body)
            .await;
        assert_configuration_arn_resolves_to_nothing(
            &gone,
            &format!("{} update-after-removal", row.case),
            &arn,
            region,
        );
        assert_eq!(
            listed_state(&emulator, region, &arn).await,
            None,
            "case {}: the configuration must leave the listing once the removal has come due",
            row.case
        );
    }
}

/// One row of the description table: what the request carries for `Description`,
/// and therefore what every reader must report.
#[derive(Debug, Clone, Copy)]
struct DescriptionRow {
    case: &'static str,
    /// `None` means the request OMITS the member.
    description: Option<&'static str>,
}

impl DescriptionRow {
    /// What every OPTIONAL `Description` member must report for this row: the
    /// submitted value, and nothing at all where the request omitted the member
    /// or carried an empty one — an empty description is no description.
    fn expected(&self) -> Option<&'static str> {
        self.description
            .filter(|description| !description.is_empty())
    }
}

/// The table: every form a `Description` can take on the wire, from absent to a
/// long non-ASCII string. `Description` is a plain `__string` in the model with
/// no length or pattern constraint, so the rows span the member's forms rather
/// than a range of legal values.
fn description_rows() -> Vec<DescriptionRow> {
    vec![
        DescriptionRow {
            case: "absent",
            description: None,
        },
        DescriptionRow {
            case: "empty",
            description: Some(""),
        },
        DescriptionRow {
            case: "ascii",
            description: Some("the broker defaults"),
        },
        DescriptionRow {
            case: "non-ascii",
            description: Some("réglage — настройка 構成"),
        },
        DescriptionRow {
            case: "blank-but-not-empty",
            description: Some("   "),
        },
        DescriptionRow {
            case: "long",
            description: Some(
                "a description long enough to rule out a length the handler might impose: \
                 it is a plain string member with no modelled constraint at all",
            ),
        },
    ]
}

/// RULE: a `Description` member the model leaves OPTIONAL is reported only when
/// the configuration (or the revision) HAS a description, so a request that
/// omitted the member and a request that carried an empty one are both answered
/// with no member at all, while any other value is reported verbatim. The one
/// member the model marks `@required` (`Configuration$Description`, which
/// `ListConfigurations` reports) is always written, and carries the empty string
/// where there is no description.
#[tokio::test]
async fn an_optional_description_is_omitted_exactly_when_there_is_none() {
    let emulator = Emulator::new();

    for row in description_rows() {
        let request = ConfigurationRequest {
            description: row.description.map(str::to_string),
            ..ConfigurationRequest::named(&format!("described-{}", row.case))
        };
        let arn = emulator
            .create_configuration_row_ok_in(REGION, &request)
            .await;

        // The create's own report of revision 1.
        let created = emulator
            .call_in(REGION, "GET", &format!("/v1/configurations/{}", pct(&arn)), None)
            .await;
        assert_eq!(
            created.status, 200,
            "case {}: expected DescribeConfiguration to succeed, got {} {}",
            row.case, created.status, created.body
        );
        // The configuration's own description, through the reader whose member is
        // optional: absent when the configuration has none.
        assert_eq!(
            created.body.get("description").and_then(Value::as_str),
            row.expected(),
            "case {}: DescribeConfiguration must report the description exactly as given, \
             and no member when there is none, got {}",
            row.case,
            created.body
        );
        assert_eq!(
            created.body.get("description").is_some(),
            row.expected().is_some(),
            "case {}: the Description member must be present exactly when the \
             configuration has a description, got {}",
            row.case,
            created.body
        );
        // Revision 1 carries the same description, through the same rule.
        assert_eq!(
            created.at("/latestRevision/description").as_str(),
            row.expected(),
            "case {}: revision 1 must report the create's description, got {}",
            row.case,
            created.body
        );
        let revision = emulator.describe_configuration_revision(&arn, 1).await;
        assert_eq!(
            revision.body.get("description").and_then(Value::as_str),
            row.expected(),
            "case {}: DescribeConfigurationRevision must report the stored description, got {}",
            row.case,
            revision.body
        );

        // `Configuration$Description` is `@required` on the shape the listing
        // reports, so that member is always written — with the empty string
        // standing in where the configuration has no description.
        let listed = emulator
            .call_in(REGION, "GET", "/v1/configurations", None)
            .await;
        let entry = listed.body["configurations"]
            .as_array()
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.get("arn").and_then(Value::as_str) == Some(arn.as_str()))
            })
            .unwrap_or_else(|| panic!("case {}: {arn} is missing from {}", row.case, listed.body));
        assert_eq!(
            entry.get("description").and_then(Value::as_str),
            Some(row.expected().unwrap_or_default()),
            "case {}: the listing's required Description member must always be written, \
             got {entry}",
            row.case
        );
        assert_eq!(
            entry.pointer("/latestRevision/description").and_then(Value::as_str),
            row.expected(),
            "case {}: the listed revision's optional Description follows the same rule as \
             the reader's, got {entry}",
            row.case
        );

        // The same rule holds for a revision an UPDATE stores, and each revision
        // keeps its own description: every form in the table is appended to this
        // one configuration and read back.
        for (index, appended) in description_rows().iter().enumerate() {
            let expected_revision = index as i64 + 2;
            let updated = emulator
                .update_configuration(&arn, appended.description, b"num.partitions=2\n")
                .await;
            assert_eq!(
                (updated.status, updated.at("/latestRevision/revision").as_i64()),
                (200, Some(expected_revision)),
                "case {}/{}: expected revision {expected_revision}, got {} {}",
                row.case,
                appended.case,
                updated.status,
                updated.body
            );
            assert_eq!(
                updated
                    .at("/latestRevision")
                    .get("description")
                    .and_then(Value::as_str),
                appended.expected(),
                "case {}/{}: the appended revision must report its own description, and no \
                 member when it has none, got {}",
                row.case,
                appended.case,
                updated.body
            );
            let read = emulator
                .describe_configuration_revision(&arn, expected_revision)
                .await;
            assert_eq!(
                read.body.get("description").and_then(Value::as_str),
                appended.expected(),
                "case {}/{}: the stored revision must read back with its own description, got {}",
                row.case,
                appended.case,
                read.body
            );
            // The configuration's OWN description is the one it was created
            // with: an update describes the revision it adds, not the resource.
            let described = describe_in(&emulator, REGION, &arn).await;
            assert_eq!(
                described.body.get("description").and_then(Value::as_str),
                row.expected(),
                "case {}/{}: an update must not rewrite the configuration's own description, \
                 got {}",
                row.case,
                appended.case,
                described.body
            );
        }
    }

    // A configuration a cluster is created with is unaffected by any of this:
    // the rule is about how an absent member is rendered, not about which
    // members exist. One cluster, created against the last row's configuration,
    // proves the stored record is still usable.
    let usable = ConfigurationRequest::named("described-and-applied");
    let arn = emulator
        .create_configuration_row_ok_in(REGION, &usable)
        .await;
    let mut body = json!({
        "clusterName": "uses-a-described-configuration",
        "kafkaVersion": "3.6.0",
        "numberOfBrokerNodes": 3,
        "brokerNodeGroupInfo": {
            "instanceType": "kafka.m5.large",
            "clientSubnets": ["subnet-0a1b2c3d", "subnet-1a2b3c4d", "subnet-2a3b4c5d"],
        },
    });
    body["configurationInfo"] = json!({ "arn": arn, "revision": 1 });
    let created = emulator.call("POST", "/v1/clusters", Some(body)).await;
    assert_eq!(
        created.status, 200,
        "case applied: a configuration created without a description must still be \
         applicable to a cluster, got {} {}",
        created.status, created.body
    );
}
