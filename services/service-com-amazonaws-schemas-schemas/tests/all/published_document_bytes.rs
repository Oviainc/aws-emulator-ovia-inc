// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DOCUMENT the two read paths of a published schema
//! serve — `DescribeSchema`'s `Content` and `ExportSchema`'s conversion of it.
//!
//! RULE: a schema document is an opaque TEXT this service serves as the document it
//! holds. So a read answers the published document itself — its members in the order the
//! published document writes them (`openapi`, `info` with its `title` before its
//! `version`, `paths`, `components`; a component's `type` before its `properties`) — and
//! a conversion to another schema language renders the target document's own root order
//! (`$schema`, `title`, `definitions`) with every translated schema's keywords and
//! property names in name order, which is how the draft-04 export is published. What a
//! published document says about the event it describes is the publisher's: for the
//! CloudTrail-delivered family that is the CloudTrail record, whose members, nested
//! structures and formats the cases below hold every member of the family to.
//!
//! Every case drives the real wire path, through the harness in
//! `registry_schema_lifecycle.rs` — nothing here touches the state struct directly.

use serde_json::Value;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_event_types, managed_schema_title, JsonNode, ManagedEventType,
    AWS_MANAGED_REGISTRY, CLOUD_TRAIL_EVENT_FAMILY, EVENT_ENVELOPE_COMPONENT,
    JSON_SCHEMA_DRAFT4_URI,
};

use crate::registry_schema_lifecycle::{export_uri, schema_uri, Emulator};

/// The members the published document declares for a CloudTrail record, in the order it
/// writes them.
///
/// Source: the document AWS publishes for a CloudTrail-delivered event, as a recorded
/// `DescribeSchema` of `aws.events@AWSAPICallviaCloudTrail` serves it — the Smithy model
/// states nothing about a document's text, so the published document is the reference.
/// The `detail` of every member of [`CLOUD_TRAIL_EVENT_FAMILY`] is the same record
/// whichever source published it, so the rows below hold sources the issue did not cite
/// to the same set.
const PUBLISHED_RECORD_MEMBERS: &[&str] = &[
    "addendum",
    "additionalEventData",
    "annotation",
    "apiVersion",
    "awsRegion",
    "edgeDeviceDetails",
    "errorCode",
    "errorMessage",
    "eventCategory",
    "eventContext",
    "eventID",
    "eventName",
    "eventSource",
    "eventTime",
    "eventType",
    "eventVersion",
    "insightDetails",
    "managementEvent",
    "readOnly",
    "recipientAccountId",
    "requestID",
    "requestParameters",
    "resources",
    "responseElements",
    "sessionCredentialFromConsole",
    "sharedEventID",
    "sourceIPAddress",
    "tlsDetails",
    "userAgent",
    "userIdentity",
];

/// The record members the published document marks `nullable` — the ones CloudTrail
/// fills only for the records they apply to, which OpenAPI 3.0 spells as "may be present
/// and null". Source: the same published document.
const NULLABLE_RECORD_MEMBERS: &[&str] = &[
    "addendum",
    "additionalEventData",
    "edgeDeviceDetails",
    "eventContext",
    "insightDetails",
    "requestParameters",
    "responseElements",
    "tlsDetails",
];

/// The catalogue entries the document cases run over.
///
/// Source: `crate::state::aws_managed_event_types()`. Rows differ along what the rule
/// generalises over: the publishing source, which kind of CloudTrail record the event is,
/// whether the event is a CloudTrail one or the service's own, how many components the
/// document carries and how deeply they nest. `aws.events@AWSAPICallviaCloudTrail` is
/// the entry the issue cited; every other row it did not.
fn document_rows() -> Vec<ManagedEventType> {
    let wanted = [
        "aws.events@AWSAPICallviaCloudTrail",
        "aws.s3@AWSConsoleSignInviaCloudTrail",
        "aws.lambda@AWSServiceEventviaCloudTrail",
        "aws.xray@AWSNetworkActivityEventviaCloudTrail",
        "aws.amazon.connect@AWSInsightviaCloudTrail",
        "aws.ec2@EC2InstanceStateChangeNotification",
        "aws.s3@ObjectCreated",
        "aws.health@AWSHealthEvent",
        "aws.events@ScheduledEvent",
        "aws.a4b@RoomStateChange",
    ];
    let catalogue = aws_managed_event_types();
    wanted
        .iter()
        .map(|name| {
            catalogue
                .iter()
                .find(|event| event.schema_name() == *name)
                .copied()
                .unwrap_or_else(|| panic!("case {name:?}: the catalogue must publish it"))
        })
        .collect()
}

