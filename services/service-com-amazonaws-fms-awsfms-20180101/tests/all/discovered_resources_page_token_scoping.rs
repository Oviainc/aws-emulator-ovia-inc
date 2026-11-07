// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the page-token rule `ListDiscoveredResources` shares
//! with every other listing of this service whose page set is SELECTED by the
//! request.
//!
//! The rule: such a listing's `NextToken` names a page boundary only under the
//! selection the request that minted it named, so a token minted under one
//! selection is refused under another rather than resumed from as an arbitrary
//! key — a boundary of one selection may be absent from the next, or sort past
//! resources the next does hold, so resuming from it there would silently drop
//! them and report a short page, or an empty one, as a complete page. A refused
//! call reports nothing and leaves the page as it was, and the token a selection
//! does mint still walks that selection's whole page.
//!
//! Here the selection is THREE values at once — the Region the request
//! addresses, the single `MemberAccountIds` entry it names and the
//! `ResourceType` it asks for — because the inventory is read per Region, per
//! account and per type. A boundary carrying only part of that selection is
//! therefore no boundary of any of these pages either, and neither is the bare
//! resource URI.
//!
//! Asserted over a TABLE whose rows are five selections of the one inventory.
//! They differ in the resource type (a VPC, a subnet — two collections of URIs
//! that interleave, since `.../subnet/...` sorts before `.../vpc/...` under one
//! account and Region), in the Region (one whose resources were created there,
//! one holding what a fresh account starts with), and in the member account (the
//! administrator, which owns resources, and another account of the organisation,
//! which owns none and whose page is empty — the answer a caller must be able to
//! tell apart from a truncated page).
//! [`the_selections_this_suite_serves_are_different_pages_that_interleave`] pins
//! those preconditions, so the refusal rows below cannot quietly stop covering
//! the case where resuming from a foreign boundary would drop resources.
//!
//! Every call goes over the wire through one `MultiServiceRouter` with Firewall
//! Manager and the services that own the discovered resources mounted on ONE
//! shared engine — the harness in [`crate::discovered_resources`] — so the
//! assertions are about the WIRE response.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::associate_admin;
use crate::discovered_resources::{
    ec2_ok, ec2_ok_in, element, emulator, every_uri, p, put_account_scoped_policy,
};
use crate::policy_lifecycle::{call_in, error_code, ACCOUNT, REGION};

/// A second Region this suite addresses the same inventory at: the Region a
/// fresh EC2 account holds its default resources in, so the page selected there
/// holds resources of its own.
const OTHER_REGION: &str = "us-east-1";

/// An account the organisation DOES hold that owns nothing — recorded as a
/// member by a policy's `IncludeMap`, the way [`put_account_scoped_policy`]
/// does it. Its page is EMPTY, which is the answer "this member account owns
/// nothing of that type"; a caller must be able to tell it apart from a page
/// truncated by a foreign boundary.
const MEMBER_WITHOUT_RESOURCES: &str = "100000000009";

/// The CIDR blocks of the VPCs this suite creates in [`REGION`], and of the
/// subnets it creates in the first of them. More than one of each, so every
/// selection the administrator owns resources in mints a token at `MaxResults`
/// 1.
const VPC_CIDR_BLOCKS: [&str; 3] = ["10.0.0.0/16", "10.1.0.0/16", "10.2.0.0/16"];
const SUBNET_CIDR_BLOCKS: [&str; 3] = ["10.0.1.0/24", "10.0.2.0/24", "10.0.3.0/24"];

/// One selection of the discoverable inventory: the three values that select its
/// page, and the resource URIs that page holds in the ascending order it serves
/// them in.
struct Selection {
    region: &'static str,
    account: &'static str,
    resource_type: &'static str,
    keys: Vec<String>,
}

impl Selection {
    /// How this selection reads in an assertion message.
    fn label(&self) -> String {
        format!("{}/{}/{}", self.region, self.account, self.resource_type)
    }

    /// The `NextToken` this selection's page mints for the boundary `uri`.
    fn token(&self, uri: &str) -> String {
        state::encode_discovered_resources_page_token(
            self.region,
            self.account,
            self.resource_type,
            uri,
        )
    }
}

