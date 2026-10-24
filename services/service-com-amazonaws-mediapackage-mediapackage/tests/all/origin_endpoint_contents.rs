// SPDX-License-Identifier: CC0-1.0

//! What an OriginEndpoint's access-control and packaging members have to NAME.
//!
//! `tests/all/channel_lifecycle.rs` covers the SIZE bounds of these members (how many
//! whitelist entries, how many CMAF child manifests, how long a string) and
//! `tests/all/origin_endpoint_durations.rs` the numeric floors. This suite covers the rules
//! about their CONTENT: each of these members names something outside the request that the
//! egress tier or the packager has to act on, so a value this service could never act on is
//! refused with `UnprocessableEntityException` (HTTP 422) keyed by the member, storing
//! nothing — the create persists no endpoint and the update leaves the stored one exactly
//! as it was. The three rules, one group of suites each:
//!
//! 1. WHITELIST. `Whitelist` is "a list of source IP CIDR blocks that will be allowed to
//!    access the OriginEndpoint", so every entry has to be a CIDR block — an IP address,
//!    `/`, and a prefix length no wider than that address family allows (32 bits for IPv4,
//!    128 for IPv6). Anything else is no address range the egress tier could match.
//! 2. ARN MEMBERS. A member the model documents as "the ARN for" a resource of a named AWS
//!    service has to be a well-formed ARN of that service's namespace naming that kind of
//!    resource — MediaPackage reads the secret and assumes the role up front, and a value
//!    that is not such an ARN names neither. Beyond its form, a role this emulator HOLDS
//!    whose trust policy withholds `sts:AssumeRole` from `mediapackage.amazonaws.com` is
//!    one the service could not act as, so it is refused too (the same rule, and the same
//!    evaluator, `CreateHarvestJob`'s destination role is judged by).
//! 3. CMAF MANIFESTS. A `CmafPackage`'s `HlsManifests` is "a list of HLS manifest
//!    configurations" and the endpoint's CMAF playback URL ends in one of those child
//!    manifests, so a package carrying none publishes nothing to serve.
//!
//! None of the three is a modelled constraint — every one of these members targets the bare
//! `__string` or an unbounded list, with no `@pattern`
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#Authorization,com.amazonaws.mediapackage#SpekeKeyProvider,com.amazonaws.mediapackage#CmafPackageCreateOrUpdateParameters",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`)
//! — which is why the rules are the service's own and are asserted here over a SET of
//! inputs rather than on the one value an issue reported.
//!
//! The catalogs below are derived from the service's own tables ([`state::ArnRequirement`]
//! and its constants, [`state::MIN_CMAF_HLS_MANIFESTS`]) rather than restated, and the rows
//! differ along what each rule generalises over: WHICH member carries the value and how
//! deeply it is nested (top-level list, authorization block, each of the four packaging
//! formats' SPEKE provider), the address family and prefix length of a CIDR block, the
//! partition / region / account / resource form of an ARN, the vendor and resource type it
//! names, the size of a manifest list, and which operation carries it (create or update).
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — with the harness `tests/all/channel_lifecycle.rs` owns; the trust
//! rows additionally use the two-service router and the `CreateRole` helper
//! `tests/all/harvest_job_rejections.rs` owns, so roles are the records IAM really holds.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, emulator, error_code, ACCOUNT, REGION};
use crate::harvest_job_rejections::{create_role, emulator as emulator_with_iam, policy, PRINCIPAL};

/// The channel every endpoint below packages.
const CHANNEL_ID: &str = "contents-channel";

/// An ARN that satisfies [`state::IAM_ROLE_ARN`], for the rows whose subject is another
/// member. The account is `channel_lifecycle::ACCOUNT` and IAM's ARNs carry no Region.
const GOOD_ROLE: &str = "arn:aws:iam::000000000000:role/mediapackage";

