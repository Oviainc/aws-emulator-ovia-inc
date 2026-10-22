// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-servicecatalog-aws242servicecatalogservice service.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    ConstraintDetail, PortfolioDetail, ProductViewDetail, ProductViewSummary,
    ProvisioningArtifact as ModelledProvisioningArtifact, ProvisioningArtifactDetail,
    ProvisioningArtifactSummary, ServiceActionDetail, ServiceActionSummary, SourceConnectionDetail,
    Tag, TagOptionDetail,
};

/// A persisted portfolio — the record every portfolio operation reads and writes.
///
/// Tags are a map rather than a list because the API addresses them by key:
/// `UpdatePortfolio`'s `AddTags` overwrites an existing key's value and
/// `RemoveTags` deletes by key, and a portfolio therefore cannot hold the same
/// key twice. The map is a `BTreeMap` so the `Tags` list a response carries has
/// a stable, deterministic order.
#[derive(Debug, Clone)]
pub struct Portfolio {
    /// The minted portfolio id (`port-…`), also this record's key in [`ComAmazonawsServicecatalogAws242servicecatalogserviceState::portfolios`].
    pub id: String,
    /// The ARN minted at create time and read back verbatim afterwards.
    pub arn: String,
    pub display_name: String,
    pub description: Option<String>,
    pub provider_name: String,
    /// Creation instant, minted once at create time.
    pub created_time: SystemTime,
    /// Tag key -> tag value.
    pub tags: BTreeMap<String, String>,
    /// The principals associated with this portfolio, keyed by principal ARN: the
    /// API addresses an association BY that ARN, so re-associating one ARN changes
    /// its type rather than listing it twice. The stored value carries the order the
    /// associations were made in, which is the order
    /// `ListPrincipalsForPortfolio` reports and pages over.
    pub principals: BTreeMap<String, PrincipalAssociation>,
}

/// One portfolio-principal association: what was associated, and when in the
/// portfolio's life.
#[derive(Debug, Clone)]
pub struct PrincipalAssociation {
    /// `PrincipalType` (`IAM` | `IAM_PATTERN`), as the request declared it.
    pub principal_type: String,
    /// The position of this association in the order they were made, so a list
    /// reports them in that order however the ARNs sort. Re-associating an ARN keeps
    /// its original position: it is the same association with a new type.
    pub sequence: u64,
}

impl Portfolio {
    /// This record as the modelled `PortfolioDetail`: every member comes from
    /// stored state, so Create/Describe/List/Update all report the same values.
    pub fn detail(&self) -> PortfolioDetail {
        PortfolioDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            display_name: Some(self.display_name.clone()),
            description: self.description.clone(),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            provider_name: Some(self.provider_name.clone()),
        }
    }

    /// The stored tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }

    /// The stored associations in the order they were made — the one order every
    /// response that reports them walks.
    ///
    /// Neither the model nor AWS's documentation fixes an order for
    /// `ListPrincipalsForPortfolio`, so this service picks one and reports it
    /// consistently: the order the associations were made in, which is also the
    /// order its page cursor walks.
    pub fn principals_in_association_order(&self) -> Vec<(&String, &PrincipalAssociation)> {
        let mut associations: Vec<(&String, &PrincipalAssociation)> =
            self.principals.iter().collect();
        associations.sort_by_key(|(_, association)| association.sequence);
        associations
    }
}

/// A tag map as the modelled `Tags` list, in key order: every operation that
/// reports a resource's tags renders them through this one function, so they read
/// the same everywhere.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// A persisted constraint — the record `CreateConstraint` mints and
/// `DescribeConstraint` reports.
#[derive(Debug, Clone)]
pub struct Constraint {
    /// The minted constraint id (`cons-…`).
    pub id: String,
    /// `ConstraintType` (`LAUNCH`, `NOTIFICATION`, `STACKSET`, `TEMPLATE`, …), as
    /// the request declared it: the model constrains it only by length, so the
    /// stored value is the requested one.
    pub constraint_type: String,
    pub description: Option<String>,
    /// The constraint's `Parameters` document, stored verbatim so a describe
    /// reports the parameters the create was given.
    pub parameters: String,
    pub portfolio_id: String,
    pub product_id: String,
    /// The instant this constraint stops being `CREATING` and becomes `AVAILABLE`.
    /// `CreateConstraint` answers in the non-terminal `CREATING` status, so the
    /// transition to the terminal one is owed by the CLOCK: it is stored as the
    /// instant it falls due and never slept for, and every report derives the
    /// status from it rather than from who asked.
    pub available_at: SystemTime,
}

