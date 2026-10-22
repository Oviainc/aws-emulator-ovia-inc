// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lakeformation-awslakeformation service.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::ops::types::{
    Condition, DataLakePrincipal, LFTag, PrincipalResourcePermissions, Resource,
};

/// One registered data-lake location: the resource ARN Lake Formation manages, the IAM
/// role it uses to reach it, and the registration flags the caller set.
///
/// `role_arn` is stored as a plain `String` because a registration always HAS a data
/// access role — `RegisterResource` refuses a request that names neither `RoleArn` nor
/// `UseServiceLinkedRole` ("If UseServiceLinkedRole is not set to true, you must provide
/// or set the RoleArn" — `RegisterResource`'s own documentation), and
/// `UseServiceLinkedRole` resolves to [`service_linked_role_arn`].
///
/// The three nullable flags keep the `Option` the model gives them (`NullableBoolean`),
/// so a flag the caller never set is reported absent rather than invented as `false`.
#[derive(Debug, Clone)]
pub struct RegisteredResource {
    /// The ARN this registration is keyed by; echoed as `ResourceInfo$ResourceArn`.
    pub resource_arn: String,
    /// The IAM role that registered the resource (`ResourceInfo$RoleArn`).
    pub role_arn: String,
    /// `ResourceInfo$WithFederation`, as set by the last register/update.
    pub with_federation: Option<bool>,
    /// `ResourceInfo$HybridAccessEnabled`, as set by the last register/update.
    pub hybrid_access_enabled: Option<bool>,
    /// `ResourceInfo$WithPrivilegedAccess`. `RegisterResource`'s input member is
    /// modelled `Boolean` with `default false`, so a registration always has a value.
    pub with_privileged_access: Option<bool>,
    /// `ResourceInfo$ExpectedResourceOwnerAccount`, when the caller named one.
    pub expected_resource_owner_account: Option<String>,
    /// When this registration last changed — reported as `ResourceInfo$LastModified`.
    pub last_modified: SystemTime,
}

impl RegisteredResource {
    /// This registration as the modelled `ResourceInfo` structure, which is what
    /// `DescribeResource` (and, once implemented, `ListResources`) answers with.
    pub fn to_resource_info(&self) -> crate::ops::types::ResourceInfo {
        crate::ops::types::ResourceInfo {
            resource_arn: Some(self.resource_arn.clone()),
            role_arn: Some(self.role_arn.clone()),
            last_modified: Some(::aws_smithy_types::DateTime::from(self.last_modified)),
            with_federation: self.with_federation,
            hybrid_access_enabled: self.hybrid_access_enabled,
            with_privileged_access: self.with_privileged_access,
            // `VerificationStatus` reports whether the registered ROLE can actually
            // reach the location ("VERIFIED - Registered role has sufficient
            // permissions …" — `ResourceInfo$VerificationStatus`). This emulator runs
            // no IAM evaluation and touches no S3 location, so nothing performed that
            // check and the optional member is left absent rather than carrying a
            // status for work that never ran.
            verification_status: None,
            expected_resource_owner_account: self.expected_resource_owner_account.clone(),
        }
    }
}

/// One stored LF-tag definition: the Data Catalog it lives in, its key, and the ordered
/// set of values the key may take.
///
/// `tag_values` is kept ordered and de-duplicated and is never empty — `TagValueList` is
/// modelled `len 1..50` (`smithy_query(query="shape",shape="TagValueList")`), so a tag
/// whose values were all removed could not be rendered back on the wire.
#[derive(Debug, Clone, Default)]
pub struct LfTag {
    /// The catalog this tag belongs to; echoed by `GetLFTag` / `ListLFTags`.
    pub catalog_id: String,
    /// The stored key-name (`LFTagKey`, len 1..128).
    pub tag_key: String,
    /// The values the key may take: ordered, de-duplicated, 1..50 entries.
    pub tag_values: Vec<String>,
}

/// One explicitly granted Lake Formation permission: the principal it was granted to,
/// the resource it was granted on, and the two permission sets `GrantPermissions`
/// carries.
///
/// A record's identity is the (catalog, principal, resource, condition) tuple that
/// [`grant_key`] renders, so granting on the same resource to the same principal twice
/// UPDATES one record instead of adding a second — which is what makes
/// `GrantPermissions` idempotent and `ListPermissions` answer exactly one entry for a
/// repeated grant. Both permission lists are kept as sets in first-granted order, and a
/// record whose two sets are both empty is dropped rather than kept as an entry with no
/// permissions at all.
#[derive(Debug, Clone, Default)]
pub struct PermissionGrant {
    /// The Data Catalog the grant was made in ([`resolve_catalog_id`]).
    pub catalog_id: String,
    /// `DataLakePrincipal$DataLakePrincipalIdentifier` — the grantee.
    pub principal: String,
    /// The resource, as the modelled shape, with the absent catalog ids the model
    /// documents a default for filled in by [`normalize_resource`], so the same resource
    /// named twice is one record.
    pub resource: Resource,
    /// The permissions held, as a set in first-granted order.
    pub permissions: Vec<String>,
    /// The permissions held WITH the grant option, as a set in first-granted order.
    pub permissions_with_grant_option: Vec<String>,
    /// `Condition$Expression`, when the grant carried one: part of the record's identity,
    /// since the same permissions under two conditions are two grants.
    pub condition_expression: Option<String>,
}

