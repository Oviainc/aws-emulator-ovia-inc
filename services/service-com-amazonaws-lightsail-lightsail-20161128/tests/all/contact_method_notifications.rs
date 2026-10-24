// SPDX-License-Identifier: CC0-1.0

//! The Lightsail contact method — create, read, replace and delete — driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The `Authorization` header carries only a credential SCOPE, which is
//! what names the Region a request is made in.
//!
//! The rule under test: a contact method is ONE persisted record per Region and
//! `ContactProtocol`. `CreateContactMethod` stores the destination the request
//! carries verbatim against that pair, computes every member the request does NOT
//! carry from it plus this service's own allocators — the name, ARN and support
//! code from a minted id, the location from the Region the request was made in,
//! and the status from whether the protocol has a verification to wait on — so a
//! later `GetContactMethods` in that Region reports exactly that record;
//! creating the same protocol again replaces the one record rather than adding a
//! second; `DeleteContactMethod` removes the record its protocol names in that
//! Region alone and cannot be repeated; and a destination the protocol cannot
//! reach, or the SMS protocol in a Region that does not serve it, is refused and
//! stores nothing.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    MAX_E164_DIGITS, SMS_SUPPORTED_REGIONS,
};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The region a request that carries no SigV4 credential scope is served in
/// (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// Every member the modelled `ContactMethod` structure declares, in wire
/// spelling (`smithy_query(query="shape",shape="ContactMethod")`).
///
/// All ten are reported for every contact method: the request carries two of
/// them, and the service assigns the rest.
const CONTACT_METHOD_MEMBERS: &[&str] = &[
    "arn",
    "contactEndpoint",
    "createdAt",
    "location",
    "name",
    "protocol",
    "resourceType",
    "status",
    "supportCode",
    "tags",
];

/// The status an email contact method is created in — its verification has not
/// been completed yet (`ContactMethodStatus`).
const PENDING_VERIFICATION: &str = "PendingVerification";

/// The status a contact method notifications can be sent through is in
/// (`ContactMethodStatus`). An SMS contact method is created in it: the modelled
/// `ContactMethodVerificationProtocol` enum declares `Email` alone, so no other
/// protocol has a verification to wait on.
const VALID: &str = "Valid";

/// One creation case: the protocol and destination the request carries, the
/// Region it is made in, and the tags it asks for.
///
/// The rows differ along everything the rule generalises over: the protocol (and
/// so the status the contact method is created in), the Region the call is made in
/// — including a request that carries no credential scope at all — the form and
/// length of the destination, and the number of tags.
struct Row {
    case: &'static str,
    protocol: &'static str,
    contact_endpoint: &'static str,
    /// The region the request is made in, or `None` for a request that carries
    /// no credential scope at all.
    region: Option<&'static str>,
    /// The region the contact method must report — [`DEFAULT_REGION`] for a
    /// request that names none.
    reported_region: &'static str,
    /// The `ContactMethodStatus` the contact method must be created in.
    status: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The table. The first row is the destination form the modelled
/// `CreateContactMethodRequest$contactEndpoint` documentation shows for a phone
/// number (`+1XXX5550100`), in the Region a request with no scope is served in.
const ROWS: &[Row] = &[
    Row {
        case: "an SMS contact method, in no named region",
        protocol: "SMS",
        contact_endpoint: "+12065550100",
        region: None,
        reported_region: DEFAULT_REGION,
        status: VALID,
        tags: &[],
    },
    Row {
        case: "an email contact method, in another region",
        protocol: "Email",
        contact_endpoint: "ops@example.com",
        region: Some("eu-west-1"),
        reported_region: "eu-west-1",
        status: PENDING_VERIFICATION,
        tags: &[("stack", "production")],
    },
    Row {
        case: "an email address with a plus tag and a multi-label domain, tagged twice",
        protocol: "Email",
        contact_endpoint: "first.last+lightsail@mail.eu.example.co.uk",
        region: Some("ca-central-1"),
        reported_region: "ca-central-1",
        status: PENDING_VERIFICATION,
        tags: &[("owner", "platform"), ("costCentre", "")],
    },
    Row {
        case: "an SMS contact method on another continent, with a long country code",
        protocol: "SMS",
        contact_endpoint: "+819012345678",
        region: Some("ap-northeast-1"),
        reported_region: "ap-northeast-1",
        status: VALID,
        tags: &[("stack", "staging")],
    },
    Row {
        case: "the shortest email address and phone number the rules admit",
        protocol: "Email",
        contact_endpoint: "a@b.co",
        region: Some("ap-southeast-2"),
        reported_region: "ap-southeast-2",
        status: PENDING_VERIFICATION,
        tags: &[],
    },
];

/// The `tags` member of a request, in wire form.
fn tag_list(tags: &[(&str, &str)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// Create the contact method `protocol`/`contact_endpoint` in `region`, answering
/// the `operations` the call reported.
async fn create(
    emulator: &Emulator,
    region: Option<&str>,
    protocol: &str,
    contact_endpoint: &str,
    tags: &[(&str, &str)],
) -> Vec<Value> {
    let body = emulator
        .ok_at(
            None,
            region,
            "CreateContactMethod",
            json!({
                "protocol": protocol,
                "contactEndpoint": contact_endpoint,
                "tags": tag_list(tags),
            }),
        )
        .await;
    body["operations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case creating a {protocol} contact method: expected an operations array, got {body:?}")
        })
        .clone()
}

/// Delete the contact method of `protocol` in `region`, answering the
/// `operations` the call reported.
async fn delete(emulator: &Emulator, region: Option<&str>, protocol: &str) -> Vec<Value> {
    let body = emulator
        .ok_at(
            None,
            region,
            "DeleteContactMethod",
            json!({"protocol": protocol}),
        )
        .await;
    body["operations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case deleting the {protocol} contact method: expected an operations array, got {body:?}")
        })
        .clone()
}

