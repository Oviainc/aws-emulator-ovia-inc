// SPDX-License-Identifier: CC0-1.0

//! MediaLive cloudwatch alarm template groups, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each.

use std::time::SystemTime;

use serde_json::{json, Value};

use crate::support::{
    create_in, epoch_seconds, get_in, member, reported_tags, timestamp, unix_seconds, GROUPS,
};
use crate::wire::{call, error_code, medialive, ACCOUNT, DEFAULT_REGION as REGION};

/// Rule: a create mints a server-side id for the new resource, builds its ARN
/// from the region the request was signed for and that id, stores every value the
/// request supplied verbatim, stamps the creation instant on both its created and
/// modified members, and the resource's reader answers with exactly the stored
/// resource for either form of identifier the model documents.
///
/// The table spans what the rule generalises over: whether the optional members
/// (`Description`, `Tags`) are supplied at all, how many tags there are, and the
/// form and size of `Name` and `Description` — including the published maxima of
/// the shapes they target (`__stringMin1Max255PatternS`,
/// `__stringMin0Max1024`).
#[tokio::test]
async fn created_groups_round_trip_through_their_reader() {
    // (case, Name, Description, Tags)
    let table: Vec<(&str, String, Option<String>, Option<Value>)> = vec![
        ("name only", "group-minimal".to_string(), None, None),
        (
            "described",
            "group-described".to_string(),
            Some("a group of alarm templates".to_string()),
            None,
        ),
        (
            "empty description",
            "group-empty-description".to_string(),
            // `__stringMin0Max1024` admits the empty string.
            Some(String::new()),
            None,
        ),
        (
            "one tag",
            "group-one-tag".to_string(),
            None,
            Some(json!({ "owner": "video" })),
        ),
        (
            "three tags",
            "group-three-tags".to_string(),
            Some("tagged".to_string()),
            Some(json!({ "owner": "video", "env": "test", "cost-centre": "1234" })),
        ),
        (
            "empty tag map",
            "group-no-tags".to_string(),
            None,
            Some(json!({})),
        ),
        // The maxima of the shapes `Name` and `Description` target.
        ("name at 255 characters", "n".repeat(255), None, None),
        (
            "description at 1024 characters",
            "group-long-description".to_string(),
            Some("d".repeat(1024)),
            None,
        ),
        (
            "non-ascii name",
            "グループ-ライブ".to_string(),
            Some("名前".to_string()),
            None,
        ),
    ];

    let router = medialive();
    let mut minted: Vec<String> = Vec::new();

    for (case, name, description, tags) in table {
        let mut request = json!({ "name": name });
        if let Some(description) = description.as_ref() {
            request["description"] = json!(description);
        }
        if let Some(tags) = tags.as_ref() {
            request["tags"] = tags.clone();
        }

        let before = SystemTime::now();
        let (status, created) = create_in(&router, REGION, request).await;
        let after = SystemTime::now();
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {created}"
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        // `CloudWatchAlarmTemplateGroupSummary$Id`: "AWS provided template groups
        // have ids that start with `aws-`" — a caller-created one does not, and
        // `__stringMin7Max11PatternAws097` leaves it seven digits.
        assert!(
            id.len() == 7 && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a seven-digit group id, got {:?}",
            created["id"]
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: expected a fresh id, got {id:?} which was already minted: {minted:?}"
        );
        minted.push(id.clone());

        assert_eq!(
            created["arn"],
            json!(format!(
                "arn:aws:medialive:{REGION}:{ACCOUNT}:cloudwatch-alarm-template-group:{id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["name"],
            json!(name),
            "case {case:?}: expected the requested name, got {:?}",
            created["name"]
        );
        assert_eq!(
            member(&created, "description"),
            description.as_ref().map_or(Value::Null, |text| json!(text)),
            "case {case:?}: expected the requested description, got {:?}",
            member(&created, "description")
        );
        // `Tags` is always reported, as the map it is modelled as: a group created
        // with no tags reports the empty map.
        assert_eq!(
            member(&created, "tags"),
            reported_tags(tags.as_ref()),
            "case {case:?}: expected the requested tags, got {:?}",
            member(&created, "tags")
        );
        assert_eq!(
            timestamp(case, "createdAt", &created),
            timestamp(case, "modifiedAt", &created),
            "case {case:?}: a group nothing has modified yet is modified when it was created, got {created}"
        );
        // The stamp is the instant of THIS call, not a fixed instant: it falls in
        // the window the call ran in, widened by a second each way because the
        // wire rendering is second-resolution.
        let stamped = epoch_seconds(timestamp(case, "createdAt", &created));
        let (opened, closed) = (unix_seconds(before) - 1.0, unix_seconds(after) + 1.0);
        assert!(
            (opened..=closed).contains(&stamped),
            "case {case:?}: expected createdAt within the call's window [{opened}, {closed}], got {stamped}"
        );

        // ROUND TRIP: the reader answers with the stored group, reached by either
        // form `GetCloudWatchAlarmTemplateGroupRequest$Identifier` documents
        // ("Can be either be its id or current name").
        for (form, identifier) in [("id", id.as_str()), ("name", name.as_str())] {
            let (status, read) = get_in(&router, REGION, identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?} read by {form}: expected 200, got {status}: {read}"
            );
            assert_eq!(
                read, created,
                "case {case:?} read by {form}: expected the group the create reported, got {read}"
            );
        }
    }

    // Every group created above still reads back, so a later create never
    // disturbed an earlier group.
    for (index, id) in minted.iter().enumerate() {
        let (status, read) = get_in(&router, REGION, id).await;
        assert_eq!(
            status, 200,
            "case group {index} ({id}) after every create: expected 200, got {status}: {read}"
        );
        assert_eq!(
            read["id"],
            json!(id),
            "case group {index}: expected the group's own id, got {:?}",
            read["id"]
        );
    }
}

/// Rule: a resource whose `Name` member is documented as unique "within the scope
/// of a resource type in a specific region" refuses a create that reuses a name
/// already taken in that region with the operation's modelled conflict, and
/// accepts the same name in another region, where each region's resource is
/// reachable only from that region.
#[tokio::test]
async fn a_group_name_is_unique_within_one_region() {
    // (case, the name two creates compete for, how many times it repeats — the
    // row carries the name itself, so no assertion re-reads the case label)
    let table: [(&str, &str, usize); 3] = [
        ("plain name", "shared-group", 1),
        // `Name` targets `__stringMin1Max255PatternS`, so 255 is its maximum.
        ("name at 255 characters", "s", 255),
        ("non-ascii name", "共有グループ", 1),
    ];

    for (case, seed, repeats) in table {
        let name = seed.repeat(repeats);
        let router = medialive();

        let (status, first) = create_in(&router, REGION, json!({ "name": name })).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first create to succeed, got {status}: {first}"
        );

        let (status, conflict) = create_in(
            &router,
            REGION,
            json!({ "name": name, "description": "a second group with a taken name" }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the taken name to conflict, got {status}: {conflict}"
        );
        assert_eq!(
            error_code(status, &conflict),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {conflict}"
        );

        // The refused create left the first group exactly as it was.
        let (status, unchanged) = get_in(&router, REGION, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first group to survive, got {status}: {unchanged}"
        );
        assert_eq!(
            unchanged, first,
            "case {case:?}: expected the refused create to change nothing, got {unchanged}"
        );

        // Another region is another scope: the same name is free there, and the
        // group it creates is its own.
        let other = "eu-central-1";
        let (status, elsewhere) = create_in(&router, other, json!({ "name": name })).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the name to be free in {other}, got {status}: {elsewhere}"
        );
        assert_ne!(
            elsewhere["id"], first["id"],
            "case {case:?}: expected a distinct group in {other}, got {:?} twice",
            elsewhere["id"]
        );
        assert_eq!(
            elsewhere["arn"],
            json!(format!(
                "arn:aws:medialive:{other}:{ACCOUNT}:cloudwatch-alarm-template-group:{}",
                elsewhere["id"].as_str().unwrap_or_default()
            )),
            "case {case:?}: expected the ARN to name {other}, got {:?}",
            elsewhere["arn"]
        );

        // Each region sees only its own group.
        let (status, read) = get_in(&router, other, &name).await;
        assert_eq!(status, 200, "case {case:?}: {other} read: got {status}: {read}");
        assert_eq!(
            read, elsewhere,
            "case {case:?}: expected {other} to read its own group, got {read}"
        );

        let first_id = first["id"].as_str().unwrap_or_default().to_string();
        let (status, hidden) = get_in(&router, other, &first_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {REGION}'s group to be invisible in {other}, got {status}: {hidden}"
        );
    }
}

