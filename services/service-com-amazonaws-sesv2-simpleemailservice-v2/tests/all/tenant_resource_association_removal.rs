// SPDX-License-Identifier: CC0-1.0

//! Removing a tenant-resource association, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: `DeleteTenantResourceAssociation` resolves BOTH of the
//! names it is given — the tenant, and the resource the `ResourceArn` addresses — before
//! it removes anything, and then removes only the one record keyed by that pair: the
//! resource and the tenant both survive, every other association either of them holds
//! survives, and both views of the removed record agree it is gone
//! (`ListResourceTenants` for the resource no longer reports the tenant, and
//! `ListTenantResources` for the tenant no longer reports the resource). The removal is
//! idempotent — a pair that holds no association is already in the state the call asks
//! for, so the empty 200 is the answer whether or not a record was there — while a name
//! that resolves to nothing is refused with the modelled terminal 404 and removes
//! nothing, and input outside the documented form is refused 400 on the input, before
//! any lookup.
//!
//! The tables below span what the rule generalises over: the kind of resource the ARN
//! names (each `ResourceType` the model binds), the form of the tenant name (one
//! character, the documented 64-character bound, every documented character class), how
//! many tenants share one resource and how many resources one tenant holds, how many
//! times the same removal is repeated, the Region the ARN carries against the Region the
//! call executes in, and every refusal leg.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};
use crate::contact_lists::query_escaped;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state;

/// The canonical account id every emulated ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// The Region the shared harness signs its calls for, and so the Region an ARN must
/// carry to name a resource those calls can reach.
const REGION: &str = "us-east-1";

/// The longest `TenantName` SES accepts, taken from the crate's own constant so the
/// bound is stated once: its documentation states the name "can contain up to 64
/// alphanumeric characters, including letters, numbers, hyphens (-) and underscores (_)
/// only" (`smithy_query(query="shape",shape="TenantName")`).
const MAX_TENANT_NAME_CHARS: usize = state::MAX_TENANT_NAME_CHARS;

/// Convenience calls for the tenant-resource association surface, over the shared
/// harness.
///
/// Shared with the sibling suites of this test target, so every case that sets a tenant
/// and a resource up drives the same wire path.
pub(crate) trait Associations {
    async fn create_tenant(&self, name: &str) -> Reply;
    async fn get_tenant(&self, name: &str) -> Reply;
    async fn create_configuration_set(&self, name: &str) -> Reply;
    async fn create_email_identity(&self, identity: &str) -> Reply;
    async fn get_email_identity(&self, identity: &str) -> Reply;
    async fn associate(&self, tenant_name: &str, resource_arn: &str) -> Reply;
    async fn disassociate(&self, tenant_name: &str, resource_arn: &str) -> Reply;
    async fn list_resource_tenants(&self, resource_arn: &str) -> Reply;
    async fn list_tenant_resources(&self, tenant_name: &str) -> Reply;
}

impl Associations for Ses {
    async fn create_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn get_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/get",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn create_configuration_set(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({"ConfigurationSetName": name})),
        )
        .await
    }

    async fn create_email_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": identity})),
        )
        .await
    }

    async fn get_email_identity(&self, identity: &str) -> Reply {
        // `EmailIdentity` is an `@httpLabel`, so the identity reaches the service
        // escaped, the way an SDK escapes a label.
        self.call(
            "GET",
            &format!("/v2/email/identities/{}", query_escaped(identity)),
            None,
        )
        .await
    }

    async fn associate(&self, tenant_name: &str, resource_arn: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/resources",
            Some(json!({"TenantName": tenant_name, "ResourceArn": resource_arn})),
        )
        .await
    }

    async fn disassociate(&self, tenant_name: &str, resource_arn: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/resources/delete",
            Some(json!({"TenantName": tenant_name, "ResourceArn": resource_arn})),
        )
        .await
    }

    async fn list_resource_tenants(&self, resource_arn: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/resources/tenants/list",
            Some(json!({"ResourceArn": resource_arn})),
        )
        .await
    }

    async fn list_tenant_resources(&self, tenant_name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/resources/list",
            Some(json!({"TenantName": tenant_name})),
        )
        .await
    }
}

/// The `TenantName` of every tenant a resource's listing reported, in the order
/// reported.
pub(crate) fn listed_tenants(reply: &Reply) -> Vec<String> {
    reply
        .member("ResourceTenants")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|row| text(row, "TenantName"))
        .collect()
}

