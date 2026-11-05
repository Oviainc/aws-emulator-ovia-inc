// SPDX-License-Identifier: CC0-1.0

//! The S3 Access Grants grant and data-plane chain, driven end to end over
//! HTTP.
//!
//! The rule under test: a grant's scope is the result of appending its
//! `S3SubPrefix` to the scope of the registered location it narrows, and
//! `GetDataAccess` vends a temporary credential exactly when some grant of that
//! account's Access Grants instance in that Region has a scope covering the
//! requested target for the requested permission — reporting the scope the token
//! carries (the matched grant's scope, or the requested target when the request
//! asks for the `Minimal` privilege) and the matched grant's grantee — and
//! otherwise refuses with 403 `AccessDenied`, naming the permission and the S3
//! path asked about, and vends nothing.
//!
//! Every row below differs along something that rule generalises over: the form
//! of the location scope (the default location, a bucket with and without its
//! trailing slash, a bucket and prefix), whether and how the grant narrows it (a
//! bucket wildcard, a prefix wildcard, a single object key, no narrowing at
//! all), the permission granted and the permission asked for, whether the target
//! is a prefix or an object key and how deep it sits, the privilege and session
//! duration the vend asks for, and the account and Region the instance belongs
//! to.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it
//! per the Smithy model, runs the handler and serializes the restXml response.
//! The router, the request builder and the XML readers are the sibling suites',
//! reused rather than copied.

use std::collections::BTreeSet;

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, xml_escape, Emulator, Reply, ACCOUNT,
    OTHER_ACCOUNT, REGION, XMLNS,
};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ACCESS_GRANTS_INSTANCE_ID, ACCESS_GRANTS_LOCATION_DEFAULT_ID,
    DATA_ACCESS_DEFAULT_DURATION_SECONDS, DEFAULT_LOCATION_SCOPE, PERMISSION_READ,
    PERMISSION_READWRITE, PERMISSION_WRITE, PRIVILEGE_DEFAULT, PRIVILEGE_MINIMAL,
};

/// A second signing Region, used to pin that a grant belongs to the instance of
/// one account in one Region: the same target is uncovered here.
const OTHER_REGION: &str = "eu-central-1";

/// The IAM role every location in this suite is registered with. Its shape is
/// the modelled `IAMRoleArn` pattern, `^arn:[^:]+:iam::\d{12}:role/`.
const LOCATION_ROLE: &str = "arn:aws:iam::123456789012:role/s3-access-grants-location";

/// The grantee every grant in this suite is created for: an IAM role, which
/// `Grantee$GranteeIdentifier` documents as "the IAM Amazon Resource Name (ARN)
/// of the user or role" when the grantee type is `IAM`.
const GRANTEE_IDENTIFIER: &str = "arn:aws:iam::123456789012:role/data-reader";

/// The `GranteeType` of [`GRANTEE_IDENTIFIER`], one of the modelled
/// `GranteeType` values.
const GRANTEE_TYPE: &str = "IAM";

/// The characters a query value may carry unescaped: the RFC 3986 unreserved
/// set. Everything else is percent-encoded by [`query_escape`], because an S3
/// URI carries `:`, `/` and `*`.
///
/// Spelled out here rather than taken from the `percent-encoding` crate for the
/// reason the sibling suite's `XML_ENTITIES` gives: a new dev-dependency puts
/// this service's crate into `Cargo.lock`, which every other service's landing
/// also touches, and the squash merge then conflicts.
const QUERY_UNRESERVED: &str = "-._~";

/// Percent-encode one query value the way an SDK does: every byte outside
/// [`QUERY_UNRESERVED`] and the ASCII alphanumerics as `%XX`, so an S3 URI's
/// colons, slashes and wildcard reach the handler as the value the caller wrote.
///
/// `pub(crate)` because the sibling suites that put a value in a query string
/// escape it the same way rather than each writing this again.
pub(crate) fn query_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        let plain = byte.is_ascii_alphanumeric() || QUERY_UNRESERVED.contains(char::from(byte));
        if plain {
            escaped.push(char::from(byte));
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

/// Create the account's Access Grants instance in `region`.
pub(crate) async fn create_instance(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/accessgrantsinstance",
        account_id,
        &format!("<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\"/>"),
    )
    .await
}

