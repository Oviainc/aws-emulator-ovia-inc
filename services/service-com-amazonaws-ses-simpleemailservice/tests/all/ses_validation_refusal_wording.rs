// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a request it refused before any handler ran: the error code
//! it names and the message it words, driven end to end over the awsQuery wire.
//!
//! One rule per assertion, each over a SET of inputs rather than the one a report cited:
//!
//! 1. every structural refusal of this query API carries ONE error code,
//!    `ValidationError` (HTTP 400), whichever layer found the fault — a member the
//!    request left out is found while READING it, a bad value while WALKING the one that
//!    was read, and a client branches on one code for both;
//! 2. the clause of an ABSENT member states the position and the requirement and no
//!    value, because the member held none: `Value at '<path>' failed to satisfy
//!    constraint: Member must not be null`;
//! 3. the clause of a value outside an ENUM's admitted set states the position and the
//!    admitted set and no value, with the set listed in ASCENDING order over its values
//!    whatever order the model declares them in;
//! 4. every other constraint class keeps the framework's clause, the value it submitted
//!    included — so rules 2 and 3 are classes, not a blanket.
//!
//! Each row's expected clause is cited from the model
//! (`smithy_query(query="operation",operation=…)` for which member is `@required` and
//! `query="shape"` for an enum's declared value set), and the rows differ along what the
//! rules generalise over: the operation, the member, the member's position in the input,
//! which enum the member targets, how many values that enum declares, and which value the
//! request offends with.

use crate::harness::{call, element, emulator, field, xml_text, Field};

/// The one code every structural refusal of this API carries (rule 1).
const VALIDATION_ERROR: &str = "ValidationError";

/// `com.amazonaws.ses#IdentityType`, declared `EmailAddress, Domain`
/// (`smithy_query(query="shape",shape="IdentityType")`), as this service STATES it in a
/// refusal: ascending over the values, which reverses the declaration order.
const IDENTITY_TYPE_LISTED: &str = "Domain, EmailAddress";

/// `com.amazonaws.ses#TlsPolicy` (`DeliveryOptions$TlsPolicy`), declared
/// `Require, Optional` (`query="shape",shape="TlsPolicy"`), stated ascending.
const TLS_POLICY_LISTED: &str = "Optional, Require";

/// `com.amazonaws.ses#BehaviorOnMXFailure`, declared `UseDefaultValue, RejectMessage`
/// (`query="shape",shape="BehaviorOnMXFailure"`), stated ascending.
const BEHAVIOR_ON_MX_FAILURE_LISTED: &str = "RejectMessage, UseDefaultValue";

/// `com.amazonaws.ses#ReceiptFilterPolicy` (`ReceiptIpFilter$Policy`), declared
/// `Block, Allow` (`query="shape",shape="ReceiptFilterPolicy"`), stated ascending.
const RECEIPT_FILTER_POLICY_LISTED: &str = "Allow, Block";

/// `com.amazonaws.ses#ConfigurationSetAttribute`, the element of
/// `ConfigurationSetAttributeList`, declared
/// `eventDestinations, trackingOptions, deliveryOptions, reputationOptions`
/// (`query="shape",shape="ConfigurationSetAttribute"`), stated ascending — a four-valued
/// enum reached through a LIST member, so neither its arity nor its position is what the
/// order depends on.
const CONFIGURATION_SET_ATTRIBUTE_LISTED: &str =
    "deliveryOptions, eventDestinations, reputationOptions, trackingOptions";

/// `com.amazonaws.ses#NotificationType`, declared `Bounce, Complaint, Delivery`
/// (`query="shape",shape="NotificationType"`). Its declaration order already IS the
/// ascending one, so this row pins that the rule leaves such a set alone rather than
/// permuting it.
const NOTIFICATION_TYPE_LISTED: &str = "Bounce, Complaint, Delivery";

/// The whole envelope one clause is stated in.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause an ABSENT member is stated with (rule 2).
fn absent_clause(member_path: &str) -> String {
    format!(
        "Value at '{member_path}' failed to satisfy constraint: Member must not be null"
    )
}

/// The clause a value outside an enum's set is stated with (rule 3).
fn enum_clause(member_path: &str, listed: &str) -> String {
    format!(
        "Value at '{member_path}' failed to satisfy constraint: Member must satisfy enum \
         value set: [{listed}]"
    )
}