/// The `ResourceArn` of every resource a tenant's listing reported, in the order
/// reported.
pub(crate) fn listed_resources(reply: &Reply) -> Vec<String> {
    reply
        .member("TenantResources")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|row| text(row, "ResourceArn"))
        .collect()
}

/// A string member of one reported row, or `""` when absent.
fn text(row: &Value, member: &str) -> String {
    row.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The resource an accepted removal is made for: the kind, and the name SES holds it
/// under.
///
/// The three kinds a tenant may hold are the `ResourceType` values the model binds
/// (`smithy_query(query="shape",shape="ResourceType")`: `EMAIL_IDENTITY`,
/// `CONFIGURATION_SET`, `EMAIL_TEMPLATE`); the two below are the kinds this emulator can
/// be made to hold, and the template kind is covered by the refusal table, where an ARN
/// naming a resource the account does not hold belongs.
pub(crate) enum Resource {
    /// A configuration set, created with `CreateConfigurationSet`.
    ConfigurationSet(&'static str),
    /// An email identity — an address or a domain — created with `CreateEmailIdentity`.
    EmailIdentity(&'static str),
}

impl Resource {
    /// The ARN SES addresses this resource by, in the Region the call executes in.
    pub(crate) fn arn(&self) -> String {
        match self {
            Self::ConfigurationSet(name) => {
                format!("arn:aws:ses:{REGION}:{ACCOUNT_ID}:configuration-set/{name}")
            }
            Self::EmailIdentity(identity) => {
                format!("arn:aws:ses:{REGION}:{ACCOUNT_ID}:identity/{identity}")
            }
        }
    }

    /// Create the resource, failing loudly if the create is refused: a removal case is
    /// only about the removal.
    pub(crate) async fn create(&self, ses: &Ses, case: &str) {
        let created = match self {
            Self::ConfigurationSet(name) => ses.create_configuration_set(name).await,
            Self::EmailIdentity(identity) => ses.create_email_identity(identity).await,
        };
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected HTTP 200 creating the resource, got {} {:?}",
            created.status,
            created.message()
        );
    }

    /// Read the resource back through its own getter, so a case can show it survived the
    /// removal of an association it held.
    pub(crate) async fn read(&self, ses: &Ses) -> Reply {
        match self {
            Self::ConfigurationSet(name) => ses.get_set(name).await,
            Self::EmailIdentity(identity) => ses.get_email_identity(identity).await,
        }
    }
}

/// One accepted removal: the tenant and the resource whose association is removed.
struct RemovalCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `TenantName` to create and associate.
    tenant_name: String,
    /// The resource the association is made for.
    resource: Resource,
}

/// The accepted-removal table: the resource kinds the emulator can hold, against tenant
/// names spanning the documented form from its shortest to its 64-character bound and
/// every character class it allows.
fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "a configuration set, and the shortest tenant name the model allows",
            tenant_name: "a".to_string(),
            resource: Resource::ConfigurationSet("nightly"),
        },
        RemovalCase {
            case: "an email identity that is an address",
            tenant_name: "Tenant_01-beta".to_string(),
            resource: Resource::EmailIdentity("sender@example.com"),
        },
        RemovalCase {
            case: "an email identity that is a domain",
            tenant_name: "domain_tenant".to_string(),
            resource: Resource::EmailIdentity("example.com"),
        },
        RemovalCase {
            case: "a configuration set, and a tenant name at the documented 64-character bound",
            tenant_name: format!(
                "tenant-{}",
                "x".repeat(MAX_TENANT_NAME_CHARS - "tenant-".len())
            ),
            resource: Resource::ConfigurationSet("promo_set-2"),
        },
    ]
}