/// Register `location_scope` in the account's instance in `region`.
pub(crate) async fn create_location(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    location_scope: &str,
) -> Reply {
    let body = format!(
        "<CreateAccessGrantsLocationRequest xmlns=\"{XMLNS}\">\
         <LocationScope>{}</LocationScope><IAMRoleArn>{}</IAMRoleArn>\
         </CreateAccessGrantsLocationRequest>",
        xml_escape(location_scope),
        xml_escape(LOCATION_ROLE)
    );
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/accessgrantsinstance/location",
        account_id,
        &body,
    )
    .await
}

/// Read the registered location `location_id` names.
pub(crate) async fn get_location(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    location_id: &str,
) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        &format!("/v20180820/accessgrantsinstance/location/{location_id}"),
        account_id,
        "",
    )
    .await
}

/// Create a grant on `location_id`, narrowed by `sub_prefix` when there is one.
pub(crate) async fn create_grant(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    location_id: &str,
    sub_prefix: Option<&str>,
    permission: &str,
) -> Reply {
    let mut body = format!(
        "<CreateAccessGrantRequest xmlns=\"{XMLNS}\">\
         <AccessGrantsLocationId>{}</AccessGrantsLocationId>",
        xml_escape(location_id)
    );
    if let Some(sub_prefix) = sub_prefix {
        body.push_str(&format!(
            "<AccessGrantsLocationConfiguration><S3SubPrefix>{}</S3SubPrefix>\
             </AccessGrantsLocationConfiguration>",
            xml_escape(sub_prefix)
        ));
    }
    body.push_str(&format!(
        "<Grantee><GranteeType>{GRANTEE_TYPE}</GranteeType><GranteeIdentifier>{}\
         </GranteeIdentifier></Grantee><Permission>{permission}</Permission>\
         </CreateAccessGrantRequest>",
        xml_escape(GRANTEE_IDENTIFIER)
    ));
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/accessgrantsinstance/grant",
        account_id,
        &body,
    )
    .await
}

/// Read the grant `grant_id` names.
pub(crate) async fn get_grant(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    grant_id: &str,
) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        &format!("/v20180820/accessgrantsinstance/grant/{grant_id}"),
        account_id,
        "",
    )
    .await
}

/// Ask for temporary access to `target` with `permission`, optionally naming a
/// privilege and a session duration.
async fn get_data_access(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    target: &str,
    permission: &str,
    privilege: Option<&str>,
    duration_seconds: Option<i32>,
) -> Reply {
    let mut uri = format!(
        "/v20180820/accessgrantsinstance/dataaccess?target={}&permission={permission}",
        query_escape(target)
    );
    if let Some(privilege) = privilege {
        uri.push_str(&format!("&privilege={privilege}"));
    }
    if let Some(duration_seconds) = duration_seconds {
        uri.push_str(&format!("&durationSeconds={duration_seconds}"));
    }
    call_in_region(&emulator.router, region, "GET", &uri, account_id, "").await
}

/// The ARN a grant of this id must report: the `grant/<id>` subresource of the
/// Access Grants instance that holds it, so the instance segment
/// (`ACCESS_GRANTS_INSTANCE_ID`) sits between `access-grants` and `grant`.
fn expected_grant_arn(region: &str, grant_id: &str) -> String {
    format!(
        "arn:aws:s3:{region}:{}:access-grants/{}/grant/{grant_id}",
        ::emulator_server::emulator_account_id(),
        ACCESS_GRANTS_INSTANCE_ID
    )
}

/// The ARN a registered location of this id must report: the `location/<id>`
/// subresource of the same instance. See [`expected_grant_arn`].
fn expected_location_arn(region: &str, location_id: &str) -> String {
    format!(
        "arn:aws:s3:{region}:{}:access-grants/{}/location/{location_id}",
        ::emulator_server::emulator_account_id(),
        ACCESS_GRANTS_INSTANCE_ID
    )
}

/// The seconds since the epoch an ISO-8601 timestamp in a response names.
fn epoch_seconds(case: &str, timestamp: &str) -> i64 {
    ::emulator_server::aws_smithy_types::DateTime::from_str(
        timestamp,
        ::emulator_server::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: expected an ISO-8601 timestamp, got {timestamp:?}: {error}")
    })
    .secs()
}

/// Now, in seconds since the epoch — the clock a vended credential's expiration
/// is measured against.
fn now_seconds() -> i64 {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch");
    i64::try_from(since_epoch.as_secs()).expect("the epoch seconds fit in an i64")
}

/// One location-and-grant shape: how the location is registered, how the grant
/// narrows it, and the grant scope the two must produce.
struct ScopeCase {
    case: &'static str,
    location_scope: &'static str,
    sub_prefix: Option<&'static str>,
    permission: &'static str,
    grant_scope: &'static str,
}