/// How long after its create a constraint stays `CREATING`.
///
/// A span this emulator CHOOSES (AWS documents none): long enough that a caller
/// acting straight after the create still sees the transient status and the state
/// it implies, short enough that nothing waits on it.
pub const CONSTRAINT_AVAILABLE_AFTER: std::time::Duration = std::time::Duration::from_millis(250);

/// `Status`'s terminal value for a constraint that is ready for use
/// (`smithy_query query="shape" shape="Status"`: `AVAILABLE` | `CREATING` |
/// `FAILED`).
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// `Status`'s non-terminal value, which the model documents as "creation has
/// started; the [resource] is not ready for use".
pub const STATUS_CREATING: &str = "CREATING";

impl Constraint {
    /// This constraint's `Status` at `now`: the non-terminal one until the clock
    /// owes the transition, the terminal one afterwards.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.available_at {
            STATUS_CREATING
        } else {
            STATUS_AVAILABLE
        }
    }

    /// This record as the modelled `ConstraintDetail`.
    pub fn detail(&self) -> ConstraintDetail {
        ConstraintDetail {
            constraint_id: Some(self.id.clone()),
            r#type: Some(self.constraint_type.clone()),
            description: self.description.clone(),
            // `ConstraintDetail$Owner` targets `AccountId`, so it is the account that
            // created the constraint — this emulator's canonical account.
            owner: Some(::emulator_server::emulator_account_id().to_string()),
            product_id: Some(self.product_id.clone()),
            portfolio_id: Some(self.portfolio_id.clone()),
        }
    }
}

/// A persisted product — the record every product operation reads and writes.
///
/// A product owns its provisioning artifacts (its "versions"): the model binds every
/// artifact to one product (`CreateProvisioningArtifact` takes a `ProductId`), so they
/// are stored inside the product record rather than in a second, parallel map.
#[derive(Debug, Clone)]
pub struct Product {
    /// The minted product id (`prod-…`), also this record's key in
    /// [`ComAmazonawsServicecatalogAws242servicecatalogserviceState::products`].
    pub id: String,
    /// The minted product VIEW id (`prodview-…`). `ProductViewSummary` carries both
    /// `Id` (the view) and `ProductId` (the product), so they are distinct values.
    pub view_id: String,
    /// The ARN minted at create time and read back verbatim afterwards.
    pub arn: String,
    pub name: String,
    pub owner: String,
    /// `ProductViewSummary.ShortDescription`, which `CreateProduct` takes as
    /// `Description`.
    pub short_description: Option<String>,
    pub distributor: Option<String>,
    pub support_email: Option<String>,
    pub support_description: Option<String>,
    pub support_url: Option<String>,
    /// A `ProductType` value, as the request gave it.
    pub product_type: String,
    /// The product's `Status`, [`PRODUCT_STATUS_CREATED`] from create time on.
    pub status: String,
    /// Creation instant, minted once at create time.
    pub created_time: SystemTime,
    /// Tag key -> tag value.
    pub tags: BTreeMap<String, String>,
    /// The connection the product syncs from, when the create request named one.
    pub source_connection: Option<SourceConnectionDetail>,
    /// This product's provisioning artifacts, keyed by artifact id. A `BTreeMap` so
    /// every list of them has a stable, deterministic order.
    pub provisioning_artifacts: BTreeMap<String, ProvisioningArtifact>,
    /// The portfolios this product is associated with, by portfolio id. A product may
    /// live in several, and `AssociateProductWithPortfolio` addresses one association
    /// by the PAIR, so the set holds each portfolio once.
    pub portfolio_ids: std::collections::BTreeSet<String>,
}

