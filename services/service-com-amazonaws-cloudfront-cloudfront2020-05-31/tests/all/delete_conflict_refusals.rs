// SPDX-License-Identifier: CC0-1.0

//! How CloudFront words a delete it refuses because the resource is still referenced or has
//! not been disabled yet, and when that refusal is lifted — end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. One rule, asserted over every delete-conflict class the surface has:
//!
//! * the refusal carries the sentence the service serves for THAT error class and resource
//!   kind — its fixed text verbatim, with its variable parts (the resource the request
//!   named, counts read from live state) filled in — and never the identifier of the
//!   resource that holds the reference, which is no part of any of those sentences;
//! * the condition is read from LIVE state, so the same delete lands with 204 as soon as
//!   nothing references the resource any more — whether the reference was taken off by an
//!   update or the referencing resource was deleted outright.
//!
//! The rows differ along everything the rule generalises over: the error class and resource
//! kind (a function, a real-time log configuration, an origin access identity, an origin
//! access control, a connection group, a distribution tenant), HOW MANY distributions hold
//! the reference, WHERE the reference sits (the default cache behaviour or an ordered one,
//! the first or the last origin), and HOW the reference is released (an update that drops it
//! or a delete of the whole referencing distribution).
//!
//! The last suite below pins the refusal that keeps a distribution from holding both
//! origin-access mechanisms at once, because that is what decides whether the controls and
//! identities such a request names are still referenced afterwards.
//!
//! The harness (the router and the request helpers) is the one the sibling suites use,
//! reused rather than copied; every served sentence is spelled out here as a LITERAL rather
//! than read back off the crate's own constant, so a changed constant fails this suite.

use smithy_server_runtime::MultiServiceRouter;

use crate::connection_group_update::update_connection_group;
use crate::connection_groups::{create_connection_group, get_connection_group};
use crate::distribution_lifecycle::{
    call, create_distribution, delete_distribution, emulator, get_distribution, past_propagation,
    update_distribution, Answer, API_VERSION,
};
use crate::distribution_tenant_delete::{delete_tenant, get_tenant, set_enabled};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, escape, managed_certificate_request, multi_tenant_distribution,
};
use crate::function_delete::delete_function;
use crate::function_lifecycle::{create_key_value_store, create_table, describe_function, member};
use crate::function_publish::create_one;
use crate::origin_access_control_lifecycle::{create_control, get_control, table as control_table};
use crate::origin_access_identity_lifecycle::{create_identity, get_identity, Config as Identity};
use crate::realtime_log_config_lifecycle::{create_accepted, delete_config, Endpoint, Submitted};

// ---------------------------------------------------------------------------
// The sentences the service serves, as literals
// ---------------------------------------------------------------------------

/// The sentence a delete of a function a cache behaviour attaches is refused with: its fixed
/// text with the function the request named and the number of distributions that hold it.
fn function_in_use(name: &str, distributions: usize) -> String {
    format!("Cannot delete function {name}, it is in use by {distributions} distributions")
}

/// The sentence a delete of a referenced real-time log configuration is refused with.
const REALTIME_LOG_CONFIG_IN_USE: &str =
    "The CloudFront real-time log configuration is still being used.";

/// The sentence a delete of a referenced origin access identity is refused with.
const ORIGIN_ACCESS_IDENTITY_IN_USE: &str =
    "The CloudFront origin access identity is still being used.";

/// The sentence a delete of a referenced origin access control is refused with.
const ORIGIN_ACCESS_CONTROL_IN_USE: &str =
    "The CloudFront origin access control is still being used.";

/// The sentence a delete of an enabled connection group is refused with.
const CONNECTION_GROUP_NOT_DISABLED: &str =
    "You must disable the specified connection group before you can delete it.";

/// The sentence a delete of an enabled distribution tenant is refused with.
const DISTRIBUTION_TENANT_NOT_DISABLED: &str =
    "You must disable the distribution tenant before you can delete it.";

/// The sentence a configuration whose origin names both origin-access mechanisms is refused
/// with.
const ILLEGAL_ORIGIN_ACCESS_CONFIGURATION: &str =
    "Cannot use both Origin Access Control and Origin Access Identity on an origin";

// ---------------------------------------------------------------------------
// The configurations the references under test sit in
// ---------------------------------------------------------------------------

