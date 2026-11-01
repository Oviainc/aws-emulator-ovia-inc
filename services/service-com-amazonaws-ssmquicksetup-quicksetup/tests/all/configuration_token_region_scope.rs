// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for which endpoint a `ListConfigurations` page token is good
//! for, whatever store key that token carries.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a pagination token names a position in the listing ONE regional
//! endpoint serves, so a listing accepts only the tokens that endpoint issued and answers
//! a token tagged for another Region's endpoint with the 400 `ValidationException` a
//! token it did not issue earns — whatever key the token carries and wherever that key
//! falls among the requesting Region's own records, every one of which stays reachable.
//! `ListConfigurations` "returns configurations deployed by Quick Setup in the requesting
//! Amazon Web Services account and Amazon Web Services Region"
//! (`smithy_query(query="doc",shape="ListConfigurations")`), so the records a token walks
//! are a different set in every Region.
//!
//! This suite is the `ListConfigurations` half, where the key is the configuration's `Id`
//! — "a service generated identifier for the configuration"
//! (`smithy_query(query="shape",shape="ConfigurationSummary")`) — which carries neither
//! the Region whose endpoint holds it nor the order it was created in. A foreign token's
//! key can therefore fall ANYWHERE among the requesting Region's own keys, so the rows
//! place it there deterministically: before every key the store holds, inside the
//! requesting Region's own range, past every key, and at the position another Region's
//! endpoint really handed out. Every row also holds managers owning SEVERAL
//! configurations each — one per definition and deployment target — so a Region holds
//! more configurations than managers and a page boundary falls inside one manager, and
//! every row creates its managers Region by Region in turn rather than as one block per
//! Region.
//!
//! `tests/all/page_token_region_scope.rs` holds the cross-Region table both listings
//! share and the harness this suite reuses; the token's other halves — the empty string
//! an exhausted listing reports, and the refusal of a token the other listing issued —
//! are pinned by `tests/all/pagination_tokens.rs` and
//! `tests/all/configuration_managers.rs`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    encode_page_token, CONFIGURATION_PAGE_TOKEN_KIND,
};
use smithy_server_runtime::MultiServiceRouter;

// The harness the cross-Region suite already carries: one emulator mounted the way the
// router mounts it, a Region-aware call, and one page / every page of a listing.
use super::page_token_region_scope::{all_ids, call, ids_of, page, CONFIGURATION_LISTING};
use super::reserved_tag_keys::{emulator, error_code, error_message};

/// A configuration type the service publishes (`ListQuickSetupTypes`), so every create
/// below is refused only for a reason under test.
const TYPE: &str = "AWSQuickSetupType-SSMOpsCenter";

/// The organizational unit every definition below deploys to: a definition's parameters
/// must name somewhere to deploy to, and no case here is about which accounts that is.
const TARGET_OU: &str = "ou-1";

/// One Region's share of the store: how many managers its endpoint creates, how many
/// definitions each of those managers carries, and the Regions those definitions deploy
/// into.
struct Population {
    /// The Region whose endpoint creates the managers.
    region: &'static str,
    /// How many managers it creates.
    managers: usize,
    /// How many definitions each of them carries.
    definitions: usize,
    /// The comma-separated `TargetRegions` every one of those definitions names.
    target_regions: &'static str,
}

impl Population {
    /// How many configurations this Region's endpoint ends up holding: the service
    /// deploys one configuration per definition and per targeted account and Region, and
    /// every definition below targets the one account this emulator serves.
    fn configurations(&self) -> usize {
        self.managers * self.definitions * self.target_regions.split(',').count()
    }
}

