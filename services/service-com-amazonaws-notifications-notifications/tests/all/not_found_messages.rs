// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a reference to a resource it does not hold, driven over HTTP
//! through the same multi-service router the emulator boots in production.
//!
//! The rule under test: a request naming a resource that does not exist is refused with
//! the modelled `ResourceNotFoundException` (HTTP 404) whose message is built from the
//! request — the kind of resource that was looked up, the member that identified it, and
//! the value the request carried there — as
//! `<resource> with <identifier> <value> not found`. One template for every not-found
//! path of the service, so only the RESOURCE each path names varies — the service names
//! an `EventRule` one way when a read looks it up and another when a mutation does.
//!
//! The table rows vary along what the rule generalises over: which resource is looked up
//! (a NotificationHub keyed by Region, a NotificationConfiguration keyed by ARN, an
//! EventRule keyed by its own deeper ARN), which member carries the identifier (`arn`,
//! `notificationConfigurationArn`, `notificationHubRegion`), where that member travels
//! (a URI label, a query parameter, a request body), and which HTTP method and operation
//! the lookup belongs to. Every identifier used satisfies its own modelled `@pattern`, so
//! the refusal comes from the handler rather than from the boundary validator. Catalog
//! source: `smithy_query(query="operation",operation="GetNotificationConfiguration,
//! UpdateNotificationConfiguration,DeleteNotificationConfiguration,TagResource,
//! UntagResource,ListTagsForResource,ListChannels,AssociateChannel,DisassociateChannel,
//! CreateEventRule,GetEventRule,DeregisterNotificationHub",
//! model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`,
//! whose `errors` carry `ResourceNotFoundException` with `httpError` 404.

use serde_json::{json, Value};

// The HTTP harness this suite drives the service through, shared with every other suite
// in this crate.
use crate::harness::{encode_path_label, error_code, Emulator};

/// A well-formed `NotificationConfigurationArn` this emulator never minted.
const UNMINTED_CONFIGURATION_ARN: &str =
    "arn:aws:notifications::123456789012:configuration/abcdefghijklmnopqrstuvwxyz0";

/// A second one, differing in partition and id, so a row's value is not the only value
/// the rule holds for.
const OTHER_CONFIGURATION_ARN: &str =
    "arn:aws-us-gov:notifications::210987654321:configuration/zyxwvutsrqponmlkjihgfedcba9";

/// A well-formed `EventRuleArn` this emulator never minted: a configuration ARN with the
/// `/rule/<id>` segment the model appends.
const UNMINTED_EVENT_RULE_ARN: &str = "arn:aws:notifications::123456789012:configuration/abcdefghijklmnopqrstuvwxyz0/rule/0zyxwvutsrqponmlkjihgfedcba";

/// Regions no hub is registered in. Catalog source: the `Region` shape the model declares
/// for `notificationHubRegion` — `@length(2..25)` and
/// `@pattern("^([a-z]{1,4})-([a-z]{1,15}-)+([0-9])$")` — so the rows differ in prefix
/// length, segment count and total length.
const UNREGISTERED_REGIONS: &[&str] = &["eu-west-1", "us-gov-east-1", "a-b-1"];

/// `notification hub`, how the recorded refusal of a deregister names the resource.
const HUB: &str = "notification hub";
/// `notification configuration`, how the same family names a NotificationConfiguration.
const CONFIGURATION: &str = "notification configuration";
/// `EventRule`, how the refusal of a MUTATION of an EventRule names the
/// resource: not every path of this family names a resource the same way (see
/// [`EVENT_RULE`]).
const EVENT_RULE_MUTATED: &str = "EventRule";

/// `event rule`, how the refusal of a READ of an EventRule names the resource.
const EVENT_RULE: &str = "event rule";

/// One lookup of a resource that does not exist.
struct Lookup {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The kind of resource the request looked up.
    resource: &'static str,
    /// The member that identified it, named as the message names it.
    identifier: &'static str,
    /// The value the request carried at that member.
    value: &'static str,
}

/// The whole message a not-found refusal must state.
fn not_found_message(resource: &str, identifier: &str, value: &str) -> String {
    format!("{resource} with {identifier} {value} not found")
}

