// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Resolver-on-Outpost slice: `CreateOutpostResolver`,
//! `GetOutpostResolver`, `ListOutpostResolvers`, `UpdateOutpostResolver` and
//! `DeleteOutpostResolver`.
//!
//! Every case goes over the wire the service actually speaks, through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::OUTPOST_RESOLVER_STATUS_VALUES;
use service_com_amazonaws_route53resolver_route53resolver::state::{
    mint_page_token, MAX_STORED_OUTPOST_RESOLVERS, OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT,
    PROVISIONING_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, emulator, error_type, expect_error, ok, parse_time, string_member, without_status,
    ACCOUNT, REGION,
};

/// The `OutpostResolver` of a create/get/update/delete response.
fn resolver(body: &Value) -> &Value {
    body.get("OutpostResolver")
        .unwrap_or_else(|| panic!("the response carries an OutpostResolver, got {body}"))
}

/// An `OutpostArn` satisfying the member's documented pattern
/// `^arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/op-[a-f0-9]{17}$`
/// (`smithy_query(query="shape",shape="OutpostResolver")`): `partition` is the
/// `aws…` partition, `region` the Outpost's region, and `tail` the 17 hex
/// characters of the Outpost id.
fn outpost_arn(partition: &str, region: &str, tail: &str) -> String {
    assert_eq!(
        tail.len(),
        17,
        "case {tail:?}: an Outpost id carries 17 hex characters"
    );
    format!("arn:{partition}:outposts:{region}:123456789012:outpost/op-{tail}")
}

/// The default Outpost every case that does not care which Outpost it is on
/// uses.
fn default_outpost() -> String {
    outpost_arn("aws", "us-west-2", "0123456789abcdef0")
}

/// A `CreateOutpostResolver` body on [`default_outpost`].
fn create_body(creator: &str, name: &str) -> Value {
    json!({
        "CreatorRequestId": creator,
        "Name": name,
        "PreferredInstanceType": "m5.large",
        "OutpostArn": default_outpost(),
    })
}

/// Create `count` Resolvers on the given Outposts (round-robin) and return their
/// ids in creation order.
async fn seed_resolvers(
    router: &MultiServiceRouter,
    count: usize,
    outposts: &[String],
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let mut body = create_body(&format!("req-seed-{index}"), &format!("seed-{index}"));
        body["OutpostArn"] = json!(outposts[index % outposts.len()]);
        let created = ok(router, "CreateOutpostResolver", body).await;
        ids.push(string_member(resolver(&created), "Id"));
    }
    ids
}

/// The `Id`s `ListOutpostResolvers` reports for one request body.
async fn listed_ids(router: &MultiServiceRouter, request: Value) -> Vec<String> {
    let page = ok(router, "ListOutpostResolvers", request).await;
    page.get("OutpostResolvers")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| string_member(item, "Id")).collect())
        .unwrap_or_default()
}

/// Wait out the span the clock owes a `CREATING` or `UPDATING` Resolver, so the
/// next call sees the settled `OPERATIONAL` status.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

fn i64_member(value: &Value, member: &str) -> i64 {
    value
        .get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("{member} must be an integer, got {value}"))
}

/// One row of the create table: the request, and the `InstanceCount` the stored
/// Resolver must then report.
#[derive(Debug)]
struct CreateCase {
    case: &'static str,
    body: Value,
    instance_count: i64,
}