/// A fresh emulator holding the resources these selections report, with each
/// selection's own page read back off the wire.
async fn fixture() -> (MultiServiceRouter, Vec<Selection>) {
    let (router, _engine) = emulator();
    associate_admin(&router).await;
    // The second member account of the organisation, which owns nothing.
    put_account_scoped_policy(
        &router,
        "discovered-page-token-scope",
        "IncludeMap",
        &[MEMBER_WITHOUT_RESOURCES],
    )
    .await;

    let mut first_vpc = String::new();
    for cidr in VPC_CIDR_BLOCKS {
        let case = format!("vpc {cidr}");
        let created = element(
            &case,
            &ec2_ok(&case, &router, "CreateVpc", &[p("CidrBlock", cidr)]).await,
            "vpcId",
        );
        if first_vpc.is_empty() {
            first_vpc = created;
        }
        // The same block in the OTHER Region: EC2 is a regional service, so a VPC
        // created here is discoverable only here — the page selected in that Region
        // needs VPCs of its own for the rows below to be two DIFFERENT pages.
        let case = format!("vpc {cidr} in {OTHER_REGION}");
        element(
            &case,
            &ec2_ok_in(
                &case,
                &router,
                OTHER_REGION,
                "CreateVpc",
                &[p("CidrBlock", cidr)],
            )
            .await,
            "vpcId",
        );
    }
    for cidr in SUBNET_CIDR_BLOCKS {
        let case = format!("subnet {cidr}");
        element(
            &case,
            &ec2_ok(
                &case,
                &router,
                "CreateSubnet",
                &[p("VpcId", &first_vpc), p("CidrBlock", cidr)],
            )
            .await,
            "subnetId",
        );
    }

    let mut selections = vec![
        Selection {
            region: REGION,
            account: ACCOUNT,
            resource_type: state::RESOURCE_TYPE_EC2_VPC,
            keys: Vec::new(),
        },
        Selection {
            region: REGION,
            account: ACCOUNT,
            resource_type: state::RESOURCE_TYPE_EC2_SUBNET,
            keys: Vec::new(),
        },
        Selection {
            region: OTHER_REGION,
            account: ACCOUNT,
            resource_type: state::RESOURCE_TYPE_EC2_VPC,
            keys: Vec::new(),
        },
        Selection {
            region: OTHER_REGION,
            account: ACCOUNT,
            resource_type: state::RESOURCE_TYPE_EC2_SUBNET,
            keys: Vec::new(),
        },
        Selection {
            region: REGION,
            account: MEMBER_WITHOUT_RESOURCES,
            resource_type: state::RESOURCE_TYPE_EC2_VPC,
            keys: Vec::new(),
        },
    ];
    for selection in &mut selections {
        let keys = walk(&router, selection, None).await;
        selection.keys = keys;
    }
    (router, selections)
}

/// One `ListDiscoveredResources` call for a selection, with the optional page
/// bound and token, as `(http status, parsed body)`.
async fn list(
    router: &MultiServiceRouter,
    selection: &Selection,
    max_results: Option<usize>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({
        "MemberAccountIds": [selection.account],
        "ResourceType": selection.resource_type,
    });
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    call_in(router, "ListDiscoveredResources", &request, selection.region).await
}

/// The resource URIs one page carries, in the order it served them.
fn uris(case: &str, body: &Value) -> Vec<String> {
    body["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["URI"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: every discovered resource carries its URI, got {item}")
                })
                .to_string()
        })
        .collect()
}

/// The `NextToken` a page carries, if any.
fn next_token(body: &Value) -> Option<String> {
    body.get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every URI one selection's page holds, walking the tokens it mints at page
/// size `max_results`: each page is served, bounded by the size asked for, and
/// the walk ends at a page carrying no token.
async fn walk(
    router: &MultiServiceRouter,
    selection: &Selection,
    max_results: Option<usize>,
) -> Vec<String> {
    let case = format!("{} / MaxResults {max_results:?}", selection.label());
    let mut served: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // A bound no chain of this fixture can need, so a token that never clears
    // fails here instead of looping forever.
    for round in 0..64 {
        let (status, body) = list(router, selection, max_results, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case} / page {round}: expected 200, got {status} with {body}"
        );
        let page = uris(&case, &body);
        if let Some(bound) = max_results {
            assert!(
                page.len() <= bound,
                "case {case} / page {round}: a page must not exceed the requested size, got \
                 {page:?}"
            );
        }
        served.extend(page);
        match next_token(&body) {
            Some(next) => token = Some(next),
            None => {
                assert!(
                    served.windows(2).all(|pair| pair[0] < pair[1]),
                    "case {case}: the inventory is served in ascending URI order with no resource \
                     twice, got {served:?}"
                );
                return served;
            }
        }
    }
    panic!("case {case}: the NextToken chain did not end, served {served:?}");
}

