// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for an Outpost's private connectivity configuration, driven
//! over the wire through the same front door as [`crate::site_and_outpost_lifecycle`]
//! (whose harness this suite reuses) — `MultiServiceRouter::dispatch` with a
//! restJson1 request, exactly as a signed `aws outposts` call arrives.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `CreatePrivateConnectivityConfig` persists the VPC information the caller sent
//!    on the Outpost its `@httpLabel` names — by bare id or by Outpost ARN, the two
//!    forms the modelled `OutpostId` pattern accepts — and reports the configuration
//!    from what was persisted, with the `ENABLED` status a configured Outpost holds
//!    and the Outpost's own ID.
//! 2. `GetPrivateConnectivityConfig` answers that stored configuration field for
//!    field, for either identifier form, as often as it is asked; the configuration
//!    belongs to ONE Outpost, so configuring one Outpost is invisible to every
//!    other.
//! 3. An Outpost nobody configured private connectivity for answers HTTP 200 with
//!    no `PrivateConnectivityConfig` member at all — and reading it does not create
//!    one.
//! 4. This API models no update and no delete of the configuration, so a second
//!    create against an already configured Outpost is refused with 409
//!    `ConflictException` naming that Outpost, and the stored configuration is left
//!    exactly as it was.
//! 5. An Outpost identifier that names nothing is 404 `NotFoundException` whose body
//!    is this service's identifier echo; a request outside a modelled constraint is
//!    400 `ValidationException` and persists nothing.

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{
    address, refusal, segment, text, Outposts, Refusal, ABSENT_OUTPOST_ID, OUTPOST_ARN_PREFIX,
};

/// The route both operations are bound to:
/// `(GET|POST) /outposts/{OutpostId}/privateConnectivity`, from
/// `smithy_query(query="operation",operation="GetPrivateConnectivityConfig,CreatePrivateConnectivityConfig")`.
fn route(identifier: &str) -> String {
    format!("/outposts/{}/privateConnectivity", segment(identifier))
}

/// The status a configured Outpost's private connectivity holds.
///
/// The modelled `PrivateConnectivityStatus` enum declares exactly `ENABLED` and
/// `DISABLED` (from `smithy_query(query="shape",shape="PrivateConnectivityStatus")`),
/// and neither is a provisioning value, so a configuration that exists is already
/// terminal.
const ENABLED: &str = "ENABLED";

/// The modelled `SubnetIds` maximum (`@length(min: 1, max: 10)`), from
/// `smithy_query(query="shape",shape="SubnetIds")`.
const MAX_SUBNET_IDS: usize = 10;

/// The modelled `VpcId` / `SubnetId` maximum length (`@length(min: 1, max: 25)`).
const MAX_VPC_ID_LENGTH: usize = 25;

/// `count` distinct subnet ids of the modelled `SubnetId` form (`^[a-z0-9-]+$`,
/// len 1..25).
fn subnet_ids(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("subnet-{index:016x}"))
        .collect()
}

/// One `VpcInformation` entry carrying exactly the members a case names — every
/// member of the shape is optional, so an entry may carry any subset of them.
fn vpc_information(vpc_id: Option<&str>, subnets: Option<usize>, endpoint: Option<&str>) -> Value {
    let mut entry = json!({});
    let object = entry.as_object_mut().expect("a JSON object");
    if let Some(vpc_id) = vpc_id {
        object.insert("VpcId".to_string(), json!(vpc_id));
    }
    if let Some(count) = subnets {
        object.insert("SubnetIds".to_string(), json!(subnet_ids(count)));
    }
    if let Some(endpoint) = endpoint {
        object.insert("VpcEndpointId".to_string(), json!(endpoint));
    }
    entry
}

/// A `CreatePrivateConnectivityConfig` payload around one `VpcInformation` entry.
/// The modelled `VpcInformationList` is `@length(min: 1, max: 1)`, so a valid
/// request carries exactly one.
fn request(entry: Value) -> Value {
    json!({ "VpcInformationList": [entry] })
}

/// A fresh Outpost on a fresh site, returning its `(OutpostId, OutpostArn)`.
async fn outpost(outposts: &Outposts, name: &str) -> (String, String) {
    let site = outposts
        .create_site(&json!({
            "Name": format!("{name} site"),
            "OperatingAddress": address("US", "WA", "Seattle"),
        }))
        .await;
    let created = outposts
        .create_outpost(&json!({
            "Name": name,
            "SiteId": text(&site, "SiteId"),
        }))
        .await;
    (text(&created, "OutpostId"), text(&created, "OutpostArn"))
}