/// A create is persisted: every member of the response is computed from the
/// stored record, a `GetOutpostResolver` returns that same record, and
/// `ListOutpostResolvers` reports it.
///
/// The table spans what the rule generalises over: `InstanceCount` absent (the
/// documented default), at its documented minimum and well above it; `Name` and
/// `PreferredInstanceType` at both ends of their modelled 1..255 length; an
/// Outpost in another partition and region; and `Tags` present or absent.
#[tokio::test]
async fn a_created_resolver_on_an_outpost_is_read_back_from_state() {
    let cases = vec![
        CreateCase {
            case: "InstanceCount omitted, so the documented default applies",
            body: create_body("req-default", "default-count"),
            instance_count: i64::from(OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT),
        },
        CreateCase {
            case: "InstanceCount at the documented minimum, one-character name, tags",
            body: json!({
                "CreatorRequestId": "req-minimum",
                "Name": "r",
                "InstanceCount": OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT,
                "PreferredInstanceType": "m",
                "OutpostArn": default_outpost(),
                "Tags": [{ "Key": "team", "Value": "networking" }],
            }),
            instance_count: i64::from(OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT),
        },
        CreateCase {
            case: "InstanceCount above the minimum",
            body: json!({
                "CreatorRequestId": "req-eight",
                "Name": "eight-instances",
                "InstanceCount": 8,
                "PreferredInstanceType": "c5.xlarge",
                "OutpostArn": default_outpost(),
            }),
            instance_count: 8,
        },
        CreateCase {
            case: "a Resolver on an Outpost in another partition and region",
            body: json!({
                "CreatorRequestId": "req-govcloud",
                "Name": "gov-resolver",
                "InstanceCount": 64,
                "PreferredInstanceType": "m5.4xlarge",
                "OutpostArn": outpost_arn("aws-us-gov", "us-gov-west-1", "abcdef01234567890"),
            }),
            instance_count: 64,
        },
        CreateCase {
            case: "Name and PreferredInstanceType at the modelled maximum length",
            body: json!({
                "CreatorRequestId": "req-long",
                "Name": "n".repeat(255),
                "InstanceCount": 16,
                "PreferredInstanceType": "t".repeat(255),
                "OutpostArn": default_outpost(),
            }),
            instance_count: 16,
        },
    ];

    for CreateCase {
        case,
        body,
        instance_count,
    } in cases
    {
        // Each row gets its own emulator, so the store holds exactly this row.
        let router = emulator();
        let requested = body.clone();

        let created = ok(&router, "CreateOutpostResolver", body).await;
        let created = resolver(&created).clone();

        let id = string_member(&created, "Id");
        assert!(
            id.starts_with("rslvr-op-"),
            "case {case:?}: Id must name a Resolver on an Outpost, got {id:?}"
        );
        assert_eq!(
            string_member(&created, "Arn"),
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:outpost-resolver/{id}"),
            "case {case:?}: the Arn must be rooted in the signed region and the emulator account"
        );
        assert_eq!(
            created.get("Name"),
            requested.get("Name"),
            "case {case:?}: Name must be the one requested, got {created}"
        );
        assert_eq!(
            created.get("CreatorRequestId"),
            requested.get("CreatorRequestId"),
            "case {case:?}: CreatorRequestId must be the one that created the Resolver, got {created}"
        );
        assert_eq!(
            created.get("PreferredInstanceType"),
            requested.get("PreferredInstanceType"),
            "case {case:?}: PreferredInstanceType must be the one requested, got {created}"
        );
        assert_eq!(
            created.get("OutpostArn"),
            requested.get("OutpostArn"),
            "case {case:?}: OutpostArn must be the Outpost requested, got {created}"
        );
        assert_eq!(
            i64_member(&created, "InstanceCount"),
            instance_count,
            "case {case:?}: InstanceCount must be the requested count, or the documented default when none was named, got {created}"
        );
        assert_eq!(
            string_member(&created, "Status"),
            "CREATING",
            "case {case:?}: a just-created Resolver is still CREATING, got {created}"
        );
        assert!(
            OUTPOST_RESOLVER_STATUS_VALUES.contains(&string_member(&created, "Status").as_str()),
            "case {case:?}: Status must be a value the model's OutpostResolverStatus declares, got {created}"
        );
        let creation = parse_time(case, "CreationTime", &string_member(&created, "CreationTime"));
        let modification = parse_time(
            case,
            "ModificationTime",
            &string_member(&created, "ModificationTime"),
        );
        assert_eq!(
            creation, modification,
            "case {case:?}: a never-updated Resolver was modified when it was created, got {created}"
        );

        // The read-back: the same record, through the implemented reader.
        let fetched = ok(&router, "GetOutpostResolver", json!({ "Id": id })).await;
        let fetched = resolver(&fetched).clone();
        assert_eq!(
            without_status(&fetched),
            without_status(&created),
            "case {case:?}: GetOutpostResolver must return the record CreateOutpostResolver stored"
        );

        // And the list reports exactly that record, both unfiltered and filtered
        // on the Outpost it sits on.
        assert_eq!(
            listed_ids(&router, json!({})).await,
            vec![id.clone()],
            "case {case:?}: ListOutpostResolvers must report exactly the stored Resolver"
        );
        assert_eq!(
            listed_ids(&router, json!({ "OutpostArn": requested.get("OutpostArn") })).await,
            vec![id.clone()],
            "case {case:?}: ListOutpostResolvers must report the Resolver on the Outpost it sits on"
        );
        let other_outpost = outpost_arn("aws", "eu-west-1", "99999999999999999");
        assert!(
            listed_ids(&router, json!({ "OutpostArn": other_outpost }))
                .await
                .is_empty(),
            "case {case:?}: ListOutpostResolvers must report nothing for an Outpost holding no Resolver"
        );
    }
}