/// The token the first page of size one mints for a selection, asserting that
/// page is that selection's first key — `None` for a selection whose page fits
/// in one item or holds nothing, which mints no token at all.
async fn first_token(router: &MultiServiceRouter, selection: &Selection) -> Option<String> {
    let case = format!("{} / first page", selection.label());
    let (status, body) = list(router, selection, Some(1), None).await;
    assert_eq!(
        status, 200,
        "case {case}: the first page must be served, got {status} with {body}"
    );
    let served = uris(&case, &body);
    assert_eq!(
        served,
        selection.keys.iter().take(1).cloned().collect::<Vec<String>>(),
        "case {case}: expected the selection's first key, got {body}"
    );
    let token = next_token(&body);
    assert_eq!(
        token.is_some(),
        selection.keys.len() > 1,
        "case {case}: a token must be minted exactly while resources remain after the page, got \
         {token:?} for {:?}",
        selection.keys
    );
    token
}

/// A token that names no boundary of `target`'s page is refused with the
/// operation's modelled `InvalidInputException`, reports nothing, and leaves
/// that page as it was.
async fn refuse(router: &MultiServiceRouter, target: &Selection, token: &str, case: &str) {
    let (status, body) = list(router, target, None, Some(token)).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "case {case}: a token naming no boundary of the selected page must be refused, got \
         {status} with {body}"
    );
    assert!(
        body["Items"].is_null() && body["NextToken"].is_null(),
        "case {case}: a refused call must report nothing, got {body}"
    );
    let after = walk(router, target, None).await;
    assert_eq!(
        after, target.keys,
        "case {case}: a refused token must leave the selected page as it was, got {after:?}"
    );
}

/// The preconditions every row below reads against: these five selections are
/// DIFFERENT pages of the one inventory, they interleave in the URI order each
/// is served in — so resuming one from another's boundary would drop
/// resources — and the member account that owns nothing answers an EMPTY page.
#[tokio::test]
async fn the_selections_this_suite_serves_are_different_pages_that_interleave() {
    let (router, selections) = fixture().await;

    for selection in &selections {
        let case = selection.label();
        if selection.account == ACCOUNT {
            assert!(
                selection.keys.len() >= 2,
                "case {case}: this suite needs at least two resources in every selection the \
                 administrator owns, got {:?}",
                selection.keys
            );
        } else {
            assert!(
                selection.keys.is_empty(),
                "case {case}: a member account that owns nothing must answer an empty page, got \
                 {:?}",
                selection.keys
            );
        }
    }

    // The pages are disjoint: no URI belongs to two of these selections, so no
    // key of one is a key of another.
    for left in &selections {
        for right in &selections {
            if left.label() == right.label() {
                continue;
            }
            let shared: Vec<&String> = left
                .keys
                .iter()
                .filter(|key| right.keys.contains(key))
                .collect();
            assert!(
                shared.is_empty(),
                "case {} against {}: these selections must be different pages, got {shared:?} in \
                 both",
                left.label(),
                right.label()
            );
        }
    }

    // And they interleave: for at least one ordered pair, the boundary the one
    // mints sorts ABOVE resources the other holds, so resuming the other from
    // it would silently drop them.
    let mut dropped = 0;
    for minter in &selections {
        let Some(boundary) = minter.keys.get(1) else {
            continue;
        };
        for target in &selections {
            if target.label() == minter.label() {
                continue;
            }
            dropped += target.keys.iter().filter(|key| *key < boundary).count();
        }
    }
    assert!(
        dropped > 0,
        "this suite needs selections that interleave, so a foreign boundary would drop \
         resources; got {} selections dropping none",
        selections.len()
    );

    // The fixture's pages are what the inventory itself reports for those
    // selections, read through the sibling suite's own walker.
    let whole = every_uri(&router, ACCOUNT, state::RESOURCE_TYPE_EC2_VPC).await;
    assert_eq!(
        whole, selections[0].keys,
        "case {}: this suite must read the same page the inventory reports, got {whole:?}",
        selections[0].label()
    );
}

