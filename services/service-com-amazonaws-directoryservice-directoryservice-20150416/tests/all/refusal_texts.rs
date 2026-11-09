// SPDX-License-Identifier: CC0-1.0

//! The error CLASS and the diagnostic TEXT of this service's refusal paths, driven end to
//! end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the directory-lifecycle suite's,
//! reached as `crate::directory_lifecycle::<item>`).
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! call. A refusal states Directory Service's own sentence for its error class: the fixed
//! text verbatim, its variable parts taken from the request or from stored state, and the
//! request-id suffix every refusal of this service closes with.
//!
//! * `AddIpRoutes` of a block the directory already routes is the general CLIENT refusal —
//!   not a conflicting entity — and lists every already-routed block of the request so the
//!   caller can delete exactly those.
//! * `ShareDirectory` of a pairing already shared states the directory and the account, and
//!   discloses nothing of the shared directory the first share minted.
//! * `UpdateNumberOfDomainControllers` past the per-directory ceiling states the ceiling and
//!   not the number the request asked for.
//! * `CreateConditionalForwarder` of a remote domain already forwarded answers from the DNS
//!   server that would have hosted the zone, naming the zone as the request spelled it.
//! * `EnableLDAPS` with no certificate in service states one fixed sentence, whatever the
//!   directory holds, and an LDAPS toggle the stored STATUS does not admit states this
//!   family's other fixed sentence, naming the action and nothing else.
//! * `EnableDirectoryDataAccess` / `DisableDirectoryDataAccess` against a directory already
//!   in the desired state state one sentence naming only the directory.
//! * `DescribeSnapshots` UNSCOPED and narrowed by a snapshot id nothing is stored under
//!   reports an unusable FILTER VALUE (`InvalidParameterException`), not a missing entity.
//! * a conditional-forwarder mutation against a directory outside the stages that serve its
//!   DNS configuration is refused as a PARAMETER naming the mutation and those stages, and
//!   the same call is served once the directory reaches one of them.

use serde_json::{json, Value};

use crate::certificates::{
    certificate_pem, connector_in_service, TYPE_CLIENT_CERT_AUTH, TYPE_CLIENT_LDAPS,
};
use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, service, string, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};
use crate::shared_directories::{consumer_account, share_body, SHARE_METHOD_CATALOG};

/// Directory Service's default quota of domain controllers per directory, from the modelled
/// `DomainControllerLimitExceededException`'s own documentation: "The default limit per
/// directory is 20 domain controllers."
const DOMAIN_CONTROLLER_QUOTA: i64 = 20;

/// The `LDAPSType` the LDAP-security switch is activated for: the only value of the modelled
/// `LDAPSType` enum (`smithy_query(query="shape",shape="LDAPSType")`).
const LDAPS_TYPE: &str = "Client";

/// The stages that serve a mutation of a directory's conditional forwarders, as the refusal
/// of a directory outside them lists them (`state::CONDITIONAL_FORWARDER_SERVING_STAGES`,
/// two values of the modelled `DirectoryStage` enum).
const FORWARDER_SERVING_STAGES: &str = "Active or Impaired";

/// The HTTP status, the modelled error code and the whole `Message` of a refusal, asserted
/// to be a 400 of `expected_code` and returned for the text assertions that follow.
async fn refusal(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) -> String {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected {expected_code}, got {parsed}"
    );
    let request_id = parsed["RequestId"].as_str().unwrap_or_default();
    assert!(
        !request_id.is_empty(),
        "case {case:?}: expected the modelled RequestId, got {parsed}"
    );
    let message = parsed["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {parsed}"))
        .to_string();
    assert!(
        message.ends_with(&format!(" : RequestId: {request_id}")),
        "case {case:?}: expected the request-id suffix the answer reported, got {message:?}"
    );
    message
}

/// `message` with the request-id suffix stripped: the diagnostic the service worded, which
/// is what the templates below are asserted against.
fn stated(message: &str) -> &str {
    message
        .rsplit_once(" : RequestId: ")
        .map(|(text, _)| text)
        .unwrap_or(message)
}