/// The shapes of location scope and sub-prefix the grant-scope rule is run
/// over. Each row registers its own location, and differs from the others in
/// something the rule generalises over.
fn scope_cases() -> Vec<ScopeCase> {
    vec![
        ScopeCase {
            // `S3SubPrefix`'s own example: "if the location scope is the default
            // location s3://, the S3SubPrefx can be a <bucket-name>/*, so the
            // full grant scope path would be s3://<bucket-name>/*".
            case: "the default location narrowed to a whole bucket",
            location_scope: DEFAULT_LOCATION_SCOPE,
            sub_prefix: Some("default-loc-bucket/*"),
            permission: PERMISSION_READ,
            grant_scope: "s3://default-loc-bucket/*",
        },
        ScopeCase {
            case: "a bucket with its trailing slash, narrowed to a prefix",
            location_scope: "s3://slashed-bucket/",
            sub_prefix: Some("team-a/*"),
            permission: PERMISSION_READ,
            grant_scope: "s3://slashed-bucket/team-a/*",
        },
        ScopeCase {
            case: "a bucket without its trailing slash, narrowed to a prefix",
            location_scope: "s3://plain-bucket",
            sub_prefix: Some("team-b/*"),
            permission: PERMISSION_WRITE,
            grant_scope: "s3://plain-bucket/team-b/*",
        },
        ScopeCase {
            case: "a bucket and prefix narrowed to a single object",
            location_scope: "s3://reports-bucket/reports/",
            sub_prefix: Some("2026/q1.csv"),
            permission: PERMISSION_READWRITE,
            grant_scope: "s3://reports-bucket/reports/2026/q1.csv",
        },
        ScopeCase {
            case: "a whole bucket, narrowed by nothing",
            location_scope: "s3://whole-bucket/",
            sub_prefix: None,
            permission: PERMISSION_READWRITE,
            grant_scope: "s3://whole-bucket/",
        },
        ScopeCase {
            // Both halves carry the separator: exactly one `/` joins them, so
            // this is the same grant scope the row above it would produce from
            // the same two path parts written without the leading slash.
            case: "a bucket with its trailing slash and a sub-prefix with a leading one",
            location_scope: "s3://doubled-bucket/",
            sub_prefix: Some("/team-c/*"),
            permission: PERMISSION_READ,
            grant_scope: "s3://doubled-bucket/team-c/*",
        },
    ]
}

/// Register `case`'s location and create its grant, answering the location id
/// and the grant id.
async fn register(emulator: &Emulator, case: &ScopeCase) -> (String, String) {
    let location = create_location(emulator, ACCOUNT, REGION, case.location_scope).await;
    assert_eq!(
        location.status, 200,
        "case {:?}: expected CreateAccessGrantsLocation to answer 200, got {} with body {}",
        case.case, location.status, location.body
    );
    let location_id = location
        .text("AccessGrantsLocationId")
        .unwrap_or_else(|| panic!("case {:?}: no AccessGrantsLocationId", case.case));

    let grant = create_grant(
        emulator,
        ACCOUNT,
        REGION,
        &location_id,
        case.sub_prefix,
        case.permission,
    )
    .await;
    assert_eq!(
        grant.status, 200,
        "case {:?}: expected CreateAccessGrant to answer 200, got {} with body {}",
        case.case, grant.status, grant.body
    );
    let grant_id = grant
        .text("AccessGrantId")
        .unwrap_or_else(|| panic!("case {:?}: no AccessGrantId", case.case));
    assert_eq!(
        grant.text("GrantScope").unwrap_or_default(),
        case.grant_scope,
        "case {:?}: expected the grant scope the sub-prefix appended to the location scope \
         produces, got body {}",
        case.case,
        grant.body
    );
    (location_id, grant_id)
}