/// Every contact method `GetContactMethods` reports in `region`, narrowed to
/// `protocols` when the request names any.
async fn list(
    emulator: &Emulator,
    region: Option<&str>,
    protocols: Option<&[&str]>,
) -> Vec<Value> {
    let body = match protocols {
        Some(protocols) => json!({"protocols": protocols}),
        None => json!({}),
    };
    let reported = emulator
        .ok_at(None, region, "GetContactMethods", body)
        .await;
    reported["contactMethods"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case listing contact methods in {region:?}: expected a contactMethods array, got {reported:?}")
        })
        .clone()
}

/// The one contact method of `protocol` in `region`, or `None` when that Region
/// holds none.
async fn one(emulator: &Emulator, region: Option<&str>, protocol: &str) -> Option<Value> {
    list(emulator, region, Some(&[protocol])).await.first().cloned()
}

/// The protocols of `reported`, in the order they were reported.
fn protocols_of(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|method| method["protocol"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: every member `GetContactMethods` reports comes from the one record
/// `CreateContactMethod` stored — the caller's protocol and destination verbatim,
/// the Region of the request, and every server-assigned member computed from them
/// plus the store's own allocators.
#[tokio::test]
async fn a_contact_method_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;

        // The create answers ONE operation-log entry — it creates exactly one
        // contact method — and it is already terminal: the record exists as the
        // call returns, and whether an email address has been verified is the
        // contact method's own status, not this operation's outcome.
        let operations = create(
            &emulator,
            row.region,
            row.protocol,
            row.contact_endpoint,
            row.tags,
        )
        .await;
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateContactMethod"),
            "case {case}: expected the CreateContactMethod operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("ContactMethod"),
            "case {case}: expected the ContactMethod resource type, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: expected a Succeeded operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: the contact method exists as the call returns, so the create \
             is terminal, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.reported_region),
            "case {case}: expected the operation in the region of the request, got \
             {operation:?}"
        );
        let operation_id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&operation_id),
            "case {case}: expected a uuid-shaped operation id, got {operation_id:?}"
        );
        assert!(
            operation_ids.insert(operation_id.clone()),
            "case {case}: expected a fresh operation id, got the already-used \
             {operation_id:?}"
        );

        // The stored record, read back through the implemented reader.
        let method = one(&emulator, row.region, row.protocol)
            .await
            .unwrap_or_else(|| panic!("case {case}: expected the contact method to be reported"));
        assert_eq!(
            method["protocol"].as_str(),
            Some(row.protocol),
            "case {case}: expected the protocol the request carried, got {method:?}"
        );
        assert_eq!(
            method["contactEndpoint"].as_str(),
            Some(row.contact_endpoint),
            "case {case}: expected the destination the request carried, got {method:?}"
        );
        assert_eq!(
            method["status"].as_str(),
            Some(row.status),
            "case {case}: expected the status the protocol's verification implies, got \
             {method:?}"
        );
        assert_eq!(
            method["resourceType"].as_str(),
            Some("ContactMethod"),
            "case {case}: expected the ContactMethod resource type, got {method:?}"
        );
        assert_eq!(
            method["location"]["regionName"].as_str(),
            Some(row.reported_region),
            "case {case}: expected the region of the request, got {:?}",
            method["location"]
        );
        assert!(
            method["location"]["availabilityZone"].is_null(),
            "case {case}: a contact method is held by the region, so it must report no \
             availability zone, got {:?}",
            method["location"]
        );
        assert_eq!(
            method["tags"],
            tag_list(row.tags),
            "case {case}: expected the tags the request carried, got {:?}",
            method["tags"]
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            method["name"].as_str(),
            "case {case}: expected the operation to name the contact method, got \
             {operation:?} and {method:?}"
        );

        // The ARN carries the Region of the request and the canonical emulator
        // account, and ends in a uuid no other contact method is using; the
        // support code names the same id, and so does the assigned name.
        let arn = method["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!(
            "arn:aws:lightsail:{}:{ACCOUNT}:ContactMethod/",
            row.reported_region
        );
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );
        assert_eq!(
            method["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            method["supportCode"]
        );
        let name = method["name"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            name,
            format!("{}-{id}", row.protocol),
            "case {case}: expected the assigned name to carry the protocol and the \
             resource id, got {name:?}"
        );
        assert!(
            names.insert(name.clone()),
            "case {case}: expected a fresh name, got the already-used {name:?}"
        );
        assert!(
            method["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            method["createdAt"]
        );

        // Every member the model declares is reported, and nothing beyond them.
        let reported: BTreeSet<&str> = method
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a contact method object, got {method:?}"))
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(member, _)| member.as_str())
            .collect();
        let want: BTreeSet<&str> = CONTACT_METHOD_MEMBERS.iter().copied().collect();
        assert_eq!(
            reported, want,
            "case {case}: expected exactly the modelled members, got {method:?}"
        );
    }
}