/// The reference form an Amazon S3 origin names an origin access identity by: "The format of
/// the value is: `origin-access-identity/cloudfront/ID-of-origin-access-identity`"
/// (`S3OriginConfig$OriginAccessIdentity`). Spelled out here rather than taken from the
/// crate, so a changed reference form fails this suite.
fn identity_reference(id: &str) -> String {
    format!("origin-access-identity/cloudfront/{id}")
}

/// One distribution configuration, described by the references it carries and where.
///
/// A single builder for every row below, because the rule under test is about the
/// reference rather than about the configuration around it: a row names the reference it
/// wants and the position it wants it in, and nothing else changes.
#[derive(Debug, Clone, Default)]
struct Holder {
    caller_reference: String,
    /// `Enabled` in the submitted configuration.
    enabled: bool,
    /// How many origins the configuration carries (at least one).
    origins: usize,
    /// `(origin index, OriginAccessIdentity)` — the S3 origin that names an identity.
    identity: Option<(usize, String)>,
    /// `(origin index, OriginAccessControlId)` — the origin that names a control.
    control: Option<(usize, String)>,
    /// The function ARN the DEFAULT cache behaviour attaches, if any.
    function: Option<String>,
    /// The function ARN an ORDERED cache behaviour attaches, if any.
    ordered_function: Option<String>,
    /// `(on an ordered behaviour?, RealtimeLogConfigArn)`.
    realtime_log: Option<(bool, String)>,
}

impl Holder {
    /// A configuration carrying `origins` origins and nothing else, enabled.
    fn new(caller_reference: &str, origins: usize) -> Self {
        Self {
            caller_reference: caller_reference.to_string(),
            enabled: true,
            origins: origins.max(1),
            ..Self::default()
        }
    }

    /// The same configuration with every reference under test taken off it — the update a
    /// caller is pointed at before the delete.
    fn released(&self) -> Self {
        Self {
            caller_reference: self.caller_reference.clone(),
            enabled: self.enabled,
            origins: self.origins,
            ..Self::default()
        }
    }

    /// The same configuration with `Enabled` as `enabled` states — the update that makes a
    /// distribution deletable.
    fn with_enabled(&self, enabled: bool) -> Self {
        Self {
            enabled,
            ..self.clone()
        }
    }

    /// The `DistributionConfig` document a client sends. Members follow the model's
    /// declaration order for every shape they sit in.
    fn xml(&self) -> String {
        let mut out = format!(
            "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <CallerReference>{}</CallerReference>\
             <Origins><Quantity>{}</Quantity><Items>",
            escape(&self.caller_reference),
            self.origins
        );
        for index in 0..self.origins {
            out.push_str(&format!(
                "<Origin><Id>origin-{index}</Id>\
                 <DomainName>origin-{index}.s3.us-east-1.amazonaws.com</DomainName>"
            ));
            if let Some((at, reference)) = &self.identity {
                if *at == index {
                    out.push_str(&format!(
                        "<S3OriginConfig><OriginAccessIdentity>{}</OriginAccessIdentity>\
                         </S3OriginConfig>",
                        escape(reference)
                    ));
                }
            }
            if let Some((at, control)) = &self.control {
                if *at == index {
                    out.push_str(&format!(
                        "<OriginAccessControlId>{}</OriginAccessControlId>",
                        escape(control)
                    ));
                }
            }
            out.push_str("</Origin>");
        }
        out.push_str(
            "</Items></Origins>\
             <DefaultCacheBehavior><TargetOriginId>origin-0</TargetOriginId>\
             <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy>",
        );
        if let Some(arn) = &self.function {
            out.push_str(&function_associations(arn));
        }
        if let Some((false, arn)) = &self.realtime_log {
            out.push_str(&format!(
                "<RealtimeLogConfigArn>{}</RealtimeLogConfigArn>",
                escape(arn)
            ));
        }
        out.push_str("</DefaultCacheBehavior>");
        let ordered_log = match &self.realtime_log {
            Some((true, arn)) => Some(arn.clone()),
            _ => None,
        };
        if self.ordered_function.is_some() || ordered_log.is_some() {
            out.push_str(
                "<CacheBehaviors><Quantity>1</Quantity><Items><CacheBehavior>\
                 <PathPattern>/images/*</PathPattern><TargetOriginId>origin-0</TargetOriginId>\
                 <ViewerProtocolPolicy>https-only</ViewerProtocolPolicy>",
            );
            if let Some(arn) = &self.ordered_function {
                out.push_str(&function_associations(arn));
            }
            if let Some(arn) = &ordered_log {
                out.push_str(&format!(
                    "<RealtimeLogConfigArn>{}</RealtimeLogConfigArn>",
                    escape(arn)
                ));
            }
            out.push_str("</CacheBehavior></Items></CacheBehaviors>");
        }
        out.push_str(&format!(
            "<Comment>a distribution under the delete-conflict rules</Comment>\
             <PriceClass>PriceClass_All</PriceClass>\
             <Enabled>{}</Enabled></DistributionConfig>",
            self.enabled
        ));
        out
    }
}