#[tokio::test]
async fn a_grant_records_the_scope_its_subprefix_appends_to_its_location() {
    let emulator = Emulator::start();
    assert_eq!(
        create_instance(&emulator, ACCOUNT, REGION).await.status,
        200,
        "expected the instance create to succeed"
    );
    let mut grant_ids = BTreeSet::new();

    for case in scope_cases() {
        let (location_id, grant_id) = register(&emulator, &case).await;

        // "S3 Access Grants assigns the ID default to the default location
        // s3:// and assigns an auto-generated ID to other locations that you
        // register" (`AccessGrantsLocationId`).
        if case.location_scope == DEFAULT_LOCATION_SCOPE {
            assert_eq!(
                location_id, ACCESS_GRANTS_LOCATION_DEFAULT_ID,
                "case {:?}: expected the default location's documented id",
                case.case
            );
        } else {
            assert_ne!(
                location_id, ACCESS_GRANTS_LOCATION_DEFAULT_ID,
                "case {:?}: expected a service-assigned id for a location other than the \
                 default one",
                case.case
            );
        }
        assert!(
            grant_ids.insert(grant_id.clone()),
            "case {:?}: expected a grant id no other grant carries, got a repeat of {grant_id:?}",
            case.case
        );
        assert!(
            !grant_id.is_empty()
                && grant_id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-'),
            "case {:?}: expected an AccessGrantId the modelled pattern admits, got {grant_id:?}",
            case.case
        );

        // The location the grant was computed from reads back as registered.
        let location = get_location(&emulator, ACCOUNT, REGION, &location_id).await;
        assert_eq!(
            location.status, 200,
            "case {:?}: expected GetAccessGrantsLocation to answer 200, got {} with body {}",
            case.case, location.status, location.body
        );
        assert_eq!(
            location.text("LocationScope").unwrap_or_default(),
            case.location_scope,
            "case {:?}: expected the registered location scope, got body {}",
            case.case,
            location.body
        );
        assert_eq!(
            location.text("IAMRoleArn").unwrap_or_default(),
            LOCATION_ROLE,
            "case {:?}: expected the registered IAM role, got body {}",
            case.case,
            location.body
        );
        assert_eq!(
            location.text("AccessGrantsLocationArn").unwrap_or_default(),
            expected_location_arn(REGION, &location_id),
            "case {:?}: expected the location ARN in the signing Region, got body {}",
            case.case,
            location.body
        );

        // The grant itself reads back as created.
        let read = get_grant(&emulator, ACCOUNT, REGION, &grant_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected GetAccessGrant to answer 200, got {} with body {}",
            case.case, read.status, read.body
        );
        for (element, expected) in [
            ("AccessGrantId", grant_id.clone()),
            ("AccessGrantArn", expected_grant_arn(REGION, &grant_id)),
            ("GrantScope", case.grant_scope.to_string()),
            ("Permission", case.permission.to_string()),
            ("AccessGrantsLocationId", location_id.clone()),
            ("GranteeType", GRANTEE_TYPE.to_string()),
            ("GranteeIdentifier", GRANTEE_IDENTIFIER.to_string()),
        ] {
            let reported = read.text(element).unwrap_or_default();
            assert_eq!(
                reported, expected,
                "case {:?}: expected GetAccessGrant to report {element} as {expected:?}, got \
                 {reported:?} in body {}",
                case.case, read.body
            );
        }
        let sub_prefix = read.text("S3SubPrefix");
        assert_eq!(
            sub_prefix.as_deref(),
            case.sub_prefix,
            "case {:?}: expected the submitted location configuration reported back, got body {}",
            case.case,
            read.body
        );
        let created_at = read.text("CreatedAt").unwrap_or_default();
        assert!(
            created_at.contains('T') && created_at.ends_with('Z'),
            "case {:?}: expected an ISO-8601 CreatedAt, got {created_at:?}",
            case.case
        );

        // A read may not change what a later reader sees.
        let again = get_grant(&emulator, ACCOUNT, REGION, &grant_id).await;
        assert_eq!(
            again.body, read.body,
            "case {:?}: expected a second read to report the same grant, got body {}",
            case.case, again.body
        );
    }
}

/// One data-access question: the path asked about, the permission asked for, and
/// whether the grant of the row covers it.
struct TargetCase {
    target: &'static str,
    permission: &'static str,
    covered: bool,
}