/// The `Status` a product carries from the moment it is created.
///
/// `CreateProduct` answers a product that is ready to use — the recorded AWS responses
/// this emulator is graded against carry `CREATED`, and the calls that follow a create
/// use the new `ProductId` immediately — so the record is stored in that one terminal
/// status and no clock transition is owed. The modelled `Status` enum
/// (`smithy_query query="shape" shape="Status"`) additionally admits `AVAILABLE`,
/// `CREATING` and `FAILED`, which only the provisioning paths this emulator has not
/// implemented yet can produce.
pub const PRODUCT_STATUS_CREATED: &str = "CREATED";

impl Product {
    /// This record as the modelled `ProductViewSummary`.
    pub fn view_summary(&self) -> ProductViewSummary {
        ProductViewSummary {
            id: Some(self.view_id.clone()),
            product_id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            owner: Some(self.owner.clone()),
            short_description: self.short_description.clone(),
            r#type: Some(self.product_type.clone()),
            distributor: self.distributor.clone(),
            // "If the product does not have a default path, call ListLaunchPaths to
            // disambiguate between paths" — a product reachable through exactly one
            // portfolio needs no disambiguation, so that one path is its default;
            // zero or several portfolios leave it without one. Derived from the same
            // stored set `DescribeProduct` reports as `LaunchPaths`, so one response
            // cannot contradict itself.
            has_default_path: self.portfolio_ids.len() == 1,
            support_email: self.support_email.clone(),
            support_description: self.support_description.clone(),
            support_url: self.support_url.clone(),
        }
    }

    /// This record as the modelled `ProductViewDetail`: every member comes from stored
    /// state, so Create and Describe report the same values.
    pub fn detail(&self) -> ProductViewDetail {
        ProductViewDetail {
            product_view_summary: Some(self.view_summary()),
            status: Some(self.status.clone()),
            product_arn: Some(self.arn.clone()),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            source_connection: self.source_connection.clone(),
        }
    }

    /// The stored tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }
}

/// One persisted provisioning artifact (a product "version").
#[derive(Debug, Clone)]
pub struct ProvisioningArtifact {
    /// The minted artifact id (`pa-…`).
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// A `ProvisioningArtifactType` value, or `None` when neither the request nor the
    /// product type named one.
    pub r#type: Option<String>,
    /// Whether the artifact can be used to provision; a created artifact is active.
    pub active: bool,
    /// A `ProvisioningArtifactGuidance` value: [`ARTIFACT_GUIDANCE_DEFAULT`] until an
    /// update deprecates it.
    pub guidance: String,
    /// Creation instant, minted once at create time.
    pub created_time: SystemTime,
    /// The artifact's `Info` template source, as the request gave it.
    pub info: BTreeMap<String, String>,
}

/// The `Guidance` a newly created provisioning artifact carries.
///
/// The modelled `ProvisioningArtifactGuidance` enum is `DEFAULT | DEPRECATED`
/// (`smithy_query query="shape" shape="ProvisioningArtifactGuidance"`), and
/// `UpdateProvisioningArtifact`'s `Guidance` member is what moves an artifact to
/// `DEPRECATED`, so a create leaves it at `DEFAULT`.
pub const ARTIFACT_GUIDANCE_DEFAULT: &str = "DEFAULT";

impl ProvisioningArtifact {
    /// This record as the modelled `ProvisioningArtifactDetail`.
    pub fn detail(&self) -> ProvisioningArtifactDetail {
        ProvisioningArtifactDetail {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            r#type: self.r#type.clone(),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            active: Some(self.active),
            guidance: Some(self.guidance.clone()),
            // A source revision exists only for an artifact a connection sync created,
            // and no sync has run in this emulator.
            source_revision: None,
        }
    }

