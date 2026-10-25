// SPDX-License-Identifier: CC0-1.0

//! `ListExtensionAssociations` as a paginated report over the extension associations one
//! account holds.
//!
//! The rules under test:
//!
//! * the listing reports exactly the associations this account created, each exactly once,
//!   each summary read off the record `GetExtensionAssociation` reports — in an order that
//!   is stable across calls, cut into pages of at most `MaxResults` entries, with a
//!   `NextToken` handed back only when entries are left over and resuming the traversal
//!   where the previous page ended;
//! * every filter member NARROWS that report to the associations it addresses:
//!   `ExtensionIdentifier` (a name, an id or a version ARN) to those binding that
//!   extension, `ExtensionVersionNumber` to those binding that version, and
//!   `ResourceIdentifier` to those bound to the resource its ARN addresses — a filter value
//!   nothing is associated to reporting nothing rather than refusing;
//! * reporting changes nothing a later caller sees: the unfiltered listing is unchanged by
//!   any number of reads of it.
//!
//! The tables span what those rules generalise over: HOW MANY associations the account
//! holds (none, one, several, and more than one page), WHICH page size the request asks for
//! (absent, both published bounds of `MaxResults` and values in between), WHICH of the
//! three resource kinds an association binds, which half of the extension catalogue it
//! comes from and in which identifier form (name, id, version ARN), and how many versions
//! the extension holds. Every assertion names its case and prints the actual value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    AWS_AUTHORED_EXTENSIONS, MAX_LIST_PAGE_SIZE,
};

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, Answer,
};
use crate::extension_associations::{associate, get_association};
use crate::extensions::{actions, create_extension, get_extension, minted_id};
use crate::resource_tags::{arn, pct};