/// An ARN that satisfies [`state::SECRETS_MANAGER_SECRET_ARN`], for the rows whose subject
/// is another member (`channel_lifecycle::REGION` / `ACCOUNT`).
const GOOD_SECRET: &str = "arn:aws:secretsmanager:eu-west-1:000000000000:secret:cdn-AbCdEf";

/// A SPEKE key provider carrying `role_arn`, and `certificate_arn` when one is given.
///
/// `ResourceId`, `RoleArn`, `SystemIds` and `Url` are all required on the shape, so an
/// encryption row reaches the handler instead of being refused for a missing member.
fn speke(role_arn: &str, certificate_arn: Option<&str>) -> Value {
    let mut provider = json!({
        "resourceId": "endpoint-contents",
        "roleArn": role_arn,
        "systemIds": ["81376844-f976-481e-a84e-cc25d39b0b33"],
        "url": "https://speke.example.com/v1/copyProtection"
    });
    if let Some(certificate_arn) = certificate_arn {
        provider["certificateArn"] = json!(certificate_arn);
    }
    provider
}

/// A `CreateOriginEndpoint` body: the configuration `body` describes, plus the two ids a
/// create needs.
fn create_body(body: &Value, id: &str) -> Value {
    let mut created = body.clone();
    let object = created
        .as_object_mut()
        .expect("every row's body is a JSON object");
    object.insert("id".to_string(), json!(id));
    object.insert("channelId".to_string(), json!(CHANNEL_ID));
    created
}

/// Assert `refusal` is this service's 422, keyed by `member` and stating `requirement`.
fn assert_refused(case: &str, status: u16, refusal: &Value, member: &str, requirement: &str) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    assert_eq!(
        refusal.pointer(&format!("/errors/{member}")),
        Some(&json!(requirement)),
        "case {case:?}: the refusal must be keyed by {member:?} and state {requirement:?}, \
         got {refusal:?}"
    );
}

/// Assert the service holds no origin endpoint under `id`.
async fn assert_not_stored(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, described) = call(router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: a refused create must store no endpoint, got {status} {described:?}"
    );
    assert_eq!(
        error_code(&described),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {described:?}"
    );
}

/// Create the endpoint `id` with a well-formed configuration, and answer the record the
/// service stored for it — the baseline a refused update must leave untouched.
async fn stored_endpoint(router: &MultiServiceRouter, id: &str) -> Value {
    let body = create_body(
        &json!({ "hlsPackage": {}, "whitelist": ["10.0.0.0/8"] }),
        id,
    );
    let (status, created) = call(router, "POST", "/origin_endpoints", Some(&body)).await;
    assert_eq!(
        status, 200,
        "creating the baseline endpoint {id:?} expected 200, got {status} {created:?}"
    );
    created
}