/// `GetPrivateConnectivityConfig`, returning the whole 200 output so a case can
/// tell an absent `PrivateConnectivityConfig` member from a present one.
async fn get_config(outposts: &Outposts, identifier: &str) -> Value {
    outposts.ok("GET", &route(identifier), None).await
}

/// The `PrivateConnectivityConfig` member of an output, or [`Value::Null`] when the
/// output carries none.
fn config_of(output: &Value) -> Value {
    output
        .get("PrivateConnectivityConfig")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Rules 1 + 2: every shape of request persists, reports back what it persisted,
/// and reads back identically through both identifier forms — and no row's
/// configuration disturbs another's.
///
/// The rows differ along everything the operation generalises over: which members
/// of `VpcInformation` the entry carries, how many subnet ids it names (one, a few,
/// and the modelled maximum), and which identifier form the create named the
/// Outpost with.
#[tokio::test]
async fn create_persists_the_vpc_information_and_both_identifier_forms_read_it_back() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "vpc, one subnet and an endpoint, created by bare id",
            request(vpc_information(
                Some("vpc-0a1b2c3d"),
                Some(1),
                Some("vpce-0a1b2c3d4e5f"),
            )),
            false,
        ),
        (
            "three subnets, created by Outpost ARN",
            request(vpc_information(
                Some("vpc-11112222"),
                Some(3),
                Some("vpce-aaaabbbbcccc"),
            )),
            true,
        ),
        (
            "every subnet the modelled list takes, created by bare id",
            request(vpc_information(
                Some("vpc-33334444"),
                Some(MAX_SUBNET_IDS),
                Some("vpce-0000111122223333"),
            )),
            false,
        ),
        (
            "no VPC endpoint, created by Outpost ARN",
            request(vpc_information(Some("vpc-55556666"), Some(2), None)),
            true,
        ),
        (
            "a VPC and nothing else, created by bare id",
            request(vpc_information(Some("vpc-77778888"), None, None)),
            false,
        ),
        (
            "subnets and nothing else, created by bare id",
            request(vpc_information(None, Some(4), None)),
            false,
        ),
        (
            "an endpoint and nothing else, created by Outpost ARN",
            request(vpc_information(None, None, Some("vpce-99990000"))),
            true,
        ),
        (
            "a VPC id at the modelled length maximum, created by bare id",
            request(vpc_information(
                Some(&"v".repeat(MAX_VPC_ID_LENGTH)),
                Some(1),
                None,
            )),
            false,
        ),
    ];

    // Every row's own Outpost and the configuration it stored, re-read after the
    // whole table has run: rule 2's per-Outpost isolation.
    let mut stored: Vec<(&str, String, Value)> = Vec::new();

    for (case, payload, by_arn) in &cases {
        let (outpost_id, outpost_arn) = outpost(&outposts, case).await;
        let identifier = match by_arn {
            true => outpost_arn.clone(),
            false => outpost_id.clone(),
        };

        let created = outposts
            .ok("POST", &route(&identifier), Some(payload))
            .await;
        let config = config_of(&created);
        assert_eq!(
            config.get("VpcInformationList"),
            payload.get("VpcInformationList"),
            "case {case:?}: the create must report the VpcInformationList it was sent, got {created:?}"
        );
        // A configured Outpost's private connectivity is enabled; the modelled enum
        // declares no third, transient value.
        let status = text(&config, "PrivateConnectivityStatus");
        assert_eq!(
            status, ENABLED,
            "case {case:?}: expected PrivateConnectivityStatus {ENABLED:?}, got {status:?}"
        );
        // The modelled output member is the Outpost's ID, whichever form the
        // request named it in.
        assert_eq!(
            text(&created, "OutpostId"),
            outpost_id,
            "case {case:?}: the create must report the Outpost's id, got {created:?}"
        );

        // Rule 2: both identifier forms name the one stored configuration, and
        // reading it twice answers the same thing.
        for reader in [outpost_id.clone(), outpost_arn.clone()] {
            let read = config_of(&get_config(&outposts, &reader).await);
            assert_eq!(
                read, config,
                "case {case:?}: GetPrivateConnectivityConfig on {reader:?} must answer the \
                 stored configuration, got {read:?}"
            );
        }
        let again = config_of(&get_config(&outposts, &outpost_id).await);
        assert_eq!(
            again, config,
            "case {case:?}: a second read must answer the same configuration, got {again:?}"
        );

        stored.push((case, outpost_id, config));
    }

    for (case, outpost_id, config) in &stored {
        let read = config_of(&get_config(&outposts, outpost_id).await);
        assert_eq!(
            &read, config,
            "case {case:?}: every Outpost keeps its OWN configuration after the others were \
             configured, got {read:?}"
        );
    }
}