    /// This record as the modelled `ProvisioningArtifactSummary`.
    pub fn summary(&self) -> ProvisioningArtifactSummary {
        ProvisioningArtifactSummary {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            provisioning_artifact_metadata: Some(
                self.info
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }
    }

    /// This record as the modelled `ProvisioningArtifact`, the shorter shape
    /// `DescribeProduct` lists a product's versions as.
    pub fn listed(&self) -> ModelledProvisioningArtifact {
        ModelledProvisioningArtifact {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            guidance: Some(self.guidance.clone()),
        }
    }
}

/// A persisted self-service action.
#[derive(Debug, Clone)]
pub struct ServiceAction {
    /// The minted service-action id (`act-…`).
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// A `ServiceActionDefinitionType` value, as the request gave it.
    pub definition_type: String,
    /// The action's definition, keyed by `ServiceActionDefinitionKey`. A `BTreeMap` so
    /// the map a response carries has a stable order.
    pub definition: BTreeMap<String, String>,
}

impl ServiceAction {
    /// This record as the modelled `ServiceActionSummary`, which `ListServiceActions`
    /// reports instead of the fuller detail.
    pub fn summary(&self) -> ServiceActionSummary {
        ServiceActionSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            definition_type: Some(self.definition_type.clone()),
        }
    }