impl PermissionGrant {
    /// This record as the wire shape `ListPermissions` projects it.
    ///
    /// `AdditionalDetails` carries the RAM resource-share ARNs of a cross-account share
    /// and `LastUpdated`/`LastUpdatedBy` the audit of the last change; this emulator
    /// models neither cross-account sharing nor an IAM identity for the caller, so they
    /// are left absent rather than filled with a fabricated share or identity.
    pub fn to_wire(&self) -> PrincipalResourcePermissions {
        PrincipalResourcePermissions {
            principal: Some(DataLakePrincipal {
                data_lake_principal_identifier: Some(self.principal.clone()),
            }),
            resource: Some(self.resource.clone()),
            condition: self
                .condition_expression
                .clone()
                .map(|expression| Condition {
                    expression: Some(expression),
                }),
            permissions: Some(self.permissions.clone()),
            permissions_with_grant_option: Some(self.permissions_with_grant_option.clone()),
            additional_details: None,
            last_updated: None,
            last_updated_by: None,
        }
    }
}

/// Per-service state for com-amazonaws-lakeformation-awslakeformation. A handler reaches it through [`service_state`].
///
/// # What stops this growing without end
///
/// An LF-tag is a durable control-plane record: AWS gives it no expiry, so this store
/// has no TTL and no eviction, and a tag is released only by `DeleteLFTag` (which also
/// drops a catalog left empty). What bounds it instead are three ceilings checked by
/// `CreateLFTag`, the single site that inserts: [`MAX_LF_TAGS_PER_CATALOG`] per catalog,
/// [`MAX_LF_TAG_CATALOGS`] catalogs, and [`MAX_LF_TAGS_TOTAL`] tags across every catalog
/// — the last two are what bound the store when the caller varies `CatalogId`, since the
/// catalog id is request-supplied. Passing any of them answers the modelled
/// `ResourceNumberLimitExceededException`, never a silent insert.
///
/// A permission grant is durable in the same way — only `RevokePermissions` releases one
/// — and `GrantPermissions`, its single inserting site, refuses a record past
/// [`MAX_PERMISSION_GRANTS`]. `revoked_default_grants` is a record of the same kind and
/// counts against the same ceiling, checked where `RevokePermissions` inserts into it.
///
/// `registered_resources` is bounded the same way, by [`MAX_REGISTERED_RESOURCES`] checked
/// in `RegisterResource`, the single site that inserts into it.
#[derive(Debug, Default)]
pub struct ComAmazonawsLakeformationAwslakeformationState {
    /// Catalog id -> (tag key -> tag). Both levels are `BTreeMap`s so enumeration order
    /// is stable and sorted, which is what makes `ListLFTags`' exclusive-start
    /// continuation token resume exactly where the previous page stopped.
    pub lf_tags: BTreeMap<String, BTreeMap<String, LfTag>>,
    /// [`grant_key`] -> the explicit grant it identifies. A `BTreeMap` for the same
    /// reason: `ListPermissions` pages through it in key order and its continuation
    /// token names the key the previous page stopped on.
    pub permission_grants: BTreeMap<String, PermissionGrant>,
    /// The [`grant_key`]s whose creation-time default grant has been revoked.
    ///
    /// A default grant ([`default_database_grant`]) is not a record: nothing granted it,
    /// so there is nothing in `permission_grants` for `RevokePermissions` to trim. What
    /// the revoke leaves behind is this memory that the grant is gone, which is what
    /// makes the default revocable exactly once.
    pub revoked_default_grants: BTreeSet<String>,
    /// Resource ARN -> its registration. A `BTreeMap` for the same reason: `ListResources`
    /// pages over a stable, sorted order, and `DescribeResource` is an exact-ARN lookup.
    pub registered_resources: BTreeMap<String, RegisteredResource>,
}

impl ComAmazonawsLakeformationAwslakeformationState {
    /// How many LF-tags the store holds across every catalog — what
    /// [`MAX_LF_TAGS_TOTAL`] bounds.
    pub fn total_lf_tags(&self) -> usize {
        self.lf_tags.values().map(BTreeMap::len).sum()
    }
}