/// Assert a refused update left the stored endpoint exactly as `stored` reported it.
async fn assert_unchanged(router: &MultiServiceRouter, case: &str, id: &str, stored: &Value) {
    let (status, described) = call(router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
    );
    assert_eq!(
        &described, stored,
        "case {case:?}: a refused update must leave the stored record alone, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// 1. Whitelist entries
// ---------------------------------------------------------------------------

/// Whitelists no request may store. One row per way an entry fails the rule: the value is
/// not an address at all, names one address rather than a range, has a prefix length that is
/// not a number or is wider than its family, is an address of the wrong shape, or is the
/// second entry of a list whose first entry is fine.
const REFUSED_WHITELISTS: &[(&str, &[&str])] = &[
    ("not an address at all", &["not-a-cidr"]),
    ("the empty string", &[""]),
    ("a bare IPv4 address, no prefix", &["10.0.0.1"]),
    ("an IPv4 prefix one past the bound", &["10.0.0.0/33"]),
    ("an IPv4 octet out of range", &["999.1.1.1/8"]),
    ("too few IPv4 octets", &["10.0.0/8"]),
    ("a negative prefix length", &["10.0.0.0/-1"]),
    ("a signed prefix length", &["10.0.0.0/+8"]),
    ("a prefix length with a leading zero", &["10.0.0.0/08"]),
    ("a non-numeric prefix length", &["10.0.0.0/x"]),
    ("no prefix length after the slash", &["10.0.0.0/"]),
    ("two prefix lengths", &["10.0.0.0/8/9"]),
    ("a bare IPv6 address, no prefix", &["2001:db8::1"]),
    ("an IPv6 prefix one past the bound", &["2001:db8::/129"]),
    ("a leading space before the address", &[" 10.0.0.0/8"]),
    ("the junk entry second", &["10.9.0.0/16", "nonsense"]),
    ("the junk entry last of three", &["10.9.0.0/16", "::/0", "10.0.0.0/40"]),
];

/// The first entry of `whitelist` that the rule refuses — the one the service reports,
/// because entries are judged in the request's own order.
fn refused_entry(whitelist: &[&str]) -> String {
    whitelist
        .iter()
        .find(|entry| !state::is_cidr_block(entry))
        .map(|entry| (*entry).to_string())
        .expect("every refused row carries an entry that is not a CIDR block")
}

/// The requirement a refused whitelist states, as the service words it.
fn whitelist_requirement(entry: &str) -> String {
    format!("must carry source IP CIDR blocks; {entry:?} is not one")
}

/// Whitelists every request must store: each published prefix bound of both address
/// families, a host rule, a list of several, and the empty list.
const ACCEPTED_WHITELISTS: &[(&str, &[&str])] = &[
    ("a private IPv4 block", &["10.0.0.0/8"]),
    ("the IPv4 prefix bound 0", &["0.0.0.0/0"]),
    ("the IPv4 prefix bound 32, a single host", &["10.0.0.1/32"]),
    ("host bits set below the prefix", &["10.0.0.1/8"]),
    ("an IPv6 block", &["2001:db8::/32"]),
    ("the IPv6 prefix bound 0", &["::/0"]),
    ("the IPv6 prefix bound 128", &["2001:db8::1/128"]),
    (
        "several entries of both families",
        &["192.168.0.0/16", "2001:db8:1::/48", "203.0.113.7/32"],
    ),
    ("no entries at all", &[]),
];

#[tokio::test]
async fn a_whitelist_entry_that_is_not_a_cidr_block_is_refused_and_persists_nothing() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;
    let baseline_id = "whitelist-baseline";
    let stored = stored_endpoint(&router, baseline_id).await;

    for (index, (case, whitelist)) in REFUSED_WHITELISTS.iter().enumerate() {
        let requirement = whitelist_requirement(&refused_entry(whitelist));
        let configuration = json!({ "hlsPackage": {}, "whitelist": whitelist });

        // On the create: refused, and nothing is stored.
        let id = format!("whitelist-refused-{index}");
        let create = format!("create {case}");
        let (status, refusal) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(&configuration, &id)),
        )
        .await;
        assert_refused(
            &create,
            status,
            &refusal,
            state::WHITELIST_MEMBER,
            &requirement,
        );
        assert_not_stored(&router, &create, &id).await;

        // On the update: refused by the same rule, and the stored record is untouched.
        let update = format!("update {case}");
        let (status, refusal) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{baseline_id}"),
            Some(&configuration),
        )
        .await;
        assert_refused(
            &update,
            status,
            &refusal,
            state::WHITELIST_MEMBER,
            &requirement,
        );
        assert_unchanged(&router, &update, baseline_id, &stored).await;
    }
}