/// `GET /extensionassociations` with `query` as the raw query string (empty for a request
/// that names no filter and no page at all). Every input member of this operation is an
/// `httpQuery`: `resource_identifier`, `extension_identifier`,
/// `extension_version_number`, `max_results` and `next_token`.
async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/extensionassociations".to_string()
    } else {
        format!("/extensionassociations?{query}")
    };
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The `Items` of a listing, asserted to be a 200 (an answer with no `Items` at all reads
/// as empty, which is the shape a fresh account would answer with).
fn items(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListExtensionAssociations expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id` of every entry of a listing, in the order it was served.
fn listed_ids(case: &str, answer: &Answer) -> Vec<String> {
    items(case, answer)
        .iter()
        .map(|item| {
            item.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed association must carry an Id, got {item}")
                })
                .to_string()
        })
        .collect()
}

/// Create one extension named `name` at `version` (appending to the version history when
/// `version` is above 1) and answer its minted id. It declares no parameter, so an
/// association to it needs no values.
async fn extension(router: &MultiServiceRouter, name: &str, version: i32) -> String {
    let body = json!({
        "Name": name,
        "Actions": actions(&[ACTION_POINT_VALUES[0]]),
    });
    let created = create_extension(router, &body, (version > 1).then_some(version - 1)).await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateExtension expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    minted_id(name, &created.json())
}

/// The ARN of one VERSION of an extension, read back through `GetExtension` rather than
/// spelled here, so a filter row addresses the ARN the service actually minted.
async fn version_arn(router: &MultiServiceRouter, identifier: &str, version: i32) -> String {
    let read = get_extension(router, identifier, Some(version)).await;
    assert_eq!(
        read.status,
        200,
        "GetExtension({identifier}, {version}) expected 200, got {} / {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    read.string("Arn")
        .unwrap_or_else(|| panic!("GetExtension({identifier}, {version}) answered no Arn"))
}

/// Associate `extension_identifier` to `resource_identifier` and answer the association's
/// minted id. `version` pins a version when the row states one.
async fn association(
    router: &MultiServiceRouter,
    extension_identifier: &str,
    resource_identifier: &str,
    version: Option<i32>,
) -> String {
    let mut body = json!({
        "ExtensionIdentifier": extension_identifier,
        "ResourceIdentifier": resource_identifier,
    });
    if let Some(version) = version {
        body["ExtensionVersionNumber"] = Value::from(version);
    }
    let created = associate(router, &body).await;
    assert_eq!(
        created.status,
        201,
        "CreateExtensionAssociation({extension_identifier} -> {resource_identifier}) expected \
         201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("CreateExtensionAssociation answered no Id: {}", created.json()))
}

/// The AWS-authored extension that declares no parameter at all: read from the service's
/// published catalogue, so a case cannot drift from it by hand-copying a name.
fn authored_without_parameter() -> &'static str {
    AWS_AUTHORED_EXTENSIONS
        .iter()
        .find(|authored| authored.parameters.is_empty())
        .map(|authored| authored.name)
        .expect("the published catalogue declares at least one extension with no parameter")
}

/// One application, with an environment and a configuration profile under it: the three
/// resource kinds an extension may be associated to, each as the ARN that addresses it.
struct Tree {
    application_arn: String,
    environment_arn: String,
    profile_arn: String,
}

impl Tree {
    /// The three ARNs in a fixed order, so a row can pick one by index.
    fn resources(&self) -> [&str; 3] {
        [
            &self.application_arn,
            &self.environment_arn,
            &self.profile_arn,
        ]
    }
}

async fn tree(router: &MultiServiceRouter, name: &str) -> Tree {
    let application = create_application(router, &format!("{name}-app")).await;
    let environment = create_environment(router, &application, &format!("{name}-env")).await;
    let profile = create_profile(router, &application, &format!("{name}-profile")).await;
    Tree {
        application_arn: arn(&format!("application/{application}")),
        environment_arn: arn(&format!("application/{application}/environment/{environment}")),
        profile_arn: arn(&format!(
            "application/{application}/configurationprofile/{profile}"
        )),
    }
}

/// Walk the whole listing one page at a time and answer the entries in the order they were
/// served, asserting the per-page half of the rule as it goes: a page never exceeds the
/// size the request asked for, and a page carrying a `NextToken` is exactly that size,
/// because a traversal is only cut where a page filled up.
async fn walk(router: &MultiServiceRouter, case: &str, max_results: Option<i32>) -> Vec<Value> {
    let page_size = max_results.map_or(MAX_LIST_PAGE_SIZE, |size| size as usize);
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a listing that never ends its traversal fails instead of hanging.
    for _ in 0..=(MAX_LIST_PAGE_SIZE * 4 + 2) {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = max_results {
            query.push(format!("max_results={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("next_token={token}"));
        }
        let answer = list(router, &query.join("&")).await;
        let page = items(case, &answer);
        assert!(
            page.len() <= page_size,
            "case {case:?}: a page holds at most {page_size} entries, got {page:?}"
        );
        token = answer.string("NextToken");
        if token.is_some() {
            assert_eq!(
                page.len(),
                page_size,
                "case {case:?}: a page carrying a NextToken is a full page of {page_size}, got \
                 {page:?}"
            );
        }
        collected.extend(page);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: the paginated walk did not terminate");
}

/// The listing reports every association the account holds, each exactly once, as the same
/// record `GetExtensionAssociation` answers for it — whatever page size the walk uses.
///
/// Rows differ along how many associations the account holds (none, one, several, and more
/// than a default page) and the page size the walk asks for: both published bounds of
/// `MaxResults` (1 and 50), values in between, and no `MaxResults` at all. Each association
/// of a row binds a different one of the three resource kinds and comes from a different
/// half of the extension catalogue, so a listing that reported only one kind fails here.
#[tokio::test]
async fn a_listing_reports_every_stored_association_once_whatever_the_page_size() {
    let cases: Vec<(usize, Option<i32>)> = vec![
        (0, None),
        (0, Some(1)),
        (1, Some(1)),
        (4, Some(2)),
        (7, Some(3)),
        (6, Some(MAX_LIST_PAGE_SIZE as i32)),
        (5, None),
        // More than one default page, so the traversal is cut where a page fills up.
        (MAX_LIST_PAGE_SIZE + 3, None),
    ];

    for (count, max_results) in cases {
        let case = format!("{count} associations at page size {max_results:?}");
        let router = router();
        let tree = tree(&router, "listed").await;
        let created_id = extension(&router, "listed-extension", 1).await;
        let authored = authored_without_parameter().to_string();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            // Alternating ends of the catalogue and all three resource kinds.
            let identifier = if index % 2 == 0 {
                created_id.as_str()
            } else {
                authored.as_str()
            };
            let resource = tree.resources()[index % 3].to_string();
            expected.push(association(&router, identifier, &resource, None).await);
        }
        // Association ids are minted, not created in listing order, so the expectation is
        // sorted the way the listing orders its report.
        expected.sort();

        let walked = walk(&router, &case, max_results).await;
        let ids: Vec<String> = walked
            .iter()
            .map(|item| {
                item.get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: a summary carried no Id: {item}"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected every stored association once, in listing order"
        );

        // Each summary is the stored record, narrowed to the members
        // `ExtensionAssociationSummary` carries: a listing that recomputed a value would
        // differ from the reader here.
        for summary in &walked {
            let id = summary
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a summary carried no Id: {summary}"));
            let read = get_association(&router, id).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: GetExtensionAssociation({id}) expected 200, got {} / {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            for member in ["Id", "ExtensionArn", "ResourceArn"] {
                assert_eq!(
                    summary.get(member).cloned(),
                    read.json().get(member).cloned(),
                    "case {case:?}: the summary's {member} must be the stored one, got {summary}"
                );
            }
        }
    }
}

/// Every filter member narrows the listing to the associations it addresses, and a filter
/// value nothing is associated to reports nothing at all.
///
/// Rows differ along WHICH filter is applied and in WHICH identifier form: the extension by
/// name, by minted id and by the ARN of one of its versions (which addresses that version),
/// the version number on its own and beside an extension, each of the three resource kinds
/// by ARN, and two filters together. The last rows are values nothing is associated to: an
/// extension name nobody created, an ARN addressing a resource that does not exist, an ARN
/// of another service entirely, and a version number nobody bound.
#[tokio::test]
async fn every_filter_narrows_the_listing_to_the_associations_it_addresses() {
    let router = router();
    let tree = tree(&router, "filtered").await;
    let listed = extension(&router, "filtered-extension", 1).await;
    // A second version of the same extension, so a version filter has something to pick
    // between.
    extension(&router, "filtered-extension", 2).await;
    let other = extension(&router, "other-extension", 1).await;
    let authored = authored_without_parameter().to_string();
    let listed_v1_arn = version_arn(&router, &listed, 1).await;
    let listed_v2_arn = version_arn(&router, &listed, 2).await;

    let on_application = association(&router, &listed, &tree.application_arn, Some(1)).await;
    let on_environment = association(&router, &listed, &tree.environment_arn, Some(1)).await;
    let on_profile = association(&router, &listed, &tree.profile_arn, Some(2)).await;
    let other_on_application = association(&router, &other, &tree.application_arn, None).await;
    let authored_on_environment = association(&router, &authored, &tree.environment_arn, None).await;

    let all = {
        let mut ids = vec![
            on_application.clone(),
            on_environment.clone(),
            on_profile.clone(),
            other_on_application.clone(),
            authored_on_environment.clone(),
        ];
        ids.sort();
        ids
    };

    let table: Vec<(String, String, Vec<String>)> = vec![
        (
            "the extension by name".to_string(),
            "extension_identifier=filtered-extension".to_string(),
            vec![
                on_application.clone(),
                on_environment.clone(),
                on_profile.clone(),
            ],
        ),
        (
            "the extension by its minted id".to_string(),
            format!("extension_identifier={}", pct(&listed)),
            vec![
                on_application.clone(),
                on_environment.clone(),
                on_profile.clone(),
            ],
        ),
        (
            // An extension ARN addresses ONE version, so it narrows to that version too.
            "the extension by the ARN of its first version".to_string(),
            format!("extension_identifier={}", pct(&listed_v1_arn)),
            vec![on_application.clone(), on_environment.clone()],
        ),
        (
            "the extension by the ARN of its second version".to_string(),
            format!("extension_identifier={}", pct(&listed_v2_arn)),
            vec![on_profile.clone()],
        ),
        (
            "a version number beside the extension name".to_string(),
            "extension_identifier=filtered-extension&extension_version_number=1".to_string(),
            vec![on_application.clone(), on_environment.clone()],
        ),
        (
            "the other version beside the extension name".to_string(),
            "extension_identifier=filtered-extension&extension_version_number=2".to_string(),
            vec![on_profile.clone()],
        ),
        (
            "a version number on its own".to_string(),
            "extension_version_number=2".to_string(),
            vec![on_profile.clone()],
        ),
        (
            "the application by ARN".to_string(),
            format!("resource_identifier={}", pct(&tree.application_arn)),
            vec![on_application.clone(), other_on_application.clone()],
        ),
        (
            "the environment by ARN".to_string(),
            format!("resource_identifier={}", pct(&tree.environment_arn)),
            vec![on_environment.clone(), authored_on_environment.clone()],
        ),
        (
            "the configuration profile by ARN".to_string(),
            format!("resource_identifier={}", pct(&tree.profile_arn)),
            vec![on_profile.clone()],
        ),
        (
            "a resource and an extension together".to_string(),
            format!(
                "resource_identifier={}&extension_identifier={}",
                pct(&tree.application_arn),
                pct(&other)
            ),
            vec![other_on_application.clone()],
        ),
        (
            "an AWS-authored extension by name".to_string(),
            format!("extension_identifier={}", pct(&authored)),
            vec![authored_on_environment.clone()],
        ),
        (
            "no filter at all".to_string(),
            String::new(),
            all.clone(),
        ),
        (
            "an extension name nobody created".to_string(),
            "extension_identifier=nobody-created-this".to_string(),
            Vec::new(),
        ),
        (
            "an ARN addressing an application that does not exist".to_string(),
            format!("resource_identifier={}", pct(&arn("application/nosuch"))),
            Vec::new(),
        ),
        (
            "an ARN of another service entirely".to_string(),
            format!(
                "resource_identifier={}",
                pct("arn:aws:lambda:us-east-1:123456789012:function:configuration-hook")
            ),
            Vec::new(),
        ),
        (
            "a version number nobody bound".to_string(),
            "extension_version_number=9".to_string(),
            Vec::new(),
        ),
        (
            "an extension and a version it was never bound at".to_string(),
            "extension_identifier=other-extension&extension_version_number=2".to_string(),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in table {
        let mut expected = expected;
        expected.sort();
        let answer = list(&router, &query).await;
        let ids = listed_ids(&case, &answer);
        assert_eq!(
            ids, expected,
            "case {case:?}: expected exactly the associations the filter addresses, got {ids:?}"
        );
        assert!(
            answer.string("NextToken").is_none(),
            "case {case:?}: a listing that fits one page hands out no NextToken, got {:?}",
            answer.string("NextToken")
        );
    }

    // Every filtered read narrowed the REPORT rather than the store: the unfiltered
    // listing still reports all of them, and twice in a row reports the same order.
    let first = listed_ids("after the filters", &list(&router, "").await);
    let again = listed_ids("after the filters", &list(&router, "").await);
    assert_eq!(
        first, all,
        "the unfiltered listing must still report every association, got {first:?}"
    );
    assert_eq!(
        first, again,
        "two listings of the same store must report the same order, got {first:?} then {again:?}"
    );
}

/// A `NextToken` this service did not mint for this listing is refused with the modelled
/// `BadRequestException` rather than read as "start from the beginning".
///
/// Rows differ along HOW the token is wrong: not hex at all, hex over a payload carrying no
/// listing tag, hex over ANOTHER listing's tag (the token a sibling list operation hands
/// out), and a bare association id.
#[tokio::test]
async fn a_token_this_listing_did_not_mint_is_refused() {
    let router = router();
    let tree = tree(&router, "tokened").await;
    let id = association(
        &router,
        authored_without_parameter(),
        &tree.application_arn,
        None,
    )
    .await;

    let forged = [
        ("not hex", "not-a-token".to_string()),
        ("hex over an untagged cursor", hex::encode(&id)),
        (
            "hex over another listing's tag",
            hex::encode(format!("ListExtensions\u{1f}{id}")),
        ),
        (
            "hex over a tag that is no listing",
            hex::encode(format!("ExtensionAssociations\u{1f}{id}")),
        ),
        ("a bare id", id.clone()),
    ];

    for (case, token) in forged {
        let answer = list(&router, &format!("next_token={token}")).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 for token {token:?}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer
                .error_code()
                .split('#')
                .next_back()
                .unwrap_or_default(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {}",
            answer.error_code()
        );
    }

    // The listing itself still works, so the refusals above are about the token.
    let ids = listed_ids("after the refusals", &list(&router, "").await);
    assert_eq!(
        ids,
        vec![id],
        "the stored association must still be listed, got {ids:?}"
    );
}
