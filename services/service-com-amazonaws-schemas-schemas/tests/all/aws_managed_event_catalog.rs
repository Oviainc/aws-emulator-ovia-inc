// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the AWS-managed event-schema catalogue: the read-only
//! `aws.events` registry every account sees, which holds one schema per AWS event
//! type.
//!
//! The rule under test is the catalogue's MEMBERSHIP and readability: a source the
//! catalogue publishes CloudTrail-delivered schemas for publishes an API-call event
//! type, every event type the catalogue holds is named `<source>@<EventName>`, and each
//! member is readable end to end — it is listed (through `NextToken`, because the
//! catalogue is far larger than one page), described with its document and its derived
//! description, exported, and found by a search.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use serde_json::Value;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, ManagedSchema, AWS_EVENT_SOURCES, AWS_MANAGED_REGISTRY,
    AWS_MANAGED_SCHEMA_TYPE, AWS_SERVICE_EVENT_TYPES, CLOUD_TRAIL_EVENT_NAME,
    EVENT_ENVELOPE_COMPONENT, MAX_PAGE_SIZE,
};

use crate::registry_schema_lifecycle::{
    export_uri, schema_names, schema_uri, walk, Emulator,
};

/// The listing of the AWS-managed registry's schemas.
fn catalogue_uri() -> String {
    format!("/v1/registries/name/{AWS_MANAGED_REGISTRY}/schemas")
}

/// The catalogue entry published under `name` — what the case compares the service's
/// answer against. The catalogue itself is rendered once per process by the crate.
fn entry(name: &str) -> &'static ManagedSchema {
    aws_managed_schemas()
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("case {name:?}: the catalogue must hold it"))
}

/// The rows the readability cases run over: members of the catalogue that differ along
/// what the rule generalises over — the publishing service, whether the event type is
/// the API-call event every source publishes or a service-specific one, and how many
/// components the document carries (one, two or three, flat or nested).
///
/// Source: `crate::state::AWS_EVENT_SOURCES` and
/// `crate::state::AWS_SERVICE_EVENT_TYPES`. The first row is the one the issue cited;
/// the rest are entries it did not.
fn readable_rows() -> Vec<String> {
    vec![
        format!("aws.events@{CLOUD_TRAIL_EVENT_NAME}"),
        format!("aws.a4b@{CLOUD_TRAIL_EVENT_NAME}"),
        format!("aws.abuse@{CLOUD_TRAIL_EVENT_NAME}"),
        format!("aws.autoscaling@{CLOUD_TRAIL_EVENT_NAME}"),
        format!("aws.xray@{CLOUD_TRAIL_EVENT_NAME}"),
        "aws.ec2@EC2InstanceStateChangeNotification".to_string(),
        "aws.ecs@ECSTaskStateChange".to_string(),
        "aws.events@ScheduledEvent".to_string(),
        "aws.health@AWSHealthEvent".to_string(),
        "aws.s3@ObjectCreated".to_string(),
        "aws.tag@TagChangeonResource".to_string(),
    ]
}

/// RULE: the AWS-managed registry holds the API-call event type of every source it
/// publishes CloudTrail-delivered schemas for, published under `<source>@<EventName>`.
///
/// Driven over EVERY such source the catalogue names, not a sample: the rule is about
/// the catalogue's membership, so each source is a row. Each is selected by its own
/// name prefix, so the case is independent of which page the entry lands on.
#[tokio::test]
async fn the_catalogue_holds_the_api_call_event_type_of_every_cloud_trail_source() {
    let emulator = Emulator::new();

    for source in AWS_EVENT_SOURCES {
        let expected = format!("{source}@{CLOUD_TRAIL_EVENT_NAME}");
        let listed = emulator
            .call(
                "GET",
                &format!("{}?schemaNamePrefix={source}@", catalogue_uri()),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {source:?}: expected 200, got {} {:?}",
            listed.status, listed.body
        );
        let names = schema_names(&listed);
        assert!(
            names.contains(&expected),
            "case {source:?}: the catalogue must publish {expected:?}, got {names:?}"
        );
        for name in &names {
            assert!(
                name.starts_with(&format!("{source}@")),
                "case {source:?}: every entry of this source is {source}@<EventName>, got {name:?}"
            );
        }
    }

    // Every service-specific event type the catalogue names is published too. The
    // prefix is the whole name, so the listing holds that entry — and, where the
    // catalogue publishes a longer name starting with it, those entries too.
    for event in AWS_SERVICE_EVENT_TYPES {
        let expected = event.schema_name();
        let listed = emulator
            .call(
                "GET",
                &format!("{}?schemaNamePrefix={expected}", catalogue_uri()),
                None,
            )
            .await;
        let names = schema_names(&listed);
        assert!(
            names.contains(&expected),
            "case {expected:?}: the catalogue must publish it, got {names:?}"
        );
        for name in &names {
            assert!(
                name.starts_with(&expected),
                "case {expected:?}: a prefix selects only names starting with it, \
                 got {name:?}"
            );
        }
    }
}

