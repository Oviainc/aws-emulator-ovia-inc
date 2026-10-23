// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-datazone-datazone service.
//!
//! The DataZone emulator stores every resource it serves here: a handler reads and
//! writes this struct and NOTHING else, so a create is observable by the next
//! get/list and a delete is observable by the next call.
//!
//! # Time and clock-owed transitions
//!
//! Instants are kept as milliseconds since the Unix epoch ([`now_ms`]) and rendered
//! with [`date_time`]. A domain AWS answers `CREATING` becomes `AVAILABLE`, and one it
//! answers `DELETING` disappears, once the clock passes the instant recorded in
//! `status_due_ms` — materialised by [`ComAmazonawsDatazoneDatazoneState::settle_clock`],
//! which every handler calls first. No handler ever sleeps for it.

use std::collections::BTreeMap;

/// The span the emulator takes to finish a domain lifecycle step AWS reports as
/// in-progress (`CREATING` -> `AVAILABLE`, `DELETING` -> gone).
///
/// AWS does not document how long either takes, so this is a CHOSEN span. It is
/// deliberately non-zero: a caller that polls immediately after the call must still
/// observe the in-progress status the real service reports.
pub const LIFECYCLE_SETTLE_MS: i64 = 300;

/// `DomainStatus` values this emulator uses (model enum `com.amazonaws.datazone#DomainStatus`:
/// AVAILABLE | CREATING | CREATION_FAILED | DELETED | DELETING | DELETION_FAILED).
pub const DOMAIN_STATUS_CREATING: &str = "CREATING";
pub const DOMAIN_STATUS_AVAILABLE: &str = "AVAILABLE";
pub const DOMAIN_STATUS_DELETING: &str = "DELETING";

/// The `ProjectStatus` a project AWS has finished creating carries (model enum
/// `com.amazonaws.datazone#ProjectStatus`: ACTIVE | DELETING | UPDATING | DELETE_FAILED).
pub const PROJECT_STATUS_ACTIVE: &str = "ACTIVE";

/// The one `UserDesignation` that points at a designation the caller created, rather
/// than naming a role the service defines (model enum
/// `com.amazonaws.datazone#UserDesignation`: PROJECT_OWNER | PROJECT_CONTRIBUTOR |
/// PROJECT_CATALOG_VIEWER | PROJECT_CATALOG_CONSUMER | PROJECT_CATALOG_STEWARD |
/// USER_CREATED_DESIGNATION).
pub const DESIGNATION_USER_CREATED: &str = "USER_CREATED_DESIGNATION";

/// The `SortOrder` that reverses a listing (model enum
/// `com.amazonaws.datazone#SortOrder`: ASCENDING | DESCENDING).
pub const SORT_ORDER_DESCENDING: &str = "DESCENDING";

/// The `DomainVersion` a domain gets when the request names none (model enum
/// `com.amazonaws.datazone#DomainVersion`: V1 | V2).
pub const DEFAULT_DOMAIN_VERSION: &str = "V1";

/// The `FormTypeStatus` a metadata form type gets when the request names none (model
/// enum `com.amazonaws.datazone#FormTypeStatus`: ENABLED | DISABLED). A form type is
/// usable as soon as it is created, so the default is the usable member.
pub const FORM_TYPE_STATUS_ENABLED: &str = "ENABLED";

/// `FormTypeName`'s modelled maximum length, 1..128 (smithy_query shape="FormTypeName").
pub const FORM_TYPE_NAME_MAX_LEN: usize = 128;

/// The `@pattern` the model declares on `FormTypeName`, the shape a form type's `name`
/// and a form entry's reported `typeName` both target.
///
/// SOURCE: `smithy_query(query="shape", shape="FormTypeName")`. The negative lookahead
/// is why it is checked here at all: the vendored boundary validator compiles a
/// `@pattern` with the `regex` crate, which has no look-around, and treats a pattern it
/// cannot compile as matching (`vendor/smithy-schema-server/src/constraints.rs`), so
/// this pattern would otherwise reach the store unchecked.
pub const FORM_TYPE_NAME_PATTERN: &str =
    r"^(amazon.datazone.)?(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$";

/// [`FORM_TYPE_NAME_PATTERN`], checked directly rather than compiled.
///
/// The pattern needs look-around, which the `regex` crate does not offer, and a Smithy
/// `@pattern` is an ECMA-262 expression, where `\w` is the ASCII class `[A-Za-z0-9_]`.
/// So each alternative is read off the pattern and checked in turn:
///
/// * `^_\w*[a-zA-Z0-9]\w*$` — the NAME ITSELF starts with an underscore and some ASCII
///   alphanumeric follows it. The `amazon.datazone.` prefix belongs to the other
///   alternative only, so `amazon.datazone._0` is NOT a form type name.
/// * `^(amazon.datazone.)?(?![0-9_])\w+$` — otherwise, every character is an ASCII word
///   character once the optional literal prefix is taken off, and the first of them is
///   a letter (the look-ahead rules out a digit, and an underscore there would have
///   matched the first alternative).
fn matches_form_type_name_pattern(name: &str) -> bool {
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    if let Some(rest) = name.strip_prefix('_') {
        if rest.bytes().all(is_word) && rest.bytes().any(|byte| byte.is_ascii_alphanumeric()) {
            return true;
        }
    }
    let bare = name.strip_prefix("amazon.datazone.").unwrap_or(name);
    bare.bytes().all(is_word)
        && bare
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
}

/// Whether `name` satisfies `FormTypeName`'s modelled length and pattern.
///
/// Shared by every handler that takes or reports a form type name — `CreateFormType`
/// validates its `name` with it, and `CreateAssetType` decides with it whether a form
/// entry's `typeIdentifier` is already a well-formed name — so the two cannot drift.
pub fn is_form_type_name(name: &str) -> bool {
    (1..=FORM_TYPE_NAME_MAX_LEN).contains(&name.chars().count())
        && matches_form_type_name_pattern(name)
}

/// The `GlossaryStatus` members (model enum `com.amazonaws.datazone#GlossaryStatus`:
/// DISABLED | ENABLED).
pub const GLOSSARY_STATUS_ENABLED: &str = "ENABLED";
pub const GLOSSARY_STATUS_DISABLED: &str = "DISABLED";

/// The `Revision` (`com.amazonaws.datazone#Revision`, len 1..64) a freshly created
/// revisioned resource carries: DataZone numbers revisions from 1, as a decimal string.
pub const FIRST_REVISION: &str = "1";

/// The `ListingStatus` members (model enum `com.amazonaws.datazone#ListingStatus`:
/// CREATING | ACTIVE | INACTIVE).
pub const LISTING_STATUS_CREATING: &str = "CREATING";
pub const LISTING_STATUS_ACTIVE: &str = "ACTIVE";
pub const LISTING_STATUS_INACTIVE: &str = "INACTIVE";

/// The `ChangeAction` members (model enum `com.amazonaws.datazone#ChangeAction`:
/// PUBLISH | UNPUBLISH) — the two change sets `CreateListingChangeSet` applies.
pub const CHANGE_ACTION_PUBLISH: &str = "PUBLISH";
pub const CHANGE_ACTION_UNPUBLISH: &str = "UNPUBLISH";

/// The `EntityType` members (model enum `com.amazonaws.datazone#EntityType`:
/// ASSET | DATA_PRODUCT) — what a listing can be a record OF.
pub const ENTITY_TYPE_ASSET: &str = "ASSET";
pub const ENTITY_TYPE_DATA_PRODUCT: &str = "DATA_PRODUCT";

/// The only `DataZoneEntityType` member (model enum
/// `com.amazonaws.datazone#DataZoneEntityType`: DOMAIN_UNIT) — the kind of entity
/// `AddEntityOwner`, `ListEntityOwners` and `RemoveEntityOwner` work on, so an owner is
/// kept on a stored domain unit.
pub const DATA_ZONE_ENTITY_TYPE_DOMAIN_UNIT: &str = "DOMAIN_UNIT";

/// The `listingRevision` that names a listing's newest revision: the `@default` the
/// model puts on `GetListingInput::listingRevision` (smithy_query operation="GetListing").
pub const LISTING_REVISION_LATEST: &str = "latest";

/// `ListingName`'s modelled maximum length, 1..64 (smithy_query shape="ListingName").
pub const LISTING_NAME_MAX_LEN: usize = 64;

/// Milliseconds since the Unix epoch, now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Render an epoch-millisecond instant as the modelled timestamp type.
pub fn date_time(ms: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(ms as f64 / 1000.0)
}

