// SPDX-License-Identifier: CC0-1.0

//! What ONE region's endpoint can see of another region's resources, end to end
//! over the restJson1 wire.
//!
//! The rule: MediaLive is a regional service, so a resource belongs to the
//! region whose endpoint created it — the region its ARN names — and the
//! endpoint of any other region neither lists it, resolves its id, nor mutates
//! it; it answers for that id with the operation's own modelled `NotFoundException`
//! (404). A reference inside a configuration is resolved in the same region, so
//! an id of another region's resource is as unresolvable as one nothing minted.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests signed for a named
//! region, so what is asserted is the emulator's wire behaviour and not an
//! internal function's return value.
//!
//! The suites are TABLES. The first spans the resource KINDS the rule holds for
//! — one per create operation that mints an id of its own, each in a region pair
//! of its own, so no row can pass on another's region or on one resource's
//! store. The issue's `ListChannels` through a second region is ONE row of it.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    not_found_message, unknown_reference_message, MAX_RESOURCE_REGIONS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{channel_encoder_settings, percent_encoded};
use crate::wire::{call_in_region, error_code, error_message, medialive, ACCOUNT};

/// The tag collection every taggable resource's tags hang off
/// (`smithy_query(query="operation", operation="ListTagsForResource")`: GET
/// /prod/tags/{ResourceArn}).
const TAGS: &str = "/prod/tags";

/// One resource kind the rule holds for: where its collection lives, how its
/// create answers, how its listing reports it, and the resource type its
/// refusals name.
struct Kind {
    /// The kind, as the case names spell it.
    case: &'static str,
    /// The collection URI its create, its listing and its own
    /// describe/delete hang off.
    collection: &'static str,
    /// The member its create wraps the resource in, where it wraps it at all
    /// (`CreateNetwork`'s response is flat).
    wrapper: Option<&'static str>,
    /// The member its listing reports the page under.
    listed: &'static str,
    /// The resource type its not-found refusal opens with, as
    /// [`not_found_message`] composes it.
    resource_type: &'static str,
    /// The region its row creates the resource in.
    created_in: &'static str,
    /// A second region, whose endpoint must see nothing of it.
    elsewhere: &'static str,
    /// Its create body, built for the region the create is signed for.
    body: fn(&str) -> Value,
}

/// The kinds, each with a region pair of its own.
///
/// The regions differ per row — and no row uses the harness's default — because
/// the rule is about whatever region an endpoint serves rather than about any
/// one region or any one pair.
const KINDS: [Kind; 6] = [
    Kind {
        case: "channel",
        collection: "/prod/channels",
        wrapper: Some("channel"),
        listed: "channels",
        resource_type: "Channel",
        created_in: "us-east-1",
        elsewhere: "eu-west-1",
        body: channel_body,
    },
    Kind {
        case: "input",
        collection: "/prod/inputs",
        wrapper: Some("input"),
        listed: "inputs",
        resource_type: "Input",
        created_in: "eu-west-1",
        elsewhere: "us-east-1",
        body: input_body,
    },
    Kind {
        case: "input security group",
        collection: "/prod/inputSecurityGroups",
        wrapper: Some("securityGroup"),
        listed: "inputSecurityGroups",
        resource_type: "InputSecurityGroup",
        created_in: "ap-southeast-2",
        elsewhere: "ap-northeast-1",
        body: input_security_group_body,
    },
    Kind {
        case: "SdiSource",
        collection: "/prod/sdiSources",
        wrapper: Some("sdiSource"),
        listed: "sdiSources",
        resource_type: "SDI source",
        created_in: "sa-east-1",
        elsewhere: "ca-central-1",
        body: sdi_source_body,
    },
    Kind {
        case: "multiplex",
        collection: "/prod/multiplexes",
        wrapper: Some("multiplex"),
        listed: "multiplexes",
        resource_type: "Multiplex",
        created_in: "eu-central-1",
        elsewhere: "us-east-2",
        body: multiplex_body,
    },
    Kind {
        case: "network",
        collection: "/prod/networks",
        wrapper: None,
        listed: "networks",
        resource_type: "Network",
        created_in: "us-west-1",
        elsewhere: "af-south-1",
        body: network_body,
    },
];