/// The targets each grant shape is asked about. Every row of
/// [`scope_cases`] gets a set, chosen so that each set has both arms: paths the
/// grant covers and paths it does not, including a sibling prefix, a permission
/// the grant does not carry, and another bucket.
fn target_cases(case: &ScopeCase) -> Vec<TargetCase> {
    let covered = |target: &'static str, permission: &'static str| TargetCase {
        target,
        permission,
        covered: true,
    };
    let denied = |target: &'static str, permission: &'static str| TargetCase {
        target,
        permission,
        covered: false,
    };
    match case.grant_scope {
        "s3://default-loc-bucket/*" => vec![
            covered("s3://default-loc-bucket/*", PERMISSION_READ),
            covered("s3://default-loc-bucket/any/key.txt", PERMISSION_READ),
            denied("s3://default-loc-bucket/*", PERMISSION_WRITE),
            denied("s3://default-loc-bucket-2/key.txt", PERMISSION_READ),
            denied("s3://", PERMISSION_READ),
        ],
        "s3://slashed-bucket/team-a/*" => vec![
            covered("s3://slashed-bucket/team-a/*", PERMISSION_READ),
            covered("s3://slashed-bucket/team-a/file.txt", PERMISSION_READ),
            covered(
                "s3://slashed-bucket/team-a/nested/deeper/file.txt",
                PERMISSION_READ,
            ),
            denied("s3://slashed-bucket/team-b/*", PERMISSION_READ),
            denied("s3://slashed-bucket/", PERMISSION_READ),
            denied("s3://slashed-bucket/team-a/file.txt", PERMISSION_READWRITE),
        ],
        "s3://plain-bucket/team-b/*" => vec![
            covered("s3://plain-bucket/team-b/*", PERMISSION_WRITE),
            covered("s3://plain-bucket/team-b/upload.bin", PERMISSION_WRITE),
            denied("s3://plain-bucket/team-b/upload.bin", PERMISSION_READ),
            denied("s3://plain-bucket/team-a/*", PERMISSION_WRITE),
        ],
        "s3://reports-bucket/reports/2026/q1.csv" => vec![
            // A READWRITE grant "grant[s] both read and write access"
            // (`Permission`), so it satisfies a request for either.
            covered("s3://reports-bucket/reports/2026/q1.csv", PERMISSION_READ),
            covered("s3://reports-bucket/reports/2026/q1.csv", PERMISSION_WRITE),
            covered(
                "s3://reports-bucket/reports/2026/q1.csv",
                PERMISSION_READWRITE,
            ),
            denied("s3://reports-bucket/reports/2026/q2.csv", PERMISSION_READ),
            denied("s3://reports-bucket/reports/*", PERMISSION_READ),
        ],
        "s3://whole-bucket/" => vec![
            covered("s3://whole-bucket/", PERMISSION_READ),
            covered("s3://whole-bucket/*", PERMISSION_READWRITE),
            covered("s3://whole-bucket/deep/key.txt", PERMISSION_WRITE),
            denied("s3://whole-bucket-2/key.txt", PERMISSION_READ),
        ],
        "s3://doubled-bucket/team-c/*" => vec![
            covered("s3://doubled-bucket/team-c/*", PERMISSION_READ),
            covered("s3://doubled-bucket/team-c/key.txt", PERMISSION_READ),
            // The grant scope carries ONE separator, so a path that doubles it
            // is not under the bucket's `team-c` prefix.
            denied("s3://doubled-bucket//team-c/key.txt", PERMISSION_READ),
            denied("s3://doubled-bucket/team-d/key.txt", PERMISSION_READ),
        ],
        other => panic!("no targets written for the grant scope {other:?}"),
    }
}