/// The largest page `ListLFTags` can answer, and the page size it uses when the caller
/// names none.
///
/// Grounded in the model, not chosen: the output member `LFTags` targets `LFTagsList`,
/// which is `len 1..50`, so no single response can carry more than 50 LF-tag pairs even
/// though `MaxResults` is modelled `range 1..1000`.
pub const MAX_LF_TAGS_PAGE: i32 = 50;

/// The most values one LF-tag can hold.
///
/// Grounded in the model: `TagValues` targets `TagValueList`, which is `len 1..50`
/// (`smithy_query(query="shape",shape="TagValueList")`), so a tag can neither lose every
/// value nor grow past 50.
pub const MAX_TAG_VALUES: usize = 50;

/// The most LF-tags one Data Catalog can hold here.
///
/// That the number of LF-tags a catalog may hold IS bounded comes from the model:
/// `CreateLFTag` declares `ResourceNumberLimitExceededException` — "A resource numerical
/// limit was exceeded" (HTTP 400). The model does not carry the number, and AWS publishes
/// it as an adjustable service quota, so this ceiling is the emulator's own choice; what
/// the model fixes is that passing it answers that error instead of growing the store
/// without limit.
pub const MAX_LF_TAGS_PER_CATALOG: usize = 1000;

/// The most Data Catalogs this store will hold LF-tags for.
///
/// `CatalogId` is request-supplied, so the per-catalog ceiling alone would let a caller
/// that varies it grow the store without end. This bounds the partition count itself;
/// a create that would open one catalog too many answers the modelled
/// `ResourceNumberLimitExceededException`. Emulator-chosen, like the ceiling above.
pub const MAX_LF_TAG_CATALOGS: usize = 100;

/// The most LF-tags this store will hold across every catalog.
///
/// The store-wide ceiling, so no partitioning of the same number of create calls can
/// grow resident state past it. A create that would pass it answers the modelled
/// `ResourceNumberLimitExceededException`. Emulator-chosen, like the ceilings above.
pub const MAX_LF_TAGS_TOTAL: usize = 10_000;

/// The principal the data lake settings' default grant names: AWS's own group standing
/// for "every IAM principal that IAM itself allows", spelled exactly like this on the
/// wire.
pub const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The one permission the "use only IAM access control" default grant carries.
///
/// From the model, not chosen: `DataLakeSettings$CreateDatabaseDefaultPermissions`
/// documents that "A value that assigns ALL to IAM_ALLOWED_PRINCIPALS indicates access
/// control by IAM permissions. This is referred to as the setting \"Use only IAM access
/// control\", and is for backward compatibility with the Glue permission model", and that
/// "The only permitted values are an empty array or an array that contains a single JSON
/// object that grants ALL to IAM_ALLOWED_PRINCIPALS"
/// (`smithy_query(query="doc",shape="DataLakeSettings",member="CreateDatabaseDefaultPermissions")`).
pub const DEFAULT_IAM_ACCESS_CONTROL_PERMISSION: &str = "ALL";

/// The grant a Data Catalog DATABASE is created carrying, until a revoke takes it away:
/// `ALL` to `IAM_ALLOWED_PRINCIPALS`, the account's "use only IAM access control" default
/// (see [`DEFAULT_IAM_ACCESS_CONTROL_PERMISSION`]). `None` — nothing is held by default —
/// for anything that default does not cover.
///
/// Why it stops at databases: the default a resource is created with comes from the
/// account's data lake settings for a DATABASE
/// (`DataLakeSettings$CreateDatabaseDefaultPermissions`), but for a TABLE it comes from
/// the parent database's own `CreateTableDefaultPermissions` in the Data Catalog — which
/// is another service's state and not readable from here — so a table holds nothing until
/// something grants it. The database default is this service's own setting, so it is the
/// one this store can answer for.
///
/// It is rendered per request rather than written when the database is created, because
/// this store never sees a database being created. That is observationally the same:
/// `RevokePermissions` is the only call that can take the default away, and what it leaves
/// behind is recorded in `revoked_default_grants`, which answers every later call.
///
/// Two consequences of having no view of the Data Catalog, both deliberate:
///
/// * Every database NAME carries the default, including one that was never created, so a
///   revoke against a database that does not exist answers 200 where the real service
///   answers `EntityNotFoundException`. Refusing that would need the catalog's list of
///   databases, which is the glue service's state; a later change that can read it should
///   check existence here.
/// * `ListPermissions` does not report default grants, which is what its own documentation
///   says it does: "This operation returns only those permissions that have been explicitly
///   granted" (`smithy_query(query="doc",shape="ListPermissions")`). A default grant is
///   held but was never granted by a call, so a reader that enumerates records is right to
///   omit it — and with no set of databases to enumerate, a request that NAMES one is the
///   only thing that can render its default.
pub fn default_database_grant(
    resource: &Resource,
    principal: &str,
    condition: Option<&str>,
) -> Option<Vec<String>> {
    // The default carries no condition, so a condition-scoped request addresses a
    // different grant from the default one.
    if resource.database.is_none() || principal != IAM_ALLOWED_PRINCIPALS || condition.is_some() {
        return None;
    }
    Some(vec![DEFAULT_IAM_ACCESS_CONTROL_PERMISSION.to_string()])
}