/// RULE: every member of the catalogue is readable — `DescribeSchema` answers 200 with
/// the member's stored `OpenApi3` document, the description AWS derives from its name,
/// its newest version, its (empty) tag set and the empty `SchemaArn` a schema AWS
/// publishes carries.
#[tokio::test]
async fn every_catalogue_member_is_described_from_the_seeded_document() {
    let emulator = Emulator::new();

    for name in readable_rows() {
        let seeded = entry(&name);
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_at("SchemaName"),
            name,
            "case {name:?}: DescribeSchema names the schema asked for"
        );
        assert_eq!(
            described.str_at("Content"),
            seeded.content,
            "case {name:?}: DescribeSchema reports the seeded document verbatim"
        );
        let (source, event_name) = name
            .split_once('@')
            .unwrap_or_else(|| panic!("case {name:?}: a catalogue name is <source>@<EventName>"));
        assert_eq!(
            described.str_at("Description"),
            format!("Schema for event type {event_name}, published by AWS service {source}"),
            "case {name:?}: the description is the sentence the name yields"
        );
        assert_eq!(
            described.str_at("Type"),
            AWS_MANAGED_SCHEMA_TYPE,
            "case {name:?}: AWS stores its event schemas as {AWS_MANAGED_SCHEMA_TYPE}"
        );
        assert_eq!(
            described.str_at("SchemaVersion"),
            seeded.version_count.to_string(),
            "case {name:?}: the read answers the catalogue's newest version"
        );
        assert_eq!(
            described.str_at("SchemaArn"),
            "",
            "case {name:?}: a schema AWS publishes is no customer resource and carries \
             no ARN"
        );
        assert_eq!(
            described.value_at("tags"),
            &serde_json::json!({}),
            "case {name:?}: an untagged schema reports an empty tag set"
        );
        described.value_at("LastModified");
        described.value_at("VersionCreatedDate");

        // The document describes the event it is published for: it defines the
        // component named after the event, and nothing around it.
        let document: Value = serde_json::from_str(&seeded.content)
            .unwrap_or_else(|e| panic!("case {name:?}: the document must be JSON: {e}"));
        assert!(
            document["components"]["schemas"][event_name]["properties"].is_object(),
            "case {name:?}: the document defines the event component, got {document:?}"
        );
        assert!(
            document["components"]["schemas"][EVENT_ENVELOPE_COMPONENT].is_null(),
            "case {name:?}: a published document declares no event envelope, got {document:?}"
        );
        assert_eq!(
            document["info"]["title"]
                .as_str()
                .unwrap_or_default()
                .rsplit(" published by ")
                .next()
                .unwrap_or_default(),
            source,
            "case {name:?}: the document titles the event its source publishes, \
             got {document:?}"
        );
    }
}

/// RULE: every member of the catalogue exports as the `JSONSchemaDraft4` the service
/// converts it to (draft-04 named at the root, the document's components as its
/// definitions), and the language it is STORED as is no export target, so asking for
/// that is refused on the request.
#[tokio::test]
async fn every_catalogue_member_exports_to_the_draft_4_target() {
    let emulator = Emulator::new();

    for name in readable_rows() {
        let seeded = entry(&name);

        let refused = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &name, AWS_MANAGED_SCHEMA_TYPE),
                None,
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: the stored language is no export target, got {} {:?}",
            refused.status, refused.body
        );
        // The document is still served as it is stored — by the reader, not the export.
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            described.str_at("Content"),
            seeded.content,
            "case {name:?}: the stored document is what a read of it answers"
        );

        let converted = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &name, "JSONSchemaDraft4"),
                None,
            )
            .await;
        assert_eq!(
            converted.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            converted.status, converted.body
        );
        assert_eq!(
            converted.str_at("Type"),
            "JSONSchemaDraft4",
            "case {name:?}: the export reports the language it produced"
        );
        let content: Value = serde_json::from_str(&converted.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {name:?}: Content must be JSON: {e}"));
        assert!(
            content["$schema"]
                .as_str()
                .unwrap_or_default()
                .contains("draft-04"),
            "case {name:?}: the export must name draft-04, got {content:?}"
        );
        let event_name = name.split_once('@').map(|(_, event)| event).unwrap_or("");
        assert!(
            content["definitions"][event_name]["properties"].is_object(),
            "case {name:?}: every component of the document is a definition, \
             got {content:?}"
        );
        assert!(
            content["definitions"][EVENT_ENVELOPE_COMPONENT].is_null()
                && content["properties"].is_null(),
            "case {name:?}: a published document names no root to promote, \
             got {content:?}"
        );
    }
}