#[tokio::test]
async fn a_removal_clears_both_views_and_repeats_without_a_refusal() {
    for RemovalCase {
        case,
        tenant_name,
        resource,
    } in removal_cases()
    {
        let ses = Ses::new();
        let resource_arn = resource.arn();

        let created = ses.create_tenant(&tenant_name).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected HTTP 200 creating the tenant, got {} {:?}",
            created.status,
            created.message()
        );
        resource.create(&ses, case).await;

        let associated = ses.associate(&tenant_name, &resource_arn).await;
        assert_eq!(
            associated.status,
            200,
            "case {case:?}: expected HTTP 200 associating the resource, got {} {:?}",
            associated.status,
            associated.message()
        );
        let before = ses.list_resource_tenants(&resource_arn).await;
        assert_eq!(
            listed_tenants(&before),
            vec![tenant_name.clone()],
            "case {case:?}: expected the association to be reported before the removal, got {:?}",
            listed_tenants(&before)
        );

        // The removal itself: an empty 200, like every other call on this surface.
        let removed = ses.disassociate(&tenant_name, &resource_arn).await;
        assert_eq!(
            removed.status,
            200,
            "case {case:?}: expected HTTP 200 removing the association, got {} {:?}",
            removed.status,
            removed.message()
        );
        assert!(
            removed.is_empty_body(),
            "case {case:?}: expected an empty response body, got {:?}",
            removed.member("")
        );

        // Both views of the one record agree it is gone.
        let resource_tenants = ses.list_resource_tenants(&resource_arn).await;
        assert!(
            listed_tenants(&resource_tenants).is_empty(),
            "case {case:?}: expected the resource to report no tenants after the removal, got {:?}",
            listed_tenants(&resource_tenants)
        );
        let tenant_resources = ses.list_tenant_resources(&tenant_name).await;
        assert!(
            listed_resources(&tenant_resources).is_empty(),
            "case {case:?}: expected the tenant to report no resources after the removal, got {:?}",
            listed_resources(&tenant_resources)
        );

        // Repeating the removal asks for a state the store is already in, so it is
        // answered exactly as the first one was.
        for repeat in 2..=3 {
            let again = ses.disassociate(&tenant_name, &resource_arn).await;
            assert_eq!(
                again.status,
                200,
                "case {case:?}: expected removal {repeat} to be an idempotent HTTP 200, got {} {:?}",
                again.status,
                again.message()
            );
            assert!(
                again.is_empty_body(),
                "case {case:?}: expected removal {repeat} to answer an empty body, got {:?}",
                again.member("")
            );
        }

        // "The resource itself is not deleted, only its association with the specific
        // tenant is removed" — and the tenant is not deleted either.
        let tenant = ses.get_tenant(&tenant_name).await;
        assert_eq!(
            tenant.status,
            200,
            "case {case:?}: expected the tenant to survive the removal, got {} {:?}",
            tenant.status,
            tenant.message()
        );
        let read_back = resource.read(&ses).await;
        assert_eq!(
            read_back.status,
            200,
            "case {case:?}: expected the resource to survive the removal, got {} {:?}",
            read_back.status,
            read_back.message()
        );
    }
}

#[tokio::test]
async fn removing_one_association_leaves_every_other_one() {
    // The rule generalises over how many tenants share one resource and how many
    // resources one tenant holds, so the store holds both fan-outs at once and exactly
    // one pair is removed.
    let case = "two resources across three tenants, one pair removed";
    let ses = Ses::new();
    let shared = Resource::ConfigurationSet("shared_set");
    let other = Resource::ConfigurationSet("other-set");
    let shared_arn = shared.arn();
    let other_arn = other.arn();
    shared.create(&ses, case).await;
    other.create(&ses, case).await;

    for tenant_name in ["tenant-1", "tenant-2", "tenant-3"] {
        let created = ses.create_tenant(tenant_name).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected HTTP 200 creating tenant {tenant_name:?}, got {} {:?}",
            created.status,
            created.message()
        );
    }
    for (tenant_name, resource_arn) in [
        ("tenant-1", &shared_arn),
        ("tenant-2", &shared_arn),
        ("tenant-1", &other_arn),
        ("tenant-3", &other_arn),
    ] {
        let associated = ses.associate(tenant_name, resource_arn).await;
        assert_eq!(
            associated.status,
            200,
            "case {case:?}: expected HTTP 200 associating {resource_arn} with {tenant_name:?}, \
             got {} {:?}",
            associated.status,
            associated.message()
        );
    }

    let removed = ses.disassociate("tenant-1", &shared_arn).await;
    assert_eq!(
        removed.status,
        200,
        "case {case:?}: expected HTTP 200 removing the association, got {} {:?}",
        removed.status,
        removed.message()
    );

    let shared_tenants = ses.list_resource_tenants(&shared_arn).await;
    assert_eq!(
        listed_tenants(&shared_tenants),
        vec!["tenant-2".to_string()],
        "case {case:?}: expected the removed resource to keep its other tenant, got {:?}",
        listed_tenants(&shared_tenants)
    );
    let other_tenants = ses.list_resource_tenants(&other_arn).await;
    assert_eq!(
        listed_tenants(&other_tenants),
        vec!["tenant-1".to_string(), "tenant-3".to_string()],
        "case {case:?}: expected the untouched resource to keep every tenant, got {:?}",
        listed_tenants(&other_tenants)
    );
    let first_tenant = ses.list_tenant_resources("tenant-1").await;
    assert_eq!(
        listed_resources(&first_tenant),
        vec![other_arn.clone()],
        "case {case:?}: expected the tenant to keep its other resource, got {:?}",
        listed_resources(&first_tenant)
    );
    let second_tenant = ses.list_tenant_resources("tenant-2").await;
    assert_eq!(
        listed_resources(&second_tenant),
        vec![shared_arn.clone()],
        "case {case:?}: expected the other tenant of the removed resource to keep it, got {:?}",
        listed_resources(&second_tenant)
    );
}