/// RULE: a `NextToken` of this listing names a page boundary only under the
/// selection — Region, member account, resource type — that minted it. Replayed
/// under another selection, sent as the bare resource URI, or carrying only part
/// of a selection, it is refused with the modelled `InvalidInputException`;
/// under its own selection it resumes the page.
#[tokio::test]
async fn a_token_is_a_boundary_only_under_the_selection_that_minted_it() {
    let (router, selections) = fixture().await;

    for minter in &selections {
        let Some(token) = first_token(&router, minter).await else {
            continue;
        };

        // Under ANOTHER selection the same token names no boundary — including
        // the one whose page is empty, where accepting it would be
        // indistinguishable from "this member account owns nothing".
        for target in &selections {
            if target.label() == minter.label() {
                continue;
            }
            let case = format!(
                "token minted under {} replayed under {}",
                minter.label(),
                target.label()
            );
            refuse(&router, target, &token, &case).await;
        }

        // Under the selection that minted it, it resumes exactly after the page
        // it was minted for — so the refusals above are not refusing every
        // token.
        let case = format!("token minted under {} replayed under itself", minter.label());
        let (status, body) = list(&router, minter, None, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {case}: the minting selection's own token must resume its page, got {status} \
             with {body}"
        );
        assert_eq!(
            uris(&case, &body),
            minter.keys[1..].to_vec(),
            "case {case}: expected the keys after the first, got {body}"
        );
    }

    // A boundary carrying only PART of what selects the page is no boundary of
    // it: every one of the three values is what tells one selection's keys from
    // another's. The bare URI — the unstamped form — is no boundary either.
    for target in &selections {
        let Some(key) = target.keys.first() else {
            continue;
        };
        let partial = [
            ("the bare URI, carrying no selection", hex::encode(key)),
            (
                "only the Region",
                state::encode_scoped_page_token(target.region, key),
            ),
            (
                "only the member account",
                state::encode_scoped_page_token(target.account, key),
            ),
            (
                "only the resource type",
                state::encode_scoped_page_token(target.resource_type, key),
            ),
            (
                "the selection with no resource type",
                state::encode_discovered_resources_page_token(
                    target.region,
                    target.account,
                    "",
                    key,
                ),
            ),
            (
                "the selection with no Region",
                state::encode_discovered_resources_page_token(
                    "",
                    target.account,
                    target.resource_type,
                    key,
                ),
            ),
            (
                "the selection with no member account",
                state::encode_discovered_resources_page_token(
                    target.region,
                    "",
                    target.resource_type,
                    key,
                ),
            ),
        ];
        for (origin, token) in partial {
            let case = format!("{origin}, sent under {}", target.label());
            refuse(&router, target, &token, &case).await;
        }
    }

    // A key of another selection, stamped with the TARGET's selection, is still
    // only a lower bound in the target's own key order: the stamp decides which
    // page a token belongs to, the URI decides where in it the page resumes.
    for target in &selections {
        for other in &selections {
            if other.label() == target.label() {
                continue;
            }
            let Some(key) = other.keys.first() else {
                continue;
            };
            let case = format!(
                "a key of {} stamped with {}",
                other.label(),
                target.label()
            );
            let (status, body) = list(&router, target, None, Some(&target.token(key))).await;
            assert_eq!(
                status, 200,
                "case {case}: a boundary stamped with this selection is served, got {status} with \
                 {body}"
            );
            let expected: Vec<String> = target
                .keys
                .iter()
                .filter(|held| *held >= key)
                .cloned()
                .collect();
            assert_eq!(
                uris(&case, &body),
                expected,
                "case {case}: expected the page to resume at the first key at or after that URI, \
                 got {body}"
            );
        }
    }
}

/// RULE: the token a selection mints decodes through the same stamp, so walking
/// the chain serves every resource of THAT selection exactly once, in the page's
/// own order, at every page size.
#[tokio::test]
async fn the_token_a_selection_mints_walks_that_selections_whole_page() {
    let (router, selections) = fixture().await;

    // Page sizes differing in whether a page holds one resource, some of them,
    // or the whole selection at the largest page this service serves.
    for max_results in [1usize, 2, state::MAX_PAGE_SIZE] {
        for selection in &selections {
            let walked = walk(&router, selection, Some(max_results)).await;
            assert_eq!(
                walked,
                selection.keys,
                "case {} / MaxResults {max_results}: walking the minted tokens must serve every \
                 resource of that selection exactly once, in the page's order, got {walked:?}",
                selection.label()
            );
        }
    }
}