#[tokio::test]
async fn data_access_is_vended_for_a_covered_target_and_refused_for_an_uncovered_one() {
    let emulator = Emulator::start();
    assert_eq!(
        create_instance(&emulator, ACCOUNT, REGION).await.status,
        200,
        "expected the instance create to succeed"
    );
    let mut access_key_ids = BTreeSet::new();

    for case in scope_cases() {
        register(&emulator, &case).await;

        for target in target_cases(&case) {
            let label = format!("{} / {} {}", case.case, target.permission, target.target);
            let before = now_seconds();
            let reply = get_data_access(
                &emulator,
                ACCOUNT,
                REGION,
                target.target,
                target.permission,
                None,
                None,
            )
            .await;

            if !target.covered {
                assert_eq!(
                    reply.status, 403,
                    "case {label:?}: expected an uncovered target to answer 403, got {} with \
                     body {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.code(),
                    "AccessDenied",
                    "case {label:?}: expected the AccessDenied code, got body {}",
                    reply.body
                );
                let message = reply.text("Message").unwrap_or_default();
                assert_eq!(
                    message,
                    format!(
                        "You do not have {} permissions to the requested S3 Prefix: {}",
                        target.permission, target.target
                    ),
                    "case {label:?}: expected the refusal built from the request, got body {}",
                    reply.body
                );
                assert!(
                    reply.text("AccessKeyId").is_none()
                        && reply.text("SessionToken").is_none()
                        && reply.text("SecretAccessKey").is_none(),
                    "case {label:?}: expected a refusal to carry no credential, got body {}",
                    reply.body
                );
                continue;
            }

            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected a covered target to answer 200, got {} with body {}",
                reply.status, reply.body
            );
            let access_key_id = reply.text("AccessKeyId").unwrap_or_default();
            assert!(
                !access_key_id.is_empty(),
                "case {label:?}: expected a vended AccessKeyId, got body {}",
                reply.body
            );
            assert!(
                access_key_ids.insert(access_key_id.clone()),
                "case {label:?}: expected a credential of its own per vend, got a repeat of \
                 {access_key_id:?}"
            );
            for element in ["SecretAccessKey", "SessionToken"] {
                assert!(
                    reply
                        .text(element)
                        .is_some_and(|secret| !secret.is_empty()),
                    "case {label:?}: expected the vended credential to carry {element}, got \
                     body {}",
                    reply.body
                );
            }
            assert_eq!(
                reply.text("MatchedGrantTarget").unwrap_or_default(),
                case.grant_scope,
                "case {label:?}: expected the matched grant's scope, got body {}",
                reply.body
            );
            assert_eq!(
                reply.text("GranteeIdentifier").unwrap_or_default(),
                GRANTEE_IDENTIFIER,
                "case {label:?}: expected the matched grant's grantee, got body {}",
                reply.body
            );
            let expiration = reply.text("Expiration").unwrap_or_default();
            let expires_at = epoch_seconds(&label, &expiration);
            let lifetime = expires_at - before;
            let default = i64::from(DATA_ACCESS_DEFAULT_DURATION_SECONDS);
            assert!(
                (default..=default + 5).contains(&lifetime),
                "case {label:?}: expected the documented default session duration of \
                 {default} seconds, got {lifetime} from {expiration:?}"
            );
        }
    }
}

/// One vend: the privilege and session duration it asks for, the scope its
/// token must carry and how long it must last.
struct VendCase {
    label: &'static str,
    privilege: Option<&'static str>,
    duration_seconds: Option<i32>,
    matched: &'static str,
    lifetime: i64,
}

#[tokio::test]
async fn the_vended_token_follows_the_requested_privilege_and_duration() {
    let emulator = Emulator::start();
    create_instance(&emulator, ACCOUNT, REGION).await;
    let case = ScopeCase {
        case: "a prefix grant asked about one object beneath it",
        location_scope: "s3://privilege-bucket/",
        sub_prefix: Some("team-a/*"),
        permission: PERMISSION_READWRITE,
        grant_scope: "s3://privilege-bucket/team-a/*",
    };
    register(&emulator, &case).await;
    let target = "s3://privilege-bucket/team-a/report.csv";

    // `Privilege` names the two scopes a token can carry; an absent member is
    // the `Default` one. `DurationSeconds` is modelled `range 900..43200`, with
    // both bounds and the omitted default exercised here.
    let rows = vec![
        VendCase {
            label: "no privilege, no duration",
            privilege: None,
            duration_seconds: None,
            matched: case.grant_scope,
            lifetime: i64::from(DATA_ACCESS_DEFAULT_DURATION_SECONDS),
        },
        VendCase {
            label: "the Default privilege at the shortest session",
            privilege: Some(PRIVILEGE_DEFAULT),
            duration_seconds: Some(900),
            matched: case.grant_scope,
            lifetime: 900,
        },
        VendCase {
            label: "the Minimal privilege at the longest session",
            privilege: Some(PRIVILEGE_MINIMAL),
            duration_seconds: Some(43_200),
            matched: target,
            lifetime: 43_200,
        },
        VendCase {
            label: "the Minimal privilege with no duration",
            privilege: Some(PRIVILEGE_MINIMAL),
            duration_seconds: None,
            matched: target,
            lifetime: i64::from(DATA_ACCESS_DEFAULT_DURATION_SECONDS),
        },
    ];

    for VendCase {
        label,
        privilege,
        duration_seconds,
        matched,
        lifetime,
    } in rows
    {
        let before = now_seconds();
        let reply = get_data_access(
            &emulator,
            ACCOUNT,
            REGION,
            target,
            PERMISSION_READ,
            privilege,
            duration_seconds,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("MatchedGrantTarget").unwrap_or_default(),
            matched,
            "case {label:?}: expected the scope the requested privilege carries, got body {}",
            reply.body
        );
        let expiration = reply.text("Expiration").unwrap_or_default();
        let actual = epoch_seconds(label, &expiration) - before;
        assert!(
            (lifetime..=lifetime + 5).contains(&actual),
            "case {label:?}: expected a session of {lifetime} seconds, got {actual} from \
             {expiration:?}"
        );
    }
}

