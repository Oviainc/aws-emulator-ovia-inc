// SPDX-License-Identifier: CC0-1.0

//! How this service CLASSIFIES and WORDS the refusals a client branches on — asserted as
//! rules over tables, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is shared with
//! `directory_lifecycle`). Nothing here reaches into the state struct, so an answer that is
//! not computed from persisted state cannot pass.
//!
//! The rules under test, each over a TABLE rather than one recorded call:
//!
//! * a constraint broken by a list's ELEMENTS is stated once at the LIST member — the entries
//!   of that member the constraint refused, the member's own path with no element position in
//!   it, and the element shape's whole constraint set nested inside
//!   `Member must satisfy constraint: [ … ]`;
//! * a directory and an account that are not shared are named UNQUOTED in the not-shared
//!   refusal, under the capitalised `Account:` label;
//! * a multi-Region call is refused by the EDITION gate first (a directory that does not
//!   replicate across Regions supports none of these calls) and only then by the per-Region
//!   replica lookup, which reports a Region holding no replica — the primary one included —
//!   as a directory that does not exist there;
//! * an alias is assigned only to a directory that has reached its serving stage; before
//!   that the call is refused and the alias stays unassigned AND unreserved, so the
//!   identical call succeeds once the directory is in service;
//! * `DescribeDirectories` is a reporting call whose `DirectoryIds` is a FILTER: an id of a
//!   directory this account HAD and deleted narrows the page to nothing (HTTP 200), an id
//!   the account never had is refused, and a MUTATION of a deleted id still refuses.
//!
//! The one rule of this class that needs another service — that the two subnets of a
//! placement sit in two different Availability Zones, which only Amazon EC2 can answer —
//! is asserted beside the router, in `core/router/tests/all/ds_placement_zones.rs`.

use serde_json::{json, Value};