/// `OutpostArn` selects the Resolvers on ONE Outpost, and the list is in
/// creation order whether or not it is filtered.
#[tokio::test]
async fn listing_selects_the_resolvers_on_the_requested_outpost() {
    let outposts = [
        outpost_arn("aws", "us-west-2", "0123456789abcdef0"),
        outpost_arn("aws", "us-west-2", "1111111111111111a"),
        outpost_arn("aws-cn", "cn-north-1", "22222222222222222"),
    ];
    let router = emulator();
    // Seven Resolvers round-robin over three Outposts, so the three selections
    // have different lengths and none of them is the whole store.
    let all = seed_resolvers(&router, 7, &outposts).await;

    assert_eq!(
        listed_ids(&router, json!({})).await,
        all,
        "an unfiltered list must report every stored Resolver in creation order"
    );

    for (index, outpost) in outposts.iter().enumerate() {
        let expected: Vec<String> = all
            .iter()
            .enumerate()
            .filter(|(position, _)| position % outposts.len() == index)
            .map(|(_, id)| id.clone())
            .collect();
        assert_eq!(
            listed_ids(&router, json!({ "OutpostArn": outpost })).await,
            expected,
            "case {outpost:?}: the list must report exactly the Resolvers on that Outpost, in creation order"
        );
    }
}

/// Paging walks every stored Resolver exactly once, whatever the page size: the
/// `NextToken` resumes where the previous page stopped, and the last page
/// carries none.
#[tokio::test]
async fn paging_walks_every_resolver_exactly_once() {
    let router = emulator();
    let expected = seed_resolvers(&router, 5, &[default_outpost()]).await;

    // Page sizes spanning the modelled MaxResults range (1..100): both published
    // bounds, sizes that do and do not divide the collection, exactly the
    // collection size, and MaxResults omitted altogether.
    for max_results in [Some(1), Some(2), Some(3), Some(5), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({});
            if let Some(size) = max_results {
                request["MaxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = ok(&router, "ListOutpostResolvers", request).await;
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );

            let items = page
                .get("OutpostResolvers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(size) = max_results {
                assert!(
                    i64::try_from(items.len()).unwrap_or(i64::MAX) <= i64::from(size),
                    "case {case:?}: a page must hold at most MaxResults items, got {page}"
                );
            }
            walked.extend(items.iter().map(|item| string_member(item, "Id")));

            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: paging must walk every stored Resolver exactly once, in creation order"
        );
    }
}

/// `MaxResults` is modelled `range 1..100`, so a request outside it is refused
/// at the boundary rather than clamped into a page the caller did not ask for.
///
/// The refusal is the boundary validator's, so only its HTTP status is asserted
/// here: a handler-returned error and a boundary one differ in the error code
/// they render (see `crate::scaffolded`).
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    seed_resolvers(&router, 2, &[default_outpost()]).await;

    for (case, max_results, accepted) in [
        ("one below the published minimum", 0, false),
        ("the published minimum", 1, true),
        ("the published maximum", 100, true),
        ("one above the published maximum", 101, false),
        ("far above the published maximum", 10_000, false),
    ] {
        let (status, body) = call(
            &router,
            "ListOutpostResolvers",
            json!({ "MaxResults": max_results }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: MaxResults={max_results} is inside the modelled range, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: MaxResults={max_results} is outside the modelled range, got {status}: {body}"
            );
        }
    }
}