/// The table.
fn lookups() -> Vec<Lookup> {
    let mut rows = Vec::new();

    for region in UNREGISTERED_REGIONS {
        rows.push(Lookup {
            case: "DeregisterNotificationHub on a Region holding no hub",
            method: "DELETE",
            uri: format!("/notification-hubs/{}", encode_path_label(region)),
            body: None,
            resource: HUB,
            identifier: "region",
            value: region,
        });
    }

    for arn in [UNMINTED_CONFIGURATION_ARN, OTHER_CONFIGURATION_ARN] {
        let label = encode_path_label(arn);
        rows.extend([
            Lookup {
                case: "GetNotificationConfiguration on an unminted arn",
                method: "GET",
                uri: format!("/notification-configurations/{label}"),
                body: None,
                resource: CONFIGURATION,
                identifier: "arn",
                value: arn,
            },
            Lookup {
                case: "UpdateNotificationConfiguration on an unminted arn",
                method: "PUT",
                uri: format!("/notification-configurations/{label}"),
                body: Some(json!({ "description": "a new description" })),
                resource: CONFIGURATION,
                identifier: "arn",
                value: arn,
            },
            Lookup {
                case: "DeleteNotificationConfiguration on an unminted arn",
                method: "DELETE",
                uri: format!("/notification-configurations/{label}"),
                body: None,
                resource: CONFIGURATION,
                identifier: "arn",
                value: arn,
            },
            Lookup {
                case: "TagResource on an unminted arn",
                method: "POST",
                uri: format!("/tags/{label}"),
                body: Some(json!({ "tags": { "env": "prod" } })),
                resource: CONFIGURATION,
                identifier: "arn",
                value: arn,
            },
            Lookup {
                case: "UntagResource on an unminted arn",
                method: "DELETE",
                uri: format!("/tags/{label}?tagKeys=env"),
                body: None,
                resource: CONFIGURATION,
                identifier: "arn",
                value: arn,
            },
            Lookup {
                case: "ListTagsForResource on an unminted arn",
                method: "GET",
                uri: format!("/tags/{label}"),
                body: None,
                resource: CONFIGURATION,
                identifier: "arn",
                value: arn,
            },
            Lookup {
                case: "ListChannels for an unminted configuration, named in a query member",
                method: "GET",
                uri: format!("/channels?notificationConfigurationArn={label}"),
                body: None,
                resource: CONFIGURATION,
                identifier: "arn",
                value: arn,
            },
            Lookup {
                case: "CreateEventRule for an unminted configuration, named in the body",
                method: "POST",
                uri: "/event-rules".to_string(),
                body: Some(json!({
                    "notificationConfigurationArn": arn,
                    "source": "aws.ec2",
                    "eventType": "Demo Event",
                    "regions": ["us-east-1"],
                })),
                resource: CONFIGURATION,
                identifier: "arn",
                value: arn,
            },
            Lookup {
                case: "AssociateChannel with an unminted configuration, named in the body",
                method: "POST",
                uri: format!(
                    "/channels/associate/{}",
                    encode_path_label(
                        "arn:aws:chatbot:us-east-1:123456789012:chat-configuration/slack/ops"
                    )
                ),
                body: Some(json!({ "notificationConfigurationArn": arn })),
                resource: CONFIGURATION,
                identifier: "arn",
                value: arn,
            },
        ]);
    }

    rows.push(Lookup {
        case: "GetEventRule on an unminted rule arn",
        method: "GET",
        uri: format!("/event-rules/{}", encode_path_label(UNMINTED_EVENT_RULE_ARN)),
        body: None,
        resource: EVENT_RULE,
        identifier: "arn",
        value: UNMINTED_EVENT_RULE_ARN,
    });

    // The mutations of the same resource, which name it the way the service's
    // own refusal of a mutation names it.
    rows.push(Lookup {
        case: "DeleteEventRule on an unminted rule arn",
        method: "DELETE",
        uri: format!("/event-rules/{}", encode_path_label(UNMINTED_EVENT_RULE_ARN)),
        body: None,
        resource: EVENT_RULE_MUTATED,
        identifier: "arn",
        value: UNMINTED_EVENT_RULE_ARN,
    });
    rows.push(Lookup {
        case: "UpdateEventRule on an unminted rule arn",
        method: "PUT",
        uri: format!("/event-rules/{}", encode_path_label(UNMINTED_EVENT_RULE_ARN)),
        body: Some(json!({ "regions": ["us-east-1"] })),
        resource: EVENT_RULE_MUTATED,
        identifier: "arn",
        value: UNMINTED_EVENT_RULE_ARN,
    });

    rows
}

/// The `message` member of an error body, which an error response must carry.
fn message(case: &str, response: &Value) -> String {
    response["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error body carries a message, got {response:?}"))
        .to_string()
}

/// RULE: a request naming a resource this service does not hold is refused with the
/// modelled `ResourceNotFoundException` (HTTP 404) whose message names the resource, the
/// member that identified it and the value the request carried there.
#[tokio::test]
async fn a_lookup_of_a_resource_that_does_not_exist_names_what_was_looked_up() {
    let emulator = Emulator::new();

    for row in lookups() {
        let case = format!("{} ({})", row.case, row.value);
        let (status, refused) = emulator.send(row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an absent resource is refused with the modelled 404, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ResourceNotFoundException"),
            "case {case:?}: that refusal is a ResourceNotFoundException, got {refused:?}"
        );
        let stated = message(&case, &refused);
        let expected = not_found_message(row.resource, row.identifier, row.value);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected message {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: the same lookup SUCCEEDS once the resource exists, so the refusal above states
/// an absence rather than a rejection of the identifier's shape. Driven through the
/// operations that mint the two resources the table looks up.
#[tokio::test]
async fn the_same_lookup_succeeds_once_the_resource_exists() {
    let emulator = Emulator::new();

    for (index, region) in UNREGISTERED_REGIONS.iter().enumerate() {
        let case = format!("a hub registered in {region}");
        let (status, registered) = emulator
            .send(
                "POST",
                "/notification-hubs",
                Some(json!({ "notificationHubRegion": region })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: a register answers the modelled 201, got {registered:?}"
        );
        assert_eq!(
            registered["notificationHubRegion"],
            json!(region),
            "case {case:?}: the register reports the Region it was given, got {registered:?}"
        );

        let name = format!("exists-{index}");
        let (status, created) = emulator
            .send(
                "POST",
                "/notification-configurations",
                Some(json!({ "name": name, "description": "a configuration that exists" })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: a create answers the modelled 201, got {created:?}"
        );
        let arn = created["arn"].as_str().unwrap_or_default().to_string();
        let (status, got) = emulator
            .send(
                "GET",
                &format!("/notification-configurations/{}", encode_path_label(&arn)),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a get of a minted arn answers 200, got {got:?}"
        );
        assert_eq!(
            got["arn"],
            json!(arn),
            "case {case:?}: the read reports the arn it was asked for, got {got:?}"
        );
    }
}