/// A `CreateChannel` body: the encoder settings every channel configuration has
/// to declare, and nothing else, because this suite's rule is about the region
/// rather than about the configuration.
fn channel_body(_region: &str) -> Value {
    json!({ "name": "regional", "encoderSettings": channel_encoder_settings() })
}

/// A `CreateInput` body of the one type that needs no other resource in the
/// account, with the redundant pair of source URLs every PULL-type input needs.
fn input_body(_region: &str) -> Value {
    json!({
        "name": "regional",
        "type": "URL_PULL",
        "sources": [
            { "url": "https://example.com/a.m3u8" },
            { "url": "https://example.com/b.m3u8" },
        ],
    })
}

fn input_security_group_body(_region: &str) -> Value {
    json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] })
}

fn sdi_source_body(_region: &str) -> Value {
    json!({ "name": "regional" })
}

/// A `CreateMultiplex` body, whose availability zones are in the region the
/// create is signed for — an availability zone is a zone OF a region.
fn multiplex_body(region: &str) -> Value {
    json!({
        "name": "regional",
        "requestId": "region-isolation",
        "availabilityZones": [format!("{region}a"), format!("{region}c")],
        "multiplexSettings": { "transportStreamBitrate": 1_000_000, "transportStreamId": 1 },
    })
}

fn network_body(_region: &str) -> Value {
    json!({ "name": "regional", "ipPools": [{ "cidr": "10.0.0.0/16" }] })
}

/// `POST` of one kind's create body, signed for `region`.
async fn create_in(router: &MultiServiceRouter, kind: &Kind, region: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "POST",
        kind.collection,
        Some((kind.body)(region)),
    )
    .await
}

/// The created resource, with the create asserted to have succeeded, as the
/// members a client reads: the wrapper its own response carries, unwrapped.
async fn created(router: &MultiServiceRouter, kind: &Kind, region: &str) -> Value {
    let (status, response) = create_in(router, kind, region).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected the create in {region} to answer 201, got {status}: {response}",
        case = kind.case
    );
    match kind.wrapper {
        Some(wrapper) => response[wrapper].clone(),
        None => response,
    }
}

/// One member of a created resource, read as a string.
fn member(case: &str, resource: &Value, name: &str) -> String {
    resource[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} member, got {resource}"))
        .to_string()
}