/// One row of the idempotency table: the token each of two creates carries, the
/// name the second one asks for, and whether the two must name one group.
struct Retry {
    case: &'static str,
    first_token: Option<&'static str>,
    second_token: Option<&'static str>,
    second_name: &'static str,
    same: bool,
}

/// The name the first create of every [`Retry`] row asks for.
const FIRST_NAME: &str = "idem";

/// Rule: `RequestId` is an `@idempotencyToken`, so two creates carrying the same
/// one yield one resource — the first — and creates carrying different ones (or
/// none) each yield their own.
#[tokio::test]
async fn a_repeated_request_id_creates_one_group() {
    let table = [
        Retry {
            case: "same token, same name",
            first_token: Some("token-a"),
            second_token: Some("token-a"),
            second_name: FIRST_NAME,
            same: true,
        },
        Retry {
            // The token identifies the create, so a retry that drifted in its
            // other members still returns the group the first one made.
            case: "same token, other name",
            first_token: Some("token-a"),
            second_token: Some("token-a"),
            second_name: "idem-drifted",
            same: true,
        },
        Retry {
            case: "different tokens",
            first_token: Some("token-a"),
            second_token: Some("token-b"),
            second_name: "idem-second",
            same: false,
        },
        Retry {
            case: "no token",
            first_token: None,
            second_token: None,
            second_name: "idem-second",
            same: false,
        },
    ];

    for Retry {
        case,
        first_token,
        second_token,
        second_name,
        same,
    } in table
    {
        let router = medialive();

        let mut first = json!({ "name": FIRST_NAME });
        if let Some(token) = first_token {
            first["requestId"] = json!(token);
        }
        let mut second = json!({ "name": second_name });
        if let Some(token) = second_token {
            second["requestId"] = json!(token);
        }

        let (status, one) = create_in(&router, REGION, first).await;
        assert_eq!(status, 201, "case {case:?}: first create got {status}: {one}");
        let (status, two) = create_in(&router, REGION, second).await;
        assert_eq!(status, 201, "case {case:?}: second create got {status}: {two}");

        assert_eq!(
            one["id"] == two["id"],
            same,
            "case {case:?}: expected same-group={same}, got {:?} then {:?}",
            one["id"],
            two["id"]
        );
        if same {
            assert_eq!(
                two, one,
                "case {case:?}: expected the retry to report the first group, got {two}"
            );
            // The retry stored nothing new: the name the second request named
            // belongs to no group.
            let (status, body) = get_in(&router, REGION, second_name).await;
            let expected = if second_name == FIRST_NAME { 200 } else { 404 };
            assert_eq!(
                status, expected,
                "case {case:?}: expected reading {second_name:?} to answer {expected}, got {status}: {body}"
            );
        } else {
            let (status, read) = get_in(&router, REGION, second_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the second group to exist, got {status}: {read}"
            );
            assert_eq!(
                read, two,
                "case {case:?}: expected the second group as created, got {read}"
            );
        }
    }
}