/// The most revoked-default records this store will hold, beside the explicit grants
/// [`MAX_PERMISSION_GRANTS`] bounds.
///
/// A revoked default is keyed by a request-supplied database name, so it needs a ceiling
/// of its own for the same reason the grant store does; it gets a separate one so neither
/// store's capacity depends on how full the other is. `RevokePermissions`, the single site
/// that inserts one, refuses past this ceiling with its modelled `InvalidInputException`.
/// Emulator-chosen, like the ceilings above.
pub const MAX_REVOKED_DEFAULT_GRANTS: usize = 10_000;

/// The most data-lake locations this store will hold registrations for.
///
/// That the number of registered locations IS bounded comes from the model:
/// `RegisterResource` declares `ResourceNumberLimitExceededException` — "A resource
/// numerical limit was exceeded" (HTTP 400). The model does not carry the number, and AWS
/// publishes it as an adjustable service quota, so this ceiling is the emulator's own
/// choice; what the model fixes is that passing it answers that error instead of growing
/// the store without limit.
pub const MAX_REGISTERED_RESOURCES: usize = 1_000;

/// The Lake Formation service-linked role a registration uses when the request sets
/// `UseServiceLinkedRole` and names no `RoleArn`.
///
/// The role name is the one `RegisterResource`'s documentation names
/// ("AWSServiceRoleForLakeFormationDataAccess"); the ARN is built for this emulator's
/// account, and matches `IAMRoleArn`'s `^arn:aws:iam::[0-9]*:role/` pattern.
pub fn service_linked_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/aws-service-role/lakeformation.amazonaws.com/AWSServiceRoleForLakeFormationDataAccess",
        ::engine::emulator_account_id()
    )
}

/// `Ok(())` when `arn` has the shape of an Amazon Resource Name, else why it does not.
///
/// `ResourceArnString` carries no pattern in the model, so the boundary validator admits
/// any string and the handler is what must refuse a value that is not an ARN at all. The
/// six-field `arn:partition:service:region:account-id:resource` layout is the general ARN
/// format; `region` and `account-id` are empty for an Amazon S3 location
/// (`arn:aws:s3:::my-bucket/`), so only `partition`, `service` and `resource` are required
/// to be non-empty.
pub fn validate_resource_arn(arn: &str) -> Result<(), String> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let malformed = fields.len() < 6
        || fields[0] != "arn"
        || fields[1].is_empty()
        || fields[2].is_empty()
        || fields[5].is_empty();
    if malformed {
        return Err(format!(
            "ResourceArn {arn:?} is not a valid Amazon Resource Name: expected \
             arn:<partition>:<service>:<region>:<account-id>:<resource>"
        ));
    }
    Ok(())
}

/// The Data Catalog a request addresses: its `CatalogId` when it carries one, else the
/// caller's account id ("The identifier for the Data Catalog. By default, the account
/// ID." — `CatalogIdString`'s own documentation).
pub fn resolve_catalog_id(requested: Option<&str>) -> String {
    match requested {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => ::engine::emulator_account_id().to_string(),
    }
}

/// The values of `values`, in first-seen order, with duplicates dropped.
///
/// An LF-tag's values are a SET of the values the key may take, so a repeated value in
/// one request, or a value already stored, must not appear twice in the stored tag.
pub fn dedup_values<'a, I>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a String>,
{
    // A linear scan of the output, not a hash set: `TagValueList` is `len 1..50`, so the
    // input is at most 50 values and first-seen order is what must be preserved.
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if !out.iter().any(|existing| existing == value) {
            out.push(value.clone());
        }
    }
    out
}

/// The largest page `ListPermissions` answers, and the page size it uses when the caller
/// names none.
///
/// Grounded in the model: `MaxResults` is `PageSize`, `range 1..1000`, and the output
/// member `PrincipalResourcePermissions` targets `PrincipalResourcePermissionsList`,
/// which carries no length limit of its own — so the modelled ceiling on `MaxResults` is
/// what bounds a page.
pub const MAX_PERMISSIONS_PAGE: i32 = 1000;

/// The most explicit permission grants this store will hold.
///
/// A grant is a durable record released only by `RevokePermissions`, and both the
/// principal and the resource are request-supplied, so without a ceiling a caller that
/// varies either would grow the store without end. `GrantPermissions` is the single site
/// that inserts, and it refuses a record past this with the modelled
/// `InvalidInputException` (the operation models no quota error of its own:
/// `smithy_query(query="operation",operation="GrantPermissions")` lists only
/// `ConcurrentModificationException`, `EntityNotFoundException` and
/// `InvalidInputException`). Emulator-chosen, like the LF-tag ceilings above.
pub const MAX_PERMISSION_GRANTS: usize = 10_000;