/// One stored DataZone domain, with the projects nested under it.
#[derive(Debug, Clone)]
pub struct Domain {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub single_sign_on: Option<StoredSingleSignOn>,
    pub domain_execution_role: String,
    pub service_role: Option<String>,
    pub kms_key_identifier: Option<String>,
    pub status: String,
    pub portal_url: Option<String>,
    pub domain_version: String,
    pub root_domain_unit_id: String,
    pub managed_account_id: String,
    pub tags: BTreeMap<String, String>,
    pub created_at_ms: i64,
    pub last_updated_at_ms: i64,
    /// When the status this domain currently reports stops being true: the instant
    /// `CREATING` becomes `AVAILABLE`, or `DELETING` becomes gone. `None` for a
    /// settled domain.
    pub status_due_ms: Option<i64>,
    /// The projects stored in this domain, so they go with it when it is deleted.
    /// Bounded by [`MAX_PROJECTS_PER_DOMAIN`].
    pub projects: BTreeMap<String, Project>,
    /// The metadata form types stored in this domain, keyed by their `FormTypeName`
    /// — the same string `GetFormType`/`DeleteFormType` address them by
    /// (`formTypeIdentifier`). Nested under the domain so they go with it when it is
    /// deleted. Bounded by [`MAX_FORM_TYPES_PER_DOMAIN`].
    pub form_types: BTreeMap<String, FormType>,
    /// The catalog assets stored in this domain, keyed by their `AssetId`, so they go
    /// with the domain when it is deleted. Bounded by [`MAX_ASSETS_PER_DOMAIN`].
    pub assets: BTreeMap<String, Asset>,
    /// The policy grants stored in this domain, keyed by `grantId`, so they go with it
    /// when it is deleted. A `BTreeMap` so every `ListPolicyGrants` iterates in one
    /// stable order and a pagination token can resume from a key. Bounded by
    /// [`MAX_POLICY_GRANTS_PER_DOMAIN`].
    pub policy_grants: BTreeMap<String, PolicyGrant>,
    /// The business glossaries stored in this domain, keyed by glossary id, so they go
    /// with the domain when it is deleted. Bounded by [`MAX_GLOSSARIES_PER_DOMAIN`].
    pub glossaries: BTreeMap<String, Glossary>,
    /// The custom asset types stored in this domain, keyed by the type's NAME — the
    /// `AssetTypeIdentifier` `GetAssetType`/`DeleteAssetType` address a type by — so a
    /// name is unique within the domain and the types go with it when it is deleted.
    /// Bounded by [`MAX_ASSET_TYPES_PER_DOMAIN`].
    pub asset_types: BTreeMap<String, AssetType>,
    /// The domain units stored in this domain, keyed by id, so they go with it when it
    /// is deleted. Always holds at least the ROOT unit `root_domain_unit_id` names,
    /// which is created with the domain itself. Bounded by
    /// [`MAX_DOMAIN_UNITS_PER_DOMAIN`].
    pub domain_units: BTreeMap<String, DomainUnit>,
    /// The catalog listings stored in this domain, keyed by their `ListingId` — the
    /// identifier `GetListing`/`DeleteListing` address one by — so they go with the
    /// domain when it is deleted. Bounded by [`MAX_LISTINGS_PER_DOMAIN`].
    pub listings: BTreeMap<String, Listing>,
}

impl Domain {
    /// The listing that records the entity `entity_type`/`entity_id`, when that entity
    /// has been published in this domain's catalog.
    ///
    /// At most one listing records one entity: a listing IS the entity's record in the
    /// catalog, and `CreateListingChangeSet` adds a revision to that record rather than
    /// a second listing. A linear scan, deliberately: the map is bounded by
    /// [`MAX_LISTINGS_PER_DOMAIN`], so one lookup rule shared by every handler is worth
    /// more than a second index to keep in step (the same trade
    /// [`ComAmazonawsDatazoneDatazoneState::live_domain_named`] makes).
    pub fn listing_for_entity(&self, entity_type: &str, entity_id: &str) -> Option<&Listing> {
        self.listings
            .values()
            .find(|listing| listing.entity_type == entity_type && listing.entity_id == entity_id)
    }

    /// [`Self::listing_for_entity`], for a change set that writes the listing.
    pub fn listing_for_entity_mut(
        &mut self,
        entity_type: &str,
        entity_id: &str,
    ) -> Option<&mut Listing> {
        self.listings
            .values_mut()
            .find(|listing| listing.entity_type == entity_type && listing.entity_id == entity_id)
    }
}

/// One stored catalog listing: an entity published in the domain's catalog.
///
/// A listing is "a record of an asset at a given time" (smithy_query
/// operation="CreateListingChangeSet"), so each PUBLISH change set records a further
/// [`ListingRevision`] — a snapshot of the published entity as it was at that instant —
/// while the listing itself keeps the identity (`id`) the catalog addresses it by.
/// An UNPUBLISH change set takes the record out of the catalog by moving `status` to
/// [`LISTING_STATUS_INACTIVE`]; it adds no snapshot, because nothing about the entity
/// was recorded.
#[derive(Debug, Clone)]
pub struct Listing {
    pub id: String,
    pub domain_id: String,
    /// Which kind of entity this listing records: one of [`ENTITY_TYPE_ASSET`] /
    /// [`ENTITY_TYPE_DATA_PRODUCT`].
    pub entity_type: String,
    /// The id of the entity this listing records, within the same domain.
    pub entity_id: String,
    /// One of [`LISTING_STATUS_CREATING`] / [`LISTING_STATUS_ACTIVE`] /
    /// [`LISTING_STATUS_INACTIVE`].
    pub status: String,
    /// When a [`LISTING_STATUS_CREATING`] listing becomes [`LISTING_STATUS_ACTIVE`].
    /// `None` for a settled listing — one that is ACTIVE or INACTIVE.
    pub status_due_ms: Option<i64>,
    /// The caller and instant of the change set that FIRST published this entity.
    pub created_by: String,
    pub created_at_ms: i64,
    /// The caller and instant of the LATEST change set applied to this listing.
    pub updated_by: String,
    pub updated_at_ms: i64,
    /// Every recorded revision, keyed by its number so the greatest key is the latest.
    /// Bounded by [`MAX_LISTING_REVISIONS`].
    pub revisions: BTreeMap<u32, ListingRevision>,
    /// The `clientToken` of the UNPUBLISH change set that last deactivated this
    /// listing, so a replay of that request is answered rather than re-applied.
    pub unpublish_client_token: Option<String>,
}

impl Listing {
    /// The greatest revision number recorded, i.e. the one a `GetListing` that asks for
    /// [`LISTING_REVISION_LATEST`] reports.
    pub fn latest_revision(&self) -> Option<&ListingRevision> {
        self.revisions.values().next_back()
    }

    /// The revision addressed by a `listingRevision` string, which is the decimal
    /// number the change set that recorded it returned. Addressing is exact: only the
    /// canonical decimal resolves, so `"01"` and `"+1"` name no revision — the same
    /// rule [`FormType::revision`] applies.
    pub fn revision(&self, revision: &str) -> Option<&ListingRevision> {
        let number = revision.parse::<u32>().ok()?;
        if number.to_string() != revision {
            return None;
        }
        self.revisions.get(&number)
    }

    /// The number the NEXT recorded revision gets: one past the greatest ever recorded,
    /// so revision numbers never go backwards.
    pub fn next_revision_number(&self) -> u32 {
        self.revisions.keys().next_back().copied().unwrap_or(0) + 1
    }

    /// The revision carrying `token`, if a PUBLISH change set with that `clientToken`
    /// already recorded one of this listing's revisions.
    pub fn revision_with_token(&self, token: &str) -> Option<&ListingRevision> {
        self.revisions
            .values()
            .find(|revision| revision.client_token.as_deref() == Some(token))
    }
}

/// One recorded revision of a listing: the published entity as it was when the PUBLISH
/// change set that recorded it ran.
///
/// The snapshot is taken at publish time deliberately — a listing is a record of the
/// entity AT A GIVEN TIME, so a later change to the entity does not rewrite a revision
/// the catalog already holds.
#[derive(Debug, Clone)]
pub struct ListingRevision {
    /// The revision number, rendered as the modelled `Revision` string: the first one
    /// recorded for a listing renders as [`FIRST_REVISION`].
    pub revision: u32,
    /// The revision of the ENTITY this snapshot records.
    pub entity_revision: String,
    /// The entity's name, clipped to [`LISTING_NAME_MAX_LEN`] by [`listing_name`].
    pub name: String,
    pub description: Option<String>,
    /// The entity's own type — an asset's `typeIdentifier`, reported as the listing
    /// item's `assetType`.
    pub entity_type_name: String,
    /// The project that owns the published entity.
    pub owning_project_id: String,
    /// When the published ENTITY was created, which is what a listing item's
    /// `createdAt` reports (smithy_query shape="AssetListing" member="createdAt").
    pub entity_created_at_ms: i64,
    /// The entity's metadata forms as the modelled `Forms` string, built by
    /// [`listing_forms`]. `None` when the entity has no form attached.
    pub forms: Option<String>,
    /// The caller and instant of the change set that recorded this revision.
    pub created_by: String,
    pub created_at_ms: i64,
    /// That change set's `clientToken`, so a replay of it is answered with this
    /// revision rather than recording a second one.
    pub client_token: Option<String>,
}

/// An entity's name as the listing that records it reports it.
///
/// `ListingName` is 1..64 while the name of a publishable entity is longer-ranged
/// (`AssetName` is 1..256, smithy_query shape="AssetName"), so a name that does not fit
/// the listing's own modelled bound is clipped to it — on a CHARACTER boundary, so the
/// result is still valid UTF-8 — rather than reported in violation of the model.
pub fn listing_name(entity_name: &str) -> String {
    entity_name.chars().take(LISTING_NAME_MAX_LEN).collect()
}

