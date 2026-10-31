// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the two WIRE-FIDELITY rules this fix introduces, each
//! asserted over a SET of inputs rather than one case:
//!
//! 1. A minted policy-store-alias ARN names the region the request was signed
//!    for in `ResourceArn`'s third colon-separated field, and the ARN a create
//!    PERSISTED is what every later read serves.
//! 2. `ResourceNotFoundException` reports, in `resourceType`, the KIND of
//!    resource the unresolved reference named, and in `resourceId` that
//!    resource's own id — the reference with its `<resource-path>/` prefix
//!    removed. The row set is every identifier member that accepts such a path.
//!
//! The third rule this change carries — a `@timestampFormat("date-time")` member
//! is an RFC 3339 string on the wire — is pinned by
//! `tests/all/date_time_wire_format.rs`, which owns it for every member of every
//! shape, so it is not repeated here.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! emulator binary serves, so a value is only visible to a later request if it
//! really reached state.

use serde_json::{json, Value};

// The wire harness is shared with every other suite in this crate, including the
// region-varying caller one of this suite's rules needs.
use super::harness::{
    call_in, error_code, ok_in, router, store_with_validation_off_in, user_pool_arn,
};

/// The prefix every policy store alias name carries, from
/// `CreatePolicyStoreAliasInput$aliasName`'s model documentation: "The alias name
/// must always be prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The resource-path prefix a `policyId` member carries when it names a policy by
/// name instead of by id.
const POLICY_NAME_PREFIX: &str = "name/";

/// `DEFAULT_REGION` in `src/state.rs`: the region a minted ARN names when the
/// request carries no AWS-shaped SigV4 credential scope.
const DEFAULT_REGION: &str = "us-east-1";

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// A Cedar policy that parses, for the rows that need a real policy.
const STATEMENT: &str = "permit(principal == User::\"alice\", action == Action::\"view\", \
                         resource == Photo::\"vacation\");";

// The signing region is the only thing that varies between rows of the ARN table
// below, which is what makes that table a test of the rule rather than of one
// recorded region. The region-varying caller is the shared harness's
// [`super::harness::call_in`] — an empty region there sends no `authorization`
// header at all — so this suite shapes no request of its own.

// ---------------------------------------------------------------------------
// 1. A minted alias ARN names the signing region
// ---------------------------------------------------------------------------