/// The clause every other class is stated with: the framework's own, value and all
/// (rule 4).
fn valued_clause(submitted: &str, member_path: &str, requirement: &str) -> String {
    format!("Value '{submitted}' at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// One row: a request whose single fault is structural, and the whole message it is
/// refused with.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<Field>,
    message: String,
}

/// Drive every row and assert the code, the status and the whole message.
async fn assert_rows(rows: Vec<Row>) {
    let router = emulator();
    for row in rows {
        let case = row.case;
        let (status, body) = call(&router, row.action, &row.params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = element(&body, "Code").unwrap_or_default();
        assert_eq!(
            code, VALIDATION_ERROR,
            "case {case:?}: expected the {VALIDATION_ERROR} code, got {code:?} in {body}"
        );
        let stated = element(&body, "Message").map(|raw| xml_text(&raw)).unwrap_or_default();
        assert_eq!(
            stated, row.message,
            "case {case:?}: expected {:?}, got {stated:?}",
            row.message
        );
    }
}

/// A `SendEmail` body with everything but the `Destination` the model requires
/// (`smithy_query(query="operation",operation="SendEmail")`: `Destination`, `Message` and
/// `Source` are each `@required`).
fn send_email_without_destination() -> Vec<Field> {
    vec![
        field("Source", "sender@example.com"),
        field("Message.Subject.Data", "hi"),
        field("Message.Body.Text.Data", "body"),
    ]
}

#[tokio::test]
async fn an_absent_required_member_is_refused_with_the_position_and_no_value() {
    assert_rows(vec![
        Row {
            case: "SendEmail with no Destination",
            action: "SendEmail",
            params: send_email_without_destination(),
            message: one_error(&absent_clause("destination")),
        },
        Row {
            case: "VerifyEmailIdentity with no EmailAddress",
            action: "VerifyEmailIdentity",
            params: Vec::new(),
            message: one_error(&absent_clause("emailAddress")),
        },
        Row {
            case: "DeleteIdentity with no Identity",
            action: "DeleteIdentity",
            params: Vec::new(),
            message: one_error(&absent_clause("identity")),
        },
        Row {
            case: "GetIdentityVerificationAttributes with no Identities list",
            action: "GetIdentityVerificationAttributes",
            params: Vec::new(),
            message: one_error(&absent_clause("identities")),
        },
        Row {
            case: "PutIdentityPolicy with no PolicyName, the member between two present ones",
            action: "PutIdentityPolicy",
            params: vec![
                field("Identity", "example.com"),
                field("Policy", "{\"Version\":\"2012-10-17\"}"),
            ],
            message: one_error(&absent_clause("policyName")),
        },
        Row {
            case: "SetIdentityNotificationTopic with no NotificationType",
            action: "SetIdentityNotificationTopic",
            params: vec![field("Identity", "example.com")],
            message: one_error(&absent_clause("notificationType")),
        },
    ])
    .await;
}

#[tokio::test]
async fn a_value_outside_an_enum_is_refused_with_the_ascending_set_and_no_value() {
    assert_rows(vec![
        Row {
            case: "ListIdentities IdentityType outside its two-valued set",
            action: "ListIdentities",
            params: vec![field("IdentityType", "Bogus")],
            message: one_error(&enum_clause("identityType", IDENTITY_TYPE_LISTED)),
        },
        Row {
            case: "ListIdentities IdentityType in the wrong case, which is still outside the set",
            action: "ListIdentities",
            params: vec![field("IdentityType", "emailaddress")],
            message: one_error(&enum_clause("identityType", IDENTITY_TYPE_LISTED)),
        },
        Row {
            case: "ListIdentities IdentityType offending with a value of ANOTHER enum",
            action: "ListIdentities",
            params: vec![field("IdentityType", "Bounce")],
            message: one_error(&enum_clause("identityType", IDENTITY_TYPE_LISTED)),
        },
        Row {
            case: "SetIdentityNotificationTopic NotificationType, a three-valued enum with no \
                   published listing",
            action: "SetIdentityNotificationTopic",
            params: vec![
                field("Identity", "example.com"),
                field("NotificationType", "Bogus"),
            ],
            message: one_error(&enum_clause("notificationType", NOTIFICATION_TYPE_LISTED)),
        },
        Row {
            case: "SetIdentityHeadersInNotificationsEnabled NotificationType, the same enum \
                   reached through another operation",
            action: "SetIdentityHeadersInNotificationsEnabled",
            params: vec![
                field("Identity", "example.com"),
                field("NotificationType", "delivery"),
                field("Enabled", "true"),
            ],
            message: one_error(&enum_clause("notificationType", NOTIFICATION_TYPE_LISTED)),
        },
        Row {
            case: "PutConfigurationSetDeliveryOptions TlsPolicy, a nested member whose enum is \
                   declared in the reverse of the stated order",
            action: "PutConfigurationSetDeliveryOptions",
            params: vec![
                field("ConfigurationSetName", "guarded-set"),
                field("DeliveryOptions.TlsPolicy", "NotAPolicy"),
            ],
            message: one_error(&enum_clause("deliveryOptions.tlsPolicy", TLS_POLICY_LISTED)),
        },
        Row {
            case: "PutConfigurationSetDeliveryOptions TlsPolicy in the wrong case, which is \
                   still outside the set",
            action: "PutConfigurationSetDeliveryOptions",
            params: vec![
                field("ConfigurationSetName", "guarded-set"),
                field("DeliveryOptions.TlsPolicy", "require"),
            ],
            message: one_error(&enum_clause("deliveryOptions.tlsPolicy", TLS_POLICY_LISTED)),
        },
        Row {
            case: "SetIdentityMailFromDomain BehaviorOnMXFailure, another two-valued enum \
                   declared in the reverse of the stated order",
            action: "SetIdentityMailFromDomain",
            params: vec![
                field("Identity", "example.com"),
                field("MailFromDomain", "mail.example.com"),
                field("BehaviorOnMXFailure", "Bogus"),
            ],
            message: one_error(&enum_clause(
                "behaviorOnMXFailure",
                BEHAVIOR_ON_MX_FAILURE_LISTED,
            )),
        },
        Row {
            case: "CreateReceiptFilter Policy, an enum two structures deep",
            action: "CreateReceiptFilter",
            params: vec![
                field("Filter.Name", "block-a-host"),
                field("Filter.IpFilter.Cidr", "10.0.0.1/32"),
                field("Filter.IpFilter.Policy", "Deny"),
            ],
            message: one_error(&enum_clause(
                "filter.ipFilter.policy",
                RECEIPT_FILTER_POLICY_LISTED,
            )),
        },
        Row {
            case: "DescribeConfigurationSet attribute name, a four-valued enum reached through \
                   a list",
            action: "DescribeConfigurationSet",
            params: vec![
                field("ConfigurationSetName", "guarded-set"),
                field("ConfigurationSetAttributeNames.member.1", "deliveryoptions"),
            ],
            message: one_error(&enum_clause(
                "configurationSetAttributeNames.1.member",
                CONFIGURATION_SET_ATTRIBUTE_LISTED,
            )),
        },
    ])
    .await;
}

#[tokio::test]
async fn every_other_constraint_class_keeps_the_value_it_submitted() {
    // `PolicyName` is `@length` 1..64 and `Policy` `@length` 1..
    // (`smithy_query(query="operation",operation="PutIdentityPolicy")`), the only
    // bounded input members this model declares.
    let too_long = "p".repeat(65);
    assert_rows(vec![
        Row {
            case: "PutIdentityPolicy PolicyName one character past its maximum",
            action: "PutIdentityPolicy",
            params: vec![
                field("Identity", "example.com"),
                field("PolicyName", too_long.clone()),
                field("Policy", "{\"Version\":\"2012-10-17\"}"),
            ],
            message: one_error(&valued_clause(
                &too_long,
                "policyName",
                "Member must have length less than or equal to 64",
            )),
        },
        Row {
            case: "PutIdentityPolicy PolicyName one character short of its minimum",
            action: "PutIdentityPolicy",
            params: vec![
                field("Identity", "example.com"),
                field("PolicyName", ""),
                field("Policy", "{\"Version\":\"2012-10-17\"}"),
            ],
            message: one_error(&valued_clause(
                "",
                "policyName",
                "Member must have length greater than or equal to 1",
            )),
        },
        Row {
            case: "PutIdentityPolicy Policy one character short of its minimum",
            action: "PutIdentityPolicy",
            params: vec![
                field("Identity", "example.com"),
                field("PolicyName", "allow-send"),
                field("Policy", ""),
            ],
            message: one_error(&valued_clause(
                "",
                "policy",
                "Member must have length greater than or equal to 1",
            )),
        },
    ])
    .await;
}

#[tokio::test]
async fn a_name_on_its_published_bound_is_accepted() {
    // The bounds themselves are inside the constraint, so the refusal above is about the
    // values PAST them and not about the bound: a name of exactly 64 characters, and one
    // of exactly 1, reach the handler.
    let router = emulator();
    for (case, policy_name) in [
        ("a PolicyName of exactly 64 characters", "p".repeat(64)),
        ("a PolicyName of exactly 1 character", "p".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "PutIdentityPolicy",
            &[
                field("Identity", "example.com"),
                field("PolicyName", policy_name),
                field("Policy", "{\"Version\":\"2012-10-17\",\"Statement\":[]}"),
            ],
        )
        .await;
        let code = element(&body, "Code").unwrap_or_default();
        assert_ne!(
            code, VALIDATION_ERROR,
            "case {case:?}: a value ON the bound must reach the handler, got {status} with {body}"
        );
    }
}