/// The `FunctionAssociations` wrapper attaching `arn` to one cache behaviour. `EventType`
/// `viewer-request` is one of the values the modelled `EventType` enum admits
/// (`smithy_query(query="shape", shape="EventType")`).
fn function_associations(arn: &str) -> String {
    format!(
        "<FunctionAssociations><Quantity>1</Quantity><Items><FunctionAssociation>\
         <FunctionARN>{}</FunctionARN><EventType>viewer-request</EventType>\
         </FunctionAssociation></Items></FunctionAssociations>",
        escape(arn)
    )
}

/// Create the distribution `holder` describes and report `(Id, ETag)`.
async fn store(router: &MultiServiceRouter, case: &str, holder: &Holder) -> (String, String) {
    let created = create_distribution(router, &holder.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from CreateDistribution, got {} with {}",
        created.status, created.raw
    );
    (
        created.root().text_at("Id").unwrap_or_default().to_string(),
        created.header("etag").to_string(),
    )
}

/// Replace a stored distribution's configuration with `holder`'s and report the version the
/// update minted.
async fn replace(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    etag: &str,
    holder: &Holder,
) -> String {
    let updated = update_distribution(router, id, &holder.xml(), Some(etag)).await;
    assert_eq!(
        updated.status, 200,
        "case {case}: expected HTTP 200 from UpdateDistribution, got {} with {}",
        updated.status, updated.raw
    );
    updated.header("etag").to_string()
}

/// Remove a stored distribution altogether: disable it, let the disabled configuration
/// reach the edge locations, then delete it. The distribution is gone when this returns.
async fn remove(router: &MultiServiceRouter, case: &str, id: &str, etag: &str, holder: &Holder) {
    let disabled = replace(router, case, id, etag, &holder.with_enabled(false)).await;
    tokio::time::sleep(past_propagation()).await;
    let deleted = delete_distribution(router, id, Some(&disabled)).await;
    assert_eq!(
        deleted.status, 204,
        "case {case}: expected HTTP 204 deleting the referencing distribution, got {} with {}",
        deleted.status, deleted.raw
    );
    assert_eq!(
        get_distribution(router, id).await.status,
        404,
        "case {case}: expected the referencing distribution to be gone"
    );
}