    /// This record as the modelled `ServiceActionDetail`.
    pub fn detail(&self) -> ServiceActionDetail {
        ServiceActionDetail {
            service_action_summary: Some(self.summary()),
            definition: Some(
                self.definition
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }
    }
}

/// A persisted TagOption.
#[derive(Debug, Clone)]
pub struct TagOption {
    /// The minted TagOption id (`tag-…`).
    pub id: String,
    pub key: String,
    pub value: String,
    /// Whether the TagOption may be associated with a resource; `UpdateTagOption`'s
    /// `Active` member is what changes it.
    pub active: bool,
    /// The account that created the TagOption.
    pub owner: String,
}

impl TagOption {
    /// This record as the modelled `TagOptionDetail`.
    pub fn detail(&self) -> TagOptionDetail {
        TagOptionDetail {
            key: Some(self.key.clone()),
            value: Some(self.value.clone()),
            active: Some(self.active),
            id: Some(self.id.clone()),
            owner: Some(self.owner.clone()),
        }
    }
}

/// The most tags one portfolio can carry.
///
/// From the model: every operation that returns a portfolio's `Tags` targets the
/// `Tags` list, whose length is constrained `0..50`
/// (`smithy_query query="operation" operation="UpdatePortfolio"`), so a stored
/// portfolio can never hold more than 50 tags.
pub const MAX_PORTFOLIO_TAGS: usize = 50;

/// The most tags one product can carry.
///
/// From the model, the same way: every operation that returns a product's `Tags`
/// targets the `Tags` list constrained `0..50`
/// (`smithy_query query="operation" operation="UpdateProduct"`), so a stored product
/// can never hold more than 50 tags either.
pub const MAX_PRODUCT_TAGS: usize = 50;

/// The most portfolios one account can hold, which also bounds this store.
///
/// AWS's documented default Service Catalog quota is 100 portfolios per account
/// per Region, and `CreatePortfolio` models `LimitExceededException` ("The current
/// limits of the service would have been exceeded by this operation") for exactly
/// that refusal. Because the live set is capped, so is every response built from
/// it: `ListPortfolios` cannot return more than this many records on a page.
pub const MAX_PORTFOLIOS: usize = 100;

/// The most principals one portfolio can carry, bounding that map the same way and
/// answered with the `LimitExceededException` that
/// `AssociatePrincipalWithPortfolio` models. The emulator's own bound, finite and
/// small, checked before every association is stored.
pub const MAX_PRINCIPALS_PER_PORTFOLIO: usize = 100;

/// The most constraints this account can hold, which also bounds this store.
///
/// `CreateConstraint` models `LimitExceededException` for a create past the account's
/// quota. As with [`MAX_PRODUCTS`] this is the emulator's own bound, not a quoted AWS
/// figure, and it is checked before every insert.
pub const MAX_CONSTRAINTS: usize = 500;

/// The most products this account can hold, which also bounds this store.
///
/// `CreateProduct` models `LimitExceededException` ("The current limits of the service
/// would have been exceeded by this operation") for a create past the account's product
/// quota. AWS's published default figure is not reproduced here — this constant is the
/// EMULATOR's bound, set at the scale of the one quota this file does cite (100
/// portfolios) — and it is what keeps a shared in-memory store from growing without
/// bound: every stored product holds its provisioning artifacts and at most one
/// idempotency entry, so bounding the count bounds all three.
pub const MAX_PRODUCTS: usize = 500;

/// The most self-service actions this account can hold, which also bounds this store.
///
/// `CreateServiceAction` models `LimitExceededException` for a create past the account's
/// quota. As with [`MAX_PRODUCTS`] this is the emulator's own bound, not a quoted AWS
/// figure.
pub const MAX_SERVICE_ACTIONS: usize = 500;

/// The most TagOptions this account can hold, which also bounds this store.
///
/// `CreateTagOption` models `LimitExceededException` for a create past the account's
/// quota. As with [`MAX_PRODUCTS`] this is the emulator's own bound, not a quoted AWS
/// figure. It also bounds the uniqueness scan every create runs over the live set.
pub const MAX_TAG_OPTIONS: usize = 1000;

/// Per-service state for com-amazonaws-servicecatalog-aws242servicecatalogservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsServicecatalogAws242servicecatalogserviceState {
    /// Every created, not-yet-deleted portfolio, keyed by portfolio id. A
    /// `BTreeMap` so `ListPortfolios` and its page tokens walk one stable order.
    pub portfolios: BTreeMap<String, Portfolio>,
    /// Every created, not-yet-deleted constraint, keyed by constraint id. A
    /// `BTreeMap` so `ListConstraintsForPortfolio` and its page tokens walk one
    /// stable order.
    pub constraints: BTreeMap<String, Constraint>,
    /// Every created, not-yet-deleted product, keyed by product id. A `BTreeMap` so
    /// every walk over the set has one stable order.
    pub products: BTreeMap<String, Product>,
    /// Every created, not-yet-deleted self-service action, keyed by action id.
    pub service_actions: BTreeMap<String, ServiceAction>,
    /// Every created, not-yet-deleted TagOption, keyed by TagOption id.
    pub tag_options: BTreeMap<String, TagOption>,
    /// `CreatePortfolio` idempotency: `IdempotencyToken` -> portfolio id, so a
    /// repeated create replays the first one instead of minting a second portfolio.
    ///
    /// PRIVATE, like the other token maps: every write goes through
    /// [`Self::remember_portfolio_token`], which prunes entries whose portfolio is gone,
    /// so the map holds at most one entry per live portfolio and a caller cycling fresh
    /// tokens cannot grow it past [`MAX_PORTFOLIOS`]. A handler reads it through
    /// [`Self::replayed_portfolio`].
    portfolio_idempotency: HashMap<String, String>,
    /// `CreateProduct` idempotency: `IdempotencyToken` -> product id. Kept apart from
    /// the portfolio map because the token namespace is per operation: the same token
    /// may legitimately create one portfolio and one product. Private and pruned on
    /// every write, exactly as above, so it is bounded by [`MAX_PRODUCTS`].
    product_idempotency: HashMap<String, String>,
    /// `CreateServiceAction` idempotency: `IdempotencyToken` -> service-action id.
    /// Private and pruned on every write, so it is bounded by [`MAX_SERVICE_ACTIONS`].
    service_action_idempotency: HashMap<String, String>,
    /// `CreateConstraint` idempotency: `IdempotencyToken` -> constraint id. Private and
    /// pruned on every write, so it is bounded by [`MAX_CONSTRAINTS`].
    constraint_idempotency: HashMap<String, String>,
    /// Counter behind every `next_*_id`. Deterministic, so a fresh
    /// emulator mints the same id sequence every run.
    pub next_id_seed: u64,
}