/// A `NextToken` this service did not mint for THIS query is refused with the
/// `ValidationException` the operation models, rather than silently read as
/// "start over".
#[tokio::test]
async fn a_next_token_this_query_did_not_mint_is_refused() {
    let outposts = [
        outpost_arn("aws", "us-west-2", "0123456789abcdef0"),
        outpost_arn("aws", "us-west-2", "1111111111111111a"),
    ];
    let router = emulator();
    seed_resolvers(&router, 4, &outposts).await;

    // A cursor minted for the endpoint list is not one this list accepts, and a
    // bare "ListOutpostResolvers" scope is one no query ever mints (every query
    // folds its `OutpostArn` selection into the scope).
    let foreign_scope = mint_page_token("ListResolverEndpoints/0000000000000000", 1);
    let scope_no_query_mints = mint_page_token("ListOutpostResolvers", 1);

    for (case, token) in [
        ("not hex at all", "next-please".to_string()),
        ("empty", String::new()),
        (
            "hex, but not a cursor",
            hex::encode("ListOutpostResolvers:1"),
        ),
        (
            "right payload, wrong checksum",
            hex::encode("ListOutpostResolvers:1:0000000000000000"),
        ),
        ("a cursor for another collection", foreign_scope),
        ("a scope no query mints", scope_no_query_mints),
    ] {
        expect_error(
            &router,
            "ListOutpostResolvers",
            json!({ "NextToken": token }),
            case,
            "ValidationException",
        )
        .await;
    }

    // The cursor a query minted resumes THAT query, and is refused by a query
    // over a different selection — so the refusals above are about provenance
    // and not about rejecting every token.
    let first_page = ok(
        &router,
        "ListOutpostResolvers",
        json!({ "OutpostArn": outposts[0], "MaxResults": 1 }),
    )
    .await;
    let token = first_page
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("two Resolvers on one Outpost paged at 1 have a second page")
        .to_string();

    let resumed = listed_ids(
        &router,
        json!({ "OutpostArn": outposts[0], "MaxResults": 1, "NextToken": token.clone() }),
    )
    .await;
    assert_eq!(
        resumed.len(),
        1,
        "the cursor this query minted must resume it, got {resumed:?}"
    );
    for (case, request) in [
        (
            "resumed without the filter it was minted under",
            json!({ "MaxResults": 1, "NextToken": token.clone() }),
        ),
        (
            "resumed under a different filter",
            json!({ "OutpostArn": outposts[1], "MaxResults": 1, "NextToken": token }),
        ),
    ] {
        expect_error(
            &router,
            "ListOutpostResolvers",
            request,
            case,
            "ValidationException",
        )
        .await;
    }
}

/// A retried create with the same `CreatorRequestId` resolves to the Resolver
/// that request already made: "CreatorRequestId ... allows failed requests to be
/// retried without the risk of running the operation twice."
#[tokio::test]
async fn a_repeated_creator_request_id_does_not_create_a_second_resolver() {
    let router = emulator();
    let body = create_body("req-retried", "once-only");

    let first = ok(&router, "CreateOutpostResolver", body.clone()).await;
    let second = ok(&router, "CreateOutpostResolver", body).await;

    assert_eq!(
        without_status(resolver(&first)),
        without_status(resolver(&second)),
        "a retried create must return the Resolver the first one made"
    );
    assert_eq!(
        listed_ids(&router, json!({})).await.len(),
        1,
        "a retried create must not store a second Resolver"
    );
}

/// The Resolver AWS answers `CREATING` reaches `OPERATIONAL` on its own. The
/// transition is owed by the clock, not by the read: a caller that reads
/// immediately still sees the transient status, and every reporting operation
/// agrees on it.
#[tokio::test]
async fn the_creating_status_settles_without_anyone_asking() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateOutpostResolver",
        create_body("req-settles", "settling"),
    )
    .await;
    let id = string_member(resolver(&created), "Id");

    let immediate = ok(&router, "GetOutpostResolver", json!({ "Id": id.clone() })).await;
    assert_eq!(
        string_member(resolver(&immediate), "Status"),
        "CREATING",
        "a read taken straight after the create must still see CREATING, got {immediate}"
    );

    settle().await;

    let settled = ok(&router, "GetOutpostResolver", json!({ "Id": id.clone() })).await;
    assert_eq!(
        string_member(resolver(&settled), "Status"),
        "OPERATIONAL",
        "the Resolver must reach OPERATIONAL on its own, got {settled}"
    );
    let listed = ok(&router, "ListOutpostResolvers", json!({})).await;
    let listed_status = listed
        .get("OutpostResolvers")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .map(|item| string_member(item, "Status"))
        .unwrap_or_default();
    assert_eq!(
        listed_status, "OPERATIONAL",
        "the list must report the same settled status as the read, got {listed}"
    );
}

/// One row of the update table: the members the request carries, and whether
/// that request writes anything at all.
#[derive(Debug)]
struct UpdateCase {
    case: &'static str,
    patch: Value,
    writes: bool,
}