/// One refused removal: the request, and the error it must be answered with.
struct RefusalCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `TenantName` the refused call carries.
    tenant_name: String,
    /// The `ResourceArn` the refused call carries.
    resource_arn: String,
    /// The HTTP status of the refusal.
    status: u16,
    /// The modelled error shape the refusal names.
    error_code: &'static str,
    /// The sentence the refusal carries.
    message: String,
}

/// The name of the one tenant every refusal row is set up against.
const LIVE_TENANT: &str = "live-tenant";

/// The name of the one configuration set every refusal row is set up against.
const LIVE_SET: &str = "live-set";

/// The refusal table: a name that resolves to nothing on either side of the association,
/// and input outside the documented form.
///
/// The missing-resource sentence is the one this surface emits, recorded for the
/// configuration-set kind as `Configuration set <NAME> does not exist:`; the other kinds
/// a tenant may hold are worded the same way, with their own label. The malformed-input
/// sentences are the service's own, read from the crate so they are stated once.
fn refusal_cases() -> Vec<RefusalCase> {
    let live_set_arn = format!("arn:aws:ses:{REGION}:{ACCOUNT_ID}:configuration-set/{LIVE_SET}");
    let over_long_name = "t".repeat(MAX_TENANT_NAME_CHARS + 1);
    vec![
        RefusalCase {
            case: "a configuration set the account never created",
            tenant_name: LIVE_TENANT.to_string(),
            resource_arn: format!("arn:aws:ses:{REGION}:{ACCOUNT_ID}:configuration-set/ghost_set"),
            status: 404,
            error_code: "NotFoundException",
            message: "Configuration set <ghost_set> does not exist:".to_string(),
        },
        RefusalCase {
            case: "an email identity the account never created",
            tenant_name: LIVE_TENANT.to_string(),
            resource_arn: format!(
                "arn:aws:ses:{REGION}:{ACCOUNT_ID}:identity/nobody@example.com"
            ),
            status: 404,
            error_code: "NotFoundException",
            message: "Email identity <nobody@example.com> does not exist:".to_string(),
        },
        RefusalCase {
            case: "an email template, a kind the account holds none of",
            tenant_name: LIVE_TENANT.to_string(),
            resource_arn: format!("arn:aws:ses:{REGION}:{ACCOUNT_ID}:template/welcome"),
            status: 404,
            error_code: "NotFoundException",
            message: "Email template <welcome> does not exist:".to_string(),
        },
        RefusalCase {
            case: "a configuration set of this account in another Region",
            tenant_name: LIVE_TENANT.to_string(),
            resource_arn: format!("arn:aws:ses:eu-west-1:{ACCOUNT_ID}:configuration-set/{LIVE_SET}"),
            status: 404,
            error_code: "NotFoundException",
            message: format!("Configuration set <{LIVE_SET}> does not exist:"),
        },
        RefusalCase {
            case: "a configuration set of another account in this Region",
            tenant_name: LIVE_TENANT.to_string(),
            resource_arn: format!("arn:aws:ses:{REGION}:210987654321:configuration-set/{LIVE_SET}"),
            status: 404,
            error_code: "NotFoundException",
            message: format!("Configuration set <{LIVE_SET}> does not exist:"),
        },
        RefusalCase {
            case: "a tenant the account never created",
            tenant_name: "ghost-tenant".to_string(),
            resource_arn: live_set_arn.clone(),
            status: 404,
            error_code: "NotFoundException",
            message: "The requested tenant <ghost-tenant> does not exist.".to_string(),
        },
        RefusalCase {
            case: "a tenant name differing from a real one only in case",
            tenant_name: "Live-Tenant".to_string(),
            resource_arn: live_set_arn.clone(),
            status: 404,
            error_code: "NotFoundException",
            message: "The requested tenant <Live-Tenant> does not exist.".to_string(),
        },
        RefusalCase {
            case: "a tenant name one character past the documented bound",
            tenant_name: over_long_name.clone(),
            resource_arn: live_set_arn.clone(),
            status: 400,
            error_code: "BadRequestException",
            message: state::invalid_tenant_name(&over_long_name),
        },
        RefusalCase {
            case: "a tenant name carrying a character the documented form excludes",
            tenant_name: "live.tenant".to_string(),
            resource_arn: live_set_arn.clone(),
            status: 400,
            error_code: "BadRequestException",
            message: state::invalid_tenant_name("live.tenant"),
        },
        RefusalCase {
            case: "an SES resource of a kind no tenant may hold",
            tenant_name: LIVE_TENANT.to_string(),
            resource_arn: format!("arn:aws:ses:{REGION}:{ACCOUNT_ID}:contact-list/announcements"),
            status: 400,
            error_code: "BadRequestException",
            message: state::UNSUPPORTED_RESOURCE_ARN.to_string(),
        },
        RefusalCase {
            case: "another vendor's resource",
            tenant_name: LIVE_TENANT.to_string(),
            resource_arn: format!("arn:aws:s3:{REGION}:{ACCOUNT_ID}:configuration-set/{LIVE_SET}"),
            status: 400,
            error_code: "BadRequestException",
            message: state::UNSUPPORTED_RESOURCE_ARN.to_string(),
        },
        RefusalCase {
            case: "an SES ARN naming a kind but no resource within it",
            tenant_name: LIVE_TENANT.to_string(),
            resource_arn: format!("arn:aws:ses:{REGION}:{ACCOUNT_ID}:configuration-set/"),
            status: 400,
            error_code: "BadRequestException",
            message: state::UNSUPPORTED_RESOURCE_ARN.to_string(),
        },
        RefusalCase {
            case: "a value that is no ARN at all",
            tenant_name: LIVE_TENANT.to_string(),
            resource_arn: LIVE_SET.to_string(),
            status: 400,
            error_code: "BadRequestException",
            message: state::UNSUPPORTED_RESOURCE_ARN.to_string(),
        },
    ]
}