/// An entity's metadata forms as the modelled `Forms` string a listing reports.
///
/// `Forms` is a plain string (smithy_query shape="Forms") and a listing reports the
/// forms attached to the published entity, so they are rendered as ONE JSON object
/// keyed by `formName` — the name a form is addressed by on its entity — whose value is
/// that form's content. Content that is itself JSON nests as that JSON, so the
/// rendering round-trips through a parser instead of double-encoding; content that is
/// not JSON stays a JSON string, and a form with no content renders as `null`.
///
/// `None` when the entity carries no form at all: there is then no object to report.
pub fn listing_forms(forms: &[StoredForm]) -> Option<String> {
    if forms.is_empty() {
        return None;
    }
    let mut rendered = serde_json::Map::new();
    for form in forms {
        let value = match form.content.as_deref() {
            None => serde_json::Value::Null,
            Some(content) => serde_json::from_str(content)
                .unwrap_or_else(|_| serde_json::Value::String(content.to_string())),
        };
        rendered.insert(form.form_name.clone(), value);
    }
    serde_json::to_string(&serde_json::Value::Object(rendered)).ok()
}

/// A recorded revision as the modelled `ListingItem` union (smithy_query
/// shape="ListingItem"), shared by every operation that reports a listing's item so the
/// forms cannot drift apart.
///
/// Which member of the union is reported follows the listing's own `entity_type`: the
/// snapshot is the same either way, and each union member names it with that entity's
/// own vocabulary (`assetId`/`assetRevision` against
/// `dataProductId`/`dataProductRevision`).
///
/// `glossaryTerms` is a `DetailedGlossaryTerms` — each term's NAME and short
/// description, not its id (smithy_query shape="DetailedGlossaryTerm") — and a term's
/// record is a resource this emulator does not store yet (`CreateGlossaryTerm` is a
/// scaffold), so no stored term can resolve the ids an entity carries and the member is
/// left absent rather than filled with an id dressed up as a name.
pub fn listing_item(
    listing: &Listing,
    revision: &ListingRevision,
) -> crate::ops::types::ListingItem {
    use crate::ops::types::{AssetListing, DataProductListing, ListingItem};

    if listing.entity_type == ENTITY_TYPE_DATA_PRODUCT {
        return ListingItem::dataProductListing(DataProductListing {
            data_product_id: Some(listing.entity_id.clone()),
            data_product_revision: Some(revision.entity_revision.clone()),
            created_at: Some(date_time(revision.entity_created_at_ms)),
            forms: revision.forms.clone(),
            glossary_terms: None,
            owning_project_id: Some(revision.owning_project_id.clone()),
            // The assets of a data product listing come from the data product's own
            // record, which no store holds yet.
            items: None,
        });
    }
    ListingItem::assetListing(AssetListing {
        asset_id: Some(listing.entity_id.clone()),
        asset_revision: Some(revision.entity_revision.clone()),
        asset_type: Some(revision.entity_type_name.clone()),
        created_at: Some(date_time(revision.entity_created_at_ms)),
        forms: revision.forms.clone(),
        // Time series data points are imported by `PostTimeSeriesDataPoints`, which is
        // still a scaffold, so no published entity has any.
        latest_time_series_data_point_forms: None,
        glossary_terms: None,
        // Governed glossary terms are attached by `AssociateGovernedTerms`, which is
        // still a scaffold, so no published entity carries any.
        governed_glossary_terms: None,
        owning_project_id: Some(revision.owning_project_id.clone()),
    })
}

/// One stored DataZone domain unit: a node of the domain's ownership hierarchy.
///
/// Every domain has exactly one ROOT unit — the one `Domain::root_domain_unit_id`
/// names, created with the domain — and every other unit names a parent, so the units
/// of one domain form a tree rooted there.
#[derive(Debug, Clone)]
pub struct DomainUnit {
    pub id: String,
    pub domain_id: String,
    pub name: String,
    pub description: Option<String>,
    /// The unit this one hangs under. `None` only for the domain's root unit.
    pub parent_domain_unit_id: Option<String>,
    /// The unit's owners, in the order they were added: the principal that created the
    /// unit, until `AddEntityOwner` adds another.
    pub owners: Vec<DomainUnitOwner>,
    pub created_by: String,
    pub created_at_ms: i64,
    /// Who last changed the unit, and when — absent until something does.
    pub last_updated_by: Option<String>,
    pub last_updated_at_ms: Option<i64>,
}

/// The stored form of the modelled `DomainUnitOwnerProperties` union, whose members
/// are `user` and `group` (smithy_query shape="DomainUnitOwnerProperties").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainUnitOwner {
    User(String),
    Group(String),
}

impl DomainUnitOwner {
    /// The principal this owner names, and the tag that tells a user from a group —
    /// the two members both owner unions declare (smithy_query
    /// shape="OwnerProperties,DomainUnitOwnerProperties").
    pub fn parts(&self) -> (&str, &'static str) {
        match self {
            Self::User(user_id) => (user_id.as_str(), "user"),
            Self::Group(group_id) => (group_id.as_str(), "group"),
        }
    }
}

/// The key one owner is identified and ordered by.
///
/// Injective in `(identifier, kind)` for the reason [`membership_key`] is: the kind tag
/// is always last and the two tags are distinct, so no two different principals key to
/// the same string. It is what makes an entity's owners a SET — `AddEntityOwner`
/// compares owners by it — and gives `ListEntityOwners` one stable total order to
/// paginate over, independent of the order the owners were added in.
pub fn owner_key(owner: &DomainUnitOwner) -> String {
    let (identifier, tag) = owner.parts();
    format!("{identifier}{PRINCIPAL_KEY_SEPARATOR}{tag}")
}

/// The owner a request's `OwnerProperties` union names, in stored form (smithy_query
/// shape="OwnerProperties": `user` carries `userIdentifier`, `group` carries
/// `groupIdentifier`). Shared by `AddEntityOwner` and `RemoveEntityOwner`, so the two
/// cannot read the same request differently.
pub fn owner_from_properties(owner: &crate::ops::types::OwnerProperties) -> DomainUnitOwner {
    use crate::ops::types::OwnerProperties;
    match owner {
        OwnerProperties::User(user) => DomainUnitOwner::User(user.user_identifier.clone()),
        OwnerProperties::Group(group) => DomainUnitOwner::Group(group.group_identifier.clone()),
    }
}

/// A stored owner as the modelled `OwnerPropertiesOutput` union (smithy_query
/// shape="OwnerPropertiesOutput": `user` carries `userId`, `group` carries `groupId`),
/// the form `ListEntityOwners` reports. The companion of [`owner_properties`], which
/// renders the same stored owner in the union `GetDomainUnit` reports.
pub fn entity_owner_output(owner: &DomainUnitOwner) -> crate::ops::types::OwnerPropertiesOutput {
    use crate::ops::types::{
        OwnerGroupPropertiesOutput, OwnerPropertiesOutput, OwnerUserPropertiesOutput,
    };
    match owner {
        DomainUnitOwner::User(user_id) => OwnerPropertiesOutput::User(OwnerUserPropertiesOutput {
            user_id: Some(user_id.clone()),
        }),
        DomainUnitOwner::Group(group_id) => {
            OwnerPropertiesOutput::Group(OwnerGroupPropertiesOutput {
                group_id: Some(group_id.clone()),
            })
        }
    }
}

/// One stored metadata form type: a name, the project that owns it, and every
/// revision created under that name.
///
/// The model gives `CreateFormType` no companion `UpdateFormType` and makes
/// `revision` a REQUIRED output of both `CreateFormType` and `GetFormType`
/// (smithy_query operation="CreateFormType,GetFormType"), so re-creating a name is
/// the only way a second revision can come about: each create under an existing name
/// appends the next revision and leaves the earlier ones readable.
#[derive(Debug, Clone)]
pub struct FormType {
    pub name: String,
    pub domain_id: String,
    /// The project named by `owningProjectIdentifier` on the create that minted
    /// revision 1; a later create under this name must name the same project.
    pub owning_project_id: String,
    /// Every revision, keyed by its number so the greatest key is the latest.
    /// Bounded by [`MAX_FORM_TYPE_REVISIONS`].
    pub revisions: BTreeMap<u32, FormTypeRevision>,
}

impl FormType {
    /// The greatest revision number stored, i.e. the one a `GetFormType` that names no
    /// `revision` reports.
    pub fn latest_revision(&self) -> Option<&FormTypeRevision> {
        self.revisions.values().next_back()
    }

    /// The revision addressed by a `revision` string, which is the decimal number the
    /// create that minted it returned. Addressing is exact: only the canonical decimal
    /// resolves, so `"01"` and `"+1"` name no revision. `None` when this form type has
    /// no such revision.
    pub fn revision(&self, revision: &str) -> Option<&FormTypeRevision> {
        let number = revision.parse::<u32>().ok()?;
        if number.to_string() != revision {
            return None;
        }
        self.revisions.get(&number)
    }
}

