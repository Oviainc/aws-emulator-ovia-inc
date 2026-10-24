// SPDX-License-Identifier: CC0-1.0

//! `CreatePartnerInput`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: one input family is one row, never the whole rule.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_INPUTS;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, error_code, medialive, ACCOUNT};

async fn create_input(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "CreateInput: expected 201, got {status}: {response}"
    );
    response["input"].clone()
}

/// `CreatePartnerInput`: `POST /prod/inputs/{InputId}/partners`
/// (`smithy_query(query="operation", operation="CreatePartnerInput")`).
async fn create_partner(
    router: &MultiServiceRouter,
    input_id: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/prod/inputs/{input_id}/partners"),
        body,
    )
    .await
}

async fn describe_input(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/prod/inputs/{id}"), None).await
}

/// One row of a pull-source list, as `InputSourceRequest` shapes it.
fn source(url: &str) -> Value {
    json!({ "url": url, "username": "u", "passwordParam": "p" })
}

/// The `ip` of every destination an input reports, in order.
fn destination_ips(input: &Value) -> Vec<Value> {
    input["destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|destination| destination["ip"].clone())
        .collect()
}

/// The `InputType` families the table below spans, each created the way its own
/// family ingests. Source for the type names and the per-family member each
/// row fills: the `CreateInput` input signature and the `InputType` enum
/// (`smithy_query(query="operation", operation="CreateInput")`).
fn primary_input_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "url pull with one source",
            json!({
                "name": "pull-primary",
                "type": "URL_PULL",
                "sources": [source("https://example.com/a.m3u8")],
                "inputSecurityGroups": ["3456789"],
            }),
        ),
        (
            "rtp push with one destination",
            json!({
                "name": "rtp-primary",
                "type": "RTP_PUSH",
                "destinations": [{ "streamName": "live/a" }],
            }),
        ),
        (
            "rtmp push with two destinations",
            json!({
                "name": "rtmp-primary",
                "type": "RTMP_PUSH",
                "destinations": [{ "streamName": "live/a" }, { "streamName": "live/b" }],
            }),
        ),
        (
            "mediaconnect with one flow",
            json!({
                "name": "mediaconnect-primary",
                "type": "MEDIACONNECT",
                "mediaConnectFlows": [
                    { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                ],
                "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
            }),
        ),
        (
            // The family partner inputs exist for in the first place, and one
            // the input-lifecycle suite does not cover.
            "aws cdi with one destination",
            json!({
                "name": "cdi-primary",
                "type": "AWS_CDI",
                "destinations": [{ "streamName": "cdi-a" }],
            }),
        ),
        (
            "sdi on premises",
            json!({
                "name": "sdi-primary",
                "type": "SDI",
                "sdiSources": ["sdi-a"],
                "inputNetworkLocation": "ON_PREMISES",
            }),
        ),
        (
            "one input device",
            json!({
                "name": "device-primary",
                "type": "INPUT_DEVICE",
                "inputDevices": [{ "id": "hd-0000000" }],
            }),
        ),
    ]
}