/// RULE: the record is held per Region and protocol, so a Region reports its own
/// contact methods and no others, and `protocols` narrows that answer without
/// refusing a protocol the Region has none of.
#[tokio::test]
async fn contact_methods_are_reported_per_region_and_filtered_by_protocol() {
    let emulator = Emulator::new();

    // A Region that has been asked for nothing reports an empty list, not a
    // refusal.
    for region in [None, Some("us-west-2")] {
        let empty = list(&emulator, region, None).await;
        assert!(
            empty.is_empty(),
            "case an untouched region {region:?}: expected no contact methods, got {empty:?}"
        );
    }

    create(&emulator, Some("us-west-2"), "Email", "ops@example.com", &[]).await;
    create(&emulator, Some("us-west-2"), "SMS", "+12065550100", &[]).await;
    create(&emulator, Some("eu-west-1"), "Email", "eu@example.com", &[]).await;

    // Both protocols of one Region, in the store's own total order.
    let both = list(&emulator, Some("us-west-2"), None).await;
    assert_eq!(
        protocols_of(&both),
        vec!["Email".to_string(), "SMS".to_string()],
        "case one region with both protocols: expected both contact methods, got {both:?}"
    );

    // Another Region reports only its own, and a Region nothing was created in
    // reports none — the two Regions hold two records under the same protocol.
    let elsewhere = list(&emulator, Some("eu-west-1"), None).await;
    assert_eq!(
        elsewhere.len(),
        1,
        "case another region: expected only that region's contact method, got {elsewhere:?}"
    );
    assert_eq!(
        elsewhere[0]["contactEndpoint"].as_str(),
        Some("eu@example.com"),
        "case another region: expected that region's own destination, got {elsewhere:?}"
    );
    let untouched = list(&emulator, Some("ap-south-1"), None).await;
    assert!(
        untouched.is_empty(),
        "case a region nothing was created in: expected no contact methods, got \
         {untouched:?}"
    );

    // The filter, over every subset of the modelled `ContactProtocol` values.
    let filters: &[(&str, &[&str], &[&str])] = &[
        ("one protocol", &["Email"], &["Email"]),
        ("the other protocol", &["SMS"], &["SMS"]),
        ("both protocols", &["Email", "SMS"], &["Email", "SMS"]),
        ("both, named in the other order", &["SMS", "Email"], &["Email", "SMS"]),
        ("no protocol at all", &[], &[]),
    ];
    for (case, protocols, want) in filters {
        let reported = list(&emulator, Some("us-west-2"), Some(protocols)).await;
        assert_eq!(
            protocols_of(&reported),
            want.iter().map(|p| (*p).to_string()).collect::<Vec<_>>(),
            "case the filter naming {case}: expected {want:?}, got {reported:?}"
        );
    }

    // A filter naming a protocol this Region holds none of reports an empty list
    // rather than refusing.
    let none_here = list(&emulator, Some("eu-west-1"), Some(&["SMS"])).await;
    assert!(
        none_here.is_empty(),
        "case a protocol the region holds none of: expected an empty list, got \
         {none_here:?}"
    );
}