/// One revision of a stored metadata form type.
#[derive(Debug, Clone)]
pub struct FormTypeRevision {
    /// The revision number, rendered as the modelled `Revision` string: the first one
    /// minted under a name renders as [`FIRST_REVISION`].
    pub revision: u32,
    /// The `smithy` member of the modelled `Model` union — the only member it has
    /// (smithy_query shape="Model") — exactly as the create supplied it.
    pub model_smithy: String,
    pub status: String,
    pub description: Option<String>,
    pub created_at_ms: i64,
    pub created_by: String,
}

impl FormTypeRevision {
    /// This revision as the modelled `Revision` string.
    pub fn revision_string(&self) -> String {
        self.revision.to_string()
    }
}

/// One stored DataZone business glossary.
///
/// A glossary belongs to a domain and is owned by a project in that domain; the terms
/// under it are a separate resource with its own operations.
#[derive(Debug, Clone)]
pub struct Glossary {
    pub id: String,
    pub domain_id: String,
    pub name: String,
    pub description: Option<String>,
    pub owning_project_id: String,
    /// One of [`GLOSSARY_STATUS_ENABLED`] / [`GLOSSARY_STATUS_DISABLED`].
    pub status: String,
    /// `usageRestrictions` as stored (model list `GlossaryUsageRestrictions`, 1..1 of
    /// `ASSET_GOVERNED_TERMS`): empty when the request named none.
    pub usage_restrictions: Vec<String>,
    pub created_by: String,
    pub created_at_ms: i64,
    /// Who last updated the glossary and when — `None` until something does, because
    /// `GetGlossary` reports `updatedAt`/`updatedBy` as optional members.
    pub updated_by: Option<String>,
    pub updated_at_ms: Option<i64>,
}

/// The stored form of the modelled `SingleSignOn` structure.
#[derive(Debug, Clone, Default)]
pub struct StoredSingleSignOn {
    pub sso_type: Option<String>,
    pub user_assignment: Option<String>,
    pub idc_instance_arn: Option<String>,
}

/// One stored DataZone project.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub domain_id: String,
    pub name: String,
    pub description: Option<String>,
    pub project_status: String,
    pub created_by: String,
    pub created_at_ms: i64,
    pub last_updated_at_ms: i64,
    pub glossary_terms: Vec<String>,
    pub domain_unit_id: Option<String>,
    pub project_profile_id: Option<String>,
    /// `resourceTags` as stored: the request takes a map, the response a list of
    /// `ResourceTag`, so the key order is kept stable by storing it sorted.
    pub resource_tags: BTreeMap<String, String>,
    pub project_category: Option<String>,
    /// This project's members, keyed by [`membership_key`] so one principal holds at
    /// most one designation in the project and every listing iterates in one stable
    /// order — the member identifier's. Bounded by [`MAX_MEMBERSHIPS_PER_PROJECT`].
    ///
    /// Nested in the project, so the memberships go with it when the project (or the
    /// domain above it) is deleted.
    pub memberships: BTreeMap<String, Membership>,
}

/// Whether a membership names a user or a group: the two arms of the modelled
/// `Member` and `MemberDetails` unions (smithy_query shape="Member,MemberDetails").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberKind {
    Group,
    User,
}

impl MemberKind {
    /// This arm's tag, as it is spelled inside a stored membership key.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::User => "user",
        }
    }

    /// The `Member` union member a request names this arm with, for a refusal message
    /// that names the input member the caller has to fix.
    pub const fn request_member(self) -> &'static str {
        match self {
            Self::Group => "member.groupIdentifier",
            Self::User => "member.userIdentifier",
        }
    }
}

/// One stored project membership: which principal holds which designation.
///
/// `ProjectMember` carries no timestamp (smithy_query shape="ProjectMember"), so a
/// membership stores nothing beyond what a listing reports.
#[derive(Debug, Clone)]
pub struct Membership {
    pub kind: MemberKind,
    pub identifier: String,
    pub designation: String,
}

/// The separator between a principal's identifier and its kind inside a key this state
/// builds for it — a membership key ([`membership_key`]) or an owner key
/// ([`owner_key`]).
///
/// A control character, so the key orders by the identifier FIRST — the only name this
/// emulator holds for a principal, and so the order `sortBy=NAME` asks for — while
/// still telling a user apart from a group that happens to share its identifier.
const PRINCIPAL_KEY_SEPARATOR: char = '\u{1}';

/// The key one membership is stored under.
///
/// Injective in `(identifier, kind)`: the kind tag is always last and the two tags are
/// distinct, so no two different members can key to the same string.
pub fn membership_key(kind: MemberKind, identifier: &str) -> String {
    format!("{identifier}{PRINCIPAL_KEY_SEPARATOR}{}", kind.tag())
}

/// The principal a `Member` union value names.
pub fn member_ref(member: &crate::ops::types::Member) -> (MemberKind, &str) {
    match member {
        crate::ops::types::Member::userIdentifier(id) => (MemberKind::User, id.as_str()),
        crate::ops::types::Member::groupIdentifier(id) => (MemberKind::Group, id.as_str()),
    }
}

/// Resolve the principal a membership names against the profiles its domain holds,
/// answering the refusal message when the domain holds none for it.
///
/// A GROUP member must already have a group profile in the domain: the service
/// resolves the profile and refuses the membership when the domain holds none, because
/// a group profile is only created implicitly for a domain that manages its own
/// identities. Group profiles are created by `CreateGroupProfile`, which is still a
/// scaffold here, so a domain holds none and the refusal is the only reachable branch —
/// the honest answer rather than a membership for a principal the domain does not
/// know. A USER needs no profile up front: the service creates one implicitly for the
/// principal the membership names.
pub fn resolve_membership_principal(
    kind: MemberKind,
    identifier: &str,
    domain_id: &str,
) -> Result<(), String> {
    if kind == MemberKind::Group {
        return Err(format!(
            "Group profile '{identifier}' not found for domain '{domain_id}', profile creation is only valid for implicit domain"
        ));
    }
    Ok(())
}

/// Why a requested membership cannot be stored.
#[derive(Debug)]
pub enum MembershipRefusal {
    /// The request breaks a rule the model documents; the string is the refusal
    /// message, naming the member and the constraint. Answer it with the operation's
    /// own `ValidationException`.
    Invalid(String),
    /// The membership names a principal the domain holds no profile for; the string is
    /// the refusal message. Answer it with the operation's own
    /// `ResourceNotFoundException`.
    ProfileNotFound(String),
    /// The membership names a resource this emulator does not serve yet; the string
    /// says which. Answer it with the scaffold's 501, never by storing something else.
    Unserved(String),
}

/// Validate one requested membership and turn it into the key and record to store.
///
/// Shared by `CreateProjectMembership` and by `CreateProject`'s
/// `membershipAssignments`, which carry the same three members and the same rules.
pub fn membership_from_request(
    member: &crate::ops::types::Member,
    designation: &str,
    user_created_designation_id: Option<&str>,
    domain_id: &str,
) -> Result<(String, Membership), MembershipRefusal> {
    let (kind, identifier) = member_ref(member);
    // The union's arms target plain `String`, so the boundary validator accepts an
    // empty one; a membership for no principal at all is a bad request.
    if identifier.is_empty() {
        return Err(MembershipRefusal::Invalid(format!(
            "{} must name a {}, but it is empty",
            kind.request_member(),
            kind.tag()
        )));
    }
    // The principal must be one the domain can hold a membership for.
    resolve_membership_principal(kind, identifier, domain_id)
        .map_err(MembershipRefusal::ProfileNotFound)?;
    // `userCreatedDesignationId` is documented as "Required when designation is
    // USER_CREATED_DESIGNATION; must not be provided otherwise" (smithy_query
    // query="doc" shape="CreateProjectMembershipInput"
    // member="userCreatedDesignationId", and the same sentence on
    // `ProjectMembershipAssignment`).
    match (
        designation == DESIGNATION_USER_CREATED,
        user_created_designation_id,
    ) {
        (true, None) => {
            return Err(MembershipRefusal::Invalid(format!(
                "userCreatedDesignationId is required when designation is {DESIGNATION_USER_CREATED}"
            )))
        }
        (false, Some(id)) => {
            return Err(MembershipRefusal::Invalid(format!(
                "userCreatedDesignationId {id} must not be provided when designation is {designation}"
            )))
        }
        // The id names a designation resource, which only `CreateDesignation` can
        // make and that operation is still a scaffold — so no such designation can
        // exist here, and accepting the membership would claim one does.
        (true, Some(_)) => {
            return Err(MembershipRefusal::Unserved(format!(
                "designation {DESIGNATION_USER_CREATED}, which names a designation resource (CreateDesignation)"
            )))
        }
        (false, None) => {}
    }
    Ok((
        membership_key(kind, identifier),
        Membership {
            kind,
            identifier: identifier.to_string(),
            designation: designation.to_string(),
        },
    ))
}