/// An update writes exactly the members the request names, leaves the others
/// alone, and moves the Resolver into `UPDATING` until the clock settles it. A
/// request that names none of them writes nothing.
#[tokio::test]
async fn an_update_writes_only_the_members_the_request_carries() {
    let cases = vec![
        UpdateCase {
            case: "Name only",
            patch: json!({ "Name": "renamed" }),
            writes: true,
        },
        UpdateCase {
            case: "InstanceCount only",
            patch: json!({ "InstanceCount": 8 }),
            writes: true,
        },
        UpdateCase {
            case: "PreferredInstanceType only",
            patch: json!({ "PreferredInstanceType": "m5.8xlarge" }),
            writes: true,
        },
        UpdateCase {
            case: "every updatable member at once",
            patch: json!({
                "Name": "fully-updated",
                "InstanceCount": 32,
                "PreferredInstanceType": "c5.24xlarge",
            }),
            writes: true,
        },
        UpdateCase {
            case: "no updatable member at all",
            patch: json!({}),
            writes: false,
        },
    ];

    for UpdateCase {
        case,
        patch,
        writes,
    } in cases
    {
        let router = emulator();
        let created = ok(
            &router,
            "CreateOutpostResolver",
            create_body("req-update", "before-update"),
        )
        .await;
        let created = resolver(&created).clone();
        let id = string_member(&created, "Id");
        // An update is only valid from a settled Resolver, so the row waits out
        // the span the create owes before asking for one.
        settle().await;

        let mut request = patch.clone();
        request["Id"] = json!(id.clone());
        let updated = ok(&router, "UpdateOutpostResolver", request).await;
        let updated = resolver(&updated).clone();

        assert_eq!(
            string_member(&updated, "Id"),
            id,
            "case {case:?}: an update must not change the Id, got {updated}"
        );
        assert_eq!(
            updated.get("Arn"),
            created.get("Arn"),
            "case {case:?}: an update must not change the Arn, got {updated}"
        );
        assert_eq!(
            updated.get("OutpostArn"),
            created.get("OutpostArn"),
            "case {case:?}: an update cannot move a Resolver to another Outpost, got {updated}"
        );
        assert_eq!(
            updated.get("CreationTime"),
            created.get("CreationTime"),
            "case {case:?}: an update must not change the CreationTime, got {updated}"
        );
        for member in ["Name", "PreferredInstanceType"] {
            let expected = patch.get(member).or_else(|| created.get(member));
            assert_eq!(
                updated.get(member),
                expected,
                "case {case:?}: {member} must be the updated value, or the stored one when the request omitted it, got {updated}"
            );
        }
        let expected_count = patch
            .get("InstanceCount")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| i64_member(&created, "InstanceCount"));
        assert_eq!(
            i64_member(&updated, "InstanceCount"),
            expected_count,
            "case {case:?}: InstanceCount must be the updated count, or the stored one when the request omitted it, got {updated}"
        );
        assert_eq!(
            string_member(&updated, "Status"),
            if writes { "UPDATING" } else { "OPERATIONAL" },
            "case {case:?}: a write must move the Resolver into UPDATING, and a request that writes nothing must leave it settled, got {updated}"
        );

        let creation = parse_time(case, "CreationTime", &string_member(&updated, "CreationTime"));
        let modification = parse_time(
            case,
            "ModificationTime",
            &string_member(&updated, "ModificationTime"),
        );
        if writes {
            assert!(
                modification >= creation,
                "case {case:?}: an update must not move ModificationTime backwards, got {updated}"
            );
        } else {
            assert_eq!(
                updated.get("ModificationTime"),
                created.get("ModificationTime"),
                "case {case:?}: a request that writes nothing must not move ModificationTime, got {updated}"
            );
        }

        // The update is persisted, and the UPDATING status settles on its own.
        let fetched = ok(&router, "GetOutpostResolver", json!({ "Id": id.clone() })).await;
        assert_eq!(
            without_status(resolver(&fetched)),
            without_status(&updated),
            "case {case:?}: GetOutpostResolver must return the record the update stored"
        );
        settle().await;
        let settled = ok(&router, "GetOutpostResolver", json!({ "Id": id })).await;
        assert_eq!(
            string_member(resolver(&settled), "Status"),
            "OPERATIONAL",
            "case {case:?}: an updated Resolver must settle back to OPERATIONAL, got {settled}"
        );
    }
}

