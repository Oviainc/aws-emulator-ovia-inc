// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the SCOPE a policy store alias name lives in.
//!
//! The rule under test, from `CreatePolicyStoreAliasInput$aliasName`: "The name
//! must be unique within your Amazon Web Services account and Amazon Web
//! Services Region." An alias therefore belongs to the Region its creating
//! request was signed for: the same name is free in every other Region, a
//! reference to it resolves only from its own Region, and a read, a list or a
//! delete addressed to another Region neither sees nor touches it.
//!
//! Every assertion goes over the wire through the shared [`super::harness`],
//! signed for the Region the row means, so an alias is only in a Region if it
//! really reached state keyed under it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call_in, error_code, ok_in, router, store_with_validation_off_in as store_in,
};

/// The prefix every alias name carries, from
/// `CreatePolicyStoreAliasInput$aliasName`'s model documentation: "The alias name
/// must always be prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// A Cedar policy that parses, for the rows that need a policy in a store.
const STATEMENT: &str = "permit(principal == User::\"alice\", action == Action::\"view\", \
                         resource == Photo::\"vacation\");";

/// The ARN an alias of `alias_name` created in `region` must carry: the alias
/// name as the resource, under the account and the Region the request was signed
/// for. The same shape `tests/all/policy_store_alias.rs` pins for one Region.
fn expected_alias_arn(region: &str, alias_name: &str) -> String {
    format!("arn:aws:verifiedpermissions:{region}:{ACCOUNT_ID}:{alias_name}")
}

/// Create an alias in `region`, returning `(status, body)` so a row can assert
/// either outcome.
async fn create_alias(
    router: &MultiServiceRouter,
    region: &str,
    alias_name: &str,
    store_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
    )
    .await
}

/// Create an alias in `region`, requiring success.
async fn create_alias_ok(
    router: &MultiServiceRouter,
    region: &str,
    alias_name: &str,
    store_id: &str,
) -> Value {
    ok_in(
        router,
        region,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
    )
    .await
}

/// The `aliasName`s `ListPolicyStoreAliases` reports in `region`, over every
/// page, in the order they arrived.
async fn listed_names(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "maxResults": 50 });
        if let Some(cursor) = &token {
            body["nextToken"] = json!(cursor);
        }
        let page = ok_in(router, region, "ListPolicyStoreAliases", &body).await;
        let items = page["policyStoreAliases"]
            .as_array()
            .unwrap_or_else(|| panic!("policyStoreAliases is required, got {page}"))
            .clone();
        names.extend(
            items
                .iter()
                .map(|item| item["aliasName"].as_str().unwrap_or_default().to_string()),
        );
        match page["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
}

// ---------------------------------------------------------------------------
// One name, one store per Region
// ---------------------------------------------------------------------------