#[tokio::test]
async fn the_grant_closest_to_the_target_is_the_one_that_matches() {
    let emulator = Emulator::start();
    create_instance(&emulator, ACCOUNT, REGION).await;
    let location = create_location(&emulator, ACCOUNT, REGION, "s3://layered-bucket/").await;
    let location_id = location
        .text("AccessGrantsLocationId")
        .expect("the registered location's id");

    // Two grants over the same location: one on the whole bucket, one on a
    // prefix inside it with a wider permission.
    for (sub_prefix, permission) in [
        (Some("*"), PERMISSION_READ),
        (Some("team-a/*"), PERMISSION_READWRITE),
    ] {
        let created = create_grant(
            &emulator,
            ACCOUNT,
            REGION,
            &location_id,
            sub_prefix,
            permission,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {sub_prefix:?}: expected the grant create to answer 200, got {} with body {}",
            created.status, created.body
        );
    }

    // (case, target, permission, the scope the token must carry or None for a
    // refusal). A target inside the narrower grant matches it rather than the
    // bucket-wide one, and a permission only the narrower grant carries is
    // refused outside it.
    let rows: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "inside the narrower grant",
            "s3://layered-bucket/team-a/file.txt",
            PERMISSION_READ,
            Some("s3://layered-bucket/team-a/*"),
        ),
        (
            "outside it, under the bucket-wide grant",
            "s3://layered-bucket/team-b/file.txt",
            PERMISSION_READ,
            Some("s3://layered-bucket/*"),
        ),
        (
            "a write inside the narrower grant",
            "s3://layered-bucket/team-a/file.txt",
            PERMISSION_WRITE,
            Some("s3://layered-bucket/team-a/*"),
        ),
        (
            "a write outside it, where only READ is granted",
            "s3://layered-bucket/team-b/file.txt",
            PERMISSION_WRITE,
            None,
        ),
    ];

    for (label, target, permission, matched) in rows {
        let reply =
            get_data_access(&emulator, ACCOUNT, REGION, target, permission, None, None).await;
        match matched {
            Some(matched) => {
                assert_eq!(
                    reply.status, 200,
                    "case {label:?}: expected 200, got {} with body {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.text("MatchedGrantTarget").unwrap_or_default(),
                    matched,
                    "case {label:?}: expected the closest covering grant's scope, got body {}",
                    reply.body
                );
            }
            None => {
                assert_eq!(
                    reply.status, 403,
                    "case {label:?}: expected 403, got {} with body {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.code(),
                    "AccessDenied",
                    "case {label:?}: expected the AccessDenied code, got body {}",
                    reply.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_grant_belongs_to_one_account_and_region() {
    let emulator = Emulator::start();
    for region in [REGION, OTHER_REGION] {
        create_instance(&emulator, ACCOUNT, region).await;
    }
    let location = create_location(&emulator, ACCOUNT, REGION, "s3://scoped-bucket/").await;
    let location_id = location
        .text("AccessGrantsLocationId")
        .expect("the registered location's id");
    create_grant(
        &emulator,
        ACCOUNT,
        REGION,
        &location_id,
        Some("team-a/*"),
        PERMISSION_READ,
    )
    .await;
    let target = "s3://scoped-bucket/team-a/file.txt";

    let granted = get_data_access(
        &emulator,
        ACCOUNT,
        REGION,
        target,
        PERMISSION_READ,
        None,
        None,
    )
    .await;
    assert_eq!(
        granted.status, 200,
        "expected the owning account in the owning Region to be vended credentials, got {} \
         with body {}",
        granted.status, granted.body
    );

    // The instance of another Region holds no grant covering this target, so
    // the credential is not vended there — the grant is the instance's, not the
    // account's.
    let refused = get_data_access(
        &emulator,
        ACCOUNT,
        OTHER_REGION,
        target,
        PERMISSION_READ,
        None,
        None,
    )
    .await;
    assert_eq!(
        refused.status, 403,
        "expected a grant of another instance not to cover this target, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.code(),
        "AccessDenied",
        "expected the AccessDenied code, got body {}",
        refused.body
    );

    // A request addressed in another account never reaches a grant at all: it
    // is refused because of the account it names, which the refusal states by
    // naming that account.
    assert_foreign_account_refused(
        "GetDataAccess for another account",
        OTHER_ACCOUNT,
        &get_data_access(
            &emulator,
            OTHER_ACCOUNT,
            REGION,
            target,
            PERMISSION_READ,
            None,
            None,
        )
        .await,
    );
}

#[tokio::test]
async fn the_chain_refuses_what_it_cannot_carry_out() {
    let emulator = Emulator::start();

    // A location is registered IN an instance, so there is nothing to register
    // into before one exists.
    let no_instance = create_location(&emulator, ACCOUNT, REGION, "s3://early-bucket/").await;
    assert_eq!(
        no_instance.status, 404,
        "expected a register with no instance to answer 404, got {} with body {}",
        no_instance.status, no_instance.body
    );
    assert_eq!(
        no_instance.code(),
        "AccessGrantsInstanceNotExistsError",
        "expected the missing-instance code, got body {}",
        no_instance.body
    );

    // With no instance, and so no grants, no target is covered.
    let unknown_target = get_data_access(
        &emulator,
        ACCOUNT,
        REGION,
        "s3://early-bucket/key.txt",
        PERMISSION_READ,
        None,
        None,
    )
    .await;
    assert_eq!(
        unknown_target.status, 403,
        "expected a vend with no instance to answer 403, got {} with body {}",
        unknown_target.status, unknown_target.body
    );

    create_instance(&emulator, ACCOUNT, REGION).await;
    let location = create_location(&emulator, ACCOUNT, REGION, "s3://refusal-bucket/").await;
    let location_id = location
        .text("AccessGrantsLocationId")
        .expect("the registered location's id");

    // One registration per S3 path.
    let repeat = create_location(&emulator, ACCOUNT, REGION, "s3://refusal-bucket/").await;
    assert_eq!(
        repeat.status, 409,
        "expected a second registration of one scope to answer 409, got {} with body {}",
        repeat.status, repeat.body
    );
    assert_eq!(
        repeat.code(),
        "AccessGrantsLocationAlreadyExistsError",
        "expected the location-conflict code, got body {}",
        repeat.body
    );
    assert_eq!(
        get_location(&emulator, ACCOUNT, REGION, &location_id)
            .await
            .text("LocationScope")
            .unwrap_or_default(),
        "s3://refusal-bucket/",
        "expected the refused registration to leave the location alone"
    );

    // A grant's scope is computed FROM a registered location's scope, so a
    // grant over a location id that names none has no scope at all.
    let unknown_location = create_grant(
        &emulator,
        ACCOUNT,
        REGION,
        "no-such-location",
        Some("team-a/*"),
        PERMISSION_READ,
    )
    .await;
    assert_eq!(
        unknown_location.status, 404,
        "expected a grant over an unregistered location to answer 404, got {} with body {}",
        unknown_location.status, unknown_location.body
    );
    assert_eq!(
        unknown_location.code(),
        "NoSuchAccessGrantsLocationError",
        "expected the missing-location code, got body {}",
        unknown_location.body
    );

    // "If you are passing the default location, you cannot create an access
    // grant for the entire default location"
    // (`CreateAccessGrant$AccessGrantsLocationId`).
    create_location(&emulator, ACCOUNT, REGION, DEFAULT_LOCATION_SCOPE).await;
    let whole_default = create_grant(
        &emulator,
        ACCOUNT,
        REGION,
        ACCESS_GRANTS_LOCATION_DEFAULT_ID,
        None,
        PERMISSION_READ,
    )
    .await;
    assert_eq!(
        whole_default.status, 400,
        "expected a grant over the whole default location to answer 400, got {} with body {}",
        whole_default.status, whole_default.body
    );
    assert_eq!(
        whole_default.code(),
        "InvalidRequest",
        "expected the invalid-request code, got body {}",
        whole_default.body
    );
    // The refused create recorded nothing, so nothing is covered by it.
    let after_refusal = get_data_access(
        &emulator,
        ACCOUNT,
        REGION,
        "s3://anything/key.txt",
        PERMISSION_READ,
        None,
        None,
    )
    .await;
    assert_eq!(
        after_refusal.status, 403,
        "expected the refused create to have recorded no grant, got {} with body {}",
        after_refusal.status, after_refusal.body
    );

    // An id that names no grant.
    let unknown_grant = get_grant(&emulator, ACCOUNT, REGION, "no-such-grant").await;
    assert_eq!(
        unknown_grant.status, 404,
        "expected a read of an unknown grant to answer 404, got {} with body {}",
        unknown_grant.status, unknown_grant.body
    );
    assert_eq!(
        unknown_grant.code(),
        "NoSuchAccessGrantError",
        "expected the missing-grant code, got body {}",
        unknown_grant.body
    );
}