/// The ids one region's endpoint lists for `kind`, in one page — the store's own
/// bound is below the largest page, and every row of this suite creates a single
/// resource, so one page is the whole listing.
async fn listed_ids(
    router: &MultiServiceRouter,
    kind: &Kind,
    region: &str,
) -> Vec<String> {
    let (status, body) = call_in_region(router, region, "GET", kind.collection, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the listing in {region} to answer 200, got {status}: {body}",
        case = kind.case
    );
    body[kind.listed]
        .as_array()
        .map(|page| {
            page.iter()
                .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Assert that `status`/`body` is the modelled 404 for `identifier`.
fn refused_as_not_found(
    case: &str,
    resource_type: &str,
    identifier: &str,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404 for {identifier}, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "NotFoundException",
        "case {case:?}: expected the modelled NotFoundException, got {body}"
    );
    let expected = not_found_message(resource_type, identifier);
    assert_eq!(
        error_message(body),
        expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
        error_message(body)
    );
}

/// Rule: a resource belongs to the region whose endpoint created it. Its ARN
/// names that region; its own region lists it and reads it back; and a second
/// region's endpoint lists nothing of it, refuses its id with the modelled
/// `NotFoundException` on a read AND on a delete, and leaves it exactly as it
/// was.
///
/// The rows are the resource kinds, each in a region pair of its own, because the
/// rule is about a MediaLive resource rather than about channels.
#[tokio::test]
async fn a_resource_belongs_to_the_region_that_created_it() {
    for kind in &KINDS {
        let router = medialive();
        let case = kind.case;
        let here = kind.created_in;
        let there = kind.elsewhere;

        let resource = created(&router, kind, here).await;
        let id = member(case, &resource, "id");
        let arn = member(case, &resource, "arn");

        // The ARN names the region the create was signed for.
        let prefix = format!("arn:aws:medialive:{here}:{ACCOUNT}:");
        assert!(
            arn.starts_with(&prefix),
            "case {case:?}: expected an ARN under {prefix:?}, got {arn:?}"
        );
        assert!(
            !arn.contains(there),
            "case {case:?}: expected the ARN not to name {there}, got {arn:?}"
        );

        // Its own region holds it: the listing reports it and the read answers.
        assert_eq!(
            listed_ids(&router, kind, here).await,
            vec![id.clone()],
            "case {case:?}: expected {here} to list exactly the resource it created"
        );
        let (status, read) =
            call_in_region(&router, here, "GET", &format!("{}/{id}", kind.collection), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {here} to read {id} back, got {status}: {read}"
        );

        // The other region holds nothing of it.
        assert_eq!(
            listed_ids(&router, kind, there).await,
            Vec::<String>::new(),
            "case {case:?}: expected {there} to list no resource of {here}"
        );
        let (status, read) =
            call_in_region(&router, there, "GET", &format!("{}/{id}", kind.collection), None).await;
        refused_as_not_found(
            &format!("{case}: read through {there}"),
            kind.resource_type,
            &id,
            status,
            &read,
        );

        // Nor may it mutate it: the delete is refused the same way, and the
        // resource its own region holds is untouched.
        let (status, deleted) = call_in_region(
            &router,
            there,
            "DELETE",
            &format!("{}/{id}", kind.collection),
            None,
        )
        .await;
        refused_as_not_found(
            &format!("{case}: delete through {there}"),
            kind.resource_type,
            &id,
            status,
            &deleted,
        );
        let (status, after) =
            call_in_region(&router, here, "GET", &format!("{}/{id}", kind.collection), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the refused delete to leave {id} readable in {here}, got \
             {status}: {after}"
        );
        assert_eq!(
            read_arn(case, &after, kind),
            arn,
            "case {case:?}: expected the stored resource to keep its own region's ARN"
        );
    }
}

/// The ARN a read reports, whichever shape that read answers with: a describe is
/// flat for most kinds and wrapped for the SdiSource
/// (`smithy_query(query="operation", operation="DescribeSdiSource")`:
/// `sdiSource`).
fn read_arn(case: &str, body: &Value, kind: &Kind) -> String {
    match body.get("arn") {
        Some(Value::String(arn)) => arn.clone(),
        _ => match kind.wrapper {
            Some(wrapper) => member(case, &body[wrapper], "arn"),
            None => panic!("case {case:?}: expected an arn member, got {body}"),
        },
    }
}

/// Rule: two regions keep their own resources, so the SAME create against two
/// endpoints yields two resources — each listed and readable in its own region
/// only — and neither id names the other's resource.
///
/// The rows are the kinds again, because the rule is about a MediaLive resource:
/// one region holding a resource says nothing about what another holds.
#[tokio::test]
async fn each_region_keeps_its_own_resources() {
    for kind in &KINDS {
        let router = medialive();
        let case = kind.case;
        let here = kind.created_in;
        let there = kind.elsewhere;

        let mine = created(&router, kind, here).await;
        let theirs = created(&router, kind, there).await;
        let my_id = member(case, &mine, "id");
        let their_id = member(case, &theirs, "id");

        // A minted id is the account's, not the region's: no two regions hand
        // out the same one, so a client's own records cannot confuse them.
        assert_ne!(
            my_id, their_id,
            "case {case:?}: expected the two regions to mint different ids, got {my_id} twice"
        );
        for (region, resource) in [(here, &mine), (there, &theirs)] {
            let prefix = format!("arn:aws:medialive:{region}:{ACCOUNT}:");
            let arn = member(case, resource, "arn");
            assert!(
                arn.starts_with(&prefix),
                "case {case:?}: expected the resource {region} created to carry an ARN under \
                 {prefix:?}, got {arn:?}"
            );
        }

        // Each region lists its own resource and only its own.
        assert_eq!(
            listed_ids(&router, kind, here).await,
            vec![my_id.clone()],
            "case {case:?}: expected {here} to list its own resource alone"
        );
        assert_eq!(
            listed_ids(&router, kind, there).await,
            vec![their_id.clone()],
            "case {case:?}: expected {there} to list its own resource alone"
        );

        // And each refuses the other's id.
        for (region, foreign) in [(here, &their_id), (there, &my_id)] {
            let (status, read) = call_in_region(
                &router,
                region,
                "GET",
                &format!("{}/{foreign}", kind.collection),
                None,
            )
            .await;
            refused_as_not_found(
                &format!("{case}: {region} reading {foreign}"),
                kind.resource_type,
                foreign,
                status,
                &read,
            );
        }
    }
}

/// Rule: a tag operation resolves the resource its `ResourceArn` names in the
/// region the request was signed for, so a resource of another region is as
/// unknown to it as an ARN nothing minted: it answers the modelled
/// `NotFoundException` (404) and writes nothing.
#[tokio::test]
async fn a_tag_write_resolves_its_arn_in_the_region_it_was_signed_for() {
    for kind in &KINDS {
        let router = medialive();
        let case = kind.case;
        let here = kind.created_in;
        let there = kind.elsewhere;

        let resource = created(&router, kind, here).await;
        let arn = member(case, &resource, "arn");
        let uri = format!("{TAGS}/{}", percent_encoded(&arn));

        // The tag write its own region serves is what the rows below are
        // measured against. `CreateTags` answers 204
        // (`smithy_query(query="operation", operation="CreateTags")`: no output
        // shape), so the write is read back through `ListTagsForResource`.
        let (status, tagged) = call_in_region(
            &router,
            here,
            "POST",
            &uri,
            Some(json!({ "tags": { "owner": "video" } })),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the tag write in {here} to answer 204, got {status}: {tagged}"
        );

        // The other region resolves that ARN to nothing, for a read and for a
        // write.
        let (status, read) = call_in_region(&router, there, "GET", &uri, None).await;
        refused_as_not_found(
            &format!("{case}: tags read through {there}"),
            "Resource",
            &arn,
            status,
            &read,
        );
        let (status, refused) = call_in_region(
            &router,
            there,
            "POST",
            &uri,
            Some(json!({ "tags": { "owner": "elsewhere" } })),
        )
        .await;
        refused_as_not_found(
            &format!("{case}: tag write through {there}"),
            "Resource",
            &arn,
            status,
            &refused,
        );

        // The refused write changed nothing: the resource's own region still
        // reports the tag it holds.
        let (status, tags) = call_in_region(&router, here, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the tags to be readable in {here}, got {status}: {tags}"
        );
        assert_eq!(
            tags["tags"],
            json!({ "owner": "video" }),
            "case {case:?}: expected the tag {here} wrote, got {:?}",
            tags["tags"]
        );
    }
}

/// Rule: a reference inside a configuration is resolved in the region the
/// request was signed for, so an id naming another region's resource is refused
/// exactly as an id nothing minted is — the resource is not one this endpoint
/// holds.
///
/// The rows are the two reference classes this service refuses differently: a
/// channel's `InputAttachments`, whose unresolvable entry is one 422
/// `ValidationErrors` problem, and an input's `InputSecurityGroups`, whose
/// unresolvable entry is the 400 `BadRequestException` that names it.
#[tokio::test]
async fn a_reference_resolves_in_the_region_the_request_was_signed_for() {
    let here = "eu-west-2";
    let there = "us-east-1";

    // A channel attachment naming an input of another region.
    let router = medialive();
    let input = created(&router, &KINDS[1], here).await;
    let input_id = member("input", &input, "id");

    let mut body = channel_body(there);
    body["inputAttachments"] = json!([{ "inputAttachmentName": "feed", "inputId": input_id }]);
    let (status, refused) =
        call_in_region(&router, there, "POST", "/prod/channels", Some(body.clone())).await;
    let case = "a channel attaching another region's input";
    assert_eq!(
        status, 422,
        "case {case:?}: expected the 422 an unrunnable configuration earns, got {status}: {refused}"
    );
    assert_eq!(
        error_code(status, &refused),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refused}"
    );
    let problems = refused["validationErrors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected validationErrors, got {refused}"))
        .clone();
    assert_eq!(
        problems.len(),
        1,
        "case {case:?}: expected one problem, got {refused}"
    );
    assert_eq!(
        problems[0]["errorMessage"],
        json!(unknown_reference_message(
            "inputAttachments",
            "input",
            &input_id
        )),
        "case {case:?}: expected the unresolved-reference message, got {:?}",
        problems[0]
    );

    // The same attachment is served by the region that holds the input, so the
    // row above is about WHERE the reference resolves and not about the body.
    let (status, created_here) =
        call_in_region(&router, here, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected {here} to serve the same configuration, got {status}: \
         {created_here}"
    );

    // An input naming an input security group of another region.
    let router = medialive();
    let group = created(&router, &KINDS[2], here).await;
    let group_id = member("input security group", &group, "id");
    let mut body = json!({ "name": "pushed-to", "type": "RTP_PUSH", "destinations": [{}] });
    body["inputSecurityGroups"] = json!([group_id.clone()]);
    let case = "an input naming another region's security group";
    let (status, refused) =
        call_in_region(&router, there, "POST", "/prod/inputs", Some(body.clone())).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the modelled 400, got {status}: {refused}"
    );
    assert_eq!(
        error_code(status, &refused),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {refused}"
    );
    assert!(
        error_message(&refused).contains(&group_id),
        "case {case:?}: expected the message to name {group_id:?}, got {:?}",
        error_message(&refused)
    );
    let (status, served) =
        call_in_region(&router, here, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected {here} to serve the same configuration, got {status}: {served}"
    );
}