/// One alias name names a DIFFERENT store in each Region: every Region admits
/// the name, reports its own store and ARN for it, resolves a
/// `policy-store-alias/<name>` reference to its own store, and lists its own
/// alias only.
///
/// The rows differ along how many Regions hold the name and which Regions they
/// are — Region codes of the public `<partition>-<area>[-<area>]-<number>` form,
/// including a three-word code and the two Regions outside the main partition.
#[tokio::test]
async fn one_name_names_a_different_store_in_each_region() {
    let rows: &[(&str, &[&str])] = &[
        ("two regions", &["us-west-2", "eu-west-1"]),
        (
            "three regions",
            &["us-east-1", "ap-southeast-3", "eu-central-1"],
        ),
        (
            "five regions across partitions",
            &[
                "us-east-1",
                "us-west-2",
                "ap-northeast-1",
                "cn-north-1",
                "us-gov-west-1",
            ],
        ),
    ];

    for (case, regions) in rows {
        let router = router();
        let alias_name = format!("{ALIAS_PREFIX}shared");

        // One store per Region, and the SAME alias name created in each: a name
        // already held in another Region must not refuse the create.
        let mut stores = Vec::new();
        for region in *regions {
            let store_id = store_in(&router, region).await;
            let created = create_alias_ok(&router, region, &alias_name, &store_id).await;
            assert_eq!(
                created["aliasArn"].as_str().unwrap_or_default(),
                expected_alias_arn(region, &alias_name),
                "case {case:?} in {region}: the alias ARN must name the region the request \
                 was signed for, got {created}"
            );
            assert_eq!(
                created["policyStoreId"].as_str().unwrap_or_default(),
                store_id,
                "case {case:?} in {region}: unexpected policyStoreId, got {created}"
            );
            stores.push(store_id);
        }

        for (region, store_id) in regions.iter().zip(stores.iter()) {
            // The read reports the alias of THIS Region.
            let read = ok_in(
                &router,
                region,
                "GetPolicyStoreAlias",
                &json!({ "aliasName": alias_name }),
            )
            .await;
            assert_eq!(
                read["policyStoreId"].as_str().unwrap_or_default(),
                *store_id,
                "case {case:?} in {region}: the alias must report this region's store, \
                 got {read}"
            );
            assert_eq!(
                read["aliasArn"].as_str().unwrap_or_default(),
                expected_alias_arn(region, &alias_name),
                "case {case:?} in {region}: the alias ARN must name this region, got {read}"
            );

            // And a `policyStoreId` reference carrying the name resolves to it.
            let resolved = ok_in(
                &router,
                region,
                "GetPolicyStore",
                &json!({ "policyStoreId": alias_name }),
            )
            .await;
            assert_eq!(
                resolved["policyStoreId"].as_str().unwrap_or_default(),
                *store_id,
                "case {case:?} in {region}: the alias reference must resolve to this \
                 region's store, got {resolved}"
            );

            // The list is the aliases of the Region it is signed for: one here,
            // whatever the other Regions hold under the same name.
            assert_eq!(
                listed_names(&router, region).await,
                vec![alias_name.clone()],
                "case {case:?} in {region}: the list must report only this region's aliases"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A name only conflicts in the Region that holds it
// ---------------------------------------------------------------------------

/// A name held by an alias of another store conflicts in the Region that holds
/// it, and only there: the same name for the same other store is accepted in
/// every other Region.
///
/// The rows differ along the state the holding alias is in — `Active`, and
/// `PendingDeletion`, which `DeletePolicyStoreAlias` documents as also keeping
/// the name taken — and along the Region pair.
#[tokio::test]
async fn a_name_conflicts_only_in_the_region_that_holds_it() {
    // (case, the region holding the name, the deletionMode applied to the holder
    // (none leaves it Active), the other region)
    let rows: &[(&str, &str, Option<&str>, &str)] = &[
        ("active holder", "us-west-2", None, "eu-west-1"),
        (
            "holder pending deletion",
            "us-west-2",
            Some("SoftDelete"),
            "eu-west-1",
        ),
        ("other partition", "cn-north-1", None, "us-east-1"),
        (
            "three-word region holds it",
            "ap-southeast-3",
            None,
            "us-gov-west-1",
        ),
    ];

    for (case, holding_region, deletion_mode, other_region) in rows {
        let router = router();
        let alias_name = format!("{ALIAS_PREFIX}contested");
        let holder_store = store_in(&router, holding_region).await;
        let challenger_store = store_in(&router, holding_region).await;
        create_alias_ok(&router, holding_region, &alias_name, &holder_store).await;
        if let Some(mode) = deletion_mode {
            ok_in(
                &router,
                holding_region,
                "DeletePolicyStoreAlias",
                &json!({ "aliasName": alias_name, "deletionMode": mode }),
            )
            .await;
        }

        // In the Region that holds the name, another store cannot take it.
        let (status, body) =
            create_alias(&router, holding_region, &alias_name, &challenger_store).await;
        assert_eq!(
            status, 409,
            "case {case:?}: the name is taken in {holding_region}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code, got {body}"
        );

        // In another Region the same name is free, for that same store.
        let other_store = store_in(&router, other_region).await;
        let created = create_alias_ok(&router, other_region, &alias_name, &other_store).await;
        assert_eq!(
            created["aliasArn"].as_str().unwrap_or_default(),
            expected_alias_arn(other_region, &alias_name),
            "case {case:?}: the name must be free in {other_region}, got {created}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every reader is scoped to its own Region
// ---------------------------------------------------------------------------

/// An alias of a Region resolves in that Region only: every operation that reads
/// an `aliasName` or resolves a `policy-store-alias/<name>` `policyStoreId`
/// answers not-found when it is addressed to another Region.
///
/// The rows differ along the OPERATION and the member that carries the
/// reference: the alias reader itself, and a store reader, a store writer, a
/// list and an authorization call reached through a `policyStoreId`.
#[tokio::test]
async fn an_alias_resolves_only_in_its_own_region() {
    let home_region = "us-west-2";
    let foreign_region = "eu-west-1";

    let router = router();
    let alias_name = format!("{ALIAS_PREFIX}home");
    let store_id = store_in(&router, home_region).await;
    create_alias_ok(&router, home_region, &alias_name, &store_id).await;
    // A policy in the store, so the rows that read through the alias would have
    // something to report if the reference did resolve.
    ok_in(
        &router,
        home_region,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement": STATEMENT } },
        }),
    )
    .await;

    // (case, operation, the request body naming the alias)
    let rows: &[(&str, &str, Value)] = &[
        (
            "the alias reader",
            "GetPolicyStoreAlias",
            json!({ "aliasName": alias_name }),
        ),
        (
            "a store reader",
            "GetPolicyStore",
            json!({ "policyStoreId": alias_name }),
        ),
        (
            "a schema reader",
            "GetSchema",
            json!({ "policyStoreId": alias_name }),
        ),
        (
            "a list inside the store",
            "ListPolicies",
            json!({ "policyStoreId": alias_name }),
        ),
        (
            "a store writer",
            "UpdatePolicyStore",
            json!({
                "policyStoreId": alias_name,
                "validationSettings": { "mode": "OFF" },
            }),
        ),
        (
            "an authorization call",
            "IsAuthorized",
            json!({
                "policyStoreId": alias_name,
                "principal": { "entityType": "User", "entityId": "alice" },
                "action": { "actionType": "Action", "actionId": "view" },
                "resource": { "entityType": "Photo", "entityId": "vacation" },
            }),
        ),
    ];

    for (case, operation, body) in rows {
        // In its own Region the reference resolves, so the row is about the
        // Region and not about a request the service refuses anyway.
        let (home_status, home_body) = call_in(&router, home_region, operation, body).await;
        assert_eq!(
            home_status, 200,
            "case {case:?}: the alias must resolve in {home_region}, got {home_status} \
             {home_body}"
        );

        let (status, answer) = call_in(&router, foreign_region, operation, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the alias of {home_region} must not resolve in \
             {foreign_region}, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {answer}"
        );
    }

    // The batch reader reports the same miss per item, as its own
    // `POLICY_STORE_ALIAS_NOT_FOUND` code rather than as a failed call.
    let batch = ok_in(
        &router,
        foreign_region,
        "BatchGetPolicy",
        &json!({
            "requests": [
                { "policyStoreId": alias_name, "policyId": "SP0000000000000000000000" },
            ],
        }),
    )
    .await;
    let codes: Vec<&str> = batch["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("errors is required, got {batch}"))
        .iter()
        .map(|error| error["code"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        codes,
        vec!["POLICY_STORE_ALIAS_NOT_FOUND"],
        "the batch item naming another region's alias must miss on the alias, got {batch}"
    );

    // And the foreign Region lists nothing at all.
    assert!(
        listed_names(&router, foreign_region).await.is_empty(),
        "{foreign_region} holds no alias, so its list must be empty"
    );
}

// ---------------------------------------------------------------------------
// A list cursor belongs to the Region that minted it
// ---------------------------------------------------------------------------

/// A `ListPolicyStoreAliases` cursor names a position among ONE Region's alias
/// names, so it resumes nothing in another Region's listing and is refused there
/// as a cursor that listing did not issue.
///
/// The rows differ along how many aliases the minting Region holds past the page
/// it hands back, and along the page size that produces the cursor.
#[tokio::test]
async fn a_list_cursor_does_not_resume_another_regions_listing() {
    // (case, aliases in the minting region, the asked-for page size)
    let rows: &[(&str, usize, i64)] = &[
        ("one alias past the page", 3, 2),
        ("several pages", 9, 2),
        ("the smallest page", 2, 1),
    ];

    for (case, held, page_size) in rows {
        let router = router();
        let minting_region = "us-west-2";
        let other_region = "eu-west-1";
        let store_id = store_in(&router, minting_region).await;
        for i in 0..*held {
            create_alias_ok(
                &router,
                minting_region,
                &format!("{ALIAS_PREFIX}paged-{i:03}"),
                &store_id,
            )
            .await;
        }
        // The other Region holds aliases of its own, so a cursor accepted there
        // would silently truncate a real listing rather than find nothing.
        let other_store = store_in(&router, other_region).await;
        for i in 0..*held {
            create_alias_ok(
                &router,
                other_region,
                &format!("{ALIAS_PREFIX}paged-{i:03}"),
                &other_store,
            )
            .await;
        }

        let page = ok_in(
            &router,
            minting_region,
            "ListPolicyStoreAliases",
            &json!({ "maxResults": page_size }),
        )
        .await;
        let cursor = page["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a region holding more than one page must hand back a cursor, got {page}")
            })
            .to_string();

        let (status, body) = call_in(
            &router,
            other_region,
            "ListPolicyStoreAliases",
            &json!({ "maxResults": page_size, "nextToken": cursor }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: another region's cursor must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );

        // In its own Region it still resumes, so the row is about the Region and
        // not about a cursor this service never minted.
        let resumed = ok_in(
            &router,
            minting_region,
            "ListPolicyStoreAliases",
            &json!({ "maxResults": page_size, "nextToken": cursor }),
        )
        .await;
        assert!(
            !resumed["policyStoreAliases"]
                .as_array()
                .unwrap_or(&Vec::new())
                .is_empty(),
            "case {case:?}: the cursor must resume its own region's listing, got {resumed}"
        );
    }
}

// ---------------------------------------------------------------------------
// A delete is scoped to its own Region
// ---------------------------------------------------------------------------

/// Deleting an alias in one Region leaves the alias of the same name in another
/// Region untouched: a soft delete pends only its own Region's alias, and a hard
/// delete frees the name only there.
///
/// The rows differ along the `deletionMode` the first Region applies, and
/// therefore along what each Region's alias looks like afterwards.
#[tokio::test]
async fn a_delete_only_reaches_its_own_region() {
    // (case, deletionMode, the `state` the deleted region's alias is left in —
    // `None` when the record is gone)
    let rows: &[(&str, Option<&str>, Option<&str>)] = &[
        ("soft delete pends it", Some("SoftDelete"), Some("PendingDeletion")),
        ("the default mode is a soft delete", None, Some("PendingDeletion")),
        ("hard delete removes it", Some("HardDelete"), None),
    ];

    for (case, deletion_mode, expected_state) in rows {
        let router = router();
        let deleted_region = "us-west-2";
        let kept_region = "eu-west-1";
        let alias_name = format!("{ALIAS_PREFIX}paired");
        let deleted_store = store_in(&router, deleted_region).await;
        let kept_store = store_in(&router, kept_region).await;
        create_alias_ok(&router, deleted_region, &alias_name, &deleted_store).await;
        create_alias_ok(&router, kept_region, &alias_name, &kept_store).await;

        let mut delete_body = json!({ "aliasName": alias_name });
        if let Some(mode) = deletion_mode {
            delete_body["deletionMode"] = json!(mode);
        }
        ok_in(
            &router,
            deleted_region,
            "DeletePolicyStoreAlias",
            &delete_body,
        )
        .await;

        // The deleted Region's alias is in the state the mode asks for.
        let (status, read) = call_in(
            &router,
            deleted_region,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        match expected_state {
            Some(state) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a pending alias is still readable, got {status} {read}"
                );
                assert_eq!(
                    read["state"].as_str().unwrap_or_default(),
                    *state,
                    "case {case:?}: unexpected state in {deleted_region}, got {read}"
                );
            }
            None => assert_eq!(
                status, 404,
                "case {case:?}: a hard-deleted alias is gone, got {status} {read}"
            ),
        }

        // The other Region's alias is untouched: still `Active`, still its own
        // store, and still resolving as a `policyStoreId`.
        let kept = ok_in(
            &router,
            kept_region,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        assert_eq!(
            kept["state"].as_str().unwrap_or_default(),
            "Active",
            "case {case:?}: {kept_region}'s alias must be untouched, got {kept}"
        );
        assert_eq!(
            kept["policyStoreId"].as_str().unwrap_or_default(),
            kept_store,
            "case {case:?}: {kept_region}'s alias must still name its own store, got {kept}"
        );
        let resolved = ok_in(
            &router,
            kept_region,
            "GetPolicyStore",
            &json!({ "policyStoreId": alias_name }),
        )
        .await;
        assert_eq!(
            resolved["policyStoreId"].as_str().unwrap_or_default(),
            kept_store,
            "case {case:?}: {kept_region}'s alias must still resolve, got {resolved}"
        );
        assert_eq!(
            listed_names(&router, kept_region).await,
            vec![alias_name.clone()],
            "case {case:?}: {kept_region} must still list its own alias"
        );
    }
}