#[tokio::test]
async fn a_whitelist_of_cidr_blocks_is_stored_and_read_back() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for (index, (case, whitelist)) in ACCEPTED_WHITELISTS.iter().enumerate() {
        let id = format!("whitelist-stored-{index}");
        let configuration = json!({ "hlsPackage": {}, "whitelist": whitelist });
        let (status, created) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(&configuration, &id)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        assert_eq!(
            created.get("whitelist"),
            Some(&json!(whitelist)),
            "case {case:?}: the whitelist must be stored as sent, got {created:?}"
        );

        // Round trip: the later read renders the record the create stored.
        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: the read must render the stored record, got {described:?}"
        );

        // And an update replaces the list with another accepted one.
        let next: Vec<&str> = whitelist.iter().copied().rev().collect();
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{id}"),
            Some(&json!({ "hlsPackage": {}, "whitelist": next })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update expected 200, got {status} {updated:?}"
        );
        assert_eq!(
            updated.get("whitelist"),
            Some(&json!(next)),
            "case {case:?}: the update must store the list it sent, got {updated:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Members documented as an ARN
// ---------------------------------------------------------------------------

/// One ARN-documented member of an origin-endpoint write: the wire path it travels under,
/// the requirement the service holds it to, and the configuration that carries `value` in
/// it. Each body names exactly ONE packaging configuration, because an endpoint packages its
/// channel in exactly one format.
struct ArnMember {
    path: &'static str,
    requirement: state::ArnRequirement,
    body: fn(&str) -> Value,
}

/// The catalog: every member of `CreateOriginEndpoint`'s / `UpdateOriginEndpoint`'s input
/// the model documents as an ARN — the two members of a CDN `Authorization` block, and the
/// SPEKE key provider's role and certificate under each of the four packaging formats
/// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#Authorization,com.amazonaws.mediapackage#SpekeKeyProvider")`).
const ARN_MEMBERS: &[ArnMember] = &[
    ArnMember {
        path: "authorization.cdnIdentifierSecret",
        requirement: state::SECRETS_MANAGER_SECRET_ARN,
        body: |value| {
            json!({
                "hlsPackage": {},
                "authorization": { "cdnIdentifierSecret": value, "secretsRoleArn": GOOD_ROLE }
            })
        },
    },
    ArnMember {
        path: "authorization.secretsRoleArn",
        requirement: state::IAM_ROLE_ARN,
        body: |value| {
            json!({
                "hlsPackage": {},
                "authorization": { "cdnIdentifierSecret": GOOD_SECRET, "secretsRoleArn": value }
            })
        },
    },
    ArnMember {
        path: "hlsPackage.encryption.spekeKeyProvider.roleArn",
        requirement: state::IAM_ROLE_ARN,
        body: |value| {
            json!({ "hlsPackage": { "encryption": { "spekeKeyProvider": speke(value, None) } } })
        },
    },
    ArnMember {
        path: "hlsPackage.encryption.spekeKeyProvider.certificateArn",
        requirement: state::ACM_CERTIFICATE_ARN,
        body: |value| {
            json!({
                "hlsPackage": {
                    "encryption": { "spekeKeyProvider": speke(GOOD_ROLE, Some(value)) }
                }
            })
        },
    },
    ArnMember {
        path: "dashPackage.encryption.spekeKeyProvider.roleArn",
        requirement: state::IAM_ROLE_ARN,
        body: |value| {
            json!({ "dashPackage": { "encryption": { "spekeKeyProvider": speke(value, None) } } })
        },
    },
    ArnMember {
        path: "cmafPackage.encryption.spekeKeyProvider.roleArn",
        requirement: state::IAM_ROLE_ARN,
        body: |value| {
            json!({
                "cmafPackage": {
                    "hlsManifests": [{ "id": "primary" }],
                    "encryption": { "spekeKeyProvider": speke(value, None) }
                }
            })
        },
    },
    ArnMember {
        path: "mssPackage.encryption.spekeKeyProvider.roleArn",
        requirement: state::IAM_ROLE_ARN,
        body: |value| {
            json!({ "mssPackage": { "encryption": { "spekeKeyProvider": speke(value, None) } } })
        },
    },
];

/// Values that are no resource ARN at all, so no member's requirement admits any of them.
const NOT_ARNS: &[(&str, &str)] = &[
    ("plain text", "not-an-arn"),
    ("the empty string", ""),
    ("an ARN that stops before its resource", "arn:aws:iam::000000000000"),
    ("an ARN with an empty resource", "arn:aws:iam::000000000000:"),
    (
        "a value whose first field is not arn",
        "urn:aws:iam::000000000000:role/r",
    ),
    (
        "an ARN with an empty partition",
        "arn::iam::000000000000:role/r",
    ),
];

/// ARNs that are well formed but name the wrong service or the wrong kind of resource,
/// derived from the member's OWN requirement so every member is tried against its own
/// near-misses rather than against one member's.
fn near_misses(requirement: state::ArnRequirement) -> Vec<(String, String)> {
    let vendor = requirement.vendor;
    let kind = requirement.resource_type.unwrap_or("resource");
    vec![
        (
            "another vendor's ARN".to_string(),
            format!("arn:aws:other-service:{REGION}:{ACCOUNT}:{kind}/id"),
        ),
        (
            "another resource type of the same vendor".to_string(),
            format!("arn:aws:{vendor}:{REGION}:{ACCOUNT}:something-else/id"),
        ),
        (
            "the vendor in another case".to_string(),
            format!(
                "arn:aws:{}:{REGION}:{ACCOUNT}:{kind}/id",
                vendor.to_uppercase()
            ),
        ),
    ]
}

/// ARNs that satisfy the requirement, in the forms AWS writes one: the documented form, a
/// partition-global ARN (how IAM writes every ARN), the colon form in another partition, and
/// a resource id carrying a path.
fn satisfying(requirement: state::ArnRequirement) -> Vec<(String, String)> {
    let vendor = requirement.vendor;
    let kind = requirement.resource_type.unwrap_or("resource");
    vec![
        (
            "the documented form".to_string(),
            format!("arn:aws:{vendor}:{REGION}:{ACCOUNT}:{kind}/id-1"),
        ),
        (
            "a partition-global ARN".to_string(),
            format!("arn:aws:{vendor}::{ACCOUNT}:{kind}/id-2"),
        ),
        (
            "the colon form, in another partition".to_string(),
            format!("arn:aws-cn:{vendor}:cn-north-1:{ACCOUNT}:{kind}:id-3"),
        ),
        (
            "a resource id carrying a path".to_string(),
            format!("arn:aws:{vendor}:{REGION}:{ACCOUNT}:{kind}/service-role/id-4"),
        ),
    ]
}

/// The JSON pointer that reads a member back out of an endpoint projection, derived from the
/// member path so the two cannot drift apart.
fn pointer(path: &str) -> String {
    format!("/{}", path.replace('.', "/"))
}

#[tokio::test]
async fn an_arn_member_that_names_no_resource_of_its_documented_service_is_refused() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;
    let baseline_id = "arn-baseline";
    let stored = stored_endpoint(&router, baseline_id).await;

    for (member_index, member) in ARN_MEMBERS.iter().enumerate() {
        let values: Vec<(String, String)> = NOT_ARNS
            .iter()
            .map(|(case, value)| ((*case).to_string(), (*value).to_string()))
            .chain(near_misses(member.requirement))
            .collect();

        for (value_index, (label, value)) in values.iter().enumerate() {
            let case = format!("{} = {label}", member.path);
            let configuration = (member.body)(value);

            let id = format!("arn-refused-{member_index}-{value_index}");
            let (status, refusal) = call(
                &router,
                "POST",
                "/origin_endpoints",
                Some(&create_body(&configuration, &id)),
            )
            .await;
            assert_refused(
                &format!("create {case}"),
                status,
                &refusal,
                member.path,
                &member.requirement.phrase(),
            );
            assert_not_stored(&router, &format!("create {case}"), &id).await;

            let update = format!("update {case}");
            let (status, refusal) = call(
                &router,
                "PUT",
                &format!("/origin_endpoints/{baseline_id}"),
                Some(&configuration),
            )
            .await;
            assert_refused(
                &update,
                status,
                &refusal,
                member.path,
                &member.requirement.phrase(),
            );
            assert_unchanged(&router, &update, baseline_id, &stored).await;
        }
    }
}