/// A Managed Microsoft AD directory past the stage transition its create owes, so it is in
/// service for the mutations the cases make.
async fn microsoft_ad_in_service(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    case: &str,
) -> String {
    let id = create_microsoft_ad(router, minimal_microsoft_ad(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// RULE: a request naming IP address blocks the directory already routes is refused with the
/// general client refusal, listing every already-routed block the request named.
#[tokio::test]
async fn already_routed_blocks_are_listed_in_the_client_refusal() {
    let router = service();
    let directory_id = create_directory(
        &router,
        minimal_create("routed.example.com"),
        "already routed",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let stored = ["203.0.113.0/24", "198.51.100.0/24"];
    ok(
        &router,
        "AddIpRoutes",
        json!({
            "DirectoryId": &directory_id,
            "IpRoutes": [
                { "CidrIp": stored[0] },
                { "CidrIp": stored[1], "Description": "office" },
                { "CidrIpv6": "2001:db8::/32" },
            ]
        }),
        "already routed: seeding",
    )
    .await;
    // The seeding add left the directory under an update, and a change arriving inside
    // that window is refused for THAT reason. The rows below are about blocks the
    // directory already routes, so they are issued once the seeded routing has settled.
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // The rows differ along HOW the block repeats and along what is reported: one stored
    // IPv4 block, a stored IPv6 block, two stored blocks at once (so the list has more than
    // one entry), a block the request itself names twice, and a new block beside a stored
    // one — where only the stored one is already there.
    let rows: [(&str, Value, Vec<&str>); 5] = [
        (
            "one stored IPv4 block",
            json!([{ "CidrIp": stored[0] }]),
            vec![stored[0]],
        ),
        (
            "a stored IPv6 block",
            json!([{ "CidrIpv6": "2001:db8::/32" }]),
            vec!["2001:db8::/32"],
        ),
        (
            "both stored IPv4 blocks",
            json!([{ "CidrIp": stored[1] }, { "CidrIp": stored[0] }]),
            vec![stored[1], stored[0]],
        ),
        (
            "a block the request names twice",
            json!([{ "CidrIp": "192.0.2.0/24" }, { "CidrIp": "192.0.2.0/24" }]),
            vec!["192.0.2.0/24"],
        ),
        (
            "a new block beside a stored one",
            json!([{ "CidrIp": "192.0.2.128/25" }, { "CidrIp": stored[0] }]),
            vec![stored[0]],
        ),
    ];

    for (case, routes, offending) in rows {
        let message = refusal(
            &router,
            "AddIpRoutes",
            json!({ "DirectoryId": &directory_id, "IpRoutes": routes }),
            "ClientException",
            case,
        )
        .await;
        assert_eq!(
            stated(&message),
            format!(
                "The IP route(s) [{blocks}] already exist. If you want to recreate them, \
                 please delete the existing routes first.",
                blocks = offending.join(", ")
            ),
            "case {case:?}: unexpected diagnostic"
        );
        // The refusal is not a conflicting entity, and does not name the directory.
        assert!(
            !message.contains(&directory_id),
            "case {case:?}: expected no directory id in the diagnostic, got {message:?}"
        );
    }

    // A block none of this names IS accepted, so the refusals above are about the blocks
    // and not about the operation.
    ok(
        &router,
        "AddIpRoutes",
        json!({
            "DirectoryId": &directory_id,
            "IpRoutes": [{ "CidrIp": "192.0.2.0/24" }]
        }),
        "a block that is not routed yet",
    )
    .await;
}

/// RULE: a second share of a pairing already in place states the directory and the account,
/// and nothing of the shared directory the first share minted.
#[tokio::test]
async fn the_already_shared_refusal_states_the_directory_and_the_account() {
    let router = service();
    let owner = microsoft_ad_in_service(&router, "owner.example.com", "already shared").await;
    let other_owner =
        microsoft_ad_in_service(&router, "other.example.com", "already shared").await;

    // The rows differ along the owner directory, the consumer account, the method the first
    // share used and the method the second asks for: the refusal is about the PAIRING, so
    // none of that changes the sentence.
    let rows = [
        (&owner, consumer_account(1), SHARE_METHOD_CATALOG[1]),
        (&owner, consumer_account(2), SHARE_METHOD_CATALOG[0]),
        (&other_owner, consumer_account(1), SHARE_METHOD_CATALOG[1]),
    ];

    for (directory_id, account_id, first_method) in rows {
        let case = format!("{directory_id} shared with {account_id} by {first_method}");
        let first = ok(
            &router,
            "ShareDirectory",
            share_body(directory_id, &account_id, first_method, None),
            &case,
        )
        .await;
        let shared_directory_id = string(&first, "SharedDirectoryId", &case);

        for method in SHARE_METHOD_CATALOG {
            let case = format!("{case}, again by {method}");
            let message = refusal(
                &router,
                "ShareDirectory",
                share_body(directory_id, &account_id, method, None),
                "DirectoryAlreadySharedException",
                &case,
            )
            .await;
            assert_eq!(
                stated(&message),
                format!("Directory {directory_id} is already shared with Account {account_id}."),
                "case {case:?}: unexpected diagnostic"
            );
            // The consumer's handle on the owner's directory is not disclosed here.
            assert!(
                !message.contains(&shared_directory_id),
                "case {case:?}: expected no shared directory id in the diagnostic, got \
                 {message:?}"
            );
        }
    }
}

/// RULE: a resize past the per-directory ceiling states the ceiling, not the request.
#[tokio::test]
async fn the_domain_controller_quota_refusal_states_the_ceiling_only() {
    let router = service();
    let directory_id =
        microsoft_ad_in_service(&router, "controllers.example.com", "the ceiling").await;

    // The rows differ in how far past the ceiling the request reaches: one past it, a round
    // hundred, and the largest value the modelled integer carries.
    for desired in [
        DOMAIN_CONTROLLER_QUOTA + 1,
        100,
        i64::from(i32::MAX),
    ] {
        let case = format!("DesiredNumber {desired}");
        let message = refusal(
            &router,
            "UpdateNumberOfDomainControllers",
            json!({ "DirectoryId": &directory_id, "DesiredNumber": desired }),
            "DomainControllerLimitExceededException",
            &case,
        )
        .await;
        assert_eq!(
            stated(&message),
            format!(
                "Domain controllers limit exceeded. A maximum of {DOMAIN_CONTROLLER_QUOTA} \
                 domain controllers may be requested."
            ),
            "case {case:?}: unexpected diagnostic"
        );
        assert!(
            !message.contains(&desired.to_string()),
            "case {case:?}: expected the requested number to be absent, got {message:?}"
        );
    }
}

/// RULE: a second conditional forwarder for a remote domain already forwarded is answered by
/// the DNS server that would have hosted the zone, naming the zone the request spelled.
#[tokio::test]
async fn the_duplicate_forwarder_refusal_names_the_zone_and_the_dns_server() {
    let router = service();
    let domain_name = "forwarders.example.com";
    let directory_id = create_directory(
        &router,
        minimal_create(domain_name),
        "a duplicate forwarder",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // The first create spells the remote domain one way; every row of the table asks again
    // with a spelling that names the same DNS domain (case, and the trailing root dot the
    // modelled `RemoteDomainName` pattern admits), plus one of another remote domain.
    let rows = [
        ("remote.example.net", vec!["remote.example.net", "REMOTE.EXAMPLE.NET", "remote.example.net."]),
        ("partner-dept.example.org", vec!["Partner-Dept.Example.Org."]),
    ];

    for (domain, duplicates) in rows {
        let case = format!("a forwarder for {domain}");
        ok(
            &router,
            "CreateConditionalForwarder",
            json!({
                "DirectoryId": &directory_id,
                "RemoteDomainName": domain,
                "DnsIpAddrs": ["203.0.113.9"],
            }),
            &case,
        )
        .await;

        for spelling in duplicates {
            let case = format!("{case}, again as {spelling:?}");
            let message = refusal(
                &router,
                "CreateConditionalForwarder",
                json!({
                    "DirectoryId": &directory_id,
                    "RemoteDomainName": spelling,
                    "DnsIpAddrs": ["203.0.113.10"],
                }),
                "EntityAlreadyExistsException",
                &case,
            )
            .await;
            // The zone is the one the REQUEST spelled, and the server is the directory's
            // own DNS, which this emulator names by the domain the directory serves.
            assert_eq!(
                stated(&message),
                format!(
                    "Failed to create zone {spelling} on server {domain_name}. A conditional \
                     forwarding zone already exists for that name. "
                ),
                "case {case:?}: unexpected diagnostic"
            );
        }
    }
}

/// RULE: enabling LDAP security with no certificate in service states one fixed sentence,
/// whatever the directory holds.
#[tokio::test]
async fn the_no_certificate_refusal_states_one_fixed_sentence() {
    let router = service();

    // The rows differ along WHY no certificate secures the LDAP calls: the directory holds
    // none at all, it holds one of the other modelled certificate function, and it holds a
    // registration the clock has not put in service yet. The middle row's directory is an
    // AD CONNECTOR, the type client authentication is offered for, since that is the
    // function its certificate performs; LDAP security is served by that type too.
    let bare = microsoft_ad_in_service(&router, "bare.example.com", "no certificate").await;
    let other_function =
        connector_in_service(&router, "other.example.com", "another function").await;
    let registering =
        microsoft_ad_in_service(&router, "fresh.example.com", "still registering").await;

    let (other_pem, _) = certificate_pem(Some("other.example.com"), (2032, 7, 7));
    ok(
        &router,
        "RegisterCertificate",
        json!({
            "DirectoryId": &other_function,
            "CertificateData": other_pem,
            "Type": TYPE_CLIENT_CERT_AUTH,
            "ClientCertAuthSettings": { "OCSPUrl": "http://ocsp.example.com/check" }
        }),
        "another function",
    )
    .await;
    let (fresh_pem, _) = certificate_pem(Some("fresh.example.com"), (2032, 7, 7));
    ok(
        &router,
        "RegisterCertificate",
        json!({
            "DirectoryId": &registering,
            "CertificateData": fresh_pem,
            "Type": TYPE_CLIENT_LDAPS
        }),
        "still registering",
    )
    .await;

    for (case, directory_id) in [
        ("no certificate at all", &bare),
        ("a certificate of another function", &other_function),
        ("a registration still being made", &registering),
    ] {
        let message = refusal(
            &router,
            "EnableLDAPS",
            json!({ "DirectoryId": directory_id, "Type": LDAPS_TYPE }),
            "NoAvailableCertificateException",
            case,
        )
        .await;
        assert_eq!(
            stated(&message),
            "Cannot enable LDAPS since there is no valid certificate available.",
            "case {case:?}: unexpected diagnostic"
        );
        assert!(
            !message.contains(directory_id.as_str()),
            "case {case:?}: expected no directory id in the fixed sentence, got {message:?}"
        );
    }

    // The refusal was about the certificate, so the same call is admitted once the
    // registration the last row made has settled.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "EnableLDAPS",
        json!({ "DirectoryId": &registering, "Type": LDAPS_TYPE }),
        "the registration settled",
    )
    .await;

    // ...and the toggle refused by the STATUS its setting carries states this family's other
    // fixed sentence, with the action the call asked for in it and nothing of the directory,
    // the type or the status. The rows differ along the action and along the status that
    // refuses it: an enable while the enable's own transition is owed, a disable of a type
    // nothing ever enabled, and an enable of a type that is enabled.
    let rows = [
        (
            "an enable while the enable is still owed",
            "EnableLDAPS",
            &registering,
            "enable",
        ),
        (
            "a disable of a type nothing enabled",
            "DisableLDAPS",
            &bare,
            "disable",
        ),
    ];
    for (case, operation, directory_id, action) in rows {
        let message = refusal(
            &router,
            operation,
            json!({ "DirectoryId": directory_id, "Type": LDAPS_TYPE }),
            "InvalidLDAPSStatusException",
            case,
        )
        .await;
        assert_eq!(
            stated(&message),
            format!("Cannot {action} LDAPS for a directory in an invalid LDAPS status."),
            "case {case:?}: unexpected diagnostic"
        );
        assert!(
            !message.contains(directory_id.as_str()) && !message.contains(LDAPS_TYPE),
            "case {case:?}: expected neither the directory nor the type in the fixed \
             sentence, got {message:?}"
        );
    }

    tokio::time::sleep(PAST_ONE_SPAN).await;
    let enabled = refusal(
        &router,
        "EnableLDAPS",
        json!({ "DirectoryId": &registering, "Type": LDAPS_TYPE }),
        "InvalidLDAPSStatusException",
        "an enable once LDAP security is active",
    )
    .await;
    assert_eq!(
        stated(&enabled),
        "Cannot enable LDAPS for a directory in an invalid LDAPS status.",
        "case \"an enable once LDAP security is active\": unexpected diagnostic"
    );

    // The disable of that same directory, which IS a change, is accepted — so the refusals
    // above follow the stored status rather than the operation being repeated.
    ok(
        &router,
        "DisableLDAPS",
        json!({ "DirectoryId": &registering, "Type": LDAPS_TYPE }),
        "the disable that is a change",
    )
    .await;
}

/// RULE: a data-access toggle whose directory already has the setting it asks for states one
/// sentence naming only the directory — the same one for the enable and for the disable.
#[tokio::test]
async fn the_in_desired_state_refusal_names_only_the_directory() {
    let router = service();
    let enabling = create_directory(
        &router,
        minimal_create("enabling.example.com"),
        "in desired state",
    )
    .await;
    let never_enabled = create_directory(
        &router,
        minimal_create("disabled.example.com"),
        "in desired state",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "EnableDirectoryDataAccess",
        json!({ "DirectoryId": &enabling }),
        "in desired state: the first enable",
    )
    .await;

    // The rows differ along the operation and along the state the directory is in: an enable
    // while the first enable's transition is still owed, a disable of a directory nothing
    // ever enabled, and — after the clock settles — an enable of an enabled directory and a
    // disable while the disable's own transition is owed.
    let rows: [(&str, &str, &String); 2] = [
        (
            "an enable while the enable is still owed",
            "EnableDirectoryDataAccess",
            &enabling,
        ),
        (
            "a disable of a directory nothing enabled",
            "DisableDirectoryDataAccess",
            &never_enabled,
        ),
    ];
    for (case, operation, directory_id) in rows {
        let message = refusal(
            &router,
            operation,
            json!({ "DirectoryId": directory_id }),
            "DirectoryInDesiredStateException",
            case,
        )
        .await;
        assert_eq!(
            stated(&message),
            format!("Directory {directory_id} is already in the desired state."),
            "case {case:?}: unexpected diagnostic"
        );
    }

    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = refusal(
        &router,
        "EnableDirectoryDataAccess",
        json!({ "DirectoryId": &enabling }),
        "DirectoryInDesiredStateException",
        "an enable once data access is enabled",
    )
    .await;
    assert_eq!(
        stated(&settled),
        format!("Directory {enabling} is already in the desired state."),
        "case \"an enable once data access is enabled\": unexpected diagnostic"
    );

    // ...and the disable of that same directory, which IS a change, is accepted — so the
    // refusals above follow the stored state rather than the operation being repeated.
    ok(
        &router,
        "DisableDirectoryDataAccess",
        json!({ "DirectoryId": &enabling }),
        "the disable that is a change",
    )
    .await;
    let disabling = refusal(
        &router,
        "DisableDirectoryDataAccess",
        json!({ "DirectoryId": &enabling }),
        "DirectoryInDesiredStateException",
        "a disable while the disable is still owed",
    )
    .await;
    assert_eq!(
        stated(&disabling),
        format!("Directory {enabling} is already in the desired state."),
        "case \"a disable while the disable is still owed\": unexpected diagnostic"
    );
}

/// RULE: a `SnapshotIds` entry nothing is stored under is an unusable FILTER VALUE, not a
/// missing entity, while the `DirectoryId` filter keeps the lookup family's refusal.
#[tokio::test]
async fn an_unresolvable_snapshot_filter_is_an_invalid_parameter() {
    let router = service();
    let directory_id = create_directory(
        &router,
        minimal_create("snapshots.example.com"),
        "snapshot filters",
    )
    .await;
    let doomed_directory = create_directory(
        &router,
        minimal_create("doomed.example.com"),
        "snapshot filters",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let kept = string(
        &ok(
            &router,
            "CreateSnapshot",
            json!({ "DirectoryId": &directory_id, "Name": "kept" }),
            "snapshot filters",
        )
        .await,
        "SnapshotId",
        "snapshot filters",
    );
    let deleted = string(
        &ok(
            &router,
            "CreateSnapshot",
            json!({ "DirectoryId": &directory_id, "Name": "deleted" }),
            "snapshot filters",
        )
        .await,
        "SnapshotId",
        "snapshot filters",
    );
    ok(
        &router,
        "DeleteSnapshot",
        json!({ "SnapshotId": &deleted }),
        "snapshot filters: the delete",
    )
    .await;
    let with_its_directory = string(
        &ok(
            &router,
            "CreateSnapshot",
            json!({ "DirectoryId": &doomed_directory, "Name": "doomed" }),
            "snapshot filters",
        )
        .await,
        "SnapshotId",
        "snapshot filters",
    );
    ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": &doomed_directory }),
        "snapshot filters: the directory delete",
    )
    .await;

    // The rows differ along WHY the id resolves to nothing: an id nothing ever minted, one a
    // delete removed, one that went with the directory it was taken of, and a well-formed id
    // sent beside one that does resolve.
    let rows = [
        ("an id nothing minted", "s-0000000000".to_string(), None),
        ("a deleted snapshot", deleted.clone(), None),
        (
            "a snapshot of a deleted directory",
            with_its_directory.clone(),
            None,
        ),
        (
            "an absent id beside a stored one",
            "s-00000000ff".to_string(),
            Some(kept.clone()),
        ),
    ];

    for (case, absent, present) in rows {
        let filter: Vec<String> = match present {
            Some(stored) => vec![stored, absent.clone()],
            None => vec![absent.clone()],
        };
        let message = refusal(
            &router,
            "DescribeSnapshots",
            json!({ "SnapshotIds": filter }),
            "InvalidParameterException",
            case,
        )
        .await;
        assert_eq!(
            stated(&message),
            format!("Invalid snapshot ID: {absent}."),
            "case {case:?}: unexpected diagnostic"
        );
    }

    // The `DirectoryId` filter is the other family: an id no directory is stored under is
    // reported as an invalid directory id under the lookup refusal, so the two filters are
    // not collapsed into one class.
    let directory_filter = refusal(
        &router,
        "DescribeSnapshots",
        json!({ "DirectoryId": &doomed_directory }),
        "EntityDoesNotExistException",
        "a directory filter naming no directory",
    )
    .await;
    assert_eq!(
        stated(&directory_filter),
        format!("Invalid directory ID: {doomed_directory}."),
        "case \"a directory filter naming no directory\": unexpected diagnostic"
    );

    // The snapshot that IS stored still lists, so the refusals above are about the absent
    // ids alone.
    let listed = ok(
        &router,
        "DescribeSnapshots",
        json!({ "SnapshotIds": [&kept] }),
        "the stored snapshot",
    )
    .await;
    let reported: Vec<String> = listed["Snapshots"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|snapshot| snapshot["SnapshotId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported,
        vec![kept.clone()],
        "case \"the stored snapshot\": expected the stored snapshot to list, got {listed}"
    );
}

/// An otherwise valid body for each mutation of the conditional-forwarder family, so every
/// row reaches the stage guard rather than a boundary refusal.
fn forwarder_body(operation: &str, directory_id: &str) -> Value {
    let mut body = json!({
        "DirectoryId": directory_id,
        "RemoteDomainName": "remote.example.net"
    });
    if operation != "DeleteConditionalForwarder" {
        body["DnsIpAddrs"] = json!(["203.0.113.9"]);
    }
    body
}

/// RULE: a conditional-forwarder mutation against a directory outside the stages that serve
/// its DNS configuration is refused as a PARAMETER naming those stages, and the same call is
/// served once the directory reaches one of them.
#[tokio::test]
async fn a_forwarder_mutation_before_the_directory_serves_dns_names_the_stages() {
    // The three mutations of the family, each with the verb it states in the refusal: the
    // guard is the one they share, so all three state one sentence with that verb in it.
    let rows = [
        ("CreateConditionalForwarder", "create"),
        ("UpdateConditionalForwarder", "update"),
        ("DeleteConditionalForwarder", "delete"),
    ];

    for (operation, action) in rows {
        // A fresh router per row: the window before the transition falls due is what the row
        // exercises, so each row starts its own directory's clock.
        let router = service();
        let directory_id =
            create_directory(&router, minimal_create("building.example.com"), operation).await;

        let case = format!("{operation} while the directory is still being built");
        let message = refusal(
            &router,
            operation,
            forwarder_body(operation, &directory_id),
            "InvalidParameterException",
            &case,
        )
        .await;
        assert_eq!(
            stated(&message),
            format!(
                "Could not {action} conditional forwarder as directory: {directory_id} is not \
                 in {FORWARDER_SERVING_STAGES} state."
            ),
            "case {case:?}: unexpected diagnostic"
        );

        // Once the transition the create owed has fallen due the stage no longer refuses the
        // call: the create is served, and the update and the delete reach the forwarder
        // index itself (where the domain they name is simply not there yet).
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let in_service = format!("{operation} once the directory serves its DNS");
        let (status, parsed) = call(&router, operation, forwarder_body(operation, &directory_id)).await;
        let answered = parsed["__type"].as_str().unwrap_or_default().to_string();
        if operation == "CreateConditionalForwarder" {
            assert_eq!(
                status, 200,
                "case {in_service:?}: expected the create to be served, got {status} with \
                 {parsed}"
            );
        } else {
            assert!(
                answered.contains("EntityDoesNotExistException"),
                "case {in_service:?}: expected the forwarder lookup to answer instead of the \
                 stage guard, got {parsed}"
            );
        }
    }
}