/// `ConflictException`: "The requested state transition isn't valid." A Resolver
/// that has not settled yet cannot be updated — and the same request succeeds
/// once it has, so the refusal is about the transient state and not about
/// rejecting every update.
#[tokio::test]
async fn an_unsettled_resolver_cannot_be_updated() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateOutpostResolver",
        create_body("req-conflict", "conflicting"),
    )
    .await;
    let id = string_member(resolver(&created), "Id");

    expect_error(
        &router,
        "UpdateOutpostResolver",
        json!({ "Id": id.clone(), "Name": "too-soon" }),
        "while still CREATING",
        "ConflictException",
    )
    .await;

    settle().await;
    let updated = ok(
        &router,
        "UpdateOutpostResolver",
        json!({ "Id": id.clone(), "Name": "in-time" }),
    )
    .await;
    assert_eq!(
        string_member(resolver(&updated), "Name"),
        "in-time",
        "an update of a settled Resolver must be applied, got {updated}"
    );

    expect_error(
        &router,
        "UpdateOutpostResolver",
        json!({ "Id": id.clone(), "Name": "too-soon-again" }),
        "while still UPDATING",
        "ConflictException",
    )
    .await;

    // The refused update wrote nothing.
    settle().await;
    let fetched = ok(&router, "GetOutpostResolver", json!({ "Id": id })).await;
    assert_eq!(
        string_member(resolver(&fetched), "Name"),
        "in-time",
        "a refused update must leave the stored record alone, got {fetched}"
    );
}

/// `InstanceCount`'s "default and minimal value is 4": a smaller count is
/// refused, on the create path and on the update path, and the minimum itself is
/// accepted.
#[tokio::test]
async fn an_instance_count_below_the_documented_minimum_is_refused() {
    let minimum = OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT;
    for (case, instance_count, accepted) in [
        ("far below the minimum", -1, false),
        ("zero instances", 0, false),
        ("one instance", 1, false),
        ("one below the minimum", minimum - 1, false),
        ("exactly the minimum", minimum, true),
        ("above the minimum", minimum + 1, true),
    ] {
        let router = emulator();
        let mut body = create_body("req-count", "counted");
        body["InstanceCount"] = json!(instance_count);

        if !accepted {
            expect_error(
                &router,
                "CreateOutpostResolver",
                body,
                case,
                "ValidationException",
            )
            .await;
            assert!(
                listed_ids(&router, json!({})).await.is_empty(),
                "case {case:?}: a refused create must store nothing"
            );
            continue;
        }

        let created = ok(&router, "CreateOutpostResolver", body).await;
        let id = string_member(resolver(&created), "Id");
        assert_eq!(
            i64_member(resolver(&created), "InstanceCount"),
            i64::from(instance_count),
            "case {case:?}: an accepted InstanceCount must be stored as asked, got {created}"
        );

        settle().await;
        expect_error(
            &router,
            "UpdateOutpostResolver",
            json!({ "Id": id.clone(), "InstanceCount": minimum - 1 }),
            case,
            "ValidationException",
        )
        .await;
        let fetched = ok(&router, "GetOutpostResolver", json!({ "Id": id })).await;
        assert_eq!(
            i64_member(resolver(&fetched), "InstanceCount"),
            i64::from(instance_count),
            "case {case:?}: a refused update must leave InstanceCount alone, got {fetched}"
        );
    }
}

/// The service error code of the "this instance count is below the minimum"
/// fault, as recorded on AWS for the Resolver-on-Outpost write operations. It is
/// the per-fault discriminator: the HTTP status (400) and the exception shape
/// name are shared by the other `ValidationException` faults of those
/// operations.
const RSLVR_CODE_INSTANCE_COUNT: &str = "RSLVR-03002";

/// The bracketed service error code at the head of a Resolver exception
/// message, asserted to be of the service's `RSLVR-nnnnn` form.
fn rslvr_code(case: &str, message: &str) -> String {
    let tail = message.strip_prefix('[').unwrap_or_else(|| {
        panic!("case {case:?}: expected a bracketed service error code, got {message:?}")
    });
    let (code, sentence) = tail.split_once("] ").unwrap_or_else(|| {
        panic!("case {case:?}: expected \"[<code>] <sentence>\", got {message:?}")
    });
    let digits = code.strip_prefix("RSLVR-").unwrap_or_else(|| {
        panic!("case {case:?}: expected an RSLVR-nnnnn code, got {message:?}")
    });
    assert!(
        digits.len() == 5 && digits.chars().all(|c| c.is_ascii_digit()),
        "case {case:?}: expected five digits after RSLVR-, got {message:?}"
    );
    assert!(
        !sentence.is_empty() && sentence.ends_with('.'),
        "case {case:?}: expected a sentence ending in a period after the code, got {message:?}"
    );
    code.to_string()
}