/// The names of an object's members, in the order the document writes them.
fn member_names(node: &JsonNode) -> Vec<String> {
    match node {
        JsonNode::Object(members) => members.iter().map(|(name, _)| name.clone()).collect(),
        _ => Vec::new(),
    }
}

/// The document `answer`'s `Content` carries, read with its member order kept.
fn ordered_document(case: &str, content: &str) -> JsonNode {
    JsonNode::parse(content)
        .unwrap_or_else(|| panic!("case {case:?}: Content must be one JSON document: {content}"))
}

/// The `components.schemas` of a published document, in document order.
fn components(case: &str, document: &JsonNode) -> Vec<(String, JsonNode)> {
    let schemas = document
        .member("components")
        .and_then(|components| components.member("schemas"))
        .unwrap_or_else(|| panic!("case {case:?}: the document must carry components.schemas"));
    match schemas {
        JsonNode::Object(members) => members.clone(),
        other => panic!("case {case:?}: components.schemas must be an object, got {other:?}"),
    }
}

/// The first object in `node` whose members are NOT in name order, named by the path to
/// it — `None` when every object in the tree is ordered by name.
fn first_unordered_object(path: &str, node: &JsonNode) -> Option<String> {
    match node {
        JsonNode::Object(members) => {
            let names: Vec<&str> = members.iter().map(|(name, _)| name.as_str()).collect();
            let mut sorted = names.clone();
            sorted.sort_unstable();
            if names != sorted {
                return Some(format!("{path}: {names:?}"));
            }
            members
                .iter()
                .find_map(|(name, member)| first_unordered_object(&format!("{path}.{name}"), member))
        }
        JsonNode::List(items) => items
            .iter()
            .enumerate()
            .find_map(|(at, item)| first_unordered_object(&format!("{path}[{at}]"), item)),
        _ => None,
    }
}

/// RULE: a published document is served as it is published — `openapi`, then `info`
/// (`title` before `version`), then `paths`, then `components`, and a component's `type`
/// before its `properties` — because `Content` is text a client caches, diffs and hashes,
/// not a value whose members may be rearranged.
#[tokio::test]
async fn a_published_document_is_served_in_the_order_it_is_published() {
    let emulator = Emulator::new();

    for event in document_rows() {
        let name = event.schema_name();
        let title = managed_schema_title(event.source, event.detail_type);
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            described.status, described.body
        );
        let content = described.str_at("Content");

        // The document's opening bytes, as the published document writes them.
        let opening = format!(
            r#"{{"openapi":"3.0.0","info":{{"title":{},"version":"1.0.0"}},"paths":{{}},"components":{{"schemas":{{"#,
            serde_json::to_string(&title).expect("a title encodes")
        );
        assert!(
            content.starts_with(&opening),
            "case {name:?}: the document must open as published ({opening}), got {content}"
        );

        let document = ordered_document(&name, &content);
        assert_eq!(
            member_names(&document),
            vec!["openapi", "info", "paths", "components"],
            "case {name:?}: the published member order, got {content}"
        );
        assert_eq!(
            document.member("info").map(member_names),
            Some(vec!["title".to_string(), "version".to_string()]),
            "case {name:?}: `info` titles the event before it versions the document, \
             got {content}"
        );

        for (component, schema) in components(&name, &document) {
            let names = member_names(&schema);
            let type_at = names.iter().position(|member| member == "type");
            let properties_at = names.iter().position(|member| member == "properties");
            assert_eq!(
                type_at,
                Some(0),
                "case {name:?}: component {component:?} must declare its `type` first, \
                 got {names:?}"
            );
            if let Some(properties_at) = properties_at {
                assert!(
                    properties_at > 0,
                    "case {name:?}: component {component:?} must declare `type` before \
                     `properties`, got {names:?}"
                );
            }
        }

        // Reading it again answers the same bytes: a document is not re-rendered per
        // read.
        let again = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            again.str_at("Content"),
            content,
            "case {name:?}: two reads must answer one document"
        );
    }
}