/// Rule 3: an Outpost nobody configured answers 200 with no configuration member,
/// reading it does not create one, and configuring one Outpost leaves the others
/// exactly as they were.
#[tokio::test]
async fn an_unconfigured_outpost_reports_no_configuration() {
    let outposts = Outposts::new();
    let cases = [
        "unconfigured, read once",
        "unconfigured, read twice",
        "configured later",
    ];
    let mut created: Vec<(&str, String)> = Vec::new();
    for case in cases {
        let (outpost_id, _) = outpost(&outposts, case).await;
        created.push((case, outpost_id));
    }

    for (case, outpost_id) in &created {
        // Twice, because a read that materialised an empty configuration would
        // still answer "none" the first time.
        for attempt in 1..=2 {
            let output = get_config(&outposts, outpost_id).await;
            assert_eq!(
                config_of(&output),
                Value::Null,
                "case {case:?}: read {attempt} of an unconfigured Outpost must report no \
                 PrivateConnectivityConfig, got {output:?}"
            );
        }
    }

    // Configure the last Outpost only.
    let (configured_case, configured_id) = created
        .last()
        .cloned()
        .expect("the table creates at least one Outpost");
    let payload = request(vpc_information(
        Some("vpc-abcdabcd"),
        Some(2),
        Some("vpce-abcdabcd"),
    ));
    let config = config_of(&outposts.ok("POST", &route(&configured_id), Some(&payload)).await);
    assert_eq!(
        config_of(&get_config(&outposts, &configured_id).await),
        config,
        "case {configured_case:?}: the configured Outpost reports what was stored for it"
    );

    for (case, outpost_id) in created.iter().filter(|(_, id)| id != &configured_id) {
        let output = get_config(&outposts, outpost_id).await;
        assert_eq!(
            config_of(&output),
            Value::Null,
            "case {case:?}: configuring another Outpost must leave this one unconfigured, got \
             {output:?}"
        );
    }
}

/// Rule 4: a second create is refused with 409 `ConflictException` naming the
/// Outpost, whatever the second request carries, and the stored configuration is
/// unchanged afterwards.
///
/// The rows differ along how the second request relates to the first: a different
/// VPC, a different subnet count, the identical payload (so the refusal is not a
/// "the value differs" check), and a second create addressed by the other
/// identifier form.
#[tokio::test]
async fn a_second_create_conflicts_and_keeps_the_stored_configuration() {
    let first = request(vpc_information(
        Some("vpc-0a1b2c3d"),
        Some(2),
        Some("vpce-0a1b2c3d4e5f"),
    ));
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "a different VPC, by bare id",
            request(vpc_information(Some("vpc-deadbeef"), Some(1), None)),
            false,
        ),
        (
            "more subnets, by bare id",
            request(vpc_information(
                Some("vpc-0a1b2c3d"),
                Some(MAX_SUBNET_IDS),
                Some("vpce-0a1b2c3d4e5f"),
            )),
            false,
        ),
        ("the identical payload, by bare id", first.clone(), false),
        (
            "a different VPC, by Outpost ARN",
            request(vpc_information(Some("vpc-cafebabe"), Some(3), None)),
            true,
        ),
    ];

    for (case, second, by_arn) in &cases {
        let outposts = Outposts::new();
        let (outpost_id, outpost_arn) = outpost(&outposts, case).await;
        let config = config_of(&outposts.ok("POST", &route(&outpost_id), Some(&first)).await);

        let identifier = match by_arn {
            true => outpost_arn.clone(),
            false => outpost_id.clone(),
        };
        let reply = outposts
            .call("POST", &route(&identifier), Some(second))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        // The modelled `ConflictException` carries the resource the conflict is
        // about: this Outpost, named by the `ResourceType` the model's enum spells
        // `OUTPOST`.
        assert_eq!(
            text(&reply.body, "ResourceId"),
            outpost_id,
            "case {case:?}: the conflict must name the Outpost, got {}",
            reply.body
        );
        assert_eq!(
            text(&reply.body, "ResourceType"),
            "OUTPOST",
            "case {case:?}: the conflict must name the resource type, got {}",
            reply.body
        );

        let read = config_of(&get_config(&outposts, &outpost_id).await);
        assert_eq!(
            read, config,
            "case {case:?}: a refused create must leave the stored configuration as it was, got \
             {read:?}"
        );
    }
}