/// Rule: an `@idempotencyToken` is idempotent for the regional endpoint it was
/// presented to, so one token presented in two regions creates one resource in
/// each and every retry still resolves the resource its own region's call
/// created.
#[tokio::test]
async fn one_request_id_is_idempotent_within_each_region() {
    // The two regions the one token is presented in. Nothing about the pair
    // matters except that they differ.
    let regions = ["us-west-2", "eu-central-1"];
    let token = "token-shared-across-regions";
    let name = "same-name-in-both-regions";

    let router = medialive();
    let mut first: Vec<Value> = Vec::new();

    for region in regions {
        let (status, created) =
            create_in(&router, region, json!({ "name": name, "requestId": token })).await;
        assert_eq!(
            status, 201,
            "case {region}: expected the create to succeed, got {status}: {created}"
        );
        first.push(created);
    }

    assert_ne!(
        first[0]["id"], first[1]["id"],
        "case two regions: expected a group of its own in each region, got {:?} twice",
        first[0]["id"]
    );

    // The retry in each region returns that region's group, not the one the
    // other region's call created with the same token.
    for (region, expected) in regions.iter().zip(&first) {
        let (status, retried) =
            create_in(&router, region, json!({ "name": name, "requestId": token })).await;
        assert_eq!(
            status, 201,
            "case {region} retry: expected the retry to succeed, got {status}: {retried}"
        );
        assert_eq!(
            &retried, expected,
            "case {region} retry: expected the group this region's token created, got {retried}"
        );
    }
}