#[tokio::test]
async fn a_name_that_resolves_to_nothing_is_refused_and_removes_nothing() {
    for RefusalCase {
        case,
        tenant_name,
        resource_arn,
        status,
        error_code,
        message,
    } in refusal_cases()
    {
        let ses = Ses::new();
        // One real tenant holding one real resource, so each refusal can be shown to
        // leave the association table exactly as it was.
        let live_set = Resource::ConfigurationSet(LIVE_SET);
        let live_arn = live_set.arn();
        live_set.create(&ses, case).await;
        let created = ses.create_tenant(LIVE_TENANT).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected HTTP 200 creating the tenant, got {} {:?}",
            created.status,
            created.message()
        );
        let associated = ses.associate(LIVE_TENANT, &live_arn).await;
        assert_eq!(
            associated.status,
            200,
            "case {case:?}: expected HTTP 200 associating the live resource, got {} {:?}",
            associated.status,
            associated.message()
        );
        // The row's own ARN is associated too wherever the surface accepts it, so the
        // refusal is shown to be about resolution and not about a missing record.
        let row_association = ses.associate(&tenant_name, &resource_arn).await;
        let row_was_associated = row_association.status == 200;

        let refused = ses.disassociate(&tenant_name, &resource_arn).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            refused.status,
            refused.message()
        );
        assert_eq!(
            refused.error_code(),
            error_code,
            "case {case:?}: expected {error_code}, got {:?} {:?}",
            refused.error_code(),
            refused.message()
        );
        assert_eq!(
            refused.message(),
            message,
            "case {case:?}: expected message {message:?}, got {:?}",
            refused.message()
        );

        let live_tenants = ses.list_resource_tenants(&live_arn).await;
        assert_eq!(
            listed_tenants(&live_tenants),
            vec![LIVE_TENANT.to_string()],
            "case {case:?}: expected the refusal to leave the live association alone, got {:?}",
            listed_tenants(&live_tenants)
        );
        if row_was_associated {
            let row_tenants = ses.list_resource_tenants(&resource_arn).await;
            assert!(
                listed_tenants(&row_tenants).contains(&tenant_name),
                "case {case:?}: expected the refused call to remove nothing, got {:?}",
                listed_tenants(&row_tenants)
            );
        }
    }
}