#[tokio::test]
async fn an_arn_member_naming_its_documented_resource_is_stored_and_read_back() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for (member_index, member) in ARN_MEMBERS.iter().enumerate() {
        for (value_index, (label, value)) in satisfying(member.requirement).iter().enumerate() {
            let case = format!("{} = {label}", member.path);
            let id = format!("arn-stored-{member_index}-{value_index}");
            let configuration = (member.body)(value);
            let (status, created) = call(
                &router,
                "POST",
                "/origin_endpoints",
                Some(&create_body(&configuration, &id)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {created:?}"
            );
            let read = pointer(member.path);
            assert_eq!(
                created.pointer(&read),
                Some(&json!(value)),
                "case {case:?}: the create response must carry the value at {read:?}, \
                 got {created:?}"
            );

            // Round trip: the later read renders the record the create stored.
            let (status, described) =
                call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described, created,
                "case {case:?}: the read must render the stored record, got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2b. A role MediaPackage could not act as
// ---------------------------------------------------------------------------

/// One member naming a role MediaPackage itself assumes, and the configuration carrying it.
struct RoleMember {
    path: &'static str,
    body: fn(&str) -> Value,
}

/// Every such member of an origin-endpoint write: the CDN authorization role (MediaPackage
/// reads the CDN secret as it) and the SPEKE key provider's role (MediaPackage reaches the
/// key provider as it).
const ROLE_MEMBERS: &[RoleMember] = &[
    RoleMember {
        path: "authorization.secretsRoleArn",
        body: |role| {
            json!({
                "hlsPackage": {},
                "authorization": { "cdnIdentifierSecret": GOOD_SECRET, "secretsRoleArn": role }
            })
        },
    },
    RoleMember {
        path: "hlsPackage.encryption.spekeKeyProvider.roleArn",
        body: |role| {
            json!({ "hlsPackage": { "encryption": { "spekeKeyProvider": speke(role, None) } } })
        },
    },
];

#[tokio::test]
async fn a_role_mediapackage_cannot_assume_is_refused_on_an_origin_endpoint_write() {
    let router = emulator_with_iam();
    create_channel(&router, CHANNEL_ID).await;

    // (case, the trust policy the role carries, whether the write is accepted)
    let trusts: Vec<(&str, Option<String>, bool)> = vec![
        (
            "a policy granting the principal sts:AssumeRole",
            Some(policy(
                "Allow",
                "\"sts:AssumeRole\"",
                &format!("\"{PRINCIPAL}\""),
            )),
            true,
        ),
        (
            "a policy naming another service's principal only",
            Some(policy(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"mediaconvert.amazonaws.com\"",
            )),
            false,
        ),
        (
            "a policy explicitly denying the principal",
            Some(policy(
                "Deny",
                "\"sts:AssumeRole\"",
                &format!("\"{PRINCIPAL}\""),
            )),
            false,
        ),
        (
            "a policy granting an action that is not assuming the role",
            Some(policy(
                "Allow",
                "\"s3:PutObject\"",
                &format!("\"{PRINCIPAL}\""),
            )),
            false,
        ),
        ("a role ARN the IAM store holds no role for", None, true),
    ];

    for (member_index, member) in ROLE_MEMBERS.iter().enumerate() {
        for (trust_index, (label, document, accepted)) in trusts.iter().enumerate() {
            let role_arn = match document {
                Some(document) => {
                    create_role(
                        &router,
                        &format!("mediapackage-endpoint-{member_index}-{trust_index}"),
                        document,
                    )
                    .await
                }
                None => format!("arn:aws:iam::{ACCOUNT}:role/unknown-to-iam-{member_index}"),
            };
            let case = format!("{} with {label}", member.path);
            let id = format!("role-{member_index}-{trust_index}");
            let (status, answered) = call(
                &router,
                "POST",
                "/origin_endpoints",
                Some(&create_body(&(member.body)(&role_arn), &id)),
            )
            .await;

            if *accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {answered:?}"
                );
                assert_eq!(
                    answered.pointer(&pointer(member.path)),
                    Some(&json!(role_arn)),
                    "case {case:?}: the role must be stored as sent, got {answered:?}"
                );
            } else {
                assert_eq!(
                    status, 422,
                    "case {case:?}: expected 422, got {status} {answered:?}"
                );
                assert_eq!(
                    error_code(&answered),
                    "UnprocessableEntityException",
                    "case {case:?}: expected UnprocessableEntityException, got {answered:?}"
                );
                assert!(
                    answered
                        .pointer(&format!("/errors/{}", member.path))
                        .is_some(),
                    "case {case:?}: the refusal must be keyed by {:?}, got {answered:?}",
                    member.path
                );
                assert_not_stored(&router, &case, &id).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 3. A CMAF package's child manifests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cmaf_package_that_publishes_no_manifest_is_refused() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;
    let baseline_id = "cmaf-baseline";
    let stored = stored_endpoint(&router, baseline_id).await;

    // One row per way a CMAF package can carry no child manifest, with the other members it
    // carries varying around it.
    let refused: Vec<(&str, Value)> = vec![
        ("the list absent", json!({ "cmafPackage": {} })),
        (
            "the list empty",
            json!({ "cmafPackage": { "hlsManifests": [] } }),
        ),
        (
            "the list absent beside other members",
            json!({ "cmafPackage": { "segmentDurationSeconds": 4, "segmentPrefix": "p" } }),
        ),
        (
            "the list empty beside an encryption block",
            json!({
                "cmafPackage": {
                    "hlsManifests": [],
                    "encryption": { "spekeKeyProvider": speke(GOOD_ROLE, None) }
                }
            }),
        ),
    ];

    for (index, (case, configuration)) in refused.iter().enumerate() {
        let id = format!("cmaf-refused-{index}");
        let (status, refusal) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(configuration, &id)),
        )
        .await;
        assert_refused(
            &format!("create {case}"),
            status,
            &refusal,
            state::CMAF_HLS_MANIFESTS_MEMBER,
            // The service's own sentence, so the suite cannot restate the rule differently
            // from what the handler answers.
            &state::cmaf_manifests_requirement(0),
        );
        assert_not_stored(&router, &format!("create {case}"), &id).await;

        let update = format!("update {case}");
        let (status, refusal) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{baseline_id}"),
            Some(configuration),
        )
        .await;
        assert_refused(
            &update,
            status,
            &refusal,
            state::CMAF_HLS_MANIFESTS_MEMBER,
            &state::cmaf_manifests_requirement(0),
        );
        assert_unchanged(&router, &update, baseline_id, &stored).await;
    }
}

#[tokio::test]
async fn a_cmaf_package_at_or_above_the_manifest_minimum_is_stored_and_read_back() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    // Rows differ by how many child manifests the package publishes: the minimum itself,
    // one more, and a longer list.
    for count in [state::MIN_CMAF_HLS_MANIFESTS, 2, 5] {
        let case = format!("{count} child manifests");
        let id = format!("cmaf-stored-{count}");
        let manifests: Vec<Value> = (0..count)
            .map(|index| json!({ "id": format!("m-{index}") }))
            .collect();
        let configuration = json!({ "cmafPackage": { "hlsManifests": manifests } });
        let (status, created) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(&configuration, &id)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        assert_eq!(
            created
                .pointer("/cmafPackage/hlsManifests")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(count),
            "case {case:?}: every child manifest must be stored, got {created:?}"
        );

        // Round trip: the later read renders the record the create stored.
        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: the read must render the stored record, got {described:?}"
        );
    }
}