/// Rule: `CreatePartnerInput` creates a NEW input of its own (its own minted
/// numeric `Id` and its `Arn` built from the signed region and that id) that
/// ingests the configuration of the input the path named — its `Type`, network
/// location, role, security groups and per-pipeline source, destination, flow,
/// device and receiver lists — and the pair name EACH OTHER in
/// `InputPartnerIds` ("A list of IDs for all Inputs which are partners of this
/// one", `Input$InputPartnerIds`), with the partner's own ingest endpoints
/// allocated for it rather than shared with the input it partners.
#[tokio::test]
async fn a_partner_input_mirrors_the_input_it_partners_and_the_pair_names_each_other() {
    for (case, request) in primary_input_table() {
        let router = medialive();
        let primary = create_input(&router, request.clone()).await;
        let primary_id = primary["id"].as_str().unwrap_or_default().to_string();

        assert_eq!(
            primary["inputPartnerIds"],
            json!([]),
            "case {case:?}: an input created on its own has no partner, got {:?}",
            primary["inputPartnerIds"]
        );

        let (status, response) = create_partner(&router, &primary_id, None).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreatePartnerInput: expected 201, got {status}: {response}"
        );
        let partner = response["input"].clone();

        let partner_id = partner["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !partner_id.is_empty() && partner_id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a minted numeric input id, got {:?}",
            partner["id"]
        );
        assert_ne!(
            partner_id, primary_id,
            "case {case:?}: expected a partner input of its own, got the input it partners"
        );
        assert_eq!(
            partner["arn"],
            json!(format!(
                "arn:aws:medialive:us-west-2:{ACCOUNT}:input:{partner_id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            partner["arn"]
        );

        // The pair names each other.
        assert_eq!(
            partner["inputPartnerIds"],
            json!([primary_id.clone()]),
            "case {case:?}: expected the partner to name the input it partners, got {:?}",
            partner["inputPartnerIds"]
        );
        let (status, described_primary) = describe_input(&router, &primary_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInput on the partnered input: got {status}: {described_primary}"
        );
        assert_eq!(
            described_primary["inputPartnerIds"],
            json!([partner_id.clone()]),
            "case {case:?}: expected the partnered input to name its partner, got {:?}",
            described_primary["inputPartnerIds"]
        );

        // The ingest configuration is the partnered input's.
        for member in [
            "type",
            "name",
            "inputNetworkLocation",
            "roleArn",
            "securityGroups",
            "sources",
            "mediaConnectFlows",
            "inputDevices",
            "sdiSources",
            "inputClass",
            "inputSourceType",
        ] {
            assert_eq!(
                partner[member], primary[member],
                "case {case:?}: expected the partner to ingest the partnered input's {member} {:?}, got {:?}",
                primary[member], partner[member]
            );
        }
        assert_eq!(
            partner["state"],
            json!("DETACHED"),
            "case {case:?}: expected a new partner input to be DETACHED, got {:?}",
            partner["state"]
        );
        assert_eq!(
            partner["attachedChannels"],
            json!([]),
            "case {case:?}: expected a new partner input to be attached to no channel, got {:?}",
            partner["attachedChannels"]
        );

        // A destination slot per slot of the input it partners, each with an
        // ingest endpoint of the partner's OWN: `InputDestination$Ip` "remains
        // fixed for the lifetime of the input", so the partner cannot share the
        // one the partnered input holds.
        let primary_ips = destination_ips(&primary);
        let partner_ips = destination_ips(&partner);
        assert_eq!(
            partner_ips.len(),
            primary_ips.len(),
            "case {case:?}: expected one destination per partnered destination, got {partner_ips:?}"
        );
        for (slot, (primary_ip, partner_ip)) in primary_ips.iter().zip(&partner_ips).enumerate() {
            if primary_ip.is_string() {
                // A type with a push ingest protocol: both hold an address, and
                // the partner's is not the partnered input's.
                assert!(
                    partner_ip.is_string(),
                    "case {case:?}: destination slot {slot}: expected the partner's own ingest \
                     address, got {partner_ip:?}"
                );
                assert_ne!(
                    partner_ip, primary_ip,
                    "case {case:?}: destination slot {slot}: expected an address of the \
                     partner's own, got the partnered input's {primary_ip:?}"
                );
            } else {
                // A type the service allocates no push endpoint for reports
                // none for either input.
                assert_eq!(
                    partner_ip, primary_ip,
                    "case {case:?}: destination slot {slot}: expected no ingest address on a \
                     type that has no push endpoint, got {partner_ip:?}"
                );
            }
        }

        // Round trip: the partner reads back out of state through DescribeInput
        // exactly as the create reported it.
        let (status, described_partner) = describe_input(&router, &partner_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInput on the partner: got {status}: {described_partner}"
        );
        assert_eq!(
            described_partner, partner,
            "case {case:?}: expected DescribeInput to report the created partner input, got {described_partner}"
        );
    }
}

/// Rule: the partner input's `Tags` are the partner request's own ("A
/// collection of key-value pairs", `CreatePartnerInput$Tags`) — never a copy of
/// the partnered input's — and creating a partner leaves that input's tags
/// alone.
#[tokio::test]
async fn a_partner_input_carries_the_tags_its_own_request_supplied() {
    // (case, partner request body, expected partner tags)
    let table: [(&str, Option<Value>, Value); 4] = [
        ("no body at all", None, json!({})),
        ("no tags", Some(json!({})), json!({})),
        (
            "one tag",
            Some(json!({ "tags": { "stage": "prod" } })),
            json!({ "stage": "prod" }),
        ),
        (
            "two tags",
            Some(json!({ "tags": { "stage": "test", "team": "video" } })),
            json!({ "stage": "test", "team": "video" }),
        ),
    ];

    for (case, body, expected_tags) in table {
        let router = medialive();
        let primary = create_input(
            &router,
            json!({
                "name": "tagged-primary",
                "type": "URL_PULL",
                "sources": [source("https://example.com/a.m3u8")],
                "tags": { "owner": "primary" },
            }),
        )
        .await;
        let primary_id = primary["id"].as_str().unwrap_or_default().to_string();

        let (status, response) = create_partner(&router, &primary_id, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreatePartnerInput: expected 201, got {status}: {response}"
        );
        assert_eq!(
            response["input"]["tags"], expected_tags,
            "case {case:?}: expected the partner's own tags {expected_tags}, got {:?}",
            response["input"]["tags"]
        );

        let (_, described_primary) = describe_input(&router, &primary_id).await;
        assert_eq!(
            described_primary["tags"],
            json!({ "owner": "primary" }),
            "case {case:?}: expected the partnered input to keep its own tags, got {:?}",
            described_primary["tags"]
        );
    }
}

/// Rule: `CreatePartnerInput`'s `RequestId` is an `@idempotencyToken`, so two
/// calls carrying the same one yield ONE partner input, and calls carrying
/// different ones (or none) each yield their own — every partner of the same
/// input being reported in that input's `InputPartnerIds`.
#[tokio::test]
async fn a_repeated_request_id_creates_one_partner_input() {
    // (case, first request id, second request id, same partner expected)
    let table: [(&str, Option<&str>, Option<&str>, bool); 3] = [
        ("same token", Some("token-a"), Some("token-a"), true),
        ("different tokens", Some("token-a"), Some("token-b"), false),
        ("no token", None, None, false),
    ];

    for (case, first_token, second_token, same) in table {
        let router = medialive();
        let primary = create_input(
            &router,
            json!({
                "name": "idempotent-primary",
                "type": "AWS_CDI",
                "destinations": [{ "streamName": "cdi-a" }],
            }),
        )
        .await;
        let primary_id = primary["id"].as_str().unwrap_or_default().to_string();

        let mut ids: Vec<String> = Vec::new();
        for token in [first_token, second_token] {
            let body = token.map(|token| json!({ "requestId": token }));
            let (status, response) = create_partner(&router, &primary_id, body).await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreatePartnerInput: expected 201, got {status}: {response}"
            );
            ids.push(
                response["input"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }

        assert_eq!(
            ids[0] == ids[1],
            same,
            "case {case:?}: expected same_partner={same}, got ids {ids:?}"
        );

        let (_, described_primary) = describe_input(&router, &primary_id).await;
        let mut expected: Vec<String> = ids.clone();
        expected.sort();
        expected.dedup();
        assert_eq!(
            described_primary["inputPartnerIds"],
            json!(expected),
            "case {case:?}: expected the partnered input to name every partner, got {:?}",
            described_primary["inputPartnerIds"]
        );
        for id in &ids {
            let (status, described) = describe_input(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: every id a create answered must describe, got {status}: {described}"
            );
        }
    }
}

/// Rule: a replay is a replay only of a partner of the SAME input — a
/// `RequestId` an earlier create of another input (or of a partner of another
/// input) spent names an input this request did not ask for, so this call
/// creates its own partner rather than answering with that one.
#[tokio::test]
async fn a_request_id_spent_on_another_create_is_not_a_partner_replay() {
    // (case, how the token was first spent)
    let table: [(&str, &str); 2] = [
        ("the token created a plain input", "create-input"),
        ("the token created a partner of another input", "other-partner"),
    ];

    for (case, first_use) in table {
        let router = medialive();
        let body = json!({
            "name": "token-primary",
            "type": "AWS_CDI",
            "destinations": [{ "streamName": "cdi-a" }],
        });
        let primary = create_input(&router, body.clone()).await;
        let primary_id = primary["id"].as_str().unwrap_or_default().to_string();
        let other = create_input(&router, body.clone()).await;
        let other_id = other["id"].as_str().unwrap_or_default().to_string();

        // Spend the token on a create that is NOT a partner of `primary_id`.
        let token = "shared-token";
        let spent_on = if first_use == "create-input" {
            let mut with_token = body.clone();
            with_token["requestId"] = json!(token);
            create_input(&router, with_token).await["id"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        } else {
            let (status, response) =
                create_partner(&router, &other_id, Some(json!({ "requestId": token }))).await;
            assert_eq!(
                status, 201,
                "case {case:?}: the first partner create: got {status}: {response}"
            );
            response["input"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };

        let (status, response) =
            create_partner(&router, &primary_id, Some(json!({ "requestId": token }))).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreatePartnerInput: expected 201, got {status}: {response}"
        );
        let partner_id = response["input"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            partner_id, spent_on,
            "case {case:?}: expected a partner of the requested input, got the input the token \
             had already created"
        );
        assert_eq!(
            response["input"]["inputPartnerIds"],
            json!([primary_id.clone()]),
            "case {case:?}: expected the new partner to name the requested input, got {:?}",
            response["input"]["inputPartnerIds"]
        );
        let (_, described_primary) = describe_input(&router, &primary_id).await;
        assert_eq!(
            described_primary["inputPartnerIds"],
            json!([partner_id]),
            "case {case:?}: expected the requested input to have gained its partner, got {:?}",
            described_primary["inputPartnerIds"]
        );
    }
}

/// Rule: the partner's `Arn` is built from the region the request was signed
/// for, like every other ARN this service mints, so the same call in another
/// region answers an ARN in that region.
#[tokio::test]
async fn the_partner_arn_names_the_signed_region() {
    for region in ["us-west-2", "eu-central-1", "ap-southeast-2"] {
        let router = medialive();
        let (status, created) = call_in_region(
            &router,
            region,
            "POST",
            "/prod/inputs",
            Some(json!({ "name": "region-primary", "type": "AWS_CDI",
                         "destinations": [{ "streamName": "cdi-a" }] })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {region:?}: CreateInput: got {status}: {created}"
        );
        let primary_id = created["input"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, response) = call_in_region(
            &router,
            region,
            "POST",
            &format!("/prod/inputs/{primary_id}/partners"),
            None,
        )
        .await;
        assert_eq!(
            status, 201,
            "case {region:?}: CreatePartnerInput: got {status}: {response}"
        );
        let partner = response["input"].clone();
        let partner_id = partner["id"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            partner["arn"],
            json!(format!(
                "arn:aws:medialive:{region}:{ACCOUNT}:input:{partner_id}"
            )),
            "case {region:?}: expected the ARN to name the signed region, got {:?}",
            partner["arn"]
        );
    }
}

/// Rule: a partner can only be created for an input this account still holds
/// and may still change, so an id no input has — and one whose delete this
/// service has already accepted — is refused with this operation's modelled
/// `BadRequestException` (400, the only client refusal
/// `smithy_query(query="operation", operation="CreatePartnerInput")` declares
/// besides the authorization and throttling ones) and creates nothing.
#[tokio::test]
async fn a_partner_of_an_unusable_input_is_refused() {
    // (case, whether the id names a created input that is then deleted, id)
    let table: [(&str, bool, &str); 4] = [
        ("an id no input has", false, "0000000"),
        ("a non-numeric id", false, "nope"),
        ("the id after the last minted one", false, "1000001"),
        ("an input whose delete was accepted", true, ""),
    ];

    for (case, delete_first, unknown_id) in table {
        let router = medialive();
        let primary = create_input(
            &router,
            json!({
                "name": "refusal-primary",
                "type": "URL_PULL",
                "sources": [source("https://example.com/a.m3u8")],
            }),
        )
        .await;
        let primary_id = primary["id"].as_str().unwrap_or_default().to_string();

        let target = if delete_first {
            let (status, deleted) =
                call(&router, "DELETE", &format!("/prod/inputs/{primary_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteInput: got {status}: {deleted}"
            );
            primary_id.clone()
        } else {
            unknown_id.to_string()
        };

        let (status, body) = create_partner(&router, &target, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the partner create to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected the operation's own BadRequestException, got {body}"
        );

        // A refused create stored nothing: the next input minted is the one that
        // would have been minted had the refusal never been attempted.
        let next = create_input(
            &router,
            json!({ "name": "after-refusal", "type": "URL_PULL",
                    "sources": [source("https://example.com/b.m3u8")] }),
        )
        .await;
        assert_eq!(
            next["id"],
            json!("1000001"),
            "case {case:?}: a refused partner create must not have minted an input, got {:?}",
            next["id"]
        );
        assert_eq!(
            next["inputPartnerIds"],
            json!([]),
            "case {case:?}: a refused partner create must not have partnered anything, got {:?}",
            next["inputPartnerIds"]
        );
    }
}

/// Rule: a partner input IS an input, so it is stored under the same finite
/// ceiling — once the store is full the create answers the operation's modelled
/// `TooManyRequestsException` (429) rather than growing without bound.
#[tokio::test]
async fn the_partner_create_respects_the_input_store_bound() {
    let router = medialive();
    let primary = create_input(
        &router,
        json!({ "name": "bound-primary", "type": "URL_PULL",
                "sources": [source("https://example.com/a.m3u8")] }),
    )
    .await;
    let primary_id = primary["id"].as_str().unwrap_or_default().to_string();

    // One partner per remaining slot, then one too many.
    for index in 1..MAX_INPUTS {
        let (status, response) = create_partner(&router, &primary_id, None).await;
        assert_eq!(
            status, 201,
            "case partner {index}: expected 201, got {status}: {response}"
        );
    }

    let (status, body) = create_partner(&router, &primary_id, None).await;
    assert_eq!(
        status, 429,
        "case {MAX_INPUTS} stored: expected the partner create to be throttled, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "TooManyRequestsException",
        "case {MAX_INPUTS} stored: expected TooManyRequestsException, got {body}"
    );
}