/// One stored DataZone catalog asset.
///
/// An asset is versioned: `revision` is the newest revision's number and the
/// `first_revision_*` fields remember who made revision one. This slice stores the
/// latest revision only — `CreateAssetRevision` and `ListAssetRevisions` are still
/// scaffolds — so the two agree until a revision is added.
#[derive(Debug, Clone)]
pub struct Asset {
    pub id: String,
    pub domain_id: String,
    pub owning_project_id: String,
    pub name: String,
    pub description: Option<String>,
    pub external_identifier: Option<String>,
    pub type_identifier: String,
    pub type_revision: String,
    pub revision: String,
    pub created_by: String,
    pub created_at_ms: i64,
    pub first_revision_created_by: String,
    pub first_revision_created_at_ms: i64,
    pub glossary_terms: Vec<String>,
    /// The metadata forms attached to the asset, in the order the request listed them
    /// and keyed by form name (a name appears at most once).
    pub forms: Vec<StoredForm>,
    /// The `predictionConfiguration` the request set, mirrored member for member so a
    /// read reports exactly what was written.
    pub prediction_configuration: Option<StoredPredictionConfiguration>,
}

/// One metadata form attached to an asset: the stored form of the modelled
/// `FormInput`/`FormOutput` pair.
#[derive(Debug, Clone, Default)]
pub struct StoredForm {
    pub form_name: String,
    pub type_name: Option<String>,
    pub type_revision: Option<String>,
    pub content: Option<String>,
}

/// The stored form of the modelled `PredictionConfiguration`: present-or-absent is
/// meaningful at every level, so the structure is kept rather than flattened.
#[derive(Debug, Clone, Default)]
pub struct StoredPredictionConfiguration {
    pub business_name_generation: Option<StoredBusinessNameGeneration>,
}

/// The stored form of the modelled `BusinessNameGenerationConfiguration`.
#[derive(Debug, Clone, Default)]
pub struct StoredBusinessNameGeneration {
    pub enabled: Option<bool>,
}

/// One stored policy grant: an authorization policy of one `policyType`, held by one
/// principal, on one entity (`entityType` + `entityIdentifier`) inside a domain.
///
/// `principal` and `detail` are kept as the modelled unions the request carried, so
/// `ListPolicyGrants` reports back exactly what `AddPolicyGrant` persisted rather than
/// a lossy projection of it.
#[derive(Debug, Clone)]
pub struct PolicyGrant {
    /// The grant's own id, minted by [`ComAmazonawsDatazoneDatazoneState::mint_grant_id`].
    pub grant_id: String,
    /// `TargetEntityType`: ASSET_TYPE | DOMAIN_UNIT | ENVIRONMENT_BLUEPRINT_CONFIGURATION
    /// | ENVIRONMENT_PROFILE (model enum `com.amazonaws.datazone#TargetEntityType`).
    pub entity_type: String,
    /// The id of the entity the grant is attached to.
    pub entity_identifier: String,
    /// `ManagedPolicyType` (model enum `com.amazonaws.datazone#ManagedPolicyType`).
    pub policy_type: String,
    pub principal: crate::ops::types::PolicyGrantPrincipal,
    pub detail: crate::ops::types::PolicyGrantDetail,
    pub created_at_ms: i64,
    pub created_by: String,
    /// The `@idempotencyToken` the request that created this grant carried, so a replay
    /// of that request answers with this grant instead of adding a second one. Stored on
    /// the grant (rather than in a side map) so the token is released with the grant.
    pub client_token: Option<String>,
}

/// The canonical key of a `PolicyGrantPrincipal`: two principals are the same principal
/// exactly when their keys are equal.
///
/// `RemovePolicyGrant` identifies the grant to drop by its principal, and the modelled
/// unions and structures carry no `PartialEq`, so the comparison is made explicit here:
/// every member of every variant is rendered, separated by a byte no identifier can
/// contain (`\x1f`), so no two different principals can collide.
pub fn principal_key(principal: &crate::ops::types::PolicyGrantPrincipal) -> String {
    use crate::ops::types::{
        DomainUnitGrantFilter, GroupPolicyGrantPrincipal, PolicyGrantPrincipal, ProjectGrantFilter,
        UserPolicyGrantPrincipal,
    };
    const SEP: char = '\x1f';
    match principal {
        PolicyGrantPrincipal::User(UserPolicyGrantPrincipal::userIdentifier(id)) => {
            format!("user{SEP}userIdentifier{SEP}{id}")
        }
        PolicyGrantPrincipal::User(UserPolicyGrantPrincipal::allUsersGrantFilter(_)) => {
            format!("user{SEP}allUsersGrantFilter")
        }
        PolicyGrantPrincipal::Group(GroupPolicyGrantPrincipal::groupIdentifier(id)) => {
            format!("group{SEP}groupIdentifier{SEP}{id}")
        }
        PolicyGrantPrincipal::Project(project) => {
            let filter = match project.project_grant_filter.as_ref() {
                Some(ProjectGrantFilter::domainUnitFilter(f)) => format!(
                    "domainUnitFilter{SEP}{}{SEP}{}",
                    f.domain_unit,
                    f.include_child_domain_units.unwrap_or(false)
                ),
                None => String::new(),
            };
            format!(
                "project{SEP}{}{SEP}{}{SEP}{}{SEP}{filter}",
                project.project_designation,
                project.user_created_designation_id.as_deref().unwrap_or(""),
                project.project_identifier.as_deref().unwrap_or(""),
            )
        }
        PolicyGrantPrincipal::domainUnit(unit) => {
            let filter = match unit.domain_unit_grant_filter.as_ref() {
                Some(DomainUnitGrantFilter::allDomainUnitsGrantFilter(_)) => {
                    "allDomainUnitsGrantFilter"
                }
                None => "",
            };
            format!(
                "domainUnit{SEP}{}{SEP}{}{SEP}{filter}",
                unit.domain_unit_designation,
                unit.domain_unit_identifier.as_deref().unwrap_or(""),
            )
        }
    }
}

/// The `ManagedPolicyType` a `PolicyGrantDetail` member describes.
///
/// The model declares the two in one-to-one correspondence: every `ManagedPolicyType`
/// value has the `PolicyGrantDetail` member of the same name (smithy_query
/// shape="ManagedPolicyType" / shape="PolicyGrantDetail"), so a request whose `detail`
/// is not the member its `policyType` names describes a different policy from the one it
/// asks to grant.
pub fn policy_type_of_detail(detail: &crate::ops::types::PolicyGrantDetail) -> &'static str {
    use crate::ops::types::PolicyGrantDetail as D;
    match detail {
        D::createDomainUnit(_) => "CREATE_DOMAIN_UNIT",
        D::overrideDomainUnitOwners(_) => "OVERRIDE_DOMAIN_UNIT_OWNERS",
        D::addToProjectMemberPool(_) => "ADD_TO_PROJECT_MEMBER_POOL",
        D::overrideProjectOwners(_) => "OVERRIDE_PROJECT_OWNERS",
        D::createGlossary(_) => "CREATE_GLOSSARY",
        D::createFormType(_) => "CREATE_FORM_TYPE",
        D::createAssetType(_) => "CREATE_ASSET_TYPE",
        D::createProject(_) => "CREATE_PROJECT",
        D::createEnvironmentProfile(_) => "CREATE_ENVIRONMENT_PROFILE",
        D::delegateCreateEnvironmentProfile(_) => "DELEGATE_CREATE_ENVIRONMENT_PROFILE",
        D::createEnvironment(_) => "CREATE_ENVIRONMENT",
        D::createEnvironmentFromBlueprint(_) => "CREATE_ENVIRONMENT_FROM_BLUEPRINT",
        D::createProjectFromProjectProfile(_) => "CREATE_PROJECT_FROM_PROJECT_PROFILE",
        D::useAssetType(_) => "USE_ASSET_TYPE",
    }
}

/// One stored DataZone asset type, at one revision.
///
/// An asset type is a named, revisioned schema: the metadata forms an asset of that
/// type carries. `CreateAssetType` mints revision [`FIRST_REVISION`] and the model
/// declares no update operation for it, so a stored type keeps the revision it was
/// created at.
#[derive(Debug, Clone)]
pub struct AssetType {
    pub name: String,
    pub domain_id: String,
    pub revision: String,
    pub description: Option<String>,
    /// The metadata forms attached to the type, keyed by form name. A `BTreeMap` so the
    /// forms are reported in one stable order whatever order the request listed them in;
    /// the modelled `FormsOutputMap` is bounded at 10 entries, which the request-level
    /// constraint on `formsInput` already enforces.
    pub forms: BTreeMap<String, AssetTypeForm>,
    pub owning_project_id: String,
    pub created_at_ms: i64,
    pub created_by: String,
    pub updated_at_ms: i64,
    pub updated_by: String,
}

/// One metadata form attached to an asset type: the stored form of the modelled
/// `FormEntryOutput` (`typeName`, `typeRevision`, `required`).
#[derive(Debug, Clone)]
pub struct AssetTypeForm {
    pub type_name: String,
    pub type_revision: String,
    pub required: Option<bool>,
}

/// The most domains this emulator stores.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop cannot
/// grow the emulator's heap — or the per-request [`ComAmazonawsDatazoneDatazoneState::settle_clock`]
/// scan — without bound. `CreateDomain` models `ServiceQuotaExceededException`, which
/// is the refusal once the store is full.
pub const MAX_DOMAINS: usize = 100;