impl ComAmazonawsServicecatalogAws242servicecatalogserviceState {
    /// The portfolio a create carrying `token` already made, if that create's portfolio
    /// is still stored. `None` for an unseen token, or one whose portfolio was deleted —
    /// which is no longer a replay.
    pub fn replayed_portfolio(&self, token: &str) -> Option<&Portfolio> {
        self.portfolios.get(self.portfolio_idempotency.get(token)?)
    }

    /// Remember a `CreatePortfolio` idempotency token so a repeat replays
    /// `portfolio_id`.
    ///
    /// Every entry whose portfolio is no longer stored is dropped first, so the map holds
    /// at most one entry per live portfolio: a caller cycling fresh tokens — including
    /// after a delete, or once the quota is reached — cannot grow it past
    /// [`MAX_PORTFOLIOS`].
    pub fn remember_portfolio_token(&mut self, token: String, portfolio_id: String) {
        let portfolios = &self.portfolios;
        self.portfolio_idempotency
            .retain(|_, id| portfolios.contains_key(id));
        self.portfolio_idempotency.insert(token, portfolio_id);
    }

    /// Drop every idempotency token pointing at `portfolio_id`, which a delete has just
    /// removed: a create repeating one of them creates a new portfolio.
    pub fn forget_portfolio_tokens(&mut self, portfolio_id: &str) {
        self.portfolio_idempotency
            .retain(|_, id| id != portfolio_id);
    }

    /// The product a create carrying `token` already made, if it is still stored.
    pub fn replayed_product(&self, token: &str) -> Option<&Product> {
        self.products.get(self.product_idempotency.get(token)?)
    }

    /// Remember a `CreateProduct` idempotency token so a repeat replays `product_id`,
    /// bounded exactly as [`Self::remember_portfolio_token`] is.
    pub fn remember_product_token(&mut self, token: String, product_id: String) {
        let products = &self.products;
        self.product_idempotency
            .retain(|_, id| products.contains_key(id));
        self.product_idempotency.insert(token, product_id);
    }

    /// The self-service action a create carrying `token` already made, if it is still
    /// stored.
    pub fn replayed_service_action(&self, token: &str) -> Option<&ServiceAction> {
        self.service_actions
            .get(self.service_action_idempotency.get(token)?)
    }

    /// Remember a `CreateServiceAction` idempotency token, bounded the same way.
    pub fn remember_service_action_token(&mut self, token: String, action_id: String) {
        let actions = &self.service_actions;
        self.service_action_idempotency
            .retain(|_, id| actions.contains_key(id));
        self.service_action_idempotency.insert(token, action_id);
    }

    /// Drop every idempotency token pointing at `action_id`, which a delete has just
    /// removed.
    pub fn forget_service_action_tokens(&mut self, action_id: &str) {
        self.service_action_idempotency
            .retain(|_, id| id != action_id);
    }

    /// The constraint a create carrying `token` already made, if it is still stored.
    pub fn replayed_constraint(&self, token: &str) -> Option<&Constraint> {
        self.constraints.get(self.constraint_idempotency.get(token)?)
    }

    /// Remember a `CreateConstraint` idempotency token, bounded the same way.
    pub fn remember_constraint_token(&mut self, token: String, constraint_id: String) {
        let constraints = &self.constraints;
        self.constraint_idempotency
            .retain(|_, id| constraints.contains_key(id));
        self.constraint_idempotency.insert(token, constraint_id);
    }

    /// Drop every idempotency token pointing at `constraint_id`, which a delete has just
    /// removed.
    pub fn forget_constraint_tokens(&mut self, constraint_id: &str) {
        self.constraint_idempotency
            .retain(|_, id| id != constraint_id);
    }