use crate::ad_connector::connect_body;
use crate::directory_lifecycle::{
    call, call_in_region, create_directory, describe_one, minimal_create, ok, service, string,
    PAST_ONE_SPAN, REGION,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The two subnets every create below places its servers in.
const SERVABLE_SUBNETS: [&str; 2] = ["subnet-0a1b2c3d", "subnet-1a2b3c4d"];

/// A Region this account never creates a directory in, so no directory has a replica entry
/// for it until `AddRegion` records one.
const FOREIGN_REGION: &str = "eu-west-1";

/// The FIXED sentence a multi-Region call on a directory that does not replicate across
/// Regions is refused with (`state::MULTI_REGION_ENTERPRISE_ONLY`).
const MULTI_REGION_ENTERPRISE_ONLY: &str = "Multi-region replication is only supported for \
                                            AWS Managed Microsoft AD (Enterprise edition) \
                                            directories.";

/// The `Message` of a refusal body and the `RequestId` it reports.
fn refusal(parsed: &Value, case: &str) -> (String, String) {
    let message = string(parsed, "Message", case);
    let request_id = string(parsed, "RequestId", case);
    (message, request_id)
}

/// This service's refusal envelope: the diagnostic, then the call's own request id.
fn with_request_id(diagnostic: &str, request_id: &str) -> String {
    format!("{diagnostic} : RequestId: {request_id}")
}

/// A `VpcSettings` block naming [`SERVABLE_SUBNETS`].
fn vpc_settings() -> Value {
    json!({ "VpcId": "vpc-0a1b2c3d", "SubnetIds": SERVABLE_SUBNETS })
}

/// An `UnshareDirectory` body for the pairing of `directory_id` and `account_id`.
fn unshare_body(directory_id: &str, account_id: &str) -> Value {
    json!({
        "DirectoryId": directory_id,
        "UnshareTarget": { "Id": account_id, "Type": "ACCOUNT" },
    })
}

/// RULE: the not-shared refusal states the pairing the call asked about — the directory id
/// and the target account id, both taken from the request and NEITHER quoted — under the
/// capitalised `Account:` label, with the clause closed by a full stop before the request-id
/// separator.
#[tokio::test]
async fn the_not_shared_refusal_names_the_pairing_unquoted() {
    let router = service();

    // Rows differing in the kind of directory the call names (every create that makes one)
    // and in the account id the target names, since both are what the sentence interpolates.
    let managed = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("unshared-managed.example.com"),
        "a Managed Microsoft AD",
    )
    .await;
    let simple = create_directory(
        &router,
        minimal_create("unshared-simple.example.com"),
        "a Simple AD",
    )
    .await;
    let connector = ok(
        &router,
        "ConnectDirectory",
        connect_body("unshared-connected.example.com", "Small", &SERVABLE_SUBNETS),
        "an AD Connector",
    )
    .await;
    let connector = string(&connector, "DirectoryId", "an AD Connector");

    let rows: [(&str, &str, &str); 5] = [
        ("a Managed Microsoft AD", managed.as_str(), "111122223333"),
        ("a Simple AD", simple.as_str(), "000000000000"),
        ("an AD Connector", connector.as_str(), "999999999999"),
        (
            "the same directory, another account",
            managed.as_str(),
            "123456789012",
        ),
        (
            "an account id of all nines but one",
            simple.as_str(),
            "999999999998",
        ),
    ];

    for (case, directory_id, account_id) in rows {
        let (status, parsed) = call(&router, "UnshareDirectory", unshare_body(directory_id, account_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the unshared pairing to be refused, got {status} with \
             {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("DirectoryNotSharedException"),
            "case {case:?}: expected the modelled not-shared refusal, got {parsed}"
        );
        let (message, request_id) = refusal(&parsed, case);
        assert_eq!(
            message,
            with_request_id(
                &format!("Directory {directory_id} is not shared with Account: {account_id}."),
                &request_id
            ),
            "case {case:?}: expected the not-shared sentence naming the pairing unquoted, \
             got {message:?}"
        );
        // Neither identifier is quoted, and the label is not lower-cased.
        for unexpected in [
            format!("'{directory_id}'"),
            format!("'{account_id}'"),
            "account:".to_string(),
            "Directory:".to_string(),
        ] {
            assert!(
                !message.contains(&unexpected),
                "case {case:?}: expected no {unexpected:?} in the sentence, got {message:?}"
            );
        }
    }
}

/// One row of the multi-Region table: the directory a call names, the Region it is signed
/// for, and the refusal that Region's call must get.
struct RegionCase {
    case: String,
    directory_id: String,
    region: String,
    code: &'static str,
    message: String,
}

/// RULE: a multi-Region call is refused by the EDITION gate before any Region is looked up —
/// a directory that does not replicate across Regions (a Simple AD, an AD Connector, a
/// Standard-edition Managed Microsoft AD) is refused `UnsupportedOperationException` with
/// that rule's own fixed sentence, in EVERY Region — and a directory that does replicate is
/// then refused per Region: a Region it holds no replica entry for, its own primary Region
/// included, is a directory that does not exist there.
#[tokio::test]
async fn the_multi_region_ladder_gates_on_the_edition_before_the_region() {
    let router = service();

    // One directory per kind, so the gate is decided by the directory and not by the call.
    let simple = create_directory(
        &router,
        minimal_create("region-simple.example.com"),
        "a Simple AD",
    )
    .await;
    let connector = string(
        &ok(
            &router,
            "ConnectDirectory",
            connect_body("region-connected.example.com", "Small", &SERVABLE_SUBNETS),
            "an AD Connector",
        )
        .await,
        "DirectoryId",
        "an AD Connector",
    );
    let standard = create_microsoft_ad(
        &router,
        json!({
            "Name": "region-standard.example.com",
            "Password": crate::directory_lifecycle::PASSWORD,
            "Edition": "Standard",
            "VpcSettings": vpc_settings(),
        }),
        "a Standard-edition Managed Microsoft AD",
    )
    .await;
    let enterprise = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("region-enterprise.example.com"),
        "an Enterprise Managed Microsoft AD",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // The directory-lookup sentence this service states for an id it cannot act on.
    let not_there = |directory_id: &str| format!("Directory: '{directory_id}' does not exist.");

    let mut rows: Vec<RegionCase> = Vec::new();
    // The edition gate: every kind that does not replicate, in its own Region and in a
    // foreign one, so the gate is shown to come FIRST.
    for (kind, directory_id) in [
        ("a Simple AD", simple.as_str()),
        ("an AD Connector", connector.as_str()),
        ("a Standard-edition Managed Microsoft AD", standard.as_str()),
    ] {
        for region in [REGION, FOREIGN_REGION] {
            rows.push(RegionCase {
                case: format!("{kind} in {region}"),
                directory_id: directory_id.to_string(),
                region: region.to_string(),
                code: "UnsupportedOperationException",
                message: MULTI_REGION_ENTERPRISE_ONLY.to_string(),
            });
        }
    }
    // The replica lookup, on the directory that DOES replicate: its primary Region holds no
    // replica entry (the primary Region is removed with `DeleteDirectory`), and neither does
    // a Region it was never replicated to.
    for region in [REGION, FOREIGN_REGION, "ap-northeast-1"] {
        rows.push(RegionCase {
            case: format!("an Enterprise Managed Microsoft AD in {region}, replicated nowhere"),
            directory_id: enterprise.clone(),
            region: region.to_string(),
            code: "DirectoryDoesNotExistException",
            message: not_there(&enterprise),
        });
    }

    for row in &rows {
        let (status, parsed) = call_in_region(
            &router,
            "RemoveRegion",
            json!({ "DirectoryId": &row.directory_id }),
            &row.region,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected the call to be refused, got {status} with {parsed}",
            row.case
        );
        assert_eq!(
            parsed["__type"],
            json!(row.code),
            "case {:?}: expected {} , got {parsed}",
            row.case,
            row.code
        );
        let (message, request_id) = refusal(&parsed, &row.case);
        assert_eq!(
            message,
            with_request_id(&row.message, &request_id),
            "case {:?}: expected this branch's own sentence, got {message:?}",
            row.case
        );
    }

    // The same edition gate, with the same sentence, on the other multi-Region call: the
    // rule is the directory's, not one operation's.
    let (status, parsed) = call(
        &router,
        "AddRegion",
        json!({
            "DirectoryId": &standard,
            "RegionName": FOREIGN_REGION,
            "VPCSettings": vpc_settings(),
        }),
    )
    .await;
    let case = "AddRegion on a Standard-edition Managed Microsoft AD";
    assert_eq!(
        (status, &parsed["__type"]),
        (400, &json!("UnsupportedOperationException")),
        "case {case:?}: expected the edition gate, got {status} with {parsed}"
    );
    let (message, request_id) = refusal(&parsed, case);
    assert_eq!(
        message,
        with_request_id(MULTI_REGION_ENTERPRISE_ONLY, &request_id),
        "case {case:?}: expected the shared edition sentence, got {message:?}"
    );

    // And the branch the ladder serves: a replica entry the add recorded is removable in
    // exactly that Region, and once it is gone that Region holds no entry again.
    ok(
        &router,
        "AddRegion",
        json!({
            "DirectoryId": &enterprise,
            "RegionName": FOREIGN_REGION,
            "VPCSettings": vpc_settings(),
        }),
        "the replication the add records",
    )
    .await;
    let (status, parsed) = call_in_region(
        &router,
        "RemoveRegion",
        json!({ "DirectoryId": &enterprise }),
        FOREIGN_REGION,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"removing the recorded replication\": expected 200, got {status} with {parsed}"
    );
    let (status, parsed) = call_in_region(
        &router,
        "RemoveRegion",
        json!({ "DirectoryId": &enterprise }),
        FOREIGN_REGION,
    )
    .await;
    assert_eq!(
        (status, &parsed["__type"]),
        (400, &json!("DirectoryDoesNotExistException")),
        "case \"a second removal in that Region\": expected the entry to be gone, got \
         {status} with {parsed}"
    );
}

/// RULE: an alias is assigned to a directory that has reached its serving stage. Before
/// that the call is refused with the modelled `InvalidParameterException` stating the stage
/// the call needs, the directory keeps no alias, and the alias is not reserved either — so
/// the IDENTICAL call succeeds once the same directory is in service, whichever create made
/// it.
#[tokio::test]
async fn an_alias_is_assigned_only_once_the_directory_is_serving() {
    let router = service();

    // One row per create that makes a directory an alias can be assigned to, each with an
    // alias differing along the modelled `AliasName` forms (hyphenated, digit-bearing,
    // single character).
    let rows: [(&str, &str, Value, &str); 3] = [
        (
            "a Simple AD",
            "CreateDirectory",
            minimal_create("stage-simple.example.com"),
            "stage-simple",
        ),
        (
            "a Managed Microsoft AD",
            "CreateMicrosoftAD",
            minimal_microsoft_ad("stage-managed.example.com"),
            "stage2managed",
        ),
        (
            "an AD Connector",
            "ConnectDirectory",
            connect_body("stage-connected.example.com", "Small", &SERVABLE_SUBNETS),
            "s",
        ),
    ];

    for (case, operation, body, alias) in rows {
        let created = ok(&router, operation, body, case).await;
        let directory_id = string(&created, "DirectoryId", case);

        // Straight after the create, while the transition it owes is still outstanding.
        let (status, parsed) = call(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": &directory_id, "Alias": alias }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the alias to be refused before the directory serves, \
             got {status} with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("InvalidParameterException"),
            "case {case:?}: expected the modelled parameter refusal, got {parsed}"
        );
        let (message, request_id) = refusal(&parsed, case);
        // The MESSAGE is not pinned to a literal here: the exact sentence AWS words for
        // this error class is not stated by the model and is not in this issue's report, so
        // what is asserted is what the refusal is built FROM — the directory the request
        // named, the stage the call needs, and this service's own request-id envelope —
        // rather than a sentence this implementation chose.
        assert!(
            message.contains(&directory_id)
                && message.contains("Active")
                && message.ends_with(&format!(" : RequestId: {request_id}")),
            "case {case:?}: expected the refusal to name the directory from the request and \
             the stage the call needs, got {message:?}"
        );
        // Whatever the sentence is, it is not the one this service states for an id that
        // resolves to NOTHING: this directory exists, and the read below finds it.
        assert!(
            !message.contains("does not exist"),
            "case {case:?}: the directory exists, so the refusal must not report it as \
             absent, got {message:?}"
        );

        // The refused call stored nothing: the directory still reports its id as its alias.
        let described = describe_one(&router, &directory_id, case).await;
        assert_eq!(
            described["Alias"],
            json!(&directory_id),
            "case {case:?}: expected the refused call to leave the alias unassigned, got \
             {described}"
        );

        // Once the directory is in service the IDENTICAL call is served, and the alias it
        // asked for is the one the stored record then reports — so the refusal reserved
        // nothing.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let assigned = ok(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": &directory_id, "Alias": alias }),
            case,
        )
        .await;
        assert_eq!(
            assigned["Alias"],
            json!(alias),
            "case {case:?}: expected the serving directory to take the alias, got {assigned}"
        );
        let settled = describe_one(&router, &directory_id, case).await;
        assert_eq!(
            (&settled["Alias"], &settled["AccessUrl"]),
            (&json!(alias), &json!(format!("{alias}.awsapps.com"))),
            "case {case:?}: expected the stored alias to be reported, got {settled}"
        );
    }
}

/// RULE: `DirectoryIds` is a FILTER over the directories this reporting call reports, so an
/// id of a directory this account HAD and has since deleted narrows the page to nothing and
/// is answered HTTP 200 — while an id the account never had is a value the listing cannot be
/// narrowed by and is still refused, naming exactly those ids.
#[tokio::test]
async fn a_deleted_id_filters_to_an_empty_page_while_an_unknown_one_is_refused() {
    let router = service();
    let case = "the deleted filter";

    // Two directories deleted and one kept, so a filter can mix them. The deletes are
    // waited out: a directory still being torn down is one the listing still reports, and
    // these rows are about an id the account no longer holds at all.
    let mut deleted: Vec<String> = Vec::new();
    for index in 0..2 {
        let id = create_directory(
            &router,
            minimal_create(&format!("gone{index}.example.com")),
            case,
        )
        .await;
        ok(
            &router,
            "DeleteDirectory",
            json!({ "DirectoryId": &id }),
            case,
        )
        .await;
        deleted.push(id);
    }
    tokio::time::sleep(crate::harness::PAST_THE_TEARDOWN).await;
    let kept = create_directory(&router, minimal_create("kept.example.com"), case).await;

    // Well-formed ids (`^d-[0-9a-f]{10}$`) this account never had, varied along their
    // digits so no row can pass by matching one value.
    let unknown = ["d-0000000000", "d-ffffffffff", "d-abcdef0123"];

    // The served rows: every id in the filter is one this account had, so the page holds
    // exactly the directories that are still there. Rows differ in count, order, repetition
    // and whether a live id is mixed in.
    let served: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("one deleted id", vec![deleted[0].clone()], vec![]),
        (
            "both deleted ids",
            vec![deleted[0].clone(), deleted[1].clone()],
            vec![],
        ),
        (
            "a deleted id the filter repeats",
            vec![deleted[1].clone(), deleted[1].clone()],
            vec![],
        ),
        (
            "a deleted id before a live one",
            vec![deleted[0].clone(), kept.clone()],
            vec![kept.clone()],
        ),
        (
            "a live id between two deleted ones",
            vec![deleted[0].clone(), kept.clone(), deleted[1].clone()],
            vec![kept.clone()],
        ),
        ("the live id alone", vec![kept.clone()], vec![kept.clone()]),
    ];
    for (row, filter, expected) in &served {
        let answer = ok(
            &router,
            "DescribeDirectories",
            json!({ "DirectoryIds": filter }),
            row,
        )
        .await;
        let reported: Vec<String> = answer["DirectoryDescriptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {row:?}: expected DirectoryDescriptions, got {answer}"))
            .iter()
            .map(|description| string(description, "DirectoryId", row))
            .collect();
        assert_eq!(
            &reported, expected,
            "case {row:?}: expected only the directories that are still there, got {reported:?}"
        );
        assert!(
            answer["NextToken"].is_null(),
            "case {row:?}: expected no page token on a page this short, got {answer}"
        );
    }

    // The refused rows: an id the account never had is reported, in filter order, and a
    // deleted id beside it is NOT — the two are different failures.
    let refused_rows: Vec<(&str, Vec<String>, Vec<&str>)> = vec![
        ("one unknown id", vec![unknown[0].to_string()], vec![unknown[0]]),
        (
            "an unknown id after a deleted one",
            vec![deleted[0].clone(), unknown[1].to_string()],
            vec![unknown[1]],
        ),
        (
            "two unknown ids around a deleted one",
            vec![
                unknown[2].to_string(),
                deleted[1].clone(),
                unknown[0].to_string(),
            ],
            vec![unknown[2], unknown[0]],
        ),
        (
            "an unknown id beside the live one",
            vec![kept.clone(), unknown[1].to_string()],
            vec![unknown[1]],
        ),
    ];
    for (row, filter, named) in &refused_rows {
        let (status, parsed) = call(
            &router,
            "DescribeDirectories",
            json!({ "DirectoryIds": filter }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected an id this account never had to be refused, got {status} \
             with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("EntityDoesNotExistException"),
            "case {row:?}: expected the modelled filter-value refusal, got {parsed}"
        );
        let (message, request_id) = refusal(&parsed, row);
        assert_eq!(
            message,
            with_request_id(
                &format!("Invalid directory ID(s): {}.", named.join(",")),
                &request_id
            ),
            "case {row:?}: expected only the ids this account never had to be named, got \
             {message:?}"
        );
    }

    // A MUTATION of a deleted id still refuses: there is no directory to act on, and only
    // the reporting call reads the deletion off an empty page.
    for (operation, body) in [
        ("DeleteDirectory", json!({ "DirectoryId": &deleted[0] })),
        (
            "CreateAlias",
            json!({ "DirectoryId": &deleted[0], "Alias": "revived" }),
        ),
        (
            "AddTagsToResource",
            json!({
                "ResourceId": &deleted[0],
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
        ),
    ] {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected a mutation of a deleted directory to be refused, \
             got {status} with {parsed}"
        );
    }
}

/// One row of the list-element table: a request breaking a constraint of one list member's
/// ELEMENTS, and the clause that one validation error must be stated as.
struct ElementCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The member path the clause names: the LIST member, lowerCamel, with no element
    /// position in it.
    path: &'static str,
    /// The entries of that member the constraint refused, as the clause quotes them.
    quoted: &'static str,
    /// The element shape's whole declared constraint set, as the clause nests it.
    requirements: &'static str,
}