/// The `DataLakeResourceType` values that select `resource`.
///
/// From the model: `DataLakeResourceType` is `CATALOG | DATABASE | DATA_LOCATION |
/// LF_NAMED_TAG_EXPRESSION | LF_TAG | LF_TAG_POLICY | LF_TAG_POLICY_DATABASE |
/// LF_TAG_POLICY_TABLE | TABLE` (`crate::ops::types::DATA_LAKE_RESOURCE_TYPE_VALUES`),
/// while `Resource` carries ten members — so the mapping is not one-to-one:
///
/// * a column-level grant (`TableWithColumns`) is a grant on a TABLE;
/// * an LF-tag policy is selected both by the general `LF_TAG_POLICY` and by the
///   specialisation its own `ResourceType` (`DATABASE | TABLE`) names;
/// * `DataCellsFilter` and `RLSPolicy` have NO `DataLakeResourceType` member, so no
///   `ResourceType` filter selects them (a listing that names no `ResourceType` still
///   does).
pub fn resource_types(resource: &Resource) -> Vec<&'static str> {
    if resource.catalog.is_some() {
        vec!["CATALOG"]
    } else if resource.database.is_some() {
        vec!["DATABASE"]
    } else if resource.table.is_some() || resource.table_with_columns.is_some() {
        vec!["TABLE"]
    } else if resource.data_location.is_some() {
        vec!["DATA_LOCATION"]
    } else if resource.lf_tag.is_some() {
        vec!["LF_TAG"]
    } else if let Some(policy) = &resource.lf_tag_policy {
        match policy.resource_type.as_str() {
            "DATABASE" => vec!["LF_TAG_POLICY", "LF_TAG_POLICY_DATABASE"],
            "TABLE" => vec!["LF_TAG_POLICY", "LF_TAG_POLICY_TABLE"],
            _ => vec!["LF_TAG_POLICY"],
        }
    } else if resource.lf_tag_expression.is_some() {
        vec!["LF_NAMED_TAG_EXPRESSION"]
    } else {
        Vec::new()
    }
}

/// Why `resource` cannot name a resource, or `Ok(())` when it names exactly one.
///
/// `Resource` is modelled as a STRUCTURE whose ten members are all optional, but it
/// stands for a choice: every member is a different resource, so naming none leaves the
/// grant with no target and naming two leaves it ambiguous. The two members that are
/// themselves a choice — a table named by `Name` or by `TableWildcard`, and the columns
/// of a `TableWithColumns` named by `ColumnNames` or by `ColumnWildcard` — are checked
/// the same way. The caller is refused with the modelled `InvalidInputException`; the
/// message names the member, which is the rule the boundary validator follows for the
/// constraints the model CAN express.
pub fn validate_resource(resource: &Resource) -> Result<(), String> {
    let named: Vec<&str> = [
        ("Catalog", resource.catalog.is_some()),
        ("Database", resource.database.is_some()),
        ("Table", resource.table.is_some()),
        ("TableWithColumns", resource.table_with_columns.is_some()),
        ("DataLocation", resource.data_location.is_some()),
        ("DataCellsFilter", resource.data_cells_filter.is_some()),
        ("LFTag", resource.lf_tag.is_some()),
        ("LFTagPolicy", resource.lf_tag_policy.is_some()),
        ("LFTagExpression", resource.lf_tag_expression.is_some()),
        ("RLSPolicy", resource.rls_policy.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, present)| present.then_some(name))
    .collect();
    match named.len() {
        1 => {}
        0 => {
            return Err(
                "Resource must name exactly one resource type, but none was provided".to_string(),
            )
        }
        count => {
            return Err(format!(
                "Resource must name exactly one resource type, but {count} were provided: {}",
                named.join(", ")
            ))
        }
    }

    if let Some(table) = &resource.table {
        match (table.name.is_some(), table.table_wildcard.is_some()) {
            (true, true) => {
                return Err(
                    "Resource.Table must name either Name or TableWildcard, not both".to_string(),
                )
            }
            (false, false) => {
                return Err("Resource.Table must name either Name or TableWildcard".to_string())
            }
            _ => {}
        }
    }

    if let Some(table) = &resource.table_with_columns {
        match (&table.column_names, &table.column_wildcard) {
            (Some(_), Some(_)) => {
                return Err(
                    "Resource.TableWithColumns must name either ColumnNames or ColumnWildcard, \
                     not both"
                        .to_string(),
                )
            }
            (None, None) => {
                return Err(
                    "Resource.TableWithColumns must name either ColumnNames or ColumnWildcard"
                        .to_string(),
                )
            }
            (Some(columns), None) if columns.is_empty() => {
                return Err(
                    "Resource.TableWithColumns.ColumnNames must name at least one column"
                        .to_string(),
                )
            }
            _ => {}
        }
    }

    Ok(())
}