/// Rule: the emulator keeps resources for at most [`MAX_RESOURCE_REGIONS`]
/// regions — the region is whatever the caller signed for, so the ceiling is what
/// stops a loop over made-up region names from growing the emulator's heap
/// without limit — and a request for a region beyond it is answered with the
/// modelled server error rather than served out of another region's store.
///
/// Every region inside the ceiling keeps answering, and the resource the first
/// one created is still there afterwards: the ceiling refuses, it does not evict.
#[tokio::test]
async fn the_store_keeps_resources_for_a_bounded_number_of_regions() {
    let router = medialive();
    let regions: Vec<String> = (0..MAX_RESOURCE_REGIONS)
        .map(|index| format!("emulator-region-{index}"))
        .collect();

    let first = created(&router, &KINDS[3], &regions[0]).await;
    let first_id = member("SdiSource", &first, "id");

    // Fill the store: one region per partition, each reached by a listing, which
    // is the cheapest request that needs that region's resources.
    for region in &regions[1..] {
        assert_eq!(
            listed_ids(&router, &KINDS[3], region).await,
            Vec::<String>::new(),
            "expected {region} to hold no SdiSource of its own"
        );
    }

    // One region past the ceiling: no partition for it, so the request earns the
    // modelled server error instead of another region's answer.
    let case = "a region past the ceiling";
    let (status, refused) =
        call_in_region(&router, "emulator-region-overflow", "GET", "/prod/sdiSources", None).await;
    assert_eq!(
        status, 500,
        "case {case:?}: expected the modelled server error, got {status}: {refused}"
    );
    assert_eq!(
        error_code(status, &refused),
        "InternalServerErrorException",
        "case {case:?}: expected InternalServerErrorException, got {refused}"
    );

    // The regions the store does hold are unaffected, and nothing was evicted.
    assert_eq!(
        listed_ids(&router, &KINDS[3], &regions[0]).await,
        vec![first_id],
        "expected the first region to still hold the SdiSource it created"
    );
}