/// Every Resolver-on-Outpost write refused for an `InstanceCount` below the
/// documented minimum states the fault with Route 53 Resolver's own service
/// error code inside the message, because the status and the exception shape
/// name alone do not say which fault occurred.
///
/// The table spans what the rule generalises over: the operation that enforces
/// the minimum (create and update), and the rejected count along its magnitude —
/// one below the minimum, one, zero, negative and the smallest `Integer` the
/// wire carries. The counts the minimum admits are checked too, so the refusal
/// is keyed on the bound and not on "an InstanceCount was sent".
#[tokio::test]
async fn an_instance_count_below_the_minimum_is_refused_with_its_rslvr_error_code() {
    let minimum = OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT;
    // The sentence states the documented minimum (`InstanceCount`: "The default
    // and minimal value is 4."), never the rejected count, so it is the same for
    // every row below.
    let expected_message = format!(
        r#"[{RSLVR_CODE_INSTANCE_COUNT}] The instance count must be equal to or greater than "{minimum}"."#
    );

    let router = emulator();
    // One settled Resolver, so an update case is refused for its count and not
    // for a missing record or an unsettled status.
    let created = ok(
        &router,
        "CreateOutpostResolver",
        create_body("req-rslvr-code", "coded"),
    )
    .await;
    let id = string_member(resolver(&created), "Id");
    settle().await;

    for (case, count) in [
        ("one below the minimum", minimum - 1),
        ("a single instance", 1),
        ("no instances", 0),
        ("a negative count", -7),
        ("the smallest Integer on the wire", i32::MIN),
    ] {
        let mut create = create_body(&format!("req-{count}"), "coded");
        create["InstanceCount"] = json!(count);
        let requests = [
            ("CreateOutpostResolver", create),
            (
                "UpdateOutpostResolver",
                json!({ "Id": id.clone(), "InstanceCount": count }),
            ),
        ];

        for (operation, body) in requests {
            let (status, parsed) = call(&router, operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: expected HTTP 400, got {status}: {parsed}"
            );
            assert!(
                error_type(&parsed).contains("ValidationException"),
                "case {case:?}/{operation}: expected ValidationException, got {parsed}"
            );

            let message = string_member(&parsed, "Message");
            let code = rslvr_code(&format!("{case}/{operation}"), &message);
            assert_eq!(
                code, RSLVR_CODE_INSTANCE_COUNT,
                "case {case:?}/{operation}: expected the instance-count service error code, got {message:?}"
            );
            assert_eq!(
                message, expected_message,
                "case {case:?}/{operation}: expected Message {expected_message:?}, got {message:?}"
            );
        }
    }

    // The refusal is keyed on the documented bound: the minimum itself, a count
    // above it and an absent count are all accepted, and the stored Resolver
    // reports the count the last accepted write asked for (an absent count
    // leaves the stored one alone).
    let mut stored = i64_member(resolver(&created), "InstanceCount");
    for (case, count) in [
        ("exactly the minimum", Some(minimum)),
        ("above the minimum", Some(minimum + 1)),
        ("no InstanceCount at all", None),
    ] {
        let mut body = json!({ "Id": id.clone(), "Name": "coded" });
        if let Some(count) = count {
            body["InstanceCount"] = json!(count);
            stored = i64::from(count);
        }
        let updated = ok(&router, "UpdateOutpostResolver", body).await;
        assert_eq!(
            i64_member(resolver(&updated), "InstanceCount"),
            stored,
            "case {case:?}: an accepted InstanceCount must be stored as asked, got {updated}"
        );
        settle().await;
    }

    // The code is a per-fault discriminator, so another fault of the same
    // resource does not answer with this one.
    let (_, parsed) = call(
        &router,
        "GetOutpostResolver",
        json!({ "Id": "rslvr-op-0000000000000000" }),
    )
    .await;
    let message = string_member(&parsed, "Message");
    assert!(
        !message.contains(RSLVR_CODE_INSTANCE_COUNT),
        "a missing-Resolver refusal must not carry the instance-count code, got {message:?}"
    );
}

