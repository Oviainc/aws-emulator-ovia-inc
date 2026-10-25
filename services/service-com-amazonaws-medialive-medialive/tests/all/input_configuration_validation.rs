// SPDX-License-Identifier: CC0-1.0

//! What an input CONFIGURATION may be: the references it names and the ingest
//! list its `Type` ingests through, over `CreateInput` and `UpdateInput`.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the issue's `RTP_PUSH`-with-an-unminted-group case is
//! ONE row. The rows differ along what the rule generalises over — which
//! reference list the configuration names, whether the id it names is one the
//! account holds, which `InputType` family the input belongs to, and whether it
//! is reached in the AWS cloud, inside a VPC or on premises. The wire harness
//! (`medialive`, `call`, `error_code`) and the resource setup
//! (`input_security_group`, `sdi_source`, `claimed_device`) are the crate's
//! shared ones, reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{claimed_device, input_security_group, sdi_source};
use crate::wire::{call, error_code, error_message, medialive};

/// `CreateInput` (`smithy_query(query="operation", operation="CreateInput")`:
/// POST /prod/inputs).
async fn create_input(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    if status == 201 {
        (status, response["input"].clone())
    } else {
        (status, response)
    }
}

/// `UpdateInput` (PUT /prod/inputs/{InputId}).
async fn update_input(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    let (status, response) = call(router, "PUT", &format!("/prod/inputs/{id}"), Some(body)).await;
    if status == 200 {
        (status, response["input"].clone())
    } else {
        (status, response)
    }
}