/// RULE: one Region holds ONE contact method per protocol and the model declares
/// no operation that updates one, so creating the same protocol again replaces
/// that record — a fresh identity for the new destination — and leaves the other
/// protocol and the other Regions alone.
#[tokio::test]
async fn creating_a_protocol_again_replaces_the_one_record() {
    let emulator = Emulator::new();
    create(&emulator, Some("us-east-1"), "Email", "first@example.com", &[("stack", "a")]).await;
    create(&emulator, Some("us-east-1"), "SMS", "+12065550100", &[]).await;
    create(&emulator, Some("eu-west-1"), "Email", "eu@example.com", &[]).await;
    let before = one(&emulator, Some("us-east-1"), "Email").await.expect(
        "case replacing a contact method: expected the first contact method to be reported",
    );

    create(&emulator, Some("us-east-1"), "Email", "second@example.com", &[("stack", "b")]).await;

    let emails = list(&emulator, Some("us-east-1"), Some(&["Email"])).await;
    assert_eq!(
        emails.len(),
        1,
        "case replacing a contact method: expected one record per protocol, got {emails:?}"
    );
    let after = emails[0].clone();
    assert_eq!(
        after["contactEndpoint"].as_str(),
        Some("second@example.com"),
        "case replacing a contact method: expected the destination of the second create, \
         got {after:?}"
    );
    assert_eq!(
        after["tags"],
        tag_list(&[("stack", "b")]),
        "case replacing a contact method: expected the tags of the second create, got \
         {after:?}"
    );
    assert_ne!(
        after["arn"], before["arn"],
        "case replacing a contact method: a different destination is a different contact \
         method, so it carries its own ARN, got {after:?}"
    );
    assert_ne!(
        after["name"], before["name"],
        "case replacing a contact method: a different destination is a different contact \
         method, so it carries its own name, got {after:?}"
    );

    // The other protocol in the same Region, and the same protocol in another
    // Region, are untouched.
    let sms = one(&emulator, Some("us-east-1"), "SMS").await.expect(
        "case replacing a contact method: expected the other protocol to survive",
    );
    assert_eq!(
        sms["contactEndpoint"].as_str(),
        Some("+12065550100"),
        "case replacing a contact method: expected the other protocol untouched, got {sms:?}"
    );
    let elsewhere = one(&emulator, Some("eu-west-1"), "Email").await.expect(
        "case replacing a contact method: expected the other region's record to survive",
    );
    assert_eq!(
        elsewhere["contactEndpoint"].as_str(),
        Some("eu@example.com"),
        "case replacing a contact method: expected the other region untouched, got \
         {elsewhere:?}"
    );
}