/// RULE: a published document describes the event its publisher publishes — for every
/// member of the CloudTrail-delivered family, the CloudTrail record: its member set, the
/// nested structures it declares inline, the members that may arrive null, and the
/// format and pattern of the instant it carries.
#[tokio::test]
async fn a_cloud_trail_document_describes_the_published_record() {
    let emulator = Emulator::new();

    // Rows: the whole CloudTrail family, published by sources the issue did not cite
    // beside the one it did — the record is the same whichever source and whichever kind
    // of record it is.
    let sources = ["aws.events", "aws.s3", "aws.amazon.connect", "aws.xray"];
    for source in sources {
        for (detail_type, event_name) in CLOUD_TRAIL_EVENT_FAMILY {
            let name = format!("{source}@{event_name}");
            let described = emulator
                .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {name:?}: expected 200, got {} {:?}",
                described.status, described.body
            );
            let content = described.str_at("Content");
            let document: Value = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("case {name:?}: Content must be JSON: {e}"));
            let record = &document["components"]["schemas"][*event_name];
            // Every member SET below is read with the document's own order kept (a
            // `serde_json` map holds members by name or by insertion depending on the
            // build's features, so it could not pin an order), and asserted in the order
            // the published document writes them.
            let ordered = ordered_document(&name, &content);
            let ordered_record = ordered
                .member("components")
                .and_then(|components| components.member("schemas"))
                .and_then(|schemas| schemas.member(event_name))
                .unwrap_or_else(|| panic!("case {name:?}: the document declares its event"));

            let declared = ordered_record
                .member("properties")
                .map(member_names)
                .unwrap_or_else(|| panic!("case {name:?}: the record declares properties"));
            assert_eq!(
                declared, PUBLISHED_RECORD_MEMBERS,
                "case {name:?}: the published record member set, got {declared:?}"
            );

            // The record's nested structures are declared inline, so the document
            // carries no component beside the event's own and the envelope's.
            let carried: Vec<String> = components(&name, &ordered)
                .into_iter()
                .map(|(component, _)| component)
                .collect();
            assert_eq!(
                carried,
                vec![(*event_name).to_string(), EVENT_ENVELOPE_COMPONENT.to_string()],
                "case {name:?}: the published document's components, got {carried:?}"
            );

            for member in NULLABLE_RECORD_MEMBERS {
                assert_eq!(
                    record["properties"][*member]["nullable"],
                    serde_json::json!(true),
                    "case {name:?}: {member} may arrive null, got {:?}",
                    record["properties"][*member]
                );
            }
            for member in PUBLISHED_RECORD_MEMBERS {
                let declared = &record["properties"][*member];
                let nullable = NULLABLE_RECORD_MEMBERS.contains(member);
                assert_eq!(
                    declared["nullable"] == serde_json::json!(true),
                    nullable,
                    "case {name:?}: {member} is {}nullable in the published document, \
                     got {declared:?}",
                    if nullable { "" } else { "not " }
                );
            }

            // The record's instant: the type, the format and the pattern CloudTrail
            // writes it in, which is what a generated binding parses it with.
            let event_time = &record["properties"]["eventTime"];
            assert_eq!(
                event_time["type"],
                serde_json::json!("string"),
                "case {name:?}: eventTime is a string, got {event_time:?}"
            );
            assert_eq!(
                event_time["format"],
                serde_json::json!("date-time"),
                "case {name:?}: eventTime carries the date-time format, got {event_time:?}"
            );
            assert_eq!(
                event_time["pattern"],
                serde_json::json!("yyyy-MM-dd'T'HH:mm:ss'Z'"),
                "case {name:?}: eventTime carries the record's pattern, got {event_time:?}"
            );

            // A resource of `resources`, the TLS details and the identity are declared
            // where the record carries them.
            let resource = &record["properties"]["resources"]["items"];
            let resource_members = ordered_record
                .member("properties")
                .and_then(|properties| properties.member("resources"))
                .and_then(|resources| resources.member("items"))
                .and_then(|items| items.member("properties"))
                .map(member_names)
                .unwrap_or_else(|| panic!("case {name:?}: a resource declares properties"));
            assert_eq!(
                resource_members,
                vec!["accountId", "ARN", "ARNPrefix", "type"],
                "case {name:?}: a resource of the record, got {resource:?}"
            );
            let tls_members = ordered_record
                .member("properties")
                .and_then(|properties| properties.member("tlsDetails"))
                .and_then(|tls| tls.member("properties"))
                .map(member_names)
                .unwrap_or_else(|| panic!("case {name:?}: tlsDetails declares properties"));
            assert_eq!(
                tls_members,
                vec![
                    "cipherSuite",
                    "clientProvidedHostHeader",
                    "keyExchange",
                    "tlsVersion"
                ],
                "case {name:?}: the record's TLS details, got {tls_members:?}"
            );
            assert!(
                record["properties"]["userIdentity"]["properties"]["sessionContext"]
                    ["properties"]["sessionIssuer"]
                    .is_object(),
                "case {name:?}: the identity's session context is declared inline, got {:?}",
                record["properties"]["userIdentity"]
            );

            // And the detail-type the schema is named after is what the document says it
            // describes.
            assert_eq!(
                record["x-amazon-events-detail-type"],
                serde_json::json!(detail_type),
                "case {name:?}: the record declares the event it describes, got {record:?}"
            );
        }
    }
}