/// The most projects this emulator stores in one domain, for the same reason and with
/// the same refusal (`CreateProject` models `ServiceQuotaExceededException` too).
pub const MAX_PROJECTS_PER_DOMAIN: usize = 500;

/// The most metadata form types this emulator stores in one domain, for the same
/// reason and with the same refusal (`CreateFormType` models
/// `ServiceQuotaExceededException` too).
pub const MAX_FORM_TYPES_PER_DOMAIN: usize = 500;

/// The most revisions this emulator keeps for one metadata form type. Re-creating a
/// name appends a revision, so this is the bound on that caller-driven growth; the
/// refusal past it is `CreateFormType`'s `ServiceQuotaExceededException`.
pub const MAX_FORM_TYPE_REVISIONS: usize = 100;

/// The most domain units this emulator stores in one domain, for the same reason and
/// with the same refusal (`CreateDomainUnit` models `ServiceQuotaExceededException`).
/// It also bounds the depth of the hierarchy, since every unit but the root hangs
/// under a parent that is itself stored.
pub const MAX_DOMAIN_UNITS_PER_DOMAIN: usize = 100;

/// The most owners this emulator keeps on one entity, for the same reason and with the
/// same refusal (`AddEntityOwner` models `ServiceQuotaExceededException` too): adding
/// owners is caller-driven growth of a stored entity.
pub const MAX_OWNERS_PER_ENTITY: usize = 100;

/// The most assets this emulator stores in one domain, for the same reason and with the
/// same refusal (`CreateAsset` models `ServiceQuotaExceededException` too).
pub const MAX_ASSETS_PER_DOMAIN: usize = 500;

/// The most policy grants this emulator stores in one domain, for the same reason and
/// with the same refusal (`AddPolicyGrant` models `ServiceQuotaExceededException` too).
pub const MAX_POLICY_GRANTS_PER_DOMAIN: usize = 500;

/// The most members this emulator stores in one project, for the same reason: a
/// ceiling so an add loop cannot grow the store without bound. The refusal is each
/// operation's own — `ServiceQuotaExceededException` where it is modelled
/// (`CreateProject`), `ValidationException` where it is not
/// (`CreateProjectMembership`).
pub const MAX_MEMBERSHIPS_PER_PROJECT: usize = 200;

/// The most business glossaries this emulator stores in one domain, for the same reason
/// and with the same refusal (`CreateGlossary` models `ServiceQuotaExceededException`).
pub const MAX_GLOSSARIES_PER_DOMAIN: usize = 500;

/// The most asset types this emulator stores in one domain, for the same reason and
/// with the same refusal (`CreateAssetType` models `ServiceQuotaExceededException` too).
pub const MAX_ASSET_TYPES_PER_DOMAIN: usize = 500;

/// The most catalog listings this emulator stores in one domain, for the same reason
/// and with the same refusal (`CreateListingChangeSet` models
/// `ServiceQuotaExceededException` too). It also bounds the
/// [`Domain::listing_for_entity`] scan and the per-request
/// [`ComAmazonawsDatazoneDatazoneState::settle_clock`] walk over listings.
pub const MAX_LISTINGS_PER_DOMAIN: usize = 500;

/// The most revisions this emulator records for one listing. Publishing an entity that
/// already has a listing records a further revision, so this is the bound on that
/// caller-driven growth; the refusal past it is `CreateListingChangeSet`'s
/// `ServiceQuotaExceededException`.
pub const MAX_LISTING_REVISIONS: usize = 100;

/// The most tags this emulator keeps on one resource.
///
/// A CHOSEN ceiling, not a published quota: `TagResource` is a caller-driven growth
/// site (the keys come from the request), so the per-resource tag set has to be
/// bounded or a tag loop grows the emulator's heap without limit. The refusal once a
/// request would cross it is `ValidationException`, the client-input error
/// `TagResource` models.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The ARN namespace this service's resources live in: `arnNamespace` on the model's
/// `aws.api#service` trait (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "datazone";

/// Whether `candidate` has the shape of an ARN of THIS service —
/// `arn:<partition>:datazone:<region>:<account>:<resource>`.
///
/// `resourceArn` is modelled as a plain `String` with no pattern, so the boundary
/// validator passes any text at all and the handler is where a string that cannot
/// name a DataZone resource is recognised. Region and account may be empty (an ARN
/// is allowed to omit either), the partition and the resource part may not.
pub fn is_service_arn(candidate: &str) -> bool {
    let mut fields = candidate.splitn(6, ':');
    fields.next() == Some("arn")
        && fields.next().is_some_and(|partition| !partition.is_empty())
        && fields.next() == Some(ARN_NAMESPACE)
        && fields.next().is_some()
        && fields.next().is_some()
        && fields.next().is_some_and(|resource| !resource.is_empty())
}

/// The hard ceiling on remembered `@idempotencyToken`s.
///
/// The token keys are CALLER-supplied, so the map is a caller-driven growth site and
/// must be bounded: [`ComAmazonawsDatazoneDatazoneState::bound_idempotency`] first
/// releases every token whose resource is gone (such a token can never be replayed into
/// anything), then evicts the oldest entries beyond this ceiling. At most one token is
/// recorded per stored resource, and a token evicted here only costs its request the
/// idempotent replay — the resource it created stays stored and readable.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_DOMAINS * 16;

/// Which kind of resource a remembered `@idempotencyToken` created.
///
/// Part of the map KEY, not just the value: two operations can be handed the same
/// caller-chosen token string, and a `CreateGlossary` repeat must not consume
/// (or overwrite) the record a `CreateDomain` left under that token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdempotentResource {
    Domain,
    DomainUnit,
    Glossary,
    Asset,
}

/// The key one remembered `@idempotencyToken` is filed under.
///
/// TOTAL scoping, so no two creates can collide on one caller-chosen string: the
/// operation's resource kind, the domain the create is scoped to, and the token.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IdempotencyKey {
    kind: IdempotentResource,
    /// The domain the create happens IN — empty for a create that is scoped to the
    /// service rather than to a domain (a domain itself).
    domain_id: String,
    token: String,
}

impl IdempotencyKey {
    /// The key a `CreateDomain` token is filed under. A domain create is scoped to the
    /// service, so the key carries no domain.
    pub fn domain(token: &str) -> Self {
        Self {
            kind: IdempotentResource::Domain,
            domain_id: String::new(),
            token: token.to_string(),
        }
    }

    /// The key a `CreateDomainUnit` token is filed under: the same token in another
    /// domain is a different key, so each domain keeps its own record.
    pub fn domain_unit(domain_id: &str, token: &str) -> Self {
        Self {
            kind: IdempotentResource::DomainUnit,
            domain_id: domain_id.to_string(),
            token: token.to_string(),
        }
    }

    /// The key a `CreateGlossary` token is filed under: the same token in another
    /// domain is a different key, so each domain keeps its own record.
    pub fn glossary(domain_id: &str, token: &str) -> Self {
        Self {
            kind: IdempotentResource::Glossary,
            domain_id: domain_id.to_string(),
            token: token.to_string(),
        }
    }

    /// The key a `CreateAsset` token is filed under, scoped the same way: an asset is
    /// created IN a domain, so the same token in another domain is a different key.
    pub fn asset(domain_id: &str, token: &str) -> Self {
        Self {
            kind: IdempotentResource::Asset,
            domain_id: domain_id.to_string(),
            token: token.to_string(),
        }
    }
}

/// What one remembered `@idempotencyToken` points at, plus when it was remembered so
/// the oldest can be evicted first.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// The id of the resource the first request carrying the token created, inside the
    /// scope its [`IdempotencyKey`] names.
    pub resource_id: String,
    pub recorded_at_ms: i64,
}

/// Per-service state for com-amazonaws-datazone-datazone. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDatazoneDatazoneState {
    /// Every live domain, keyed by its `dzd_…` id. A `BTreeMap` so every listing
    /// iterates in one stable order and a pagination token can resume from a key.
    /// Bounded by [`MAX_DOMAINS`].
    pub domains: BTreeMap<String, Domain>,
    /// [`IdempotencyKey`] -> the resource the first request carrying that token created,
    /// so a repeated `@idempotencyToken` request returns that resource instead of
    /// creating a second one.
    ///
    /// Bounded: see [`MAX_IDEMPOTENCY_TOKENS`]. Write it through
    /// [`Self::record_idempotency`], which applies the bound, rather than inserting
    /// directly.
    idempotency: BTreeMap<IdempotencyKey, IdempotencyRecord>,
    /// Monotonic counter behind every minted id, so no two resources in one process
    /// ever share an id even when minted in the same millisecond.
    id_seq: u64,
}

impl ComAmazonawsDatazoneDatazoneState {
    /// The domain a `clientToken` already created, if that domain is still stored.
    pub fn idempotent_domain(&self, token: &str) -> Option<&Domain> {
        let record = self.idempotency.get(&IdempotencyKey::domain(token))?;
        self.domains.get(&record.resource_id)
    }