/// The whole message a request breaking ONE list member's element constraint is refused
/// with: the framework's envelope, the entries that member has to have fixed, and the element
/// shape's set nested at the list member.
fn one_element_error(quoted: &str, path: &str, requirements: &str) -> String {
    format!(
        "1 validation error detected: Value '[{quoted}]' at '{path}' failed to satisfy \
         constraint: Member must satisfy constraint: [{requirements}]"
    )
}

/// The `@pattern` each element shape under test declares, read off the model
/// (`smithy_query(query="shape", shape="DnsIpAddrs,DirectoryIds,DomainControllerIds,CidrIps,TagKeys")`).
const IP_ADDR_PATTERN: &str =
    r"^(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$";
const DIRECTORY_ID_PATTERN: &str = r"^d-[0-9a-f]{10}$";
const DOMAIN_CONTROLLER_ID_PATTERN: &str = r"^dc-[0-9a-f]{10}$";
const CIDR_IP_PATTERN: &str = r"^(([0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])\.){3}([0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(\/([1-9]|[1-2][0-9]|3[0-2]))$";
const TAG_KEY_PATTERN: &str = r"^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$";
const SUBNET_ID_PATTERN: &str = r"^(subnet-[0-9a-f]{8}|subnet-[0-9a-f]{17})$";