/// RULE: `DeleteContactMethod` removes the record its protocol names, so the
/// Region stops reporting it and a second delete of the same protocol is refused
/// with Lightsail's `DoesNotExist` envelope — HTTP 400, `NotFoundException` on
/// the wire, `code` `DoesNotExist` — rather than reporting a deletion that did
/// not happen.
#[tokio::test]
async fn a_deleted_contact_method_is_gone_and_cannot_be_deleted_twice() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        create(
            &emulator,
            row.region,
            row.protocol,
            row.contact_endpoint,
            row.tags,
        )
        .await;
        let created = one(&emulator, row.region, row.protocol)
            .await
            .unwrap_or_else(|| panic!("case {case}: expected the contact method to be reported"));

        let operations = delete(&emulator, row.region, row.protocol).await;
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {operations:?}"
        );
        let operation = operations[0].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("DeleteContactMethod"),
            "case {case}: expected the DeleteContactMethod operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("ContactMethod"),
            "case {case}: expected the ContactMethod resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            created["name"].as_str(),
            "case {case}: expected the operation to name the deleted contact method, got \
             {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Succeeded"),
            "case {case}: expected a Succeeded operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: the record is gone as the call returns, so the delete is \
             terminal, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.reported_region),
            "case {case}: expected the operation in the region of the request, got \
             {operation:?}"
        );

        // Gone from the Region's answer, whether or not the read filters.
        let after = list(&emulator, row.region, None).await;
        assert!(
            after.is_empty(),
            "case {case}: expected the contact method to be gone, got {after:?}"
        );
        assert!(
            one(&emulator, row.region, row.protocol).await.is_none(),
            "case {case}: expected the filtered read to report it gone too"
        );

        // And the second delete is refused.
        let (status, refused) = emulator
            .call_in(row.region, "DeleteContactMethod", json!({"protocol": row.protocol}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the DoesNotExist status for a second delete, got \
             {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "NotFoundException",
            "case {case}: expected NotFoundException on the wire, got {refused:?}"
        );
        assert_eq!(
            refused["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist code, got {refused:?}"
        );
        assert!(
            refused["message"].as_str().is_some_and(|message| {
                message.contains("ContactMethod") && message.contains(row.protocol)
            }),
            "case {case}: expected a message naming the resource type and the protocol \
             the request carried, got {refused:?}"
        );
    }
}

/// RULE: the record a delete removes is the one of THAT protocol in THAT Region —
/// "to delete an Email and an SMS contact method if you added both, you must run
/// separate DeleteContactMethod actions to delete each protocol" — so every other
/// record survives, and a delete in a Region that holds none is refused while the
/// record held elsewhere stays put.
#[tokio::test]
async fn a_delete_removes_one_protocol_in_one_region() {
    let emulator = Emulator::new();
    create(&emulator, Some("us-west-2"), "Email", "ops@example.com", &[]).await;
    create(&emulator, Some("us-west-2"), "SMS", "+12065550100", &[]).await;
    create(&emulator, Some("eu-west-1"), "Email", "eu@example.com", &[]).await;

    delete(&emulator, Some("us-west-2"), "SMS").await;

    let remaining = list(&emulator, Some("us-west-2"), None).await;
    assert_eq!(
        protocols_of(&remaining),
        vec!["Email".to_string()],
        "case deleting one of two protocols: expected the other to survive, got \
         {remaining:?}"
    );
    let elsewhere = list(&emulator, Some("eu-west-1"), None).await;
    assert_eq!(
        protocols_of(&elsewhere),
        vec!["Email".to_string()],
        "case deleting in one region: expected another region untouched, got {elsewhere:?}"
    );

    // A Region that holds no contact method of that protocol refuses the delete,
    // and the record another Region holds under the same protocol survives it.
    let (status, refused) = emulator
        .call_in(Some("eu-west-1"), "DeleteContactMethod", json!({"protocol": "SMS"}))
        .await;
    assert_eq!(
        status, 400,
        "case deleting a protocol the region holds none of: expected the DoesNotExist \
         status, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "NotFoundException",
        "case deleting a protocol the region holds none of: expected NotFoundException, \
         got {refused:?}"
    );
    let (status, refused) = emulator
        .call_in(Some("us-west-2"), "DeleteContactMethod", json!({"protocol": "Email"}))
        .await;
    assert_eq!(
        status, 200,
        "case deleting the last protocol of a region: expected it to succeed, got \
         {status} {refused:?}"
    );
    let survivor = one(&emulator, Some("eu-west-1"), "Email").await.expect(
        "case deleting in one region: expected the other region's record to survive",
    );
    assert_eq!(
        survivor["contactEndpoint"].as_str(),
        Some("eu@example.com"),
        "case deleting in one region: expected the other region untouched, got {survivor:?}"
    );
}

/// One destination case: whether the protocol can reach it.
struct Endpoint {
    case: &'static str,
    protocol: &'static str,
    contact_endpoint: &'static str,
    accepted: bool,
}

/// A phone number with exactly [`MAX_E164_DIGITS`] digits — the published bound —
/// and one with a digit more.
const E164_AT_THE_BOUND: &str = "+123456789012345";
const E164_PAST_THE_BOUND: &str = "+1234567890123456";

/// The destination table. The E.164 rows walk both ends of the digit range the
/// modelled `CreateContactMethodRequest$contactEndpoint` documentation publishes
/// (one digit, [`MAX_E164_DIGITS`] digits, one past it, none at all), and the
/// email rows walk the ways an address can be malformed.
const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        case: "the modelled phone number form",
        protocol: "SMS",
        contact_endpoint: "+12065550100",
        accepted: true,
    },
    Endpoint {
        case: "a one-digit phone number",
        protocol: "SMS",
        contact_endpoint: "+1",
        accepted: true,
    },
    Endpoint {
        case: "a phone number at the published digit bound",
        protocol: "SMS",
        contact_endpoint: E164_AT_THE_BOUND,
        accepted: true,
    },
    Endpoint {
        case: "a phone number one digit past the published bound",
        protocol: "SMS",
        contact_endpoint: E164_PAST_THE_BOUND,
        accepted: false,
    },
    Endpoint {
        case: "a phone number with no digits at all",
        protocol: "SMS",
        contact_endpoint: "+",
        accepted: false,
    },
    Endpoint {
        case: "a phone number without the plus character",
        protocol: "SMS",
        contact_endpoint: "12065550100",
        accepted: false,
    },
    Endpoint {
        case: "a phone number with separators",
        protocol: "SMS",
        contact_endpoint: "+1 206 555 0100",
        accepted: false,
    },
    Endpoint {
        case: "an email address where a phone number belongs",
        protocol: "SMS",
        contact_endpoint: "ops@example.com",
        accepted: false,
    },
    Endpoint {
        case: "a plain email address",
        protocol: "Email",
        contact_endpoint: "ops@example.com",
        accepted: true,
    },
    Endpoint {
        case: "an email address with no at sign",
        protocol: "Email",
        contact_endpoint: "ops.example.com",
        accepted: false,
    },
    Endpoint {
        case: "an email address with two at signs",
        protocol: "Email",
        contact_endpoint: "ops@@example.com",
        accepted: false,
    },
    Endpoint {
        case: "an email address with no local part",
        protocol: "Email",
        contact_endpoint: "@example.com",
        accepted: false,
    },
    Endpoint {
        case: "an email address with no domain",
        protocol: "Email",
        contact_endpoint: "ops@",
        accepted: false,
    },
    Endpoint {
        case: "an email address whose domain is a single label",
        protocol: "Email",
        contact_endpoint: "ops@localhost",
        accepted: false,
    },
    Endpoint {
        case: "an email address with an empty domain label",
        protocol: "Email",
        contact_endpoint: "ops@example..com",
        accepted: false,
    },
    Endpoint {
        case: "an email address with a space in it",
        protocol: "Email",
        contact_endpoint: "two ops@example.com",
        accepted: false,
    },
    Endpoint {
        case: "a phone number where an email address belongs",
        protocol: "Email",
        contact_endpoint: "+12065550100",
        accepted: false,
    },
];