/// Rule 5: an identifier that names no Outpost is 404 with the identifier echo;
/// a request outside a modelled constraint is 400 — on both routes, for every
/// constraint class the two shapes declare — and neither persists anything.
#[tokio::test]
async fn refusals_are_classified_and_persist_nothing() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outpost(&outposts, "refusal target").await;
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let valid = request(vpc_information(Some("vpc-0a1b2c3d"), Some(1), None));
    let one_past_the_subnet_maximum = json!(subnet_ids(MAX_SUBNET_IDS + 1));

    let cases: Vec<Refusal> = vec![
        refusal(
            "GetPrivateConnectivityConfig on an Outpost nothing holds, by bare id",
            "GET",
            route(ABSENT_OUTPOST_ID),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetPrivateConnectivityConfig on an Outpost nothing holds, by Outpost ARN",
            "GET",
            route(&absent_outpost_arn),
            404,
            "NotFoundException",
        ),
        refusal(
            "CreatePrivateConnectivityConfig on an Outpost nothing holds",
            "POST",
            route(ABSENT_OUTPOST_ID),
            404,
            "NotFoundException",
        )
        .with_body(valid.clone()),
        refusal(
            "GetPrivateConnectivityConfig with a label that is no Outpost identifier",
            "GET",
            route("not-an-outpost-id"),
            400,
            "ValidationException",
        ),
        refusal(
            "GetPrivateConnectivityConfig with a label whose hex is too short",
            "GET",
            route("op-0000000000000000"),
            400,
            "ValidationException",
        ),
        refusal(
            "CreatePrivateConnectivityConfig with no VpcInformationList",
            "POST",
            route(&outpost_id),
            400,
            "ValidationException",
        )
        .with_body(json!({})),
        refusal(
            "CreatePrivateConnectivityConfig with an empty VpcInformationList",
            "POST",
            route(&outpost_id),
            400,
            "ValidationException",
        )
        .with_body(json!({ "VpcInformationList": [] })),
        refusal(
            "CreatePrivateConnectivityConfig with two VpcInformation entries",
            "POST",
            route(&outpost_id),
            400,
            "ValidationException",
        )
        .with_body(json!({
            "VpcInformationList": [
                vpc_information(Some("vpc-0a1b2c3d"), Some(1), None),
                vpc_information(Some("vpc-11112222"), Some(1), None),
            ]
        })),
        refusal(
            "CreatePrivateConnectivityConfig with one subnet past the modelled maximum",
            "POST",
            route(&outpost_id),
            400,
            "ValidationException",
        )
        .with_body(json!({
            "VpcInformationList": [{ "SubnetIds": one_past_the_subnet_maximum }]
        })),
        refusal(
            "CreatePrivateConnectivityConfig with an empty SubnetIds list",
            "POST",
            route(&outpost_id),
            400,
            "ValidationException",
        )
        .with_body(json!({
            "VpcInformationList": [{ "VpcId": "vpc-0a1b2c3d", "SubnetIds": [] }]
        })),
        refusal(
            "CreatePrivateConnectivityConfig with a VpcEndpointId outside its pattern",
            "POST",
            route(&outpost_id),
            400,
            "ValidationException",
        )
        .with_body(request(vpc_information(
            Some("vpc-0a1b2c3d"),
            Some(1),
            Some("endpoint-0a1b2c3d"),
        ))),
        refusal(
            "CreatePrivateConnectivityConfig with a VpcId one character past its maximum",
            "POST",
            route(&outpost_id),
            400,
            "ValidationException",
        )
        .with_body(request(vpc_information(
            Some(&"v".repeat(MAX_VPC_ID_LENGTH + 1)),
            Some(1),
            None,
        ))),
    ];

    outposts.expect_refusals(&cases).await;

    // Rule 5's second half: nothing a refused call touched was persisted, so the
    // Outpost every 400 above was addressed to is still unconfigured.
    let output = get_config(&outposts, &outpost_id).await;
    assert_eq!(
        config_of(&output),
        Value::Null,
        "a refused create must persist nothing, got {output:?}"
    );

    // The 404 body is this service's identifier echo: the member the identifier
    // arrived in and the value EXACTLY as supplied, so the ARN form reads its ARN
    // back rather than the bare id it resolved to.
    for (case, supplied) in [
        ("by bare id", ABSENT_OUTPOST_ID.to_string()),
        ("by Outpost ARN", absent_outpost_arn.clone()),
    ] {
        let reply = outposts.call("GET", &route(&supplied), None).await;
        assert_eq!(
            text(&reply.body, "Message"),
            format!("outpostId={supplied}"),
            "case {case:?}: the 404 body must echo the identifier as supplied, got {}",
            reply.body
        );
    }
}