/// A well-formed `DirectoryId` no create here ever mints: the refusals below are decided
/// before any handler resolves it, so no directory has to exist for them.
const SOME_DIRECTORY_ID: &str = "d-1dca5812e8";

/// RULE: a constraint broken by a list's ELEMENTS is reported at the LIST member — the entries
/// of that member the constraint refused, its own path with no `<position>.member` step, and
/// the element shape's whole declared constraint set nested inside
/// `Member must satisfy constraint: [ … ]` — so one list is ONE validation error however many
/// of its entries offend, and a client reading the path is pointed at the member it submitted.
#[tokio::test]
async fn a_constraint_a_lists_elements_break_is_stated_at_the_list_member() {
    let router = service();

    // Rows spanning what the rule generalises over: the operation and member the list
    // belongs to, the element shape's constraint set (a pattern alone, and a length pair
    // beside a pattern), how many entries the list carries, WHERE the offending entry sits,
    // and how many of them offend.
    let rows: Vec<ElementCase> = vec![
        ElementCase {
            case: "one bad DNS address",
            operation: "CreateConditionalForwarder",
            body: json!({
                "DirectoryId": SOME_DIRECTORY_ID,
                "RemoteDomainName": "remote.example.com",
                "DnsIpAddrs": ["999.999.999.999"],
            }),
            path: "dnsIpAddrs",
            quoted: "999.999.999.999",
            requirements: IP_ADDR_PATTERN,
        },
        ElementCase {
            case: "a bad DNS address after a good one",
            operation: "CreateConditionalForwarder",
            body: json!({
                "DirectoryId": SOME_DIRECTORY_ID,
                "RemoteDomainName": "remote.example.com",
                "DnsIpAddrs": ["10.0.0.1", "not-an-address"],
            }),
            path: "dnsIpAddrs",
            quoted: "not-an-address",
            requirements: IP_ADDR_PATTERN,
        },
        ElementCase {
            case: "two bad DNS addresses around a good one",
            operation: "UpdateConditionalForwarder",
            body: json!({
                "DirectoryId": SOME_DIRECTORY_ID,
                "RemoteDomainName": "remote.example.com",
                "DnsIpAddrs": ["256.0.0.1", "10.0.0.2", "10.0.0"],
            }),
            path: "dnsIpAddrs",
            quoted: "256.0.0.1, 10.0.0",
            requirements: IP_ADDR_PATTERN,
        },
        ElementCase {
            case: "a directory id of the wrong form in a filter",
            operation: "DescribeDirectories",
            body: json!({ "DirectoryIds": ["d-0000000000", "not-a-directory"] }),
            path: "directoryIds",
            quoted: "not-a-directory",
            requirements: DIRECTORY_ID_PATTERN,
        },
        ElementCase {
            case: "a domain controller id of the wrong form",
            operation: "DescribeDomainControllers",
            body: json!({
                "DirectoryId": SOME_DIRECTORY_ID,
                "DomainControllerIds": ["d-0000000000"],
            }),
            path: "domainControllerIds",
            quoted: "d-0000000000",
            requirements: DOMAIN_CONTROLLER_ID_PATTERN,
        },
        ElementCase {
            case: "an address block with no prefix length",
            operation: "RemoveIpRoutes",
            body: json!({
                "DirectoryId": SOME_DIRECTORY_ID,
                "CidrIps": ["198.51.100.0"],
            }),
            path: "cidrIps",
            quoted: "198.51.100.0",
            requirements: CIDR_IP_PATTERN,
        },
        ElementCase {
            case: "a tag key outside its character class",
            operation: "RemoveTagsFromResource",
            body: json!({
                "ResourceId": SOME_DIRECTORY_ID,
                "TagKeys": ["env", "bad!key"],
            }),
            path: "tagKeys",
            quoted: "bad!key",
            // The element shape declares a `@length` as well as a `@pattern`, and the
            // clause states the WHOLE set rather than only the constraint that failed.
            requirements: "Member must have length less than or equal to 128, Member must \
                           have length greater than or equal to 1, Member must satisfy \
                           regular expression pattern: ",
        },
        // A list INSIDE a structure the request nests is stated at its own dotted member
        // path, the same way, with no element position in it either.
        ElementCase {
            case: "a subnet id of the wrong form inside a create's VPC settings",
            operation: "CreateMicrosoftAD",
            body: json!({
                "Name": "nested.example.com",
                "Password": crate::directory_lifecycle::PASSWORD,
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-not-hex"],
                },
            }),
            path: "vpcSettings.subnetIds",
            quoted: "subnet-not-hex",
            requirements: SUBNET_ID_PATTERN,
        },
        ElementCase {
            case: "a DNS address of the wrong form inside an AD Connector's settings",
            operation: "ConnectDirectory",
            body: json!({
                "Name": "nested-connected.example.com",
                "Password": crate::directory_lifecycle::PASSWORD,
                "Size": "Small",
                "ConnectSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
                    "CustomerDnsIps": ["10.0.0.10", "999.1.1.1"],
                    "CustomerUserName": "Admin",
                },
            }),
            path: "connectSettings.customerDnsIps",
            quoted: "999.1.1.1",
            requirements: IP_ADDR_PATTERN,
        },
    ];

    for row in &rows {
        let requirements = if row.path == "tagKeys" {
            format!("{}{TAG_KEY_PATTERN}", row.requirements)
        } else {
            format!("Member must satisfy regular expression pattern: {}", row.requirements)
        };
        let (status, parsed) = call(&router, row.operation, row.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected the element constraint to refuse the request, got {status} \
             with {parsed}",
            row.case
        );
        assert_eq!(
            parsed["__type"],
            json!("ValidationException"),
            "case {:?}: expected the framework's constraint refusal, got {parsed}",
            row.case
        );
        let message = parsed["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            one_element_error(row.quoted, row.path, &requirements),
            "case {:?}: expected the clause stated at the list member, got {message:?}",
            row.case
        );
        // The path AWS never emits: no element position, and no `member` step.
        for unexpected in [".1.member", ".2.member", ".member"] {
            assert!(
                !message.contains(unexpected),
                "case {:?}: expected no element step in the path, found {unexpected:?} in \
                 {message:?}",
                row.case
            );
        }
    }

    // A list's clause and a PLAIN member's clause are one validation error each, so a
    // request breaking both is counted as two.
    let (status, parsed) = call(
        &router,
        "CreateConditionalForwarder",
        json!({
            "DirectoryId": SOME_DIRECTORY_ID,
            "RemoteDomainName": "not a domain!",
            "DnsIpAddrs": ["999.999.999.999"],
        }),
    )
    .await;
    let case = "a bad list entry beside a bad plain member";
    assert_eq!(
        status, 400,
        "case {case:?}: expected both constraints to refuse the request, got {status} with \
         {parsed}"
    );
    let message = parsed["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected two errors, one per member, got {message:?}"
    );
    assert!(
        message.contains(&format!(
            "Value '[999.999.999.999]' at 'dnsIpAddrs' failed to satisfy constraint: Member \
             must satisfy constraint: [Member must satisfy regular expression pattern: \
             {IP_ADDR_PATTERN}]"
        )),
        "case {case:?}: expected the list clause among them, got {message:?}"
    );
    assert!(
        message.contains("at 'remoteDomainName' failed to satisfy constraint:"),
        "case {case:?}: expected the plain member's own clause among them, got {message:?}"
    );

    // The rule is about a constraint the elements BREAK: a list whose entries satisfy their
    // element shape is not refused by it.
    let served = create_directory(
        &router,
        minimal_create("forwarded.example.com"),
        "a well-formed list",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let (status, parsed) = call(
        &router,
        "CreateConditionalForwarder",
        json!({
            "DirectoryId": &served,
            "RemoteDomainName": "remote.example.com",
            "DnsIpAddrs": ["10.0.0.1", "10.0.0.2"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a well-formed list\": expected the create to be served, got {status} with \
         {parsed}"
    );
}