    /// The domain unit a `clientToken` already created in `domain_id`, if it is still
    /// stored. The domain is part of the key, so the same token against a different
    /// domain creates that domain's own unit and leaves this record alone.
    pub fn idempotent_domain_unit(&self, domain_id: &str, token: &str) -> Option<&DomainUnit> {
        let record = self
            .idempotency
            .get(&IdempotencyKey::domain_unit(domain_id, token))?;
        self.domains
            .get(domain_id)?
            .domain_units
            .get(&record.resource_id)
    }

    /// The glossary a `clientToken` already created in `domain_id`, if it is still
    /// stored. The domain is part of the key, so the same token against a different
    /// domain creates that domain's own glossary and leaves this record alone.
    pub fn idempotent_glossary(&self, domain_id: &str, token: &str) -> Option<&Glossary> {
        let record = self
            .idempotency
            .get(&IdempotencyKey::glossary(domain_id, token))?;
        self.domains
            .get(domain_id)?
            .glossaries
            .get(&record.resource_id)
    }

    /// The asset a `clientToken` already created in `domain_id`, if it is still stored.
    /// Scoped to the domain the same way a glossary token is.
    pub fn idempotent_asset(&self, domain_id: &str, token: &str) -> Option<&Asset> {
        let record = self
            .idempotency
            .get(&IdempotencyKey::asset(domain_id, token))?;
        self.domains.get(domain_id)?.assets.get(&record.resource_id)
    }

    /// Whether a remembered token still resolves to something this store holds. One
    /// lookup per level, because the key carries the owning domain.
    fn record_resolves(&self, key: &IdempotencyKey, record: &IdempotencyRecord) -> bool {
        match key.kind {
            IdempotentResource::Domain => self.domains.contains_key(&record.resource_id),
            IdempotentResource::DomainUnit => self
                .domains
                .get(&key.domain_id)
                .is_some_and(|domain| domain.domain_units.contains_key(&record.resource_id)),
            IdempotentResource::Glossary => self
                .domains
                .get(&key.domain_id)
                .is_some_and(|domain| domain.glossaries.contains_key(&record.resource_id)),
            IdempotentResource::Asset => self
                .domains
                .get(&key.domain_id)
                .is_some_and(|domain| domain.assets.contains_key(&record.resource_id)),
        }
    }

    /// Remember that the token `key` names created `resource_id`, and re-apply the
    /// bound.
    ///
    /// Call it AFTER the resource is stored: a token naming a resource that is not in
    /// the store is released immediately by the bound below.
    pub fn record_idempotency(&mut self, key: IdempotencyKey, resource_id: String, now: i64) {
        self.idempotency.insert(
            key,
            IdempotencyRecord {
                resource_id,
                recorded_at_ms: now,
            },
        );
        self.bound_idempotency();
    }

    /// Keep the remembered-token map bounded: release every token whose resource is no
    /// longer stored, then evict the oldest entries beyond [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// Reached from [`Self::record_idempotency`] and from [`Self::settle_clock`] (which
    /// every handler calls), so a create/delete loop with fresh tokens releases each
    /// token as its resource disappears and can never grow the store without limit.
    fn bound_idempotency(&mut self) {
        let released: Vec<IdempotencyKey> = self
            .idempotency
            .iter()
            .filter(|(key, record)| !self.record_resolves(key, record))
            .map(|(key, _)| key.clone())
            .collect();
        for key in released {
            self.idempotency.remove(&key);
        }
        let Some(excess) = self
            .idempotency
            .len()
            .checked_sub(MAX_IDEMPOTENCY_TOKENS)
            .filter(|excess| *excess > 0)
        else {
            return;
        };
        let mut by_age: Vec<(i64, IdempotencyKey)> = self
            .idempotency
            .iter()
            .map(|(key, record)| (record.recorded_at_ms, key.clone()))
            .collect();
        by_age.sort();
        for (_, key) in by_age.into_iter().take(excess) {
            self.idempotency.remove(&key);
        }
    }
    /// Mint an opaque id suffix: lowercase base-36, so it satisfies both the
    /// `^[a-zA-Z0-9_-]{1,36}$` (project, domain) and `^[a-z0-9_\-]+$` (domain unit)
    /// patterns the model declares. Unique within a process, and never reused.
    pub fn mint_id(&mut self, now: i64) -> String {
        self.id_seq += 1;
        format!(
            "{}{}",
            base36(u64::try_from(now).unwrap_or(0)),
            base36(self.id_seq)
        )
    }

    /// Mint a `GrantIdentifier`: exactly 10 characters, which is what the model's
    /// `^[A-Za-z0-9+/]{10}$` pattern (smithy_query shape="GrantIdentifier") admits.
    ///
    /// The value encoded is the mint instant in the high digits and the per-process
    /// counter in the low ones, so two grants minted in the same millisecond still get
    /// different ids. Only the 62 alphanumeric characters of the pattern's class are
    /// used, so an id is safe to carry in a URI as well as in a body.
    pub fn mint_grant_id(&mut self, now: i64) -> String {
        /// A subset of the pattern's character class, in ASCII order.
        const ALPHABET: &[u8; 62] =
            b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        const LEN: usize = 10;

        self.id_seq += 1;
        // 62^10 is about 8.4e17, so the encoded value must stay below that: 1e11
        // milliseconds of clock (about 3 years of distinct instants) times 1e6
        // counter values is 1e17.
        let mut value = u64::try_from(now).unwrap_or(0) % 100_000_000_000 * 1_000_000
            + self.id_seq % 1_000_000;
        let mut out = [ALPHABET[0]; LEN];
        for slot in out.iter_mut().rev() {
            *slot = ALPHABET[usize::try_from(value % 62).unwrap_or(0)];
            value /= 62;
        }
        // The buffer holds one ASCII byte per digit by construction.
        String::from_utf8(out.to_vec()).expect("the alphabet is ASCII")
    }

    /// Materialise every domain lifecycle step the clock already owes at `now`: a
    /// `CREATING` domain whose span has elapsed becomes `AVAILABLE`, a `DELETING` one
    /// disappears together with its projects, and a `CREATING` listing whose span has
    /// elapsed becomes `ACTIVE`.
    ///
    /// This is the ONLY write a reporting operation makes, and it writes exactly what
    /// was already owed at this instant whether or not anybody called.
    pub fn settle_clock(&mut self, now: i64) {
        let due: Vec<String> = self
            .domains
            .iter()
            .filter(|(_, d)| d.status_due_ms.is_some_and(|due| due <= now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(domain) = self.domains.get_mut(&id) else {
                continue;
            };
            let due_at = domain.status_due_ms.unwrap_or(now);
            match domain.status.as_str() {
                DOMAIN_STATUS_CREATING => {
                    domain.status = DOMAIN_STATUS_AVAILABLE.to_string();
                    domain.status_due_ms = None;
                    domain.last_updated_at_ms = due_at;
                }
                DOMAIN_STATUS_DELETING => {
                    // The deletion the earlier DeleteDomain call started has
                    // completed: the domain and the projects under it are gone.
                    self.domains.remove(&id);
                }
                // No other status carries a due instant.
                _ => domain.status_due_ms = None,
            }
        }
        // The publish a `CreateListingChangeSet` started has finished: the listing is
        // in the catalog. Bounded by MAX_DOMAINS * MAX_LISTINGS_PER_DOMAIN, and run
        // after the domain pass so a domain that has just disappeared is not walked.
        for domain in self.domains.values_mut() {
            for listing in domain.listings.values_mut() {
                if listing.status_due_ms.is_some_and(|due| due <= now) {
                    listing.status = LISTING_STATUS_ACTIVE.to_string();
                    listing.status_due_ms = None;
                }
            }
        }
        // A token naming a domain that has just disappeared can never be replayed
        // into anything, so it is released with the domain.
        self.bound_idempotency();
    }

    /// The domain `id`, when it exists and is not already being deleted.
    pub fn live_domain(&self, id: &str) -> Option<&Domain> {
        self.domains
            .get(id)
            .filter(|d| d.status != DOMAIN_STATUS_DELETING)
    }

    /// The domain unit `unit_id` stored in domain `domain_id`, when both exist.
    pub fn domain_unit(&self, domain_id: &str, unit_id: &str) -> Option<&DomainUnit> {
        self.domains.get(domain_id)?.domain_units.get(unit_id)
    }

    /// The entity an entity-owner call names, when it is stored.
    ///
    /// `DataZoneEntityType` has exactly one member, `DOMAIN_UNIT` (smithy_query
    /// shape="DataZoneEntityType"), so the entity owners are kept on is the domain unit
    /// `entity_id` of domain `domain_id`. `None` when `entity_type` is not that member
    /// or either resource is not stored, so `AddEntityOwner`, `ListEntityOwners` and
    /// `RemoveEntityOwner` all resolve the pair by one rule.
    pub fn owned_entity(
        &self,
        domain_id: &str,
        entity_type: &str,
        entity_id: &str,
    ) -> Option<&DomainUnit> {
        if entity_type != DATA_ZONE_ENTITY_TYPE_DOMAIN_UNIT {
            return None;
        }
        self.domain_unit(domain_id, entity_id)
    }

    /// [`Self::owned_entity`], for a call that writes the entity's owners.
    pub fn owned_entity_mut(
        &mut self,
        domain_id: &str,
        entity_type: &str,
        entity_id: &str,
    ) -> Option<&mut DomainUnit> {
        if entity_type != DATA_ZONE_ENTITY_TYPE_DOMAIN_UNIT {
            return None;
        }
        self.domains
            .get_mut(domain_id)?
            .domain_units
            .get_mut(entity_id)
    }

    /// The live domain that already carries `name`, if the account has one.
    ///
    /// A domain name is unique within the account, so this is what `CreateDomain`
    /// refuses on. Every domain in this store belongs to the one account the emulator
    /// serves (`Domain::managed_account_id`), so the whole store IS the account's
    /// namespace. A domain already being deleted is on its way out and no longer holds
    /// its name — the same liveness notion [`Self::live_domain`] applies.
    ///
    /// A linear scan, deliberately: the store is bounded by [`MAX_DOMAINS`] (the same
    /// bound `settle_clock`'s per-request scan relies on), so one liveness rule shared
    /// with [`Self::live_domain`] is worth more than a second index to keep in step.
    pub fn live_domain_named(&self, name: &str) -> Option<&Domain> {
        self.domains
            .values()
            .find(|d| d.name == name && d.status != DOMAIN_STATUS_DELETING)
    }

    /// The tag set of the resource `arn` names, when this emulator stores that
    /// resource.
    ///
    /// The lookup is by the ARN the store itself minted for the resource, so every
    /// stored resource that carries one is reachable by exactly the ARN its own
    /// create/get reports — and a resource in a non-terminal status is still stored,
    /// so it is still taggable until its delete completes. That is deliberately the
    /// reach of `GetDomain`, which reports a `DELETING` domain and its tags rather
    /// than 404ing on it, so the tagging operations and the resource's own reader
    /// agree on which resources exist; [`Self::live_domain`] is the narrower lookup,
    /// for operations that must not hang a NEW resource off a dying domain.
    ///
    /// Only the domain is reachable today, because it is the only resource this store
    /// keeps an ARN for: [`Project`] and [`AssetType`] mint none, so no DataZone
    /// resource ARN can name one. (A project's `resource_tags` field is the modelled
    /// `resourceTags` member — project metadata, not the AWS resource tags these
    /// operations manage.) A later resource that stores an `arn` and a tag set joins
    /// this same lookup rather than getting a tag map of its own.
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        self.domains
            .values()
            .find(|domain| domain.arn == arn)
            .map(|domain| &domain.tags)
    }