/// A delete removes the record: it reports the Resolver as `DELETING`, and for
/// the next caller the Resolver is gone — from the reader, from the list and
/// from a second delete. Its `CreatorRequestId` is released with it.
#[tokio::test]
async fn a_deleted_resolver_is_gone_for_the_next_caller() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateOutpostResolver",
        create_body("req-deleted", "going-away"),
    )
    .await;
    let created = resolver(&created).clone();
    let id = string_member(&created, "Id");

    let deleted = ok(&router, "DeleteOutpostResolver", json!({ "Id": id.clone() })).await;
    let deleted = resolver(&deleted).clone();
    assert_eq!(
        string_member(&deleted, "Id"),
        id,
        "the delete must report the Resolver it removed, got {deleted}"
    );
    assert_eq!(
        string_member(&deleted, "Status"),
        "DELETING",
        "a deleted Resolver is on its way out, so it reports DELETING, got {deleted}"
    );

    for (case, operation, body) in [
        (
            "the reader",
            "GetOutpostResolver",
            json!({ "Id": id.clone() }),
        ),
        (
            "a second delete",
            "DeleteOutpostResolver",
            json!({ "Id": id.clone() }),
        ),
        (
            "an update",
            "UpdateOutpostResolver",
            json!({ "Id": id.clone(), "Name": "ghost" }),
        ),
    ] {
        expect_error(
            &router,
            operation,
            body,
            case,
            "ResourceNotFoundException",
        )
        .await;
    }
    assert!(
        listed_ids(&router, json!({})).await.is_empty(),
        "a deleted Resolver must be gone from the list"
    );

    // The `CreatorRequestId` is released with the record, so reusing it creates
    // a NEW Resolver rather than resolving to the deleted one.
    let recreated = ok(
        &router,
        "CreateOutpostResolver",
        create_body("req-deleted", "back-again"),
    )
    .await;
    let recreated_id = string_member(resolver(&recreated), "Id");
    assert_ne!(
        recreated_id, id,
        "a create reusing a released CreatorRequestId must mint a new Resolver, got {recreated}"
    );
    assert_eq!(
        listed_ids(&router, json!({})).await,
        vec![recreated_id],
        "the store must hold exactly the recreated Resolver"
    );
}

/// An `Id` the store holds no Resolver under is `ResourceNotFoundException` on
/// every operation that resolves one, whatever the id looks like.
#[tokio::test]
async fn an_unknown_id_is_not_found_everywhere() {
    let router = emulator();
    // One stored Resolver, so a not-found answer is about the id asked for and
    // not about an empty store.
    seed_resolvers(&router, 1, &[default_outpost()]).await;

    for unknown in [
        "rslvr-op-0000000000000000",
        "rslvr-op-ffffffffffffffff",
        "not-a-resolver-id",
        "r",
    ] {
        for (operation, body) in [
            ("GetOutpostResolver", json!({ "Id": unknown })),
            ("DeleteOutpostResolver", json!({ "Id": unknown })),
            (
                "UpdateOutpostResolver",
                json!({ "Id": unknown, "Name": "nowhere" }),
            ),
        ] {
            expect_error(
                &router,
                operation,
                body,
                unknown,
                "ResourceNotFoundException",
            )
            .await;
        }
    }
}

/// The store is bounded: once it holds `MAX_STORED_OUTPOST_RESOLVERS` Resolvers a
/// create is refused with the quota error the operation models, and a delete
/// frees a slot again.
#[tokio::test]
async fn the_outpost_resolver_store_is_bounded() {
    let router = emulator();
    let mut first_id = String::new();
    for index in 0..MAX_STORED_OUTPOST_RESOLVERS {
        let created = ok(
            &router,
            "CreateOutpostResolver",
            create_body(&format!("req-bound-{index}"), &format!("bound-{index}")),
        )
        .await;
        if index == 0 {
            first_id = string_member(resolver(&created), "Id");
        }
    }

    expect_error(
        &router,
        "CreateOutpostResolver",
        create_body("req-one-too-many", "one-too-many"),
        "one past the store bound",
        "ServiceQuotaExceededException",
    )
    .await;

    // A retry of a `CreatorRequestId` already stored allocates no record, so it
    // stays allowed at the bound.
    ok(
        &router,
        "CreateOutpostResolver",
        create_body("req-bound-0", "bound-0"),
    )
    .await;

    ok(
        &router,
        "DeleteOutpostResolver",
        json!({ "Id": first_id.clone() }),
    )
    .await;
    let created = ok(
        &router,
        "CreateOutpostResolver",
        create_body("req-after-delete", "after-delete"),
    )
    .await;
    assert_ne!(
        string_member(resolver(&created), "Id"),
        first_id,
        "a delete must free a slot for a new Resolver, got {created}"
    );
}