/// `resource` with every absent catalog id that the model DOCUMENTS a default for filled
/// in with `catalog_id`.
///
/// "The identifier for the Data Catalog. By default, the account ID." holds for the
/// catalog id INSIDE a resource too (`smithy_query(query="doc",shape="TableResource",
/// member="CatalogId")`: "By default, it is the account ID of the caller"), so a grant
/// on a resource named without one and a grant on the same resource named with the
/// request's own catalog id are the same grant — which only holds if the stored form is
/// normalised before [`resource_key`] renders it.
///
/// Two members are left exactly as the request named them, because the model documents
/// no default for either: `CatalogResource$Id` ("An identifier for the catalog
/// resource" — a catalog resource is named, not defaulted) and nothing else. A
/// `DataCellsFilterResource$TableCatalogId` ("The ID of the catalog to which the table
/// belongs") IS defaulted with the rest: the table it belongs to takes the caller's
/// account as its catalog, so the filter on that table cannot sit in another one, and
/// leaving it absent would make the same filter named with and without the account id
/// two records.
pub fn normalize_resource(resource: &Resource, catalog_id: &str) -> Resource {
    /// Fills `slot` with `catalog_id` when it names no catalog.
    fn default_catalog(slot: &mut Option<String>, catalog_id: &str) {
        if slot.as_deref().is_none_or(str::is_empty) {
            *slot = Some(catalog_id.to_string());
        }
    }

    let mut normalized = resource.clone();
    if let Some(database) = normalized.database.as_mut() {
        default_catalog(&mut database.catalog_id, catalog_id);
    }
    if let Some(table) = normalized.table.as_mut() {
        default_catalog(&mut table.catalog_id, catalog_id);
    }
    if let Some(table) = normalized.table_with_columns.as_mut() {
        default_catalog(&mut table.catalog_id, catalog_id);
    }
    if let Some(location) = normalized.data_location.as_mut() {
        default_catalog(&mut location.catalog_id, catalog_id);
    }
    if let Some(cells) = normalized.data_cells_filter.as_mut() {
        default_catalog(&mut cells.table_catalog_id, catalog_id);
    }
    if let Some(tag) = normalized.lf_tag.as_mut() {
        default_catalog(&mut tag.catalog_id, catalog_id);
    }
    if let Some(policy) = normalized.lf_tag_policy.as_mut() {
        default_catalog(&mut policy.catalog_id, catalog_id);
    }
    if let Some(expression) = normalized.lf_tag_expression.as_mut() {
        default_catalog(&mut expression.catalog_id, catalog_id);
    }
    if let Some(policy) = normalized.rls_policy.as_mut() {
        default_catalog(&mut policy.catalog_id, catalog_id);
    }
    normalized
}

/// Appends `value` to `key` length-prefixed (`5:alice`).
///
/// Length-prefixing is what makes a composite key INJECTIVE: no value a request supplies
/// — a table name with a colon, an S3 ARN with a slash — can be read as the next field,
/// so two different resources can never render the same key and share one grant record.
fn push_text(key: &mut String, value: &str) {
    key.push_str(&value.len().to_string());
    key.push(':');
    key.push_str(value);
}

/// Appends an optional value: `-` for absent, `+` and the value for present.
fn push_optional(key: &mut String, value: Option<&str>) {
    match value {
        Some(value) => {
            key.push('+');
            push_text(key, value);
        }
        None => key.push('-'),
    }
}

/// Appends a SET of values: sorted and de-duplicated, so the order a request listed them
/// in does not change which resource the key names.
fn push_set(key: &mut String, values: &[String]) {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    key.push('[');
    for value in sorted {
        push_text(key, value);
    }
    key.push(']');
}