/// RULE: a destination the protocol cannot reach is refused with the modelled
/// `InvalidInputException` (400) and stores nothing, so the Region still reports
/// no contact method of that protocol.
#[tokio::test]
async fn a_destination_the_protocol_cannot_reach_is_refused() {
    assert_eq!(
        E164_AT_THE_BOUND.len() - 1,
        MAX_E164_DIGITS,
        "case the published digit bound: the table must walk the bound this service \
         publishes, which is {MAX_E164_DIGITS}"
    );

    for endpoint in ENDPOINTS {
        let case = endpoint.case;
        // One emulator per row: a create that succeeds would otherwise replace
        // the record a later row of the same protocol asserts about.
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call(
                "CreateContactMethod",
                json!({
                    "protocol": endpoint.protocol,
                    "contactEndpoint": endpoint.contact_endpoint,
                }),
            )
            .await;
        if endpoint.accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected {} to be accepted, got {status} {body:?}",
                endpoint.contact_endpoint
            );
            let stored = one(&emulator, None, endpoint.protocol).await;
            assert_eq!(
                stored.as_ref().and_then(|method| method["contactEndpoint"].as_str()),
                Some(endpoint.contact_endpoint),
                "case {case}: expected the accepted destination to be stored, got {stored:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: expected {} to be refused, got {status} {body:?}",
            endpoint.contact_endpoint
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("contactEndpoint")),
            "case {case}: expected a message naming the member that is wrong, got {body:?}"
        );
        let stored = list(&emulator, None, None).await;
        assert!(
            stored.is_empty(),
            "case {case}: the refused create must store nothing, got {stored:?}"
        );
    }
}