/// `DescribeInput` (GET /prod/inputs/{InputId}).
async fn describe_input(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(router, "GET", &format!("/prod/inputs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInput({id}): expected 200, got {status}: {described}"
    );
    described
}

/// One row of a pull-source list, as `InputSourceRequest` shapes it.
fn source(url: &str) -> Value {
    json!({ "url": url, "username": "u", "passwordParam": "p" })
}

/// The redundant pair of source URLs a PULL-type input pulls from.
fn pull_sources() -> Value {
    json!([
        source("https://example.com/a.m3u8"),
        source("https://example.com/b.m3u8"),
    ])
}

/// An input the account holds, of a type that needs no reference at all, so a
/// row can tell "nothing was stored" from "the store is empty".
async fn bystander(router: &MultiServiceRouter) -> String {
    let (status, created) = create_input(
        router,
        json!({ "name": "bystander", "type": "URL_PULL", "sources": pull_sources() }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the bystander input to be created, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// The `Inputs` a referenced resource reports — `InputSecurityGroup$Inputs` ("The
/// list of inputs currently using this Input Security Group") and
/// `SdiSource$Inputs` ("The list of inputs that are currently using this SDI
/// source"), both DERIVED from the input store, so they are what shows that a
/// reference resolved to a real resource rather than being stored verbatim.
async fn users_of(router: &MultiServiceRouter, case: &str, uri: &str, wrapper: Option<&str>) -> Value {
    let (status, read) = call(router, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GET {uri}: expected 200, got {status}: {read}"
    );
    match wrapper {
        Some(member) => read[member]["inputs"].clone(),
        None => read["inputs"].clone(),
    }
}

/// One row of the reference table: which list the configuration names, on which
/// `InputType`, with an entry the account holds or one nothing minted.
struct ReferenceRow {
    case: &'static str,
    /// The member as the wire spells it.
    member: &'static str,
    /// The `InputType` whose ingest that list is.
    input_type: &'static str,
    /// Whether the entry the row puts in that list names a resource this account
    /// holds.
    held: bool,
    expected_status: u16,
}

/// The entries `member` carries for one row, and — when the row names resources
/// the account holds — the read that reports what is using the first of them.
async fn reference_entries(
    router: &MultiServiceRouter,
    row: &ReferenceRow,
) -> (Value, Option<(String, Option<&'static str>)>) {
    match (row.member, row.held) {
        ("inputSecurityGroups", true) => {
            let id = input_security_group(router).await;
            (
                json!([id]),
                Some((format!("/prod/inputSecurityGroups/{id}"), None)),
            )
        }
        ("inputSecurityGroups", false) => (json!(["9999999"]), None),
        ("sdiSources", true) => {
            let id = sdi_source(router, "wired").await;
            (
                json!([id]),
                Some((format!("/prod/sdiSources/{id}"), Some("sdiSource"))),
            )
        }
        ("sdiSources", false) => (json!(["not-a-source"]), None),
        ("inputDevices", true) => {
            let id = claimed_device(router, "hd-0000000000000001").await;
            (json!([{ "id": id }]), None)
        }
        ("inputDevices", false) => (json!([{ "id": "hd-0000000000000000" }]), None),
        (member, _) => panic!("case {:?}: no setup for {member}", row.case),
    }
}

/// Rule: an input may only name resources the account holds — every entry of
/// `InputSecurityGroups`, `SdiSources` and `InputDevices` has to resolve to a
/// live input security group, SdiSource or claimed input device — so a create
/// naming one that does not is refused with this operation's modelled
/// `BadRequestException` (400) and stores nothing, while one that resolves is
/// created and is reported BY the resource it names.
#[tokio::test]
async fn an_input_may_only_name_resources_the_account_holds() {
    let table: [ReferenceRow; 6] = [
        ReferenceRow {
            case: "an input security group the account holds",
            member: "inputSecurityGroups",
            input_type: "RTP_PUSH",
            held: true,
            expected_status: 201,
        },
        ReferenceRow {
            case: "an input security group id nothing minted",
            member: "inputSecurityGroups",
            input_type: "RTP_PUSH",
            held: false,
            expected_status: 400,
        },
        ReferenceRow {
            case: "an SdiSource the account holds",
            member: "sdiSources",
            input_type: "SDI",
            held: true,
            expected_status: 201,
        },
        ReferenceRow {
            case: "an SdiSource id nothing minted",
            member: "sdiSources",
            input_type: "SDI",
            held: false,
            expected_status: 400,
        },
        ReferenceRow {
            case: "a device the account has claimed",
            member: "inputDevices",
            input_type: "INPUT_DEVICE",
            held: true,
            expected_status: 201,
        },
        ReferenceRow {
            case: "a device the account has not claimed",
            member: "inputDevices",
            input_type: "INPUT_DEVICE",
            held: false,
            expected_status: 400,
        },
    ];

    for row in table {
        let case = row.case;
        let router = medialive();
        let (entries, users) = reference_entries(&router, &row).await;

        let mut body = json!({ "name": case, "type": row.input_type });
        body[row.member] = entries.clone();
        let (status, response) = create_input(&router, body).await;
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected {}, got {status}: {response}",
            row.expected_status
        );

        if row.expected_status == 400 {
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected the operation's own BadRequestException, got {response}"
            );
            let message = error_message(&response);
            assert!(
                message.contains(row.member),
                "case {case:?}: expected the message to name the member {:?}, got {message:?}",
                row.member
            );
            // A refused create stored nothing: the next input gets the first
            // minted id.
            assert_eq!(
                bystander(&router).await,
                "1000000",
                "case {case:?}: expected the refused create to have minted no input"
            );
            continue;
        }

        // Round trip: the configuration is read back out of state.
        let id = response["id"].as_str().unwrap_or_default().to_string();
        let described = describe_input(&router, case, &id).await;
        let reported = if row.member == "inputSecurityGroups" {
            "securityGroups"
        } else {
            row.member
        };
        assert_eq!(
            described[reported], entries,
            "case {case:?}: expected the stored {reported}, got {:?}",
            described[reported]
        );

        // The resource the input names reports the input back, which is what an
        // id nothing minted could never do.
        if let Some((uri, wrapper)) = users {
            assert_eq!(
                users_of(&router, case, &uri, wrapper).await,
                json!([id]),
                "case {case:?}: expected the referenced resource to report the input using it"
            );
        }
    }
}

/// Rule: the same resolution applies to an `UpdateInput` that rewrites one of
/// those lists — a merged configuration naming a resource the account does not
/// hold is refused with the modelled `BadRequestException` (400) and the stored
/// input is left exactly as it was, while one that resolves is stored and reads
/// back.
#[tokio::test]
async fn an_update_may_only_name_resources_the_account_holds() {
    // (case, the member the update rewrites, the entries it names, expected status)
    let table: [(&str, &str, bool, u16); 6] = [
        ("a group the account holds", "inputSecurityGroups", true, 200),
        ("a group id nothing minted", "inputSecurityGroups", false, 400),
        ("an SdiSource the account holds", "sdiSources", true, 200),
        ("an SdiSource id nothing minted", "sdiSources", false, 400),
        ("a device the account has claimed", "inputDevices", true, 200),
        ("a device the account has not claimed", "inputDevices", false, 400),
    ];

    for (case, member, held, expected_status) in table {
        let router = medialive();
        let id = bystander(&router).await;
        let before = describe_input(&router, case, &id).await;

        let row = ReferenceRow {
            case,
            member,
            input_type: "URL_PULL",
            held,
            expected_status,
        };
        let (entries, _) = reference_entries(&router, &row).await;
        let mut patch = json!({});
        patch[member] = entries.clone();

        let (status, response) = update_input(&router, &id, patch).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {response}"
        );
        let after = describe_input(&router, case, &id).await;

        if expected_status == 400 {
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected the operation's own BadRequestException, got {response}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused update to have written nothing, got {after}"
            );
        } else {
            let reported = if member == "inputSecurityGroups" {
                "securityGroups"
            } else {
                member
            };
            assert_eq!(
                after[reported], entries,
                "case {case:?}: expected the update to store the {reported} it named, got {:?}",
                after[reported]
            );
        }
    }
}

/// Rule: a resource whose delete has been accepted is no longer one an input may
/// name — a deleted input security group admits nothing and a deleted SdiSource
/// feeds nothing — so a create naming one is refused with the modelled
/// `BadRequestException` (400), exactly as an id nothing ever minted is.
#[tokio::test]
async fn a_deleted_resource_is_not_one_an_input_may_name() {
    // (case, the collection the resource lives in, the create body the id goes
    //  into, the member it goes into, the input type)
    let table: [(&str, &str, &str, &str); 2] = [
        (
            "a deleted input security group",
            "/prod/inputSecurityGroups",
            "inputSecurityGroups",
            "RTP_PUSH",
        ),
        (
            "a deleted SdiSource",
            "/prod/sdiSources",
            "sdiSources",
            "SDI",
        ),
    ];

    for (case, collection, member, input_type) in table {
        let router = medialive();
        let id = if member == "inputSecurityGroups" {
            input_security_group(&router).await
        } else {
            sdi_source(&router, "wired").await
        };

        // The same id is accepted while the resource is live.
        let mut live_body = json!({ "name": "while-live", "type": input_type });
        live_body[member] = json!([id.clone()]);
        let (status, live) = create_input(&router, live_body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the live resource to be nameable, got {status}: {live}"
        );
        let live_input = live["id"].as_str().unwrap_or_default().to_string();

        // Delete the input holding it, so the resource is no longer in use, then
        // delete the resource itself.
        let (status, deleting) = call(
            &router,
            "DELETE",
            &format!("/prod/inputs/{live_input}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the input's delete to be accepted, got {status}: {deleting}"
        );
        let (status, deleted) = call(&router, "DELETE", &format!("{collection}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the resource's delete to be accepted, got {status}: {deleted}"
        );

        let mut body = json!({ "name": case, "type": input_type });
        body[member] = json!([id.clone()]);
        let (status, response) = create_input(&router, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted resource to be refused, got {status}: {response}"
        );
        assert_eq!(
            error_code(status, &response),
            "BadRequestException",
            "case {case:?}: expected the operation's own BadRequestException, got {response}"
        );
        assert!(
            error_message(&response).contains(&id),
            "case {case:?}: expected the message to name the id {id:?}, got {:?}",
            error_message(&response)
        );
    }
}

/// Rule: an input an upstream system PUSHES to is reached through the whitelist
/// that admits that system, so one of those types needs at least one
/// `InputSecurityGroups` entry — unless its ingest addresses are not this
/// service's public ones, which is the case for a VPC input ("the input
/// destination addresses will be created in a VPC rather than with public
/// Internet addresses") and for an on-premises one ("On-Prem for an input in a
/// customer network") — while a type that is not pushed to needs none.
///
/// The table spans every pushed-to `InputType` (source: the `InputType` enum,
/// `smithy_query(query="shape", shape="InputType")`, and the push protocols this
/// service allocates an ingest endpoint for), each with and without the
/// whitelist, beside the VPC and on-premises rows and types that are pulled from
/// or fed by a flow instead.
#[tokio::test]
async fn a_pushed_to_input_needs_the_whitelist_that_admits_it() {
    // (case, the create body without any security group, whether a group is
    //  attached, expected status)
    let table: [(&str, Value, bool, u16); 9] = [
        (
            "rtmp push with no security group",
            json!({ "name": "rtmp", "type": "RTMP_PUSH", "destinations": [{ "streamName": "a" }] }),
            false,
            400,
        ),
        (
            "rtmp push with a security group",
            json!({ "name": "rtmp", "type": "RTMP_PUSH", "destinations": [{ "streamName": "a" }] }),
            true,
            201,
        ),
        (
            "rtp push with no security group",
            json!({ "name": "rtp", "type": "RTP_PUSH", "destinations": [{}] }),
            false,
            400,
        ),
        (
            "rtp push with a security group",
            json!({ "name": "rtp", "type": "RTP_PUSH", "destinations": [{}] }),
            true,
            201,
        ),
        (
            "udp push with no security group",
            json!({ "name": "udp", "type": "UDP_PUSH", "destinations": [{}] }),
            false,
            400,
        ),
        (
            // Not reached over the public Internet: the VPC's own addresses are
            // the ingest ones, and `Vpc` is "not compatible with the
            // inputSecurityGroups property".
            "a vpc push input with no security group",
            json!({
                "name": "vpc",
                "type": "RTP_PUSH",
                "destinations": [{}],
                "vpc": { "subnetIds": ["subnet-a", "subnet-b"] },
                "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
            }),
            false,
            201,
        ),
        (
            "an on-premises push input with no security group",
            json!({
                "name": "onprem",
                "type": "RTMP_PUSH",
                "inputNetworkLocation": "ON_PREMISES",
                "destinations": [{ "streamName": "a", "staticIpAddress": "10.0.0.5" }],
            }),
            false,
            201,
        ),
        (
            "a pulled-from type with no security group",
            json!({ "name": "pull", "type": "URL_PULL", "sources": pull_sources() }),
            false,
            201,
        ),
        (
            "a flow-fed type with no security group",
            json!({
                "name": "flow",
                "type": "MEDIACONNECT",
                "mediaConnectFlows": [
                    { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                ],
                "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
            }),
            false,
            201,
        ),
    ];

    for (case, body, attach_group, expected_status) in table {
        let router = medialive();
        let mut body = body;
        if attach_group {
            body["inputSecurityGroups"] = json!([input_security_group(&router).await]);
        }
        let (status, response) = create_input(&router, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected the operation's own BadRequestException, got {response}"
            );
            assert!(
                error_message(&response).contains("inputSecurityGroups"),
                "case {case:?}: expected the message to name the missing member, got {:?}",
                error_message(&response)
            );
        } else {
            assert!(
                response["id"].is_string(),
                "case {case:?}: expected a created input, got {response}"
            );
        }
    }
}

/// Rule: an input of a type that PULLS its media needs the URLs it pulls from —
/// `Sources` is "The source URLs for a PULL-type input" — so an input of such a
/// type carrying no source is refused with the modelled `BadRequestException`
/// (400), while a type that ingests through another list needs none.
///
/// The table spans every pulled-from `InputType` (source: the `Sources` member's
/// own documentation and the `InputType` enum) with no `Sources` member at all,
/// with the empty list, with one source and with the redundant pair, beside the
/// types that are pushed to, fed by a flow or wired to a device instead.
#[tokio::test]
async fn a_pulled_from_input_needs_the_source_urls_it_pulls() {
    // (the pulled-from type, the sources it names, expected status)
    let pull_types = ["URL_PULL", "RTMP_PULL", "MP4_FILE", "TS_FILE"];
    let source_lists: [(&str, Option<Value>, u16); 4] = [
        ("no sources member", None, 400),
        ("an empty source list", Some(json!([])), 400),
        (
            "one source",
            Some(json!([source("https://example.com/only.m3u8")])),
            201,
        ),
        ("the redundant pair", Some(pull_sources()), 201),
    ];

    for input_type in pull_types {
        for (sources_case, sources, expected_status) in source_lists.clone() {
            let case = format!("{input_type}, {sources_case}");
            let router = medialive();
            let mut body = json!({ "name": "pull", "type": input_type });
            if let Some(sources) = sources.clone() {
                body["sources"] = sources;
            }
            let (status, response) = create_input(&router, body).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected {expected_status}, got {status}: {response}"
            );
            if expected_status == 400 {
                assert_eq!(
                    error_code(status, &response),
                    "BadRequestException",
                    "case {case:?}: expected the operation's own BadRequestException, got {response}"
                );
                assert!(
                    error_message(&response).contains("sources"),
                    "case {case:?}: expected the message to name the missing member, got {:?}",
                    error_message(&response)
                );
            } else {
                assert_eq!(
                    describe_input(
                        &router,
                        &case,
                        response["id"].as_str().unwrap_or_default(),
                    )
                    .await["sources"],
                    sources.unwrap_or_else(|| json!([])),
                    "case {case:?}: expected the stored sources to read back"
                );
            }
        }
    }

    // The types that ingest through another list carry no source at all.
    let not_pulled: [(&str, Value); 3] = [
        (
            "a pushed-to type",
            json!({ "name": "push", "type": "RTP_PUSH", "destinations": [{}] }),
        ),
        (
            "a flow-fed type",
            json!({
                "name": "flow",
                "type": "MEDIACONNECT",
                "mediaConnectFlows": [
                    { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                ],
            }),
        ),
        (
            "a device-wired type",
            json!({ "name": "device", "type": "INPUT_DEVICE" }),
        ),
    ];
    for (case, body) in not_pulled {
        let router = medialive();
        let mut body = body;
        if body["type"] == json!("RTP_PUSH") {
            body["inputSecurityGroups"] = json!([input_security_group(&router).await]);
        }
        let (status, response) = create_input(&router, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected a type that pulls nothing to need no source, got {status}: {response}"
        );
        assert_eq!(
            response["sources"],
            json!([]),
            "case {case:?}: expected no sources, got {:?}",
            response["sources"]
        );
    }
}

/// Rule: the `Vpc` member's own two cross-member rules — it "requires setting
/// the roleArn property on Input creation" and is "not compatible with the
/// inputSecurityGroups property" — hold for the configuration an `UpdateInput`
/// would leave behind as much as for the one a create would store, because they
/// are rules about the input, not about which operation wrote it.
#[tokio::test]
async fn an_update_may_not_break_the_vpc_cross_member_rules() {
    // (case, the patch, expected status)
    let table: [(&str, bool, Value, u16); 3] = [
        (
            "attaching a security group to a vpc input",
            true,
            json!({}),
            400,
        ),
        (
            // An empty list names no group, so it breaks nothing.
            "an empty security group list on a vpc input",
            false,
            json!({ "inputSecurityGroups": [] }),
            200,
        ),
        (
            "renaming a vpc input",
            false,
            json!({ "name": "renamed" }),
            200,
        ),
    ];

    for (case, attach_group, patch, expected_status) in table {
        let router = medialive();
        let (status, created) = create_input(
            &router,
            json!({
                "name": "vpc",
                "type": "RTP_PUSH",
                "destinations": [{}],
                "vpc": { "subnetIds": ["subnet-a", "subnet-b"] },
                "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateInput: expected 201, got {status}: {created}"
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let before = describe_input(&router, case, &id).await;

        let mut patch = patch;
        if attach_group {
            patch["inputSecurityGroups"] = json!([input_security_group(&router).await]);
        }
        let (status, response) = update_input(&router, &id, patch).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {response}"
        );
        let after = describe_input(&router, case, &id).await;
        if expected_status == 400 {
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected the operation's own BadRequestException, got {response}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused update to have written nothing, got {after}"
            );
        } else {
            assert_eq!(
                after, response,
                "case {case:?}: expected the accepted update to read back, got {after}"
            );
        }
    }
}
/// Rule: an `UpdateInput` is held to the same rule on the configuration it would
/// LEAVE BEHIND, so it may not take away the ingest list the input's `Type`
/// ingests through — the whitelist of a pushed-to input or the sources of a
/// pulled-from one — and a refused update writes nothing.
#[tokio::test]
async fn an_update_may_not_take_away_the_ingest_list_the_type_needs() {
    // (case, the create body, the patch, expected status)
    let table: [(&str, &str, Value, u16); 5] = [
        (
            "a push input emptying its whitelist",
            "RTP_PUSH",
            json!({ "inputSecurityGroups": [] }),
            400,
        ),
        (
            "a push input renaming itself",
            "RTP_PUSH",
            json!({ "name": "renamed" }),
            200,
        ),
        (
            "a pull input emptying its sources",
            "URL_PULL",
            json!({ "sources": [] }),
            400,
        ),
        (
            "a pull input replacing its sources",
            "URL_PULL",
            json!({ "sources": [source("https://example.com/c.m3u8")] }),
            200,
        ),
        (
            "a pull input renaming itself",
            "URL_PULL",
            json!({ "name": "renamed" }),
            200,
        ),
    ];

    for (case, input_type, patch, expected_status) in table {
        let router = medialive();
        let mut body = json!({ "name": "updatable", "type": input_type });
        if input_type == "RTP_PUSH" {
            body["destinations"] = json!([{}]);
            body["inputSecurityGroups"] = json!([input_security_group(&router).await]);
        } else {
            body["sources"] = pull_sources();
        }
        let (status, created) = create_input(&router, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateInput: expected 201, got {status}: {created}"
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let before = describe_input(&router, case, &id).await;

        let (status, response) = update_input(&router, &id, patch).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {response}"
        );
        let after = describe_input(&router, case, &id).await;
        if expected_status == 400 {
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected the operation's own BadRequestException, got {response}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused update to have written nothing, got {after}"
            );
        } else {
            assert_eq!(
                after, response,
                "case {case:?}: expected the accepted update to read back, got {after}"
            );
        }
    }
}