/// RULE (an invariant): following `NextToken` walks the catalogue exactly once — the
/// concatenated pages are its whole membership in the order the registry collates
/// schema names in, with no repeat and no gap — whatever page size is asked for,
/// including one past the page the service will serve.
#[tokio::test]
async fn paging_the_catalogue_walks_every_member_exactly_once() {
    let emulator = Emulator::new();

    let mut expected: Vec<String> = aws_managed_schemas()
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    // The order a listing serves, which is the codepoint order of the schema NAMES.
    // Source: `crate::state::in_schema_name_order`.
    expected.sort();
    assert!(
        expected.len() > MAX_PAGE_SIZE,
        "the catalogue must be larger than one page for this case to page at all, got {}",
        expected.len()
    );

    // Rows: page sizes that divide the catalogue and that do not, the largest page the
    // service serves, and one larger than that — which the service clamps to its own
    // maximum, so the walk still sees every member once.
    for limit in [
        None,
        Some(50),
        Some(64),
        Some(MAX_PAGE_SIZE),
        Some(MAX_PAGE_SIZE * 2),
    ] {
        let walked = walk(&emulator, &catalogue_uri(), limit, schema_names).await;
        assert_eq!(
            walked, expected,
            "case limit={limit:?}: paging must walk the whole catalogue once, in the \
             order it collates names in"
        );
    }
}

/// RULE: `SearchSchemas` over the catalogue reports the members indexed under every
/// keyword — the event source and detail type their documents record — each with its
/// stored versions and no `SchemaArn`, which a schema AWS publishes does not have.
#[tokio::test]
async fn searching_the_catalogue_reports_members_without_arns() {
    let emulator = Emulator::new();

    // Rows: a keyword that is a source namespace, keywords of an event's detail type,
    // a source and detail-type pair that must BOTH occur, and a keyword nothing
    // carries. A search selects on what the registry indexes a schema under: the event
    // source and detail type its document declares.
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("aws.ecs", Some("aws.ecs@ECSTaskStateChange")),
        ("task,state", Some("aws.ecs@ECSTaskStateChange")),
        (
            "aws.s3,object,created",
            Some("aws.s3@ObjectCreated"),
        ),
        ("no-such-keyword-anywhere", None),
    ];

    for (keywords, expected) in cases {
        let answer = emulator
            .call(
                "GET",
                &format!(
                    "/v1/registries/name/{AWS_MANAGED_REGISTRY}/schemas/search?keywords={keywords}"
                ),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {keywords:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let matches = answer.body["Schemas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {keywords:?}: expected a Schemas array in {:?}", answer.body))
            .clone();
        match expected {
            None => assert!(
                matches.is_empty(),
                "case {keywords:?}: nothing carries this keyword, got {matches:?}"
            ),
            Some(name) => {
                let found: Vec<String> = matches
                    .iter()
                    .map(|item| item["SchemaName"].as_str().unwrap_or_default().to_string())
                    .collect();
                assert!(
                    found.contains(&name.to_string()),
                    "case {keywords:?}: expected {name:?} among the matches, got {found:?}"
                );
                let summary = matches
                    .iter()
                    .find(|item| item["SchemaName"].as_str() == Some(name))
                    .expect("the match just asserted");
                assert!(
                    summary["SchemaArn"].is_null(),
                    "case {keywords:?}: a summary of a schema AWS publishes carries no \
                     SchemaArn member, got {summary:?}"
                );
                assert_eq!(
                    summary["SchemaVersions"][0]["SchemaVersion"].as_str(),
                    Some("1"),
                    "case {keywords:?}: the versions are reported from one, got {summary:?}"
                );
            }
        }
    }
}

/// RULE: only what the catalogue holds is readable — a `<source>@<EventName>` the
/// catalogue does not publish is 404 `NotFoundException` from both readers, as the
/// model declares for a name that does not exist.
#[tokio::test]
async fn a_name_the_catalogue_does_not_publish_is_not_found() {
    let emulator = Emulator::new();

    // Rows: a source nothing publishes, a known source with an event type it does not
    // publish, and a name that is not `<source>@<EventName>` at all.
    for name in [
        "aws.no-such-service@AWSAPICallviaCloudTrail",
        "aws.ec2@NoSuchEventTypeHere",
        "not-an-event-schema",
    ] {
        for uri in [
            schema_uri(AWS_MANAGED_REGISTRY, name),
            export_uri(AWS_MANAGED_REGISTRY, name, "JSONSchemaDraft4"),
        ] {
            let answer = emulator.call("GET", &uri, None).await;
            assert_eq!(
                answer.status, 404,
                "case {name:?} at {uri}: expected 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotFoundException",
                "case {name:?} at {uri}: expected NotFoundException, got {:?}",
                answer.body
            );
        }
    }
}