    /// Mint an unused portfolio id of AWS's form: `port-` followed by 12
    /// lowercase base-36 characters, which satisfies the modelled `Id` pattern
    /// `^[a-zA-Z0-9_\-]*$` and its 1..100 length.
    pub fn next_portfolio_id(&mut self) -> String {
        loop {
            let id = self.mint_id("port");
            if !self.portfolios.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused constraint id (`cons-…`), the same shape as a portfolio id.
    pub fn next_constraint_id(&mut self) -> String {
        loop {
            let id = self.mint_id("cons");
            if !self.constraints.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused product id (`prod-…`), the same shape as a portfolio id.
    pub fn next_product_id(&mut self) -> String {
        loop {
            let id = self.mint_id("prod");
            if !self.products.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused product VIEW id (`prodview-…`), which `ProductViewSummary.Id`
    /// carries beside the product's own id.
    pub fn next_product_view_id(&mut self) -> String {
        loop {
            let id = self.mint_id("prodview");
            if !self.products.values().any(|product| product.view_id == id) {
                return id;
            }
        }
    }

    /// Mint an unused provisioning-artifact id (`pa-…`), unique across every product's
    /// artifacts.
    pub fn next_provisioning_artifact_id(&mut self) -> String {
        loop {
            let id = self.mint_id("pa");
            if !self
                .products
                .values()
                .any(|product| product.provisioning_artifacts.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// Mint an unused service-action id (`act-…`).
    pub fn next_service_action_id(&mut self) -> String {
        loop {
            let id = self.mint_id("act");
            if !self.service_actions.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused TagOption id (`tag-…`).
    pub fn next_tag_option_id(&mut self) -> String {
        loop {
            let id = self.mint_id("tag");
            if !self.tag_options.contains_key(&id) {
                return id;
            }
        }
    }

    /// One id of the form `<prefix>-<12 lowercase base-36 characters>`, advancing the
    /// shared counter. Collision against the live set is the caller's check, which is
    /// what the `next_*_id` methods above add.
    fn mint_id(&mut self, prefix: &str) -> String {
        self.next_id_seed = self.next_id_seed.wrapping_add(1);
        format!("{prefix}-{}", base36_12(scramble(self.next_id_seed)))
    }
}

/// SplitMix64's finalizer: spreads a small counter over the whole 64-bit range so
/// successive ids do not look sequential. Deterministic and injective.
fn scramble(counter: u64) -> u64 {
    let mut z = counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as exactly 12 lowercase base-36 digits (`0-9a-z`), most significant
/// first. 36^12 > 4.7e18, so the modulo keeps the full width of the alphabet.
fn base36_12(value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    const WIDTH: usize = 12;
    let mut n = value % 36u64.pow(WIDTH as u32);
    let mut out = [b'0'; WIDTH];
    for slot in out.iter_mut().rev() {
        *slot = DIGITS[(n % 36) as usize];
        n /= 36;
    }
    out.iter().map(|digit| char::from(*digit)).collect()
}

/// Encode the stored key a page resumes AT as an opaque page token: hex, so a
/// client cannot read a token as a resource id and the value stays inside the
/// modelled `PageToken` pattern and its 0..2024 length.
///
/// The one cursor codec every paged list over an id-keyed map in this service
/// shares, so a token one of them mints is the same shape as the next one's.
pub fn encode_page_cursor(key: &str) -> String {
    hex::encode(key)
}

/// The inverse of [`encode_page_cursor`]. `None` for anything this service did not
/// mint: not hex, not UTF-8, or not a value the modelled `Id` pattern
/// `^[a-zA-Z0-9_\-]*$` allows.
pub fn decode_page_cursor(token: &str) -> Option<String> {
    let key = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let well_formed = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    well_formed.then_some(key)
}

/// The failure a handler answers when this service's state cannot be reached —
/// i.e. the emulator was built on an engine type this service cannot use, so the
/// operation cannot be served at all. Unreachable in the assembled emulator,
/// which builds exactly one `InMemoryEngine`; never answer it by constructing a
/// private engine, whose writes no later request can see.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "this emulator was built on an engine this service cannot reach its state \
                  through, so no Service Catalog operation can be served"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsServicecatalogAws242servicecatalogserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsServicecatalogAws242servicecatalogserviceState>)
}

/// The region this request was signed for, which a minted ARN carries. Falls back to
/// the emulator's default region for an unsigned request, which has no scope to read a
/// region from.
///
/// Shared by every handler that mints an ARN, so one request's region cannot be read
/// two different ways.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request's ARNs are minted in.
const DEFAULT_REGION: &str = "us-east-1";