/// Create the `index`th manager of `population` through that Region's endpoint.
///
/// No case here turns on which parameters a definition names, so the ones this type's
/// documented table requires beyond the targeting below come from the fixture every suite
/// of this target shares.
async fn create_manager(
    router: &MultiServiceRouter,
    case: &str,
    population: &Population,
    index: usize,
) {
    let definitions: Vec<Value> = (0..population.definitions)
        .map(|_| {
            json!({
                "Type": TYPE,
                "Parameters": super::type_parameter_fixtures::with_required_parameters(
                    TYPE,
                    json!({
                        "TargetOrganizationalUnits": TARGET_OU,
                        "TargetRegions": population.target_regions,
                    }),
                ),
            })
        })
        .collect();
    let body = json!({
        "Name": format!("manager-{}-{index}", population.region),
        "ConfigurationDefinitions": definitions,
    });
    let (status, answer) = call(
        router,
        "POST",
        "/configurationManager",
        population.region,
        Some(&body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: CreateConfigurationManager in {} must succeed, got {status} / {answer:?}",
        population.region
    );
}

/// Fill the store from `populations`, one manager at a time and Region by Region in turn,
/// so no Region's configuration ids are minted as one block.
async fn fill(router: &MultiServiceRouter, case: &str, populations: &[Population]) {
    let rounds = populations
        .iter()
        .map(|population| population.managers)
        .max()
        .unwrap_or_default();
    for index in 0..rounds {
        for population in populations {
            if index < population.managers {
                create_manager(router, case, population, index).await;
            }
        }
    }
}

/// Where the key a foreign token carries falls in the shared store order.
#[derive(Debug, Clone, Copy)]
enum Placement {
    /// Before every configuration the store holds, in any Region.
    BeforeEveryKey,
    /// Among the requesting Region's own configurations.
    InsideTheRequestersKeys,
    /// Past every configuration the store holds, in any Region.
    PastEveryKey,
    /// Exactly where the issuing endpoint's own next page starts — the token it really
    /// handed out, replayed verbatim.
    WhereTheIssuerResumes,
}

/// A token tagged for the endpoint of `region` naming `key`: what that endpoint's own
/// listing hands out for that position.
fn token_of(region: &str, key: &str) -> String {
    encode_page_token(CONFIGURATION_PAGE_TOKEN_KIND, region, key)
}

/// One token per [`Placement`] the store can place: tagged for `issuer`, which is not the
/// Region replaying them.
///
/// The bounding keys are derived from the ids the store actually reports rather than
/// written out, since a configuration `Id` is service-generated: a strict prefix of the
/// lowest id sorts before every id, and the highest id with a character appended sorts
/// after every id. `InsideTheRequestersKeys` needs an id of the requesting Region, so it
/// is absent exactly when that Region holds none.
fn foreign_tokens(
    issuer: &str,
    issuer_ids: &[String],
    requester_ids: &[String],
    issued: &str,
) -> Vec<(Placement, String)> {
    let ordered: BTreeSet<&String> = issuer_ids.iter().chain(requester_ids).collect();
    let mut tokens = Vec::new();
    if let (Some(lowest), Some(highest)) = (ordered.iter().next(), ordered.iter().next_back()) {
        let before: String = lowest.chars().take(1).collect();
        tokens.push((Placement::BeforeEveryKey, token_of(issuer, &before)));
        tokens.push((
            Placement::PastEveryKey,
            token_of(issuer, &format!("{highest}0")),
        ));
    }
    if let Some(inside) = requester_ids.get(requester_ids.len() / 2) {
        tokens.push((Placement::InsideTheRequestersKeys, token_of(issuer, inside)));
    }
    tokens.push((Placement::WhereTheIssuerResumes, issued.to_string()));
    tokens
}

/// One row: every Region holding configurations — the FIRST issues the token and the
/// SECOND replays it — and the page bound the replaying Region reads with.
struct Case {
    populations: &'static [Population],
    max_items: Option<i64>,
}

/// The rows: what one manager owns (one definition deploying into one Region, several
/// definitions, one definition fanning out over several Regions, and both at once), how
/// many configurations each endpoint holds (including a replaying Region holding none and
/// ones holding several pages), whether a third Region holds records too, and the page
/// bound asked for.
const CASES: &[Case] = &[
    Case {
        populations: &[
            Population {
                region: "us-west-2",
                managers: 2,
                definitions: 1,
                target_regions: "us-west-2",
            },
            Population {
                region: "us-east-1",
                managers: 1,
                definitions: 2,
                target_regions: "us-east-1,eu-west-1",
            },
        ],
        max_items: Some(1),
    },
    Case {
        populations: &[
            Population {
                region: "ap-south-1",
                managers: 1,
                definitions: 3,
                target_regions: "ap-south-1",
            },
            Population {
                region: "eu-west-1",
                managers: 2,
                definitions: 1,
                target_regions: "eu-west-1,eu-central-1,us-east-2",
            },
        ],
        max_items: Some(2),
    },
    Case {
        populations: &[
            Population {
                region: "eu-central-1",
                managers: 2,
                definitions: 1,
                target_regions: "eu-central-1,eu-west-1",
            },
            // Holds nothing at all: the answer is still the refusal, not the empty page
            // that reads as "this Region holds no configurations".
            Population {
                region: "us-east-2",
                managers: 0,
                definitions: 1,
                target_regions: "us-east-2",
            },
        ],
        max_items: Some(1),
    },
    Case {
        populations: &[
            Population {
                region: "us-east-1",
                managers: 1,
                definitions: 2,
                target_regions: "us-east-1,us-west-2",
            },
            Population {
                region: "us-west-1",
                managers: 1,
                definitions: 1,
                target_regions: "us-west-1",
            },
            // A Region that neither issues nor replays, so the shared store order holds
            // keys belonging to neither side of the case.
            Population {
                region: "ca-central-1",
                managers: 2,
                definitions: 1,
                target_regions: "ca-central-1,us-east-1",
            },
        ],
        max_items: None,
    },
];

/// Every configuration the store holds, across every Region of a row: the bound on how
/// many pages one walk of the listing may take.
fn store_total(populations: &[Population]) -> usize {
    populations
        .iter()
        .map(Population::configurations)
        .sum::<usize>()
}

/// A `ListConfigurations` token tagged for one Region's endpoint is refused by every
/// other Region's endpoint — wherever the key it carries falls among the requesting
/// Region's own configuration ids — and the requesting Region's own configurations stay
/// exactly as reachable as before the refused request.
#[tokio::test]
async fn a_configuration_token_of_another_region_is_refused_wherever_its_key_falls() {
    for case in CASES {
        let [issuer, requester, ..] = case.populations else {
            panic!("every row names at least the issuing and the replaying Region");
        };
        let name = format!(
            "token from {} replayed on {} ({} and {} configurations, MaxItems={:?})",
            issuer.region,
            requester.region,
            issuer.configurations(),
            requester.configurations(),
            case.max_items
        );
        let total = store_total(case.populations);
        let router = emulator();
        fill(&router, &name, case.populations).await;

        // What each side holds, read without any token: the issuing Region's keys place
        // the foreign token, and the requesting Region's are the records no page may
        // silently drop.
        let issuer_ids = all_ids(
            &router,
            &name,
            &CONFIGURATION_LISTING,
            issuer.region,
            case.max_items,
            total,
        )
        .await;
        assert_eq!(
            issuer_ids.len(),
            issuer.configurations(),
            "case {name}: the issuing Region must report its own {} configurations, got \
             {issuer_ids:?}",
            issuer.configurations()
        );
        let own = all_ids(
            &router,
            &name,
            &CONFIGURATION_LISTING,
            requester.region,
            case.max_items,
            total,
        )
        .await;
        assert_eq!(
            own.len(),
            requester.configurations(),
            "case {name}: the replaying Region must report its own {} configurations, got {own:?}",
            requester.configurations()
        );

        // The token the issuing endpoint really hands out. Its first page carries one of
        // its own configurations and it holds more than one, so more entries follow in
        // the shared order and the token names one of them.
        let (status, answer) = page(
            &router,
            &CONFIGURATION_LISTING,
            issuer.region,
            Some(1),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: the issuing listing must succeed, got {status} / {answer:?}"
        );
        let issued = answer
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {name}: expected a NextToken, got {answer:?}"))
            .to_string();
        assert!(
            !issued.is_empty(),
            "case {name}: the issuing listing must hand out a resume token, got {answer:?}"
        );

        for (placement, token) in foreign_tokens(issuer.region, &issuer_ids, &own, &issued) {
            let name = format!("{name} / key {placement:?}");
            let (status, answer) = page(
                &router,
                &CONFIGURATION_LISTING,
                requester.region,
                case.max_items,
                Some(&token),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {name}: a token of another Region's endpoint must be refused, got {status} \
                 / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {name}: expected ValidationException, got {answer:?}"
            );
            let message = error_message(&answer);
            assert!(
                message.contains("StartingToken"),
                "case {name}: the refusal must name the member it refused, got {message:?}"
            );

            // The issuing endpoint still accepts the token it issued...
            let (status, answer) = page(
                &router,
                &CONFIGURATION_LISTING,
                issuer.region,
                Some(1),
                Some(&issued),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name}: the issuing endpoint must accept its own token, got {status} / \
                 {answer:?}"
            );

            // ...and the refused request changed nothing: the replaying Region still
            // reports exactly the configurations it held, in the same order.
            let again = all_ids(
                &router,
                &name,
                &CONFIGURATION_LISTING,
                requester.region,
                case.max_items,
                total,
            )
            .await;
            assert_eq!(
                again, own,
                "case {name}: the replaying Region's configurations must stay reachable, got \
                 {again:?}"
            );
        }
    }
}