/// The identity of the resource `resource` names: equal keys are the same resource.
///
/// Built from the members of the one `Resource` member that is set, with the lists the
/// model carries as SETS (a table's columns, an LF-tag's values, an LF-tag policy's
/// expression) rendered order-insensitively — granting on columns `[b, a]` and on
/// `[a, b]` is a grant on the same columns. A table named by `TableWildcard` is its own
/// resource ("every table in the database"), not the table whose name is absent, so it
/// gets its own key.
pub fn resource_key(resource: &Resource) -> String {
    let mut key = String::new();
    if let Some(catalog) = &resource.catalog {
        key.push_str("CATALOG|");
        push_optional(&mut key, catalog.id.as_deref());
    } else if let Some(database) = &resource.database {
        key.push_str("DATABASE|");
        push_optional(&mut key, database.catalog_id.as_deref());
        push_text(&mut key, &database.name);
    } else if let Some(table) = &resource.table {
        key.push_str("TABLE|");
        push_optional(&mut key, table.catalog_id.as_deref());
        push_text(&mut key, &table.database_name);
        match &table.name {
            Some(name) => push_text(&mut key, name),
            None => key.push('*'),
        }
    } else if let Some(table) = &resource.table_with_columns {
        key.push_str("TABLE_WITH_COLUMNS|");
        push_optional(&mut key, table.catalog_id.as_deref());
        push_text(&mut key, &table.database_name);
        push_text(&mut key, &table.name);
        match &table.column_wildcard {
            Some(wildcard) => {
                key.push_str("EXCLUDING");
                push_set(
                    &mut key,
                    wildcard.excluded_column_names.as_deref().unwrap_or_default(),
                );
            }
            None => {
                key.push_str("INCLUDING");
                push_set(&mut key, table.column_names.as_deref().unwrap_or_default());
            }
        }
    } else if let Some(location) = &resource.data_location {
        key.push_str("DATA_LOCATION|");
        push_optional(&mut key, location.catalog_id.as_deref());
        push_text(&mut key, &location.resource_arn);
    } else if let Some(filter) = &resource.data_cells_filter {
        key.push_str("DATA_CELLS_FILTER|");
        push_optional(&mut key, filter.table_catalog_id.as_deref());
        push_optional(&mut key, filter.database_name.as_deref());
        push_optional(&mut key, filter.table_name.as_deref());
        push_optional(&mut key, filter.name.as_deref());
    } else if let Some(tag) = &resource.lf_tag {
        key.push_str("LF_TAG|");
        push_optional(&mut key, tag.catalog_id.as_deref());
        push_text(&mut key, &tag.tag_key);
        push_set(&mut key, &tag.tag_values);
    } else if let Some(policy) = &resource.lf_tag_policy {
        key.push_str("LF_TAG_POLICY|");
        push_optional(&mut key, policy.catalog_id.as_deref());
        push_text(&mut key, &policy.resource_type);
        push_optional(&mut key, policy.expression_name.as_deref());
        let mut expression: Vec<&LFTag> = policy
            .expression
            .as_deref()
            .unwrap_or_default()
            .iter()
            .collect();
        expression.sort_by(|left, right| left.tag_key.cmp(&right.tag_key));
        key.push('{');
        for tag in expression {
            push_text(&mut key, &tag.tag_key);
            push_set(&mut key, &tag.tag_values);
        }
        key.push('}');
    } else if let Some(expression) = &resource.lf_tag_expression {
        key.push_str("LF_TAG_EXPRESSION|");
        push_optional(&mut key, expression.catalog_id.as_deref());
        push_text(&mut key, &expression.name);
    } else if let Some(policy) = &resource.rls_policy {
        key.push_str("RLS_POLICY|");
        push_optional(&mut key, policy.catalog_id.as_deref());
        push_text(&mut key, &policy.name);
    } else {
        // Unreachable through a handler: `validate_resource` refuses a `Resource` naming
        // no member before a grant or a filter is keyed.
        key.push_str("NONE");
    }
    key
}

/// The identity of one grant record: the (catalog, principal, resource, condition) tuple
/// a `GrantPermissions` call addresses.
///
/// Catalog first, then principal, so [`ComAmazonawsLakeformationAwslakeformationState::permission_grants`]
/// enumerates in a stable order a `ListPermissions` page can resume from.
pub fn grant_key(
    catalog_id: &str,
    principal: &str,
    resource: &Resource,
    condition: Option<&str>,
) -> String {
    let mut key = String::new();
    push_text(&mut key, catalog_id);
    push_text(&mut key, principal);
    push_optional(&mut key, condition);
    key.push_str(&resource_key(resource));
    key
}

/// Does the grant on `stored` fall under a listing filtered by the resource `filter`?
///
/// Identity is the base case. Beyond it the model's own documentation fixes two
/// relations, both only for a table named by `Name`:
///
/// * `ListPermissions$Resource` — "This operation does not support getting privileges on
///   a table with columns. Instead, call this operation on the table, and the operation
///   returns the table and the table w columns" — so a table filter also selects the
///   column-level grants on that same table;
/// * `ListPermissions$IncludeRelated` — "Set the field to TRUE to show the cell filters
///   on a table resource" — so with `include_related` a table filter also selects the
///   data-cells-filter grants on that same table.
pub fn resource_filter_matches(filter: &Resource, stored: &Resource, include_related: bool) -> bool {
    if resource_key(filter) == resource_key(stored) {
        return true;
    }
    let Some(table) = filter.table.as_ref() else {
        return false;
    };
    let Some(name) = table.name.as_deref() else {
        return false;
    };
    if let Some(columns) = &stored.table_with_columns {
        return columns.catalog_id == table.catalog_id
            && columns.database_name == table.database_name
            && columns.name == name;
    }
    if include_related {
        if let Some(cells) = &stored.data_cells_filter {
            return cells.table_catalog_id == table.catalog_id
                && cells.database_name.as_deref() == Some(table.database_name.as_str())
                && cells.table_name.as_deref() == Some(name);
        }
    }
    false
}