/// The `Message` a refusal carries.
fn message(answer: &Answer) -> String {
    answer
        .root()
        .find_text("Message")
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// A function a cache behaviour attaches: the name and the count are the message
// ---------------------------------------------------------------------------

/// One row of the function table: the function's name, and how the distributions of the
/// account attach it.
struct Attachment {
    case: &'static str,
    /// The function's `Name`, which the refusal reports.
    name: &'static str,
    /// One entry per distribution that attaches the function: `true` attaches it to the
    /// DEFAULT cache behaviour, `false` to an ordered one.
    holders: Vec<bool>,
    /// Whether the first holder attaches the function TWICE — on its default behaviour and
    /// on an ordered one — which is still one distribution to update.
    twice: bool,
}

/// `FunctionInUse` reports the function the request named and how many distributions hold
/// it, both read where they are known: the name from the request, the count from the stored
/// distributions. The count follows live state as holders are added and released, and the
/// delete lands once no distribution attaches the function.
#[tokio::test]
async fn a_function_in_use_refusal_names_the_function_and_counts_the_distributions() {
    let rows = vec![
        Attachment {
            case: "one distribution, the default cache behaviour",
            name: "conflict-fn-one",
            holders: vec![true],
            twice: false,
        },
        Attachment {
            case: "one distribution, an ordered cache behaviour",
            name: "conflict_fn_ordered",
            holders: vec![false],
            twice: false,
        },
        Attachment {
            case: "two distributions",
            name: "conflict-fn-two",
            holders: vec![true, false],
            twice: false,
        },
        Attachment {
            case: "three distributions",
            name: "a-much-longer-function-name-for-the-three-holder-case",
            holders: vec![true, true, false],
            twice: false,
        },
        Attachment {
            case: "one distribution attaching it on two behaviours",
            name: "conflict-fn-twice",
            holders: vec![true],
            twice: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = emulator();
        let kvs_arn = create_key_value_store(&router, "delete_conflict_store").await;
        let mut create = create_table(&kvs_arn)
            .first()
            .cloned()
            .expect("the create table carries at least one row");
        create.name = row.name.to_string();
        let function_etag = create_one(&router, &create).await;
        let arn = member(
            describe_function(&router, &create.name, None).await.root(),
            "FunctionMetadata/FunctionARN",
            case,
            "the describe of the created function",
        );

        // Every distribution of the row, each attaching the function where the row says.
        let mut holders: Vec<(String, String, Holder)> = Vec::new();
        for (index, on_default) in row.holders.iter().enumerate() {
            let mut holder = Holder::new(&format!("conflict-{}-{index}", row.name), 1);
            if *on_default || (index == 0 && row.twice) {
                holder.function = Some(arn.clone());
            }
            if !*on_default || (index == 0 && row.twice) {
                holder.ordered_function = Some(arn.clone());
            }
            let (id, etag) = store(&router, case, &holder).await;
            holders.push((id, etag, holder));
        }

        // The refusal names the function the request addressed and counts the distributions
        // that hold it — one entry per distribution, however many of its behaviours attach
        // the function.
        let mut outstanding = holders.len();
        let refused = delete_function(&router, &create.name, &function_etag).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 deleting the attached function, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "FunctionInUse",
            "case {case}: expected FunctionInUse, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        let actual = message(&refused);
        assert_eq!(
            actual,
            function_in_use(&create.name, outstanding),
            "case {case}: expected the served FunctionInUse sentence, got {actual:?}"
        );
        for (id, _, _) in &holders {
            assert!(
                !actual.contains(id),
                "case {case}: expected the refusal to name no distribution, got {actual:?}"
            );
        }

        // Releasing the holders one at a time: the count the next refusal reports is the one
        // live state holds, and the delete lands once the last holder has let go.
        for (index, (id, etag, holder)) in holders.iter().enumerate() {
            if index % 2 == 0 {
                replace(&router, case, id, etag, &holder.released()).await;
            } else {
                remove(&router, case, id, etag, holder).await;
            }
            outstanding -= 1;
            let answer = delete_function(&router, &create.name, &function_etag).await;
            if outstanding > 0 {
                let actual = message(&answer);
                assert_eq!(
                    actual,
                    function_in_use(&create.name, outstanding),
                    "case {case}: expected the refusal to count {outstanding} holders, got \
                     {actual:?}"
                );
            } else {
                assert_eq!(
                    answer.status, 204,
                    "case {case}: expected HTTP 204 once nothing attaches the function, got {} \
                     with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    describe_function(&router, &create.name, None).await.status,
                    404,
                    "case {case}: expected the function to be gone after the delete"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A resource a distribution still references: the kind is the whole message
// ---------------------------------------------------------------------------

/// Which resource kind a still-in-use row is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    RealtimeLogConfig,
    OriginAccessIdentity,
    OriginAccessControl,
}

/// Delete the resource of `kind` that `identifier` addresses, over the wire path its own
/// operation binds — carrying `if_match` where that operation takes a precondition.
async fn delete_addressed(
    router: &MultiServiceRouter,
    kind: Kind,
    identifier: &str,
    if_match: Option<&str>,
) -> Answer {
    match kind {
        Kind::RealtimeLogConfig => delete_config(router, Some(identifier), None).await,
        Kind::OriginAccessIdentity => {
            call(
                router,
                "DELETE",
                &format!("/{API_VERSION}/origin-access-identity/cloudfront/{identifier}"),
                None,
                if_match,
            )
            .await
        }
        Kind::OriginAccessControl => {
            call(
                router,
                "DELETE",
                &format!("/{API_VERSION}/origin-access-control/{identifier}"),
                None,
                if_match,
            )
            .await
        }
    }
}

/// One still-in-use row: a resource kind, where the reference to it sits, and how the
/// reference is released afterwards.
struct Reference {
    case: &'static str,
    kind: Kind,
    /// How many origins the referencing distribution carries.
    origins: usize,
    /// Which origin names the resource (for the origin-access kinds), or which behaviour
    /// names it (`0` the default one, `1` an ordered one) for a real-time log configuration.
    at: usize,
    /// Whether the reference is released by DELETING the referencing distribution rather
    /// than by an update that drops it.
    release_by_delete: bool,
}

/// A still-in-use refusal states the resource kind and nothing else: no identifier of the
/// resource addressed, and none of the distribution that holds the reference. The condition
/// is read from live state, so the same delete lands with 204 once the reference is gone —
/// taken off by an update, or removed with the whole distribution that held it.
#[tokio::test]
async fn a_still_in_use_refusal_names_the_resource_kind_and_no_identifier() {
    let rows = vec![
        Reference {
            case: "a real-time log configuration on the default cache behaviour",
            kind: Kind::RealtimeLogConfig,
            origins: 1,
            at: 0,
            release_by_delete: false,
        },
        Reference {
            case: "a real-time log configuration on an ordered cache behaviour, released by \
                   deleting the distribution",
            kind: Kind::RealtimeLogConfig,
            origins: 1,
            at: 1,
            release_by_delete: true,
        },
        Reference {
            case: "an origin access identity on the only origin",
            kind: Kind::OriginAccessIdentity,
            origins: 1,
            at: 0,
            release_by_delete: false,
        },
        Reference {
            case: "an origin access identity on the last of three origins, released by \
                   deleting the distribution",
            kind: Kind::OriginAccessIdentity,
            origins: 3,
            at: 2,
            release_by_delete: true,
        },
        Reference {
            case: "an origin access control on the only origin, released by deleting the \
                   distribution",
            kind: Kind::OriginAccessControl,
            origins: 1,
            at: 0,
            release_by_delete: true,
        },
        Reference {
            case: "an origin access control on the first of three origins",
            kind: Kind::OriginAccessControl,
            origins: 3,
            at: 0,
            release_by_delete: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = emulator();

        // The resource under test, and the configuration that references it.
        let mut holder = Holder::new(&format!("conflict-ref-{}", case.len()), row.origins);
        let (identifier, if_match, expected) = match row.kind {
            Kind::RealtimeLogConfig => {
                let submitted = Submitted {
                    case: "the configuration under the reference gate",
                    name: "conflict-realtime-log".to_string(),
                    sampling_rate: 50,
                    end_points: vec![Endpoint::kinesis("conflict")],
                    fields: vec!["timestamp".to_string(), "c-ip".to_string()],
                };
                let created = create_accepted(&router, &submitted).await;
                let arn = created
                    .root()
                    .find_text("ARN")
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !arn.is_empty(),
                    "case {case}: the create answered no ARN in {}",
                    created.raw
                );
                holder.realtime_log = Some((row.at > 0, arn.clone()));
                (
                    submitted.name.clone(),
                    None,
                    (400, "RealtimeLogConfigInUse", REALTIME_LOG_CONFIG_IN_USE),
                )
            }
            Kind::OriginAccessIdentity => {
                let config = Identity {
                    case: "the identity under the reference gate",
                    caller_reference: "conflict-identity".to_string(),
                    comment: "the identity an origin names".to_string(),
                };
                let created = create_identity(&router, &config.xml()).await;
                assert_eq!(
                    created.status, 201,
                    "case {case}: expected HTTP 201 creating the identity, got {} with {}",
                    created.status, created.raw
                );
                let id = created.root().find_text("Id").unwrap_or_default().to_string();
                holder.identity = Some((row.at, identity_reference(&id)));
                (
                    id,
                    Some(created.header("etag").to_string()),
                    (
                        409,
                        "CloudFrontOriginAccessIdentityInUse",
                        ORIGIN_ACCESS_IDENTITY_IN_USE,
                    ),
                )
            }
            Kind::OriginAccessControl => {
                let control = control_table()
                    .first()
                    .cloned()
                    .expect("the control table carries at least one row")
                    .with_name("the control under the reference gate", "conflict-control");
                let created = create_control(&router, &control.xml()).await;
                assert_eq!(
                    created.status, 201,
                    "case {case}: expected HTTP 201 creating the control, got {} with {}",
                    created.status, created.raw
                );
                let id = created.root().find_text("Id").unwrap_or_default().to_string();
                holder.control = Some((row.at, id.clone()));
                (
                    id,
                    Some(created.header("etag").to_string()),
                    (409, "OriginAccessControlInUse", ORIGIN_ACCESS_CONTROL_IN_USE),
                )
            }
        };
        let (status, code, sentence) = expected;
        let (distribution_id, distribution_etag) = store(&router, case, &holder).await;

        let refused = delete_addressed(&router, row.kind, &identifier, if_match.as_deref()).await;
        assert_eq!(
            refused.status, status,
            "case {case}: expected HTTP {status} while the reference stands, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case}: expected {code}, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        let actual = message(&refused);
        assert_eq!(
            actual, sentence,
            "case {case}: expected the sentence CloudFront serves for this class, got {actual:?}"
        );
        assert!(
            !actual.contains(&distribution_id),
            "case {case}: expected the refusal to name no distribution, got {actual:?}"
        );
        assert!(
            !actual.contains(&identifier),
            "case {case}: expected the refusal to carry nothing from the request, got {actual:?}"
        );

        // Releasing the reference is what lifts the refusal — and nothing else about the
        // store changed.
        if row.release_by_delete {
            remove(&router, case, &distribution_id, &distribution_etag, &holder).await;
        } else {
            replace(
                &router,
                case,
                &distribution_id,
                &distribution_etag,
                &holder.released(),
            )
            .await;
        }

        let deleted = delete_addressed(&router, row.kind, &identifier, if_match.as_deref()).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 once nothing references the resource, got {} with {}",
            deleted.status, deleted.raw
        );
        let gone = match row.kind {
            Kind::RealtimeLogConfig => delete_config(&router, Some(&identifier), None).await,
            Kind::OriginAccessIdentity => get_identity(&router, &identifier).await,
            Kind::OriginAccessControl => get_control(&router, &identifier).await,
        };
        assert_eq!(
            gone.status, 404,
            "case {case}: expected the resource to be gone after the delete, got {} with {}",
            gone.status, gone.raw
        );
    }
}

// ---------------------------------------------------------------------------
// A resource that has not been disabled yet: the message names the kind
// ---------------------------------------------------------------------------

/// `ResourceNotDisabled` is shared by several resource kinds, and the sentence each delete
/// serves names the kind the caller has to disable — so the two kinds that model it are
/// refused in their OWN words, and disabling the resource is what lifts the refusal.
#[tokio::test]
async fn a_delete_of_an_enabled_resource_names_the_kind_that_must_be_disabled() {
    assert_ne!(
        CONNECTION_GROUP_NOT_DISABLED, DISTRIBUTION_TENANT_NOT_DISABLED,
        "case the sentences: each resource kind is refused in its own words"
    );

    // The connection group arm: a group is created enabled unless the request says
    // otherwise, so the delete below is refused until an update turns it off.
    let router = emulator();
    let created = create_connection_group(
        &router,
        &format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>conflict-connection-group</Name></CreateConnectionGroupRequest>"
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case a connection group: expected HTTP 201 from the create, got {} with {}",
        created.status, created.raw
    );
    let group_id = created.root().find_text("Id").unwrap_or_default().to_string();
    let group_etag = created.header("etag").to_string();

    let refused = call(
        &router,
        "DELETE",
        &format!("/{API_VERSION}/connection-group/{group_id}"),
        None,
        Some(&group_etag),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case a connection group: expected HTTP 409 while enabled, got {} with {}",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "ResourceNotDisabled",
        "case a connection group: expected ResourceNotDisabled, got {:?} with {}",
        refused.error_code(),
        refused.raw
    );
    let actual = message(&refused);
    assert_eq!(
        actual, CONNECTION_GROUP_NOT_DISABLED,
        "case a connection group: expected the sentence CloudFront serves for this kind, got \
         {actual:?}"
    );
    assert_eq!(
        get_connection_group(&router, &group_id).await.status,
        200,
        "case a connection group: expected the refused delete to leave the group readable"
    );

    let disabled = update_connection_group(
        &router,
        &group_id,
        &group_etag,
        None,
        Some(false),
        None,
    )
    .await;
    assert_eq!(
        disabled.status, 200,
        "case a connection group: expected HTTP 200 disabling the group, got {} with {}",
        disabled.status, disabled.raw
    );
    let deleted = call(
        &router,
        "DELETE",
        &format!("/{API_VERSION}/connection-group/{group_id}"),
        None,
        Some(disabled.header("etag")),
    )
    .await;
    assert_eq!(
        deleted.status, 204,
        "case a connection group: expected HTTP 204 once disabled, got {} with {}",
        deleted.status, deleted.raw
    );

    // The distribution tenant arm: `Enabled` left out of the create enables the tenant, so
    // the same two steps apply to it under its own sentence.
    let distribution_id = multi_tenant_distribution(&router, "conflict-tenant-host").await;
    let created = create_distribution_tenant(
        &router,
        &format!(
            "<CreateDistributionTenantRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <DistributionId>{}</DistributionId><Name>conflict-tenant</Name>\
             <Domains><member><Domain>conflict-tenant.example.com</Domain></member></Domains>\
             <Parameters><member><Name>tenantName</Name><Value>conflict</Value></member>\
             </Parameters>{}</CreateDistributionTenantRequest>",
            escape(&distribution_id),
            managed_certificate_request()
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case a distribution tenant: expected HTTP 201 from the create, got {} with {}",
        created.status, created.raw
    );
    let tenant_id = created.root().find_text("Id").unwrap_or_default().to_string();
    let tenant_etag = created.header("etag").to_string();

    let refused = delete_tenant(&router, &tenant_id, Some(&tenant_etag)).await;
    assert_eq!(
        refused.status, 409,
        "case a distribution tenant: expected HTTP 409 while enabled, got {} with {}",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "ResourceNotDisabled",
        "case a distribution tenant: expected ResourceNotDisabled, got {:?} with {}",
        refused.error_code(),
        refused.raw
    );
    let actual = message(&refused);
    assert_eq!(
        actual, DISTRIBUTION_TENANT_NOT_DISABLED,
        "case a distribution tenant: expected the sentence CloudFront serves for this kind, got \
         {actual:?}"
    );
    assert_eq!(
        get_tenant(&router, &tenant_id).await.status,
        200,
        "case a distribution tenant: expected the refused delete to leave the tenant readable"
    );

    let disabled = set_enabled(&router, &tenant_id, false, &tenant_etag).await;
    assert_eq!(
        disabled.status, 200,
        "case a distribution tenant: expected HTTP 200 disabling the tenant, got {} with {}",
        disabled.status, disabled.raw
    );
    let deleted = delete_tenant(&router, &tenant_id, Some(disabled.header("etag"))).await;
    assert_eq!(
        deleted.status, 204,
        "case a distribution tenant: expected HTTP 204 once disabled, got {} with {}",
        deleted.status, deleted.raw
    );
}

// ---------------------------------------------------------------------------
// An origin cannot hold both origin-access mechanisms at once
// ---------------------------------------------------------------------------

/// One row: which origin of the configuration carries which mechanism, and whether the
/// configuration is one CloudFront stores.
struct Mechanisms {
    case: &'static str,
    origins: usize,
    /// The origin that names the origin access control, if any.
    control_at: Option<usize>,
    /// The origin that names the origin access identity, if any — `Some(index)` with an
    /// EMPTY reference stands for the documented "empty `OriginAccessIdentity` element".
    identity_at: Option<usize>,
    /// Whether the identity reference is the empty element rather than a real one.
    empty_identity: bool,
    /// Whether CloudFront stores this configuration.
    stored: bool,
}

/// "An origin cannot contain both an origin access control (OAC) and an origin access
/// identity (OAI)": a configuration whose origin names both is refused on the way in — by
/// the create and by the update alike — so nothing is stored that holds either reference,
/// and both the control and the identity the refused request named stay deletable.
#[tokio::test]
async fn an_origin_naming_both_access_mechanisms_is_refused_and_stores_nothing() {
    let rows = vec![
        Mechanisms {
            case: "the only origin names both",
            origins: 1,
            control_at: Some(0),
            identity_at: Some(0),
            empty_identity: false,
            stored: false,
        },
        Mechanisms {
            case: "the last of three origins names both",
            origins: 3,
            control_at: Some(2),
            identity_at: Some(2),
            empty_identity: false,
            stored: false,
        },
        Mechanisms {
            case: "one origin names the control and another names the identity",
            origins: 3,
            control_at: Some(0),
            identity_at: Some(2),
            empty_identity: false,
            stored: true,
        },
        Mechanisms {
            case: "the only origin names the control alone",
            origins: 1,
            control_at: Some(0),
            identity_at: None,
            empty_identity: false,
            stored: true,
        },
        Mechanisms {
            case: "the only origin names the identity alone",
            origins: 1,
            control_at: None,
            identity_at: Some(0),
            empty_identity: false,
            stored: true,
        },
        Mechanisms {
            case: "the only origin names the control and an empty identity element",
            origins: 1,
            control_at: Some(0),
            identity_at: Some(0),
            empty_identity: true,
            stored: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = emulator();

        let control = control_table()
            .first()
            .cloned()
            .expect("the control table carries at least one row")
            .with_name("the control the row names", "both-mechanisms-control");
        let created_control = create_control(&router, &control.xml()).await;
        assert_eq!(
            created_control.status, 201,
            "case {case}: expected HTTP 201 creating the control, got {} with {}",
            created_control.status, created_control.raw
        );
        let control_id = created_control
            .root()
            .find_text("Id")
            .unwrap_or_default()
            .to_string();
        let control_etag = created_control.header("etag").to_string();

        let identity_config = Identity {
            case: "the identity the row names",
            caller_reference: "both-mechanisms-identity".to_string(),
            comment: "the identity an origin names".to_string(),
        };
        let created_identity = create_identity(&router, &identity_config.xml()).await;
        assert_eq!(
            created_identity.status, 201,
            "case {case}: expected HTTP 201 creating the identity, got {} with {}",
            created_identity.status, created_identity.raw
        );
        let identity_id = created_identity
            .root()
            .find_text("Id")
            .unwrap_or_default()
            .to_string();
        let identity_etag = created_identity.header("etag").to_string();

        let mut holder = Holder::new("both-mechanisms", row.origins);
        if let Some(at) = row.control_at {
            holder.control = Some((at, control_id.clone()));
        }
        if let Some(at) = row.identity_at {
            holder.identity = Some((
                at,
                if row.empty_identity {
                    String::new()
                } else {
                    identity_reference(&identity_id)
                },
            ));
        }

        // The create, and then the update of a stored distribution onto the same
        // configuration: one rule, applied on every path that stores a configuration.
        let created = create_distribution(&router, &holder.xml()).await;
        if row.stored {
            assert_eq!(
                created.status, 201,
                "case {case}: expected the configuration to be stored, got {} with {}",
                created.status, created.raw
            );
            // Taking the distribution away again leaves both resources unreferenced.
            let id = created.root().text_at("Id").unwrap_or_default().to_string();
            remove(&router, case, &id, created.header("etag"), &holder).await;
        } else {
            assert_eq!(
                created.status, 400,
                "case {case}: expected HTTP 400 for an origin naming both mechanisms, got {} \
                 with {}",
                created.status, created.raw
            );
            assert_eq!(
                created.error_code(),
                "IllegalOriginAccessConfiguration",
                "case {case}: expected IllegalOriginAccessConfiguration, got {:?} with {}",
                created.error_code(),
                created.raw
            );
            let actual = message(&created);
            assert_eq!(
                actual, ILLEGAL_ORIGIN_ACCESS_CONFIGURATION,
                "case {case}: expected the sentence CloudFront serves for this class, got \
                 {actual:?}"
            );

            // The refused create stored nothing, so the same configuration is refused by an
            // update of a distribution that holds neither mechanism, and that distribution
            // keeps the configuration it had.
            let plain = Holder::new("both-mechanisms-plain", row.origins);
            let (id, etag) = store(&router, case, &plain).await;
            let refused = update_distribution(&router, &id, &holder.xml(), Some(&etag)).await;
            assert_eq!(
                refused.status, 400,
                "case {case}: expected the update to be refused with HTTP 400, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "IllegalOriginAccessConfiguration",
                "case {case}: expected the update to answer IllegalOriginAccessConfiguration, \
                 got {:?} with {}",
                refused.error_code(),
                refused.raw
            );
            assert_eq!(
                get_distribution(&router, &id).await.header("etag"),
                etag,
                "case {case}: expected the refused update to spend no version token"
            );
            remove(&router, case, &id, &etag, &plain).await;
        }

        // Nothing stored holds either reference now, so each resource is deletable — a
        // refused create leaves no reference behind, and a deleted distribution takes the
        // references it held with it.
        let identity_gone = call(
            &router,
            "DELETE",
            &format!("/{API_VERSION}/origin-access-identity/cloudfront/{identity_id}"),
            None,
            Some(&identity_etag),
        )
        .await;
        assert_eq!(
            identity_gone.status, 204,
            "case {case}: expected HTTP 204 deleting the unreferenced identity, got {} with {}",
            identity_gone.status, identity_gone.raw
        );
        let control_gone = call(
            &router,
            "DELETE",
            &format!("/{API_VERSION}/origin-access-control/{control_id}"),
            None,
            Some(&control_etag),
        )
        .await;
        assert_eq!(
            control_gone.status, 204,
            "case {case}: expected HTTP 204 deleting the unreferenced control, got {} with {}",
            control_gone.status, control_gone.raw
        );
    }
}