/// RULE: the draft-04 export is the published rendering of the same document — draft-04
/// named first, then the source document's title, then the definitions, with every
/// translated schema's keywords and property names in name order, OpenAPI's `nullable`
/// lowered to a type union and every component ref following the component into
/// `definitions`.
#[tokio::test]
async fn the_draft_4_export_is_rendered_as_it_is_published() {
    let emulator = Emulator::new();

    for event in document_rows() {
        let name = event.schema_name();
        let title = managed_schema_title(event.source, event.detail_type);
        let exported = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &name, "JSONSchemaDraft4"),
                None,
            )
            .await;
        assert_eq!(
            exported.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        let content = exported.str_at("Content");
        let opening = format!(
            r#"{{"$schema":{},"title":{},"definitions":{{"#,
            serde_json::to_string(JSON_SCHEMA_DRAFT4_URI).expect("the dialect encodes"),
            serde_json::to_string(&title).expect("a title encodes")
        );
        assert!(
            content.starts_with(&opening),
            "case {name:?}: the export must open as published ({opening}), got {content}"
        );

        let converted = ordered_document(&name, &content);
        assert_eq!(
            member_names(&converted),
            vec!["$schema", "title", "definitions"],
            "case {name:?}: the published root order, got {content}"
        );
        let definitions = converted
            .member("definitions")
            .unwrap_or_else(|| panic!("case {name:?}: the export carries definitions"));
        if let Some(unordered) = first_unordered_object("definitions", definitions) {
            panic!(
                "case {name:?}: the export renders every schema in name order, \
                 but {unordered} is not"
            );
        }

        // The source document's components are the definitions, and nothing of OpenAPI's
        // own vocabulary survives.
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        let source = ordered_document(&name, &described.str_at("Content"));
        let expected: Vec<String> = components(&name, &source)
            .into_iter()
            .map(|(component, _)| component)
            .collect();
        let mut expected_in_name_order = expected.clone();
        expected_in_name_order.sort();
        assert_eq!(
            member_names(definitions),
            expected_in_name_order,
            "case {name:?}: every component is a definition, got {content}"
        );
        assert!(
            !content.contains("#/components/schemas/"),
            "case {name:?}: no OpenAPI component ref may survive, got {content}"
        );
        assert!(
            !content.contains("\"nullable\""),
            "case {name:?}: draft-04 has no `nullable` keyword, got {content}"
        );

        // A nullable member of the source is a union with "null" in the export, and one
        // the source did not mark keeps its single type.
        let source_value: Value =
            serde_json::from_str(&described.str_at("Content")).expect("the source is JSON");
        let converted_value: Value = serde_json::from_str(&content).expect("the export is JSON");
        for component in expected {
            let Some(properties) =
                source_value["components"]["schemas"][&component]["properties"].as_object()
            else {
                continue;
            };
            for (member, declared) in properties {
                let Some(declared_type) = declared["type"].as_str() else {
                    continue;
                };
                let expected_type = if declared["nullable"] == serde_json::json!(true) {
                    serde_json::json!([declared_type, "null"])
                } else {
                    serde_json::json!(declared_type)
                };
                let exported_member =
                    &converted_value["definitions"][&component]["properties"][member];
                assert_eq!(
                    exported_member["type"], expected_type,
                    "case {name:?}: {component}.{member} must export as {expected_type:?}, \
                     got {exported_member:?}"
                );
            }
        }

        // Exporting the language the document is already stored in is the document
        // itself, byte for byte.
        let same = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &name, "OpenApi3"),
                None,
            )
            .await;
        assert_eq!(
            same.str_at("Content"),
            described.str_at("Content"),
            "case {name:?}: an export in the stored language is the stored document"
        );
    }
}