/// A policy-store-alias ARN carries the region the request was signed for, and
/// the stored ARN is what a later read serves whatever region that read is signed
/// for.
///
/// The rows differ along the SIGNING REGION: each AWS partition's shape
/// (`us-`/`eu-`/`ap-`/`cn-`/`us-gov-`), a three-word region, and the two cases
/// that name no usable region — an unsigned request and a scope whose region is
/// not AWS-shaped — which fall back to `DEFAULT_REGION`. Regions are the public
/// AWS region-code form `<partition>-<area>[-<area>]-<number>`.
#[tokio::test]
async fn an_alias_arn_names_the_region_the_request_was_signed_for() {
    // (case, the region in the credential scope, the region the ARN must name)
    let rows: &[(&str, &str, &str)] = &[
        ("us-east-1", "us-east-1", "us-east-1"),
        ("us-west-2", "us-west-2", "us-west-2"),
        ("eu-central-1", "eu-central-1", "eu-central-1"),
        ("three-word region", "ap-southeast-3", "ap-southeast-3"),
        // The PARTITION field stays `aws` for every row, because
        // `engine::emulator_arn` fixes it (`vendor/` is read-only here) — real
        // AWS would say `aws-cn` / `aws-us-gov` for these two. What these rows
        // assert is the REGION field, which is what this fix owns.
        ("china partition", "cn-north-1", "cn-north-1"),
        ("govcloud partition", "us-gov-west-1", "us-gov-west-1"),
        ("unsigned request", "", DEFAULT_REGION),
        ("scope region not AWS-shaped", "not a region", DEFAULT_REGION),
    ];

    for (case, signed_for, expected_region) in rows {
        let router = router();
        let store_id = store_with_validation_off_in(&router, signed_for).await;
        let alias_name = format!("{ALIAS_PREFIX}regional");
        let expected_arn =
            format!("arn:aws:verifiedpermissions:{expected_region}:{ACCOUNT_ID}:{alias_name}");

        let created = ok_in(
            &router,
            signed_for,
            "CreatePolicyStoreAlias",
            &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
        )
        .await;
        let arn = created["aliasArn"].as_str().unwrap_or_default();
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: expected the alias ARN to name {expected_region}, got {created}"
        );
        // `ResourceArn`'s pattern is `^arn:[^:]*:[^:]*:[^:]*:[^:]*:.*$`, whose
        // third colon-separated field after `arn` is the region: it must not be
        // the empty segment a region-less ARN leaves there.
        let fields: Vec<&str> = arn.splitn(6, ':').collect();
        assert_eq!(
            fields.len(),
            6,
            "case {case:?}: the ARN must have the six fields ResourceArn models, got {arn:?}"
        );
        assert_eq!(
            fields[3], *expected_region,
            "case {case:?}: ResourceArn field 3 is the region, got {arn:?}"
        );
        assert_eq!(
            fields[4], ACCOUNT_ID,
            "case {case:?}: ResourceArn field 4 is the account, got {arn:?}"
        );

        // Reading the alias back serves the PERSISTED ARN, so the ARN is not
        // rebuilt per reader. The readers here are the request forms that address
        // the alias's OWN region — the scope it was created under, and that
        // region named explicitly, which are the same region whenever the create
        // fell back to `DEFAULT_REGION`.
        for reader_region in ["", signed_for, expected_region] {
            // An unsigned read addresses `DEFAULT_REGION`, so it reaches this
            // alias only when that is the region the alias was created in; the
            // other two forms both address the alias's own region.
            if reader_region.is_empty() && *expected_region != DEFAULT_REGION {
                continue;
            }
            let read = ok_in(
                &router,
                reader_region,
                "GetPolicyStoreAlias",
                &json!({ "aliasName": alias_name }),
            )
            .await;
            assert_eq!(
                read["aliasArn"].as_str().unwrap_or_default(),
                expected_arn,
                "case {case:?} read in {reader_region:?}: the stored ARN must be served, \
                 got {read}"
            );
        }

        // An alias name is unique within an account AND Region, so the alias
        // belongs to the region it was created in: a read addressed to any OTHER
        // region holds no alias of that name. See
        // `tests/all/alias_region_scope.rs` for the whole rule.
        let foreign_region = if *expected_region == "eu-west-1" {
            "ap-northeast-1"
        } else {
            "eu-west-1"
        };
        let (status, body) = call_in(
            &router,
            foreign_region,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?} read in {foreign_region:?}: an alias of another region must not \
             resolve, got {status} {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. A not-found error names the resource the reference named
// ---------------------------------------------------------------------------

/// Which resource an unresolved identifier reference is reported as missing: the
/// kind the reference's resource path named, and that resource's own id.
///
/// The rows differ along the IDENTIFIER MEMBER and the FORM the reference takes:
/// a `policyStoreId` as a bare store id and as `policy-store-alias/<name>`, an
/// `aliasName` carrying its documented prefix, and a `policyId` as a bare
/// id and as `name/<policyName>` — across every implemented operation that takes
/// one. The alias rows also cover a reference whose alias EXISTS but is
/// `PendingDeletion`, which the model says must fail as not-found.
///
/// An `aliasName` WITHOUT that prefix is not a row here: it names no resource to
/// report as missing, so it is refused as a malformed member instead — see
/// `tests/all/alias_name_prefix.rs`.
#[tokio::test]
async fn a_not_found_error_names_the_referenced_resource_and_its_bare_id() {
    let router = router();
    let region = DEFAULT_REGION;
    let store_id = store_with_validation_off_in(&router, region).await;

    // An alias that exists but pends deletion: a reference to it must fail, and
    // the failure still names the alias by its bare name.
    let pending_suffix = "pending/deletion";
    let pending_alias = format!("{ALIAS_PREFIX}{pending_suffix}");
    ok_in(
        &router,
        region,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": pending_alias, "policyStoreId": store_id }),
    )
    .await;
    ok_in(
        &router,
        region,
        "DeletePolicyStoreAlias",
        &json!({ "aliasName": pending_alias }),
    )
    .await;

    let absent_alias = format!("{ALIAS_PREFIX}never-created");
    let authorize = |reference: &str| {
        json!({
            "policyStoreId": reference,
            "principal": { "entityType": "User", "entityId": "alice" },
            "action": { "actionType": "Action", "actionId": "view" },
            "resource": { "entityType": "Photo", "entityId": "vacation" },
        })
    };
    let create_policy = |reference: &str| {
        json!({
            "policyStoreId": reference,
            "definition": { "static": { "statement": STATEMENT } },
        })
    };
    let put_schema = |reference: &str| {
        json!({
            "policyStoreId": reference,
            "definition": { "cedarJson": "{}" },
        })
    };

    // (case, operation, request, expected resourceType, expected resourceId)
    let rows: &[(&str, &str, Value, &str, String)] = &[
        // A `policyStoreId` naming an alias reports the ALIAS, by its bare name.
        (
            "GetPolicyStore, absent alias reference",
            "GetPolicyStore",
            json!({ "policyStoreId": absent_alias }),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "GetPolicyStore, alias pending deletion",
            "GetPolicyStore",
            json!({ "policyStoreId": pending_alias }),
            "POLICY_STORE_ALIAS",
            pending_suffix.to_string(),
        ),
        // The same member naming a store id reports the STORE, id unchanged.
        (
            "GetPolicyStore, absent store id",
            "GetPolicyStore",
            json!({ "policyStoreId": "PSabsent" }),
            "POLICY_STORE",
            "PSabsent".to_string(),
        ),
        (
            "GetPolicy, absent alias reference",
            "GetPolicy",
            json!({ "policyStoreId": absent_alias, "policyId": "SPabsent" }),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "DeletePolicy, absent alias reference",
            "DeletePolicy",
            json!({ "policyStoreId": absent_alias, "policyId": "SPabsent" }),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "ListPolicies, absent alias reference",
            "ListPolicies",
            json!({ "policyStoreId": absent_alias }),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "GetSchema, absent alias reference",
            "GetSchema",
            json!({ "policyStoreId": absent_alias }),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "PutSchema, absent alias reference",
            "PutSchema",
            put_schema(&absent_alias),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "CreatePolicy, absent alias reference",
            "CreatePolicy",
            create_policy(&absent_alias),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "IsAuthorized, absent alias reference",
            "IsAuthorized",
            authorize(&absent_alias),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "GetIdentitySource, absent alias reference",
            "GetIdentitySource",
            json!({ "policyStoreId": absent_alias, "identitySourceId": "ISabsent" }),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "CreateIdentitySource, absent alias reference",
            "CreateIdentitySource",
            json!({
                "policyStoreId": absent_alias,
                "configuration": {
                    // A pool id names the region the pool was created in, which
                    // is what `state::resolve_user_pool_arn` requires.
                    "cognitoUserPoolConfiguration": {
                        "userPoolArn": user_pool_arn(
                            DEFAULT_REGION,
                            &format!("{DEFAULT_REGION}_1a2b3c4d5"),
                        )
                    }
                },
            }),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        // An `aliasName` always names an alias; its id drops the prefix.
        (
            "GetPolicyStoreAlias, prefixed name",
            "GetPolicyStoreAlias",
            json!({ "aliasName": absent_alias }),
            "POLICY_STORE_ALIAS",
            "never-created".to_string(),
        ),
        (
            "GetPolicyStoreAlias, name with nested separators",
            "GetPolicyStoreAlias",
            json!({ "aliasName": format!("{ALIAS_PREFIX}eu/prod/photos") }),
            "POLICY_STORE_ALIAS",
            "eu/prod/photos".to_string(),
        ),
        // A `policyId` names a policy either way; a `name/` reference drops it.
        (
            "GetPolicy, absent policy id",
            "GetPolicy",
            json!({ "policyStoreId": store_id, "policyId": "SPabsent" }),
            "POLICY",
            "SPabsent".to_string(),
        ),
        (
            "GetPolicy, absent policy name reference",
            "GetPolicy",
            json!({ "policyStoreId": store_id, "policyId": format!("{POLICY_NAME_PREFIX}absent") }),
            "POLICY",
            "absent".to_string(),
        ),
        // `DeletePolicy` answers the documented idempotent 200 for a policy
        // reference that resolves to nothing, so the reference it reports as
        // missing is the STORE — by id here, by alias above.
        (
            "DeletePolicy, absent store id with a name/ policy reference",
            "DeletePolicy",
            json!({ "policyStoreId": "PSabsent", "policyId": format!("{POLICY_NAME_PREFIX}gone") }),
            "POLICY_STORE",
            "PSabsent".to_string(),
        ),
    ];

    for (case, operation, request, resource_type, resource_id) in rows {
        let (status, body) = call_in(&router, region, operation, request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
        assert_eq!(
            (
                body["resourceType"].as_str().unwrap_or_default(),
                body["resourceId"].as_str().unwrap_or_default(),
            ),
            (*resource_type, resource_id.as_str()),
            "case {case:?}: the error must name the referenced resource, got {body}"
        );
    }
}