/// Paging a Region's configuration listing with the tokens IT issued visits every
/// configuration of every manager that Region holds exactly once — the same set whatever
/// the page bound — while other Regions hold configurations whose service-generated ids
/// are interleaved with them in the shared store order.
#[tokio::test]
async fn paging_a_regions_configuration_listing_visits_its_own_configurations_exactly_once() {
    for case in CASES {
        let total = store_total(case.populations);
        let router = emulator();
        let name = format!(
            "{:?} configurations per Region",
            case.populations
                .iter()
                .map(|population| (population.region, population.configurations()))
                .collect::<Vec<(&str, usize)>>()
        );
        fill(&router, &name, case.populations).await;

        for population in case.populations {
            // The set one unbounded page reports is the whole of what this Region holds,
            // since no row fills more than the modelled `MaxItems` maximum.
            let (status, answer) = page(
                &router,
                &CONFIGURATION_LISTING,
                population.region,
                None,
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name}: {} must succeed, got {status} / {answer:?}",
                population.region
            );
            let whole: BTreeSet<String> = ids_of(&name, &CONFIGURATION_LISTING, &answer)
                .into_iter()
                .collect();
            assert_eq!(
                whole.len(),
                population.configurations(),
                "case {name}: {} must report its own {} configurations, got {whole:?}",
                population.region,
                population.configurations()
            );

            for max_items in [Some(1), Some(2), Some(100), None] {
                let case_name = format!("{name} / {} MaxItems={max_items:?}", population.region);
                let paged = all_ids(
                    &router,
                    &case_name,
                    &CONFIGURATION_LISTING,
                    population.region,
                    max_items,
                    total,
                )
                .await;
                assert_eq!(
                    paged.len(),
                    population.configurations(),
                    "case {case_name}: paging must serve each of the {} configurations exactly \
                     once, got {paged:?}",
                    population.configurations()
                );
                assert_eq!(
                    paged.iter().cloned().collect::<BTreeSet<String>>(),
                    whole,
                    "case {case_name}: paging must serve exactly this Region's configurations, \
                     got {paged:?}"
                );
            }
        }
    }
}