/// Rule: a read of a resource no stored resource in the request's region answers
/// to — by either identifier form — is the operation's modelled 404, not an empty
/// success.
#[tokio::test]
async fn reads_refuse_identifiers_no_group_answers_to() {
    let router = medialive();
    let (status, created) = create_in(
        &router,
        REGION,
        json!({ "name": "present-group", "requestId": "token-present" }),
    )
    .await;
    assert_eq!(status, 201, "the fixture create got {status}: {created}");
    let present_id = created["id"].as_str().unwrap_or_default().to_string();

    // A group in another region, to prove region scoping refuses rather than
    // leaks.
    let (status, elsewhere) =
        create_in(&router, "ap-south-1", json!({ "name": "other-region-group" })).await;
    assert_eq!(status, 201, "the other-region create got {status}: {elsewhere}");
    let elsewhere_id = elsewhere["id"].as_str().unwrap_or_default().to_string();

    // The stored group's id with one digit changed, chosen so the result is
    // neither the stored id nor any other id this test minted: a near-miss is a
    // miss in its own region, not a neighbour that happens to exist.
    let minted = [present_id.clone(), elsewhere_id.clone()];
    let (head, last) = present_id.split_at(present_id.len() - 1);
    let digit = last.parse::<u32>().expect("a group id is decimal digits");
    let near_miss = (1..10)
        .map(|shift| format!("{head}{}", (digit + shift) % 10))
        .find(|candidate| !minted.contains(candidate))
        .expect("ten digits leave one no minted id uses");

    // (case, identifier read in REGION)
    let table: [(&str, String); 6] = [
        ("an id no group has", "9999999".to_string()),
        ("a name no group has", "absent-group".to_string()),
        ("another region's id", elsewhere_id),
        ("another region's name", "other-region-group".to_string()),
        ("an id-shaped name", "1234567".to_string()),
        ("a near-miss id", near_miss),
    ];

    for (case, identifier) in table {
        let (status, body) = get_in(&router, REGION, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?} ({identifier:?}): expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?} ({identifier:?}): expected NotFoundException, got {body}"
        );
    }

    // The group that does exist is untouched by every refused read.
    let (status, read) = get_in(&router, REGION, &present_id).await;
    assert_eq!(status, 200, "the present group read got {status}: {read}");
    assert_eq!(
        read, created,
        "expected the refused reads to change nothing, got {read}"
    );
}

/// Rule: the store is bounded, so a create loop is refused with the operation's
/// modelled throttle rather than growing the emulator's heap.
#[tokio::test]
async fn the_group_store_is_bounded() {
    use service_com_amazonaws_medialive_medialive::state::MAX_ALARM_TEMPLATE_GROUPS;

    let router = medialive();
    for index in 0..MAX_ALARM_TEMPLATE_GROUPS {
        let (status, body) = create_in(&router, REGION, json!({ "name": format!("g-{index}") })).await;
        assert_eq!(
            status, 201,
            "case group {index}: expected 201, got {status}: {body}"
        );
    }

    let (status, body) = create_in(&router, REGION, json!({ "name": "one-too-many" })).await;
    assert_eq!(
        status, 429,
        "case {MAX_ALARM_TEMPLATE_GROUPS} stored: expected the create to be throttled, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "TooManyRequestsException",
        "case {MAX_ALARM_TEMPLATE_GROUPS} stored: expected TooManyRequestsException, got {body}"
    );

    // The refused create stored nothing.
    let (status, body) = call(&router, "GET", &format!("{GROUPS}/one-too-many"), None).await;
    assert_eq!(
        status, 404,
        "expected the throttled name to belong to no group, got {status}: {body}"
    );
}