    /// The same lookup as [`Self::resource_tags`], for a tag write.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        self.domains
            .values_mut()
            .find(|domain| domain.arn == arn)
            .map(|domain| &mut domain.tags)
    }
}

/// The ROOT domain unit a domain is created with.
///
/// `CreateDomain` reports a `rootDomainUnitId`, so the unit it names exists from the
/// moment the domain does: it is the one unit in the domain with no parent, it is
/// owned by the principal that created the domain, and it is the parent every other
/// unit's chain of ancestors ends at.
pub fn root_domain_unit(domain: &Domain, created_by: &str, now: i64) -> DomainUnit {
    DomainUnit {
        id: domain.root_domain_unit_id.clone(),
        domain_id: domain.id.clone(),
        name: domain_unit_name_from(&domain.name, &domain.id),
        description: None,
        parent_domain_unit_id: None,
        owners: vec![DomainUnitOwner::User(created_by.to_string())],
        created_by: created_by.to_string(),
        created_at_ms: now,
        last_updated_by: None,
        last_updated_at_ms: None,
    }
}

/// A `DomainUnitName` for the root unit of a domain called `domain_name`.
///
/// The root unit carries the domain's own name, but `DomainName` declares no pattern
/// while `DomainUnitName` is `^[\w -]+$` with length 1..128 (smithy_query
/// shape="DomainUnitName"), so a character outside that class becomes `_`, the result
/// is clipped to 128 characters, and a name that is empty once clipped falls back to
/// the domain id — itself inside the class.
///
/// The class kept here is deliberately NARROWER than the modelled one: `\w` admits
/// Unicode word characters, and this keeps only ASCII alphanumerics, `_`, `-` and the
/// space, so every name it produces satisfies the model while a non-ASCII letter is
/// replaced rather than passed through.
fn domain_unit_name_from(domain_name: &str, domain_id: &str) -> String {
    let name: String = domain_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    if name.is_empty() {
        domain_id.to_string()
    } else {
        name
    }
}

/// A stored owner as the modelled `DomainUnitOwnerProperties` union
/// (smithy_query shape="DomainUnitOwnerProperties"), shared by every operation that
/// reports a domain unit's owners so the two forms cannot drift apart.
pub fn owner_properties(
    owner: &DomainUnitOwner,
) -> crate::ops::types::DomainUnitOwnerProperties {
    use crate::ops::types::{
        DomainUnitGroupProperties, DomainUnitOwnerProperties, DomainUnitUserProperties,
    };
    match owner {
        DomainUnitOwner::User(user_id) => {
            DomainUnitOwnerProperties::User(DomainUnitUserProperties {
                user_id: Some(user_id.clone()),
            })
        }
        DomainUnitOwner::Group(group_id) => {
            DomainUnitOwnerProperties::Group(DomainUnitGroupProperties {
                group_id: Some(group_id.clone()),
            })
        }
    }
}

/// The ids of `unit_id`'s ancestors in `domain`: its parent first, then that unit's
/// parent, up to and including the domain's root unit.
///
/// Empty for the root unit, which has no ancestor. Only ids with a stored record are
/// reported, and the walk is bounded by the number of stored units, so neither a
/// dangling parent pointer nor a cycle can make it report a unit that is not there or
/// run forever.
pub fn ancestor_domain_unit_ids(domain: &Domain, unit_id: &str) -> Vec<String> {
    let mut ancestors: Vec<String> = Vec::new();
    let mut cursor = domain
        .domain_units
        .get(unit_id)
        .and_then(|unit| unit.parent_domain_unit_id.clone());
    while let Some(id) = cursor {
        let Some(parent) = domain.domain_units.get(&id) else {
            break;
        };
        if ancestors.contains(&id) || ancestors.len() >= domain.domain_units.len() {
            break;
        }
        cursor = parent.parent_domain_unit_id.clone();
        ancestors.push(id);
    }
    ancestors
}

/// The resource kinds this service names in its not-found messages.
///
/// A closed set, so a handler cannot invent a third spelling of a kind or interpolate
/// a caller-supplied identifier into the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Domain,
    DomainUnit,
    Project,
    AssetType,
    Glossary,
    Asset,
    DataProduct,
    Listing,
}

impl ResourceKind {
    /// The kind as DataZone names it in a message.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Domain => "Domain",
            Self::DomainUnit => "Domain unit",
            Self::Project => "Project",
            Self::AssetType => "Asset type",
            Self::Glossary => "Glossary",
            Self::Asset => "Asset",
            Self::DataProduct => "Data product",
            Self::Listing => "Listing",
        }
    }

    /// The `ResourceNotFoundException` message DataZone answers for a resource of this
    /// kind that it cannot find: the KIND only, never the caller-supplied identifier
    /// (recorded behaviour: "Project not found", "Account Pool not found"). Build every
    /// not-found message in this service's handlers through this, so none of them can
    /// echo the request back.
    pub fn not_found_message(self) -> String {
        format!("{} not found", self.as_str())
    }
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `ConflictException` message `CreateDomain` answers when the account already has
/// a domain with the requested name. The requirement is the 409 refusal and the code;
/// this text is AWS's own recorded wording, and like the not-found text above it names
/// no caller-supplied value.
pub const DUPLICATE_DOMAIN_NAME_MESSAGE: &str =
    "Domain name already exists under this account, please use another one";

/// Lowercase base-36 encoding of `value` — the alphabet every DataZone id pattern
/// accepts.
fn base36(mut value: u64) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(ALPHABET[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Request-derived values and pagination, shared by this service's handlers.
//
// `src/ops/mod.rs` is generated (it `include!`s the generated module list) and so is
// `src/lib.rs`, so a helper several operations need lives in this file — the service's
// own hand-editable module — rather than in a module neither generated file can name.
// ---------------------------------------------------------------------------

/// The region the caller signed for, defaulting to the region a generated emulator
/// presents when the request carries no SigV4 scope (an anonymous call).
pub fn caller_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an anonymous request is served in: AWS SDK clients default to
/// `us-east-1`, so an unsigned call is reported there.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The principal that made this call, as the `createdBy` of what it creates.
///
/// Derived from the CALLER's identity — the access key id its SigV4 scope names —
/// never from the request body, so two different callers creating the same-named
/// project get different `createdBy` values.
pub fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let akid = ::engine::extract_akid(&ctx.parts().headers);
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// Encode the key a page ended on as the opaque `nextToken` a caller passes back.
///
/// Opaque to the client (hex of the key's bytes) but a pure function of the key, so
/// the next page resumes exactly after it however the stored map has changed.
pub fn encode_page_token(last_key: &str) -> String {
    hex::encode(last_key.as_bytes())
}

/// The key a `nextToken` resumes after, or `None` when the token is not one this
/// service issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDatazoneDatazoneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDatazoneDatazoneState>)
}