/// RULE: the SMS protocol is served only in the Regions this service publishes,
/// so an SMS contact method asked for in any other Region is refused with the
/// modelled `InvalidInputException` (400) and stores nothing — while an email
/// contact method is served in every Region.
///
/// The Region list is `crate::state::SMS_SUPPORTED_REGIONS`, read from the crate
/// rather than re-spelled here; its source is the modelled
/// `CreateContactMethodRequest$protocol` documentation.
#[tokio::test]
async fn sms_is_served_only_where_this_service_publishes_it() {
    let emulator = Emulator::new();
    for region in SMS_SUPPORTED_REGIONS {
        let (status, body) = emulator
            .call_in(
                Some(region),
                "CreateContactMethod",
                json!({"protocol": "SMS", "contactEndpoint": "+12065550100"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case SMS in the supported region {region}: expected it to be accepted, got \
             {status} {body:?}"
        );
    }

    // Regions of the modelled `RegionName` enum that are NOT on that list.
    for region in ["eu-central-1", "ca-central-1", "sa-east-1", "us-east-2"] {
        let (status, body) = emulator
            .call_in(
                Some(region),
                "CreateContactMethod",
                json!({"protocol": "SMS", "contactEndpoint": "+12065550100"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case SMS in the unsupported region {region}: expected it to be refused, got \
             {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case SMS in the unsupported region {region}: expected InvalidInputException, \
             got {body:?}"
        );
        let stored = list(&emulator, Some(region), None).await;
        assert!(
            stored.is_empty(),
            "case SMS in the unsupported region {region}: the refused create must store \
             nothing, got {stored:?}"
        );

        // The restriction is the protocol's, not the Region's: an email contact
        // method is served there.
        let (status, body) = emulator
            .call_in(
                Some(region),
                "CreateContactMethod",
                json!({"protocol": "Email", "contactEndpoint": "ops@example.com"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case an email contact method in {region}: expected it to be accepted, got \
             {status} {body:?}"
        );
    }
}

/// RULE: a read reports the records and never changes them, so a contact method
/// reads back the same however many times it is read and whatever else was
/// created after it — and in particular a read never completes the verification a
/// contact method is waiting on.
#[tokio::test]
async fn reading_contact_methods_changes_nothing() {
    let emulator = Emulator::new();
    for row in ROWS {
        create(
            &emulator,
            row.region,
            row.protocol,
            row.contact_endpoint,
            row.tags,
        )
        .await;
    }

    for row in ROWS {
        let case = row.case;
        let first = one(&emulator, row.region, row.protocol).await;
        let again = list(&emulator, row.region, None).await;
        let third = one(&emulator, row.region, row.protocol).await;
        assert_eq!(
            first, third,
            "case {case}: expected the same record on a later read, got {third:?}"
        );
        assert!(
            again.contains(&third.clone().unwrap_or(Value::Null)),
            "case {case}: expected the unfiltered read to report the same record, got \
             {again:?}"
        );
        assert_eq!(
            third.as_ref().and_then(|method| method["status"].as_str()),
            Some(row.status),
            "case {case}: a read must not move the verification status, got {third:?}"
        );
    }
}