/// RULE (an invariant over the whole catalogue): every entry the catalogue publishes is
/// served as a published document and exports to a published draft-04 rendering — not
/// only the entries a case names.
#[tokio::test]
async fn every_catalogue_entry_is_served_and_exported_as_published() {
    let emulator = Emulator::new();
    let catalogue = aws_managed_event_types();

    // A bounded, deterministic sample of the whole catalogue: every 131st entry, so the
    // rows span the sources, the record kinds and the service-specific event types
    // without replaying thousands of HTTP calls. The stride is coprime with the family
    // size (6), so the sample does not land on one kind of record.
    let sampled: Vec<&ManagedEventType> = catalogue.iter().step_by(131).collect();
    assert!(
        sampled.len() > 5,
        "the sample must span the catalogue, got {} of {}",
        sampled.len(),
        catalogue.len()
    );

    for event in sampled {
        let name = event.schema_name();
        let title = managed_schema_title(event.source, event.detail_type);
        let described = emulator
            .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, &name), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected 200, got {} {:?}",
            described.status, described.body
        );
        let content = described.str_at("Content");
        let document = ordered_document(&name, &content);
        assert_eq!(
            member_names(&document),
            vec!["openapi", "info", "paths", "components"],
            "case {name:?}: the published member order, got {content}"
        );
        assert_eq!(
            document
                .member("info")
                .and_then(|info| info.member("title")),
            Some(&JsonNode::Text(title.clone())),
            "case {name:?}: the document titles its event, got {content}"
        );

        let exported = emulator
            .call(
                "GET",
                &export_uri(AWS_MANAGED_REGISTRY, &name, "JSONSchemaDraft4"),
                None,
            )
            .await;
        assert_eq!(
            exported.status, 200,
            "case {name:?}: expected 200 from the export, got {} {:?}",
            exported.status, exported.body
        );
        let converted = ordered_document(&name, &exported.str_at("Content"));
        assert_eq!(
            member_names(&converted),
            vec!["$schema", "title", "definitions"],
            "case {name:?}: the published root order, got {:?}",
            exported.str_at("Content")
        );
    }
}