/// Adds `added` to `held`, keeping it a set in first-granted order.
pub fn merge_permissions(held: &mut Vec<String>, added: &[String]) {
    for permission in added {
        if !held.iter().any(|existing| existing == permission) {
            held.push(permission.clone());
        }
    }
}

/// The entries of `requested` that `held` does not carry, in first-named order: what a
/// revoke cannot take away because it was never granted.
pub fn missing_permissions(held: &[String], requested: &[String]) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for permission in requested {
        if !held.iter().any(|existing| existing == permission)
            && !missing.iter().any(|existing| existing == permission)
        {
            missing.push(permission.clone());
        }
    }
    missing
}

/// Drops every entry of `removed` from `held`.
pub fn remove_permissions(held: &mut Vec<String>, removed: &[String]) {
    held.retain(|permission| !removed.iter().any(|dropped| dropped == permission));
}

/// The field separator inside a continuation token's plaintext.
///
/// `U+001F` (unit separator) cannot appear in either field: `CatalogIdString` is
/// `^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$` and `LFTagKey` is
/// `^([\p{L}\p{Z}\p{N}_.:\/=+\-@%]*)$`, neither of which admits a C0 control other than
/// tab — so splitting on it can never cut a value in half.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// The tag identifying a token this service minted, and the token layout's version.
const TOKEN_PREFIX: &str = "lftags";
const TOKEN_VERSION: &str = "1";

/// An opaque continuation token standing for "resume strictly after this (catalog, key)".
///
/// Hex-wrapped so a caller treats it as opaque and round-trips it unchanged, and
/// self-describing so a token this service did not mint is detectable (see
/// [`decode_page_token`]).
pub fn encode_page_token(catalog_id: &str, tag_key: &str) -> String {
    let plain =
        format!("{TOKEN_PREFIX}{TOKEN_SEPARATOR}{TOKEN_VERSION}{TOKEN_SEPARATOR}{catalog_id}{TOKEN_SEPARATOR}{tag_key}");
    hex::encode(plain)
}

/// The `(catalog id, tag key)` exclusive start a token names, or `None` when the token
/// was not minted by [`encode_page_token`] (so the caller must be refused rather than
/// silently served page one).
pub fn decode_page_token(token: &str) -> Option<(String, String)> {
    let bytes = hex::decode(token).ok()?;
    let plain = String::from_utf8(bytes).ok()?;
    let mut fields = plain.split(TOKEN_SEPARATOR);
    let prefix = fields.next()?;
    let version = fields.next()?;
    let catalog_id = fields.next()?;
    let tag_key = fields.next()?;
    if prefix != TOKEN_PREFIX || version != TOKEN_VERSION || fields.next().is_some() {
        return None;
    }
    Some((catalog_id.to_string(), tag_key.to_string()))
}

/// The tag identifying a `ListPermissions` continuation token this service minted. It
/// differs from [`TOKEN_PREFIX`] so a token minted for one paginated operation is not
/// accepted by the other.
const PERMISSIONS_TOKEN_PREFIX: &str = "lfperms";

/// An opaque continuation token standing for "resume strictly after this grant key".
///
/// Hex-wrapped and self-describing for the same reasons as [`encode_page_token`].
pub fn encode_permissions_page_token(grant_key: &str) -> String {
    let plain = format!(
        "{PERMISSIONS_TOKEN_PREFIX}{TOKEN_SEPARATOR}{TOKEN_VERSION}{TOKEN_SEPARATOR}{grant_key}"
    );
    hex::encode(plain)
}

/// The grant key a token names an exclusive start at, or `None` when the token was not
/// minted by [`encode_permissions_page_token`] (so the caller is refused rather than
/// silently served page one).
///
/// The key is the token's LAST field and is taken whole (`splitn`), because a grant key
/// embeds request-supplied text — a `DataLocation` ARN is modelled with no pattern at
/// all — and must survive the round trip byte for byte.
pub fn decode_permissions_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let plain = String::from_utf8(bytes).ok()?;
    let mut fields = plain.splitn(3, TOKEN_SEPARATOR);
    let prefix = fields.next()?;
    let version = fields.next()?;
    let grant_key = fields.next()?;
    if prefix != PERMISSIONS_TOKEN_PREFIX || version != TOKEN_VERSION {
        return None;
    }
    Some(grant_key.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLakeformationAwslakeformationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLakeformationAwslakeformationState>)
}
