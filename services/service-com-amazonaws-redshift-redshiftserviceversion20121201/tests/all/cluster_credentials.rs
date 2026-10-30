// SPDX-License-Identifier: CC0-1.0

//! `GetClusterCredentials`: the temporary database credentials a cluster issues,
//! end to end over the awsQuery wire.
//!
//! The rule under test: every call MINTS one set of temporary credentials for the
//! database of the cluster the request names — by `ClusterIdentifier` or by the
//! `CustomDomainName` one of this account's clusters serves — reporting the
//! request's `DbUser` prefixed `IAMA:` when its `AutoCreate` is true and `IAM:`
//! otherwise, a freshly minted password, and an `Expiration` of the call's own
//! instant plus the lifetime the request asked for inside the documented
//! 900..=3600 second window. `AutoCreate` is the one thing the call writes: it
//! creates the named database user in that cluster's database when the database
//! holds none under that name, joined to PUBLIC and to the groups the request
//! named.
//!
//! The class the rule generalises over is therefore the request's `AutoCreate`,
//! its `DurationSeconds`, its database names (`DbUser`, `DbName`, each
//! `DbGroups` entry) and the way it names the cluster, crossed with what the
//! cluster's database already holds. Each row below is one member of that class
//! rather than one recorded request.

use std::collections::{BTreeMap, BTreeSet};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// The admin user every cluster in this suite is created with, so the database
/// of each already holds one user before any credential is issued.
const MASTER_USERNAME: &str = "admin";

/// A domain name matching `CustomDomainNameString`'s modelled pattern, and an ACM
/// certificate ARN matching `CustomDomainCertificateArnString`'s, so a cluster can
/// be named the other documented way.
const CUSTOM_DOMAIN: &str = "warehouse.example.com";
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:123456789012:certificate/abc";

/// How far the reported `Expiration` may sit from `now + DurationSeconds` and
/// still be the same instant: the slack covers only the wall-clock time the call
/// itself takes.
const CLOCK_SLACK_SECONDS: i64 = 120;

/// `call` takes borrowed pairs, so a case that builds owned parameters borrows
/// them through this.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Seconds since the Unix epoch, the unit the parsed `Expiration` is compared in.
fn unix_now() -> i64 {
    DateTime::from(std::time::SystemTime::now()).secs()
}

/// Create one cluster with an admin user, which is what puts a database user in
/// its database from the start.
async fn create_cluster(emulator: &Emulator, identifier: &str) {
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", MASTER_USERNAME),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
}

/// One `GetClusterCredentials` call carrying exactly these parameters.
async fn credentials(emulator: &Emulator, params: &[(String, String)]) -> Answer {
    emulator
        .call("GetClusterCredentials", &borrowed(params))
        .await
}

/// `GetClusterCredentials` for one cluster and one database user, with nothing
/// else named.
async fn credentials_for(emulator: &Emulator, cluster: &str, db_user: &str) -> Answer {
    credentials(
        emulator,
        &[
            ("ClusterIdentifier".to_string(), cluster.to_string()),
            ("DbUser".to_string(), db_user.to_string()),
        ],
    )
    .await
}

/// The database users one cluster's database holds, each with its groups — read
/// straight out of the same shared store the handlers write, because a database
/// user is not a modelled member of any shape and so no read operation reports
/// one.
fn database_users(emulator: &Emulator, cluster: &str) -> BTreeMap<String, Vec<String>> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store
        .cluster(cluster)
        .map(|stored| {
            stored
                .database_users
                .iter()
                .map(|(user, groups)| (user.clone(), groups.iter().cloned().collect()))
                .collect()
        })
        .unwrap_or_default()
}

/// The `Expiration` a response reports, in seconds since the Unix epoch.
fn expiration_secs(case: &str, answered: &Answer) -> i64 {
    let rendered = answered
        .text("Expiration")
        .unwrap_or_else(|| panic!("case {case}: expected an Expiration in body {}", answered.body));
    DateTime::from_str(&rendered, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case}: expected an ISO 8601 Expiration, got {rendered:?} ({error})")
        })
        .secs()
}

/// One row of the issuance table: the case name, the `AutoCreate` the call
/// carries (`None` when it carries none), the `DbUser`, the `DbGroups`, the
/// `DurationSeconds` (`None` when the call names none), the prefix the reported
/// `DbUser` must carry and the lifetime the `Expiration` must sit at.
type IssuanceCase = (
    &'static str,
    Option<bool>,
    &'static str,
    &'static [&'static str],
    Option<i32>,
    &'static str,
    i64,
);

#[tokio::test]
async fn every_call_mints_credentials_the_documented_shape() {
    let emulator = Emulator::new();
    let cluster = "credentials-cluster";
    create_cluster(&emulator, cluster).await;

    // The rows differ along everything the rule generalises over: whether
    // `AutoCreate` is named and what it says, the user named (the admin the
    // database already holds, a user it does not, and the documented punctuation
    // a user name may carry), how many groups the session joins, and whether a
    // lifetime is named at all.
    let cases: Vec<IssuanceCase> = vec![
        (
            "a request naming only the cluster and the user",
            None,
            MASTER_USERNAME,
            &[],
            None,
            "IAM:",
            i64::from(state::CREDENTIAL_DURATION_DEFAULT_SECONDS),
        ),
        (
            "AutoCreate false, at the documented minimum lifetime",
            Some(false),
            "reporting_user",
            &[],
            Some(900),
            "IAM:",
            900,
        ),
        (
            "AutoCreate true, joining one group",
            Some(true),
            "analyst",
            &["analysts"],
            Some(1800),
            "IAMA:",
            1800,
        ),
        (
            "AutoCreate true, joining three groups at the documented maximum lifetime",
            Some(true),
            "Loader.Service+1@example-corp",
            &["analysts", "loaders", "reporting_1"],
            Some(3600),
            "IAMA:",
            3600,
        ),
        (
            "a lifetime inside the window, for a user the database already holds",
            Some(false),
            MASTER_USERNAME,
            &["analysts"],
            Some(1234),
            "IAM:",
            1234,
        ),
    ];

    // A password is minted per call, so no two calls may answer the same one.
    let mut seen_passwords: BTreeSet<String> = BTreeSet::new();

    for (case, auto_create, db_user, groups, duration, prefix, lifetime) in cases {
        let mut params: Vec<(String, String)> = vec![
            ("ClusterIdentifier".to_string(), cluster.to_string()),
            ("DbUser".to_string(), db_user.to_string()),
            ("DbName".to_string(), "dev".to_string()),
        ];
        if let Some(auto_create) = auto_create {
            params.push(("AutoCreate".to_string(), auto_create.to_string()));
        }
        if let Some(duration) = duration {
            params.push(("DurationSeconds".to_string(), duration.to_string()));
        }
        for (position, group) in groups.iter().enumerate() {
            params.push((
                format!("DbGroups.DbGroup.{}", position + 1),
                (*group).to_string(),
            ));
        }

        let before = unix_now();
        let answered = credentials(&emulator, &params).await;
        let after = unix_now();
        assert_eq!(
            answered.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answered.status, answered.body
        );

        let expected_user = format!("{prefix}{db_user}");
        assert_eq!(
            answered.text("DbUser").as_deref(),
            Some(expected_user.as_str()),
            "case {case}: expected DbUser={expected_user:?}, got {:?}",
            answered.text("DbUser")
        );

        // The password is a secret, so only its length and its distinctness are
        // ever named here — never the value itself.
        let password = answered.text("DbPassword").unwrap_or_else(|| {
            panic!("case {case}: expected a DbPassword in body {}", answered.body)
        });
        assert!(
            !password.is_empty() && !password.chars().any(char::is_whitespace),
            "case {case}: expected a non-empty whitespace-free DbPassword, got one of length {}",
            password.len()
        );
        assert!(
            seen_passwords.insert(password.clone()),
            "case {case}: expected a freshly minted password, got one an earlier call \
             already answered (length {})",
            password.len()
        );

        let expiration = expiration_secs(case, &answered);
        assert!(
            (before + lifetime..=after + lifetime + CLOCK_SLACK_SECONDS).contains(&expiration),
            "case {case}: expected an Expiration {lifetime} seconds after the call \
             (between {} and {}), got {expiration}",
            before + lifetime,
            after + lifetime + CLOCK_SLACK_SECONDS
        );
    }

    // The admin user the cluster was created with was never replaced by an
    // issuance, and the one user `AutoCreate` asked for is the only one added.
    let users = database_users(&emulator, cluster);
    assert_eq!(
        users.keys().cloned().collect::<Vec<String>>(),
        vec![
            "Loader.Service+1@example-corp".to_string(),
            MASTER_USERNAME.to_string(),
            "analyst".to_string(),
        ],
        "case the database after the table: expected only the admin and the auto-created \
         users, got {:?}",
        users.keys().collect::<Vec<&String>>()
    );
}

#[tokio::test]
async fn the_lifetime_window_is_the_documented_one() {
    let emulator = Emulator::new();
    let cluster = "duration-cluster";
    create_cluster(&emulator, cluster).await;

    let min = state::CREDENTIAL_DURATION_MIN_SECONDS;
    let max = state::CREDENTIAL_DURATION_MAX_SECONDS;

    // Both published bounds, one past each, and the values a caller reaches for
    // when it means "no limit".
    let cases: Vec<(&str, i32, bool)> = vec![
        ("the published minimum", min, true),
        ("one second under the published minimum", min - 1, false),
        ("the published maximum", max, true),
        ("one second over the published maximum", max + 1, false),
        ("a lifetime inside the window", (min + max) / 2, true),
        ("no lifetime at all", 0, false),
        ("a negative lifetime", -1, false),
        ("a day", 86_400, false),
    ];

    for (case, duration, accepted) in cases {
        let answered = credentials(
            &emulator,
            &[
                ("ClusterIdentifier".to_string(), cluster.to_string()),
                ("DbUser".to_string(), MASTER_USERNAME.to_string()),
                ("DurationSeconds".to_string(), duration.to_string()),
            ],
        )
        .await;
        if accepted {
            assert_eq!(
                answered.status, 200,
                "case {case} ({duration}s): expected 200, got {} with body {}",
                answered.status, answered.body
            );
            let expiration = expiration_secs(case, &answered);
            let now = unix_now();
            assert!(
                (now + i64::from(duration) - CLOCK_SLACK_SECONDS
                    ..=now + i64::from(duration) + CLOCK_SLACK_SECONDS)
                    .contains(&expiration),
                "case {case} ({duration}s): expected an Expiration about {duration} seconds \
                 after the call, got {expiration} against a clock reading {now}"
            );
        } else {
            assert_eq!(
                (answered.status, answered.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case {case} ({duration}s): expected 400 InvalidParameterValue, got {} / {:?} \
                 with body {}",
                answered.status,
                answered.error_code(),
                answered.body
            );
            let message = answered.text("Message").unwrap_or_default();
            assert!(
                message.contains(&min.to_string()) && message.contains(&max.to_string()),
                "case {case} ({duration}s): expected the refusal to state the {min}..{max} \
                 window, got {message:?}"
            );
        }
    }
}

/// One row of the database-name table: the case name, the member the value
/// arrives on, the value, and whether the documented syntax admits it.
type NameCase = (&'static str, &'static str, String, bool);

#[tokio::test]
async fn database_names_outside_the_documented_syntax_are_refused() {
    let emulator = Emulator::new();
    let cluster = "names-cluster";
    create_cluster(&emulator, cluster).await;

    let longest = "u".repeat(state::DATABASE_NAME_MAX_LEN);
    let too_long = "u".repeat(state::DATABASE_NAME_MAX_LEN + 1);

    // `DbUser` and `DbName` admit either case; `DbGroups` entries admit only
    // lowercase. All three admit the same punctuation, are 1 to 64 characters,
    // must start with a letter, and admit neither a colon nor a slash. `DbUser`
    // alone cannot be PUBLIC.
    let cases: Vec<NameCase> = vec![
        ("a plain user", "DbUser", "awsuser".to_string(), true),
        (
            "a user carrying every documented punctuation mark",
            "DbUser",
            "Data_Loader+1.svc@example-corp".to_string(),
            true,
        ),
        ("a user at the longest documented length", "DbUser", longest.clone(), true),
        ("a user one character too long", "DbUser", too_long.clone(), false),
        ("an empty user", "DbUser", String::new(), false),
        ("the PUBLIC group as a user", "DbUser", "PUBLIC".to_string(), false),
        ("the PUBLIC group folded to lower case", "DbUser", "public".to_string(), false),
        ("a user starting with a digit", "DbUser", "1analyst".to_string(), false),
        ("a user carrying a colon", "DbUser", "iam:analyst".to_string(), false),
        ("a user carrying a slash", "DbUser", "role/analyst".to_string(), false),
        ("a user carrying a space", "DbUser", "two words".to_string(), false),
        ("a user carrying a character outside the alphabet", "DbUser", "anal#yst".to_string(), false),
        ("a plain database", "DbName", "dev".to_string(), true),
        ("a mixed-case database", "DbName", "Warehouse_1".to_string(), true),
        ("a database one character too long", "DbName", too_long.clone(), false),
        ("a database starting with a digit", "DbName", "1dev".to_string(), false),
        ("a database carrying a slash", "DbName", "dev/main".to_string(), false),
        ("a lowercase group", "DbGroups", "analysts".to_string(), true),
        (
            "a group carrying every documented punctuation mark",
            "DbGroups",
            "grp_1+2.3@example-corp".to_string(),
            true,
        ),
        ("a group carrying an uppercase letter", "DbGroups", "Analysts".to_string(), false),
        ("a group starting with a digit", "DbGroups", "1analysts".to_string(), false),
        ("a group carrying a colon", "DbGroups", "grp:1".to_string(), false),
        ("a group one character too long", "DbGroups", too_long.clone(), false),
    ];

    for (case, member, value, admitted) in cases {
        let mut params: Vec<(String, String)> = vec![
            ("ClusterIdentifier".to_string(), cluster.to_string()),
            ("DbUser".to_string(), MASTER_USERNAME.to_string()),
        ];
        match member {
            // One group the rule admits stands first, so a refused second entry
            // shows that EVERY entry is checked rather than only the first.
            "DbGroups" => {
                params.push(("DbGroups.DbGroup.1".to_string(), "loaders".to_string()));
                params.push(("DbGroups.DbGroup.2".to_string(), value.clone()));
            }
            "DbUser" => {
                params.retain(|(name, _)| name != "DbUser");
                params.push(("DbUser".to_string(), value.clone()));
            }
            other => params.push((other.to_string(), value.clone())),
        }

        let answered = credentials(&emulator, &params).await;
        if admitted {
            assert_eq!(
                answered.status, 200,
                "case {case} ({member}={value:?}): expected 200, got {} with body {}",
                answered.status, answered.body
            );
        } else {
            assert_eq!(
                (answered.status, answered.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case {case} ({member}={value:?}): expected 400 InvalidParameterValue, \
                 got {} / {:?} with body {}",
                answered.status,
                answered.error_code(),
                answered.body
            );
            let message = answered.text("Message").unwrap_or_default();
            assert!(
                message.contains(member),
                "case {case} ({member}={value:?}): expected the refusal to name the member, \
                 got {message:?}"
            );
        }
    }
}

/// One row of the resolution-refusal table: the case name, the members the call
/// carries, and the status, error code and message it must be refused with.
type ResolutionRefusalCase = (
    &'static str,
    Vec<(String, String)>,
    u16,
    &'static str,
    Option<String>,
);

#[tokio::test]
async fn the_cluster_is_resolved_by_either_name() {
    let emulator = Emulator::new();
    let cluster = "resolution-cluster";
    let other_cluster = "other-cluster";
    create_cluster(&emulator, cluster).await;
    create_cluster(&emulator, other_cluster).await;
    let associated = emulator
        .call(
            "CreateCustomDomainAssociation",
            &[
                ("ClusterIdentifier", cluster),
                ("CustomDomainName", CUSTOM_DOMAIN),
                ("CustomDomainCertificateArn", CERTIFICATE_ARN),
            ],
        )
        .await;
    assert_eq!(
        associated.status, 200,
        "case seed custom domain association: expected 200, got {} with body {}",
        associated.status, associated.body
    );

    // The identifier alone, the domain alone, and both together naming the same
    // cluster all name that one cluster.
    for (case, params) in [
        (
            "the cluster identifier alone",
            vec![("ClusterIdentifier".to_string(), cluster.to_string())],
        ),
        (
            "the custom domain name alone",
            vec![("CustomDomainName".to_string(), CUSTOM_DOMAIN.to_string())],
        ),
        (
            "both members naming the same cluster",
            vec![
                ("ClusterIdentifier".to_string(), cluster.to_string()),
                ("CustomDomainName".to_string(), CUSTOM_DOMAIN.to_string()),
            ],
        ),
    ] {
        let mut params = params;
        params.push(("DbUser".to_string(), MASTER_USERNAME.to_string()));
        let answered = credentials(&emulator, &params).await;
        assert_eq!(
            answered.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.text("DbUser").as_deref(),
            Some(format!("IAM:{MASTER_USERNAME}").as_str()),
            "case {case}: expected the reported DbUser to carry the IAM: prefix, got {:?}",
            answered.text("DbUser")
        );
    }

    // A name that resolves to no cluster of this account issues nothing.
    let cases: Vec<ResolutionRefusalCase> = vec![
        (
            "an identifier naming no cluster",
            vec![("ClusterIdentifier".to_string(), "no-such-cluster".to_string())],
            404,
            "ClusterNotFound",
            Some("Cluster no-such-cluster not found.".to_string()),
        ),
        (
            "a custom domain name no cluster serves",
            vec![("CustomDomainName".to_string(), "absent.example.com".to_string())],
            404,
            "ClusterNotFound",
            Some("CustomDomainAssociation not found: absent.example.com".to_string()),
        ),
        (
            "both members naming different clusters",
            vec![
                ("ClusterIdentifier".to_string(), other_cluster.to_string()),
                ("CustomDomainName".to_string(), CUSTOM_DOMAIN.to_string()),
            ],
            404,
            "ClusterNotFound",
            Some(format!(
                "CustomDomainAssociation not found for cluster: {other_cluster}"
            )),
        ),
        (
            "neither member, so no cluster is named at all",
            Vec::new(),
            400,
            "InvalidParameterCombination",
            None,
        ),
    ];

    for (case, params, status, code, message) in cases {
        let mut params = params;
        params.push(("DbUser".to_string(), MASTER_USERNAME.to_string()));
        let answered = credentials(&emulator, &params).await;
        assert_eq!(
            (answered.status, answered.error_code()),
            (status, Some(code.to_string())),
            "case {case}: expected {status} {code}, got {} / {:?} with body {}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        if let Some(message) = message {
            assert_eq!(
                answered.text("Message").as_deref(),
                Some(message.as_str()),
                "case {case}: expected the refusal to read {message:?}, got {:?}",
                answered.text("Message")
            );
        }
        assert_eq!(
            answered.text("DbPassword"),
            None,
            "case {case}: expected a refusal to issue no password at all, got a body of \
             length {}",
            answered.body.len()
        );
    }
}

#[tokio::test]
async fn what_one_request_can_add_to_the_store_is_bounded() {
    let emulator = Emulator::new();
    let cluster = "bounded-cluster";
    create_cluster(&emulator, cluster).await;

    // How many groups one session may join is bounded, so one request cannot
    // copy an unbounded list into a created user's memberships. Both sides of
    // the bound, and a short list for contrast.
    for (case, count, accepted) in [
        ("a short group list", 3usize, true),
        ("a group list at the bound", state::MAX_DB_GROUPS_PER_SESSION, true),
        (
            "a group list one past the bound",
            state::MAX_DB_GROUPS_PER_SESSION + 1,
            false,
        ),
    ] {
        let mut params: Vec<(String, String)> = vec![
            ("ClusterIdentifier".to_string(), cluster.to_string()),
            ("DbUser".to_string(), format!("joiner_{count}")),
            ("AutoCreate".to_string(), "true".to_string()),
        ];
        for position in 1..=count {
            params.push((
                format!("DbGroups.DbGroup.{position}"),
                format!("grp_{position}"),
            ));
        }
        let answered = credentials(&emulator, &params).await;
        if accepted {
            assert_eq!(
                answered.status, 200,
                "case {case} ({count} groups): expected 200, got {} with body {}",
                answered.status, answered.body
            );
            let joined = database_users(&emulator, cluster)
                .get(&format!("joiner_{count}"))
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                joined,
                count + 1,
                "case {case} ({count} groups): expected the created user to join the {count} \
                 groups and PUBLIC, got {joined} memberships"
            );
        } else {
            assert_eq!(
                (answered.status, answered.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case {case} ({count} groups): expected 400 InvalidParameterValue, got {} / \
                 {:?} with body {}",
                answered.status,
                answered.error_code(),
                answered.body
            );
            assert!(
                !database_users(&emulator, cluster).contains_key(&format!("joiner_{count}")),
                "case {case} ({count} groups): expected the refusal to create no user, got {:?}",
                database_users(&emulator, cluster)
                    .keys()
                    .collect::<Vec<&String>>()
            );
        }
    }

    // How many users one cluster's database holds is bounded too. The database
    // is filled to the quota straight in the store — a request per user would
    // say nothing more about the rule than the boundary does.
    {
        let store = state::service_state(&*emulator.engine).expect("the Redshift store");
        let mut store = store.lock();
        let stored = store
            .clusters
            .iter_mut()
            .find(|stored| stored.cluster.cluster_identifier.as_deref() == Some(cluster))
            .expect("the seeded cluster");
        for seeded in 0..state::DATABASE_USERS_PER_CLUSTER_QUOTA {
            stored.database_users.insert(
                format!("seeded_user_{seeded}"),
                BTreeSet::from([state::PUBLIC_DATABASE_GROUP.to_string()]),
            );
        }
    }
    let held = database_users(&emulator, cluster).len();
    assert!(
        held >= state::DATABASE_USERS_PER_CLUSTER_QUOTA,
        "case a database filled to the quota: expected at least {} users, got {held}",
        state::DATABASE_USERS_PER_CLUSTER_QUOTA
    );

    let past_quota = credentials(
        &emulator,
        &[
            ("ClusterIdentifier".to_string(), cluster.to_string()),
            ("DbUser".to_string(), "one_user_too_many".to_string()),
            ("AutoCreate".to_string(), "true".to_string()),
        ],
    )
    .await;
    assert_eq!(
        (past_quota.status, past_quota.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case AutoCreate past the users-per-database quota: expected 400 \
         InvalidParameterValue, got {} / {:?} with body {}",
        past_quota.status,
        past_quota.error_code(),
        past_quota.body
    );
    assert_eq!(
        database_users(&emulator, cluster).len(),
        held,
        "case AutoCreate past the users-per-database quota: expected the database to be \
         unchanged at {held} users, got {}",
        database_users(&emulator, cluster).len()
    );

    // The bound stops the database GROWING, and nothing else: a user it already
    // holds, and a request that asks for no user at all, are still served.
    for (case, db_user, auto_create) in [
        ("a user the full database already holds", "seeded_user_0", Some("true")),
        ("a new user, with no AutoCreate", "one_user_too_many", None),
    ] {
        let mut params: Vec<(String, String)> = vec![
            ("ClusterIdentifier".to_string(), cluster.to_string()),
            ("DbUser".to_string(), db_user.to_string()),
        ];
        if let Some(auto_create) = auto_create {
            params.push(("AutoCreate".to_string(), auto_create.to_string()));
        }
        let answered = credentials(&emulator, &params).await;
        assert_eq!(
            answered.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answered.status, answered.body
        );
    }
}

#[tokio::test]
async fn auto_create_creates_one_database_user_and_leaves_an_existing_one() {
    let emulator = Emulator::new();
    let cluster = "auto-create-cluster";
    let bystander = "bystander-cluster";
    create_cluster(&emulator, cluster).await;
    create_cluster(&emulator, bystander).await;

    // The admin user the create named is a user of the cluster's database from
    // the start, in PUBLIC.
    assert_eq!(
        database_users(&emulator, cluster),
        BTreeMap::from([(
            MASTER_USERNAME.to_string(),
            vec![state::PUBLIC_DATABASE_GROUP.to_string()],
        )]),
        "case a freshly created cluster: expected its database to hold only the admin user, \
         got {:?}",
        database_users(&emulator, cluster)
    );

    // A call that does not ask for a user creates none, and still succeeds: the
    // credentials are issued, and the connection they are for is what fails.
    for (case, auto_create) in [("no AutoCreate at all", None), ("AutoCreate false", Some(false))] {
        let mut params: Vec<(String, String)> = vec![
            ("ClusterIdentifier".to_string(), cluster.to_string()),
            ("DbUser".to_string(), "absent_user".to_string()),
        ];
        if let Some(auto_create) = auto_create {
            params.push(("AutoCreate".to_string(), auto_create.to_string()));
        }
        let answered = credentials(&emulator, &params).await;
        assert_eq!(
            answered.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answered.status, answered.body
        );
        let users = database_users(&emulator, cluster);
        assert!(
            !users.contains_key("absent_user"),
            "case {case}: expected no database user to be created, got {:?}",
            users.keys().collect::<Vec<&String>>()
        );
    }

    // `AutoCreate` true creates the user, joined to PUBLIC and to the groups the
    // request named.
    let created = credentials(
        &emulator,
        &[
            ("ClusterIdentifier".to_string(), cluster.to_string()),
            ("DbUser".to_string(), "analyst".to_string()),
            ("AutoCreate".to_string(), "true".to_string()),
            ("DbGroups.DbGroup.1".to_string(), "analysts".to_string()),
            ("DbGroups.DbGroup.2".to_string(), "loaders".to_string()),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case AutoCreate true for a user the database does not hold: expected 200, got {} \
         with body {}",
        created.status, created.body
    );
    let expected_new_user = vec![
        state::PUBLIC_DATABASE_GROUP.to_string(),
        "analysts".to_string(),
        "loaders".to_string(),
    ];
    assert_eq!(
        database_users(&emulator, cluster).get("analyst"),
        Some(&expected_new_user),
        "case AutoCreate true for a user the database does not hold: expected the user to \
         join {expected_new_user:?}, got {:?}",
        database_users(&emulator, cluster).get("analyst")
    );

    // A second issuance for a user the database already holds changes nothing
    // about that user: the groups a request names are the SESSION's, in addition
    // to the memberships the user has.
    for (case, db_user, groups, expected) in [
        (
            "a second AutoCreate for the same user, naming another group",
            "analyst",
            &["reporting"][..],
            expected_new_user.clone(),
        ),
        (
            "AutoCreate for the admin user the cluster was created with",
            MASTER_USERNAME,
            &["analysts"][..],
            vec![state::PUBLIC_DATABASE_GROUP.to_string()],
        ),
    ] {
        let mut params: Vec<(String, String)> = vec![
            ("ClusterIdentifier".to_string(), cluster.to_string()),
            ("DbUser".to_string(), db_user.to_string()),
            ("AutoCreate".to_string(), "true".to_string()),
        ];
        for (position, group) in groups.iter().enumerate() {
            params.push((
                format!("DbGroups.DbGroup.{}", position + 1),
                (*group).to_string(),
            ));
        }
        let answered = credentials(&emulator, &params).await;
        assert_eq!(
            answered.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answered.status, answered.body
        );
        assert_eq!(
            database_users(&emulator, cluster).get(db_user),
            Some(&expected),
            "case {case}: expected the existing user to keep {expected:?}, got {:?}",
            database_users(&emulator, cluster).get(db_user)
        );
    }

    // The user is created in the database of the cluster the request named, and
    // of no other.
    assert_eq!(
        database_users(&emulator, bystander),
        BTreeMap::from([(
            MASTER_USERNAME.to_string(),
            vec![state::PUBLIC_DATABASE_GROUP.to_string()],
        )]),
        "case another cluster of the same account: expected its database to be untouched, \
         got {:?}",
        database_users(&emulator, bystander)
    );

    // A refused call writes nothing either.
    let refused = credentials_for(&emulator, cluster, "PUBLIC").await;
    assert_eq!(
        refused.status, 400,
        "case a refused request: expected 400, got {} with body {}",
        refused.status, refused.body
    );
    assert!(
        !database_users(&emulator, cluster).contains_key("PUBLIC"),
        "case a refused request: expected nothing to be written, got {:?}",
        database_users(&emulator, cluster)
            .keys()
            .collect::<Vec<&String>>()
    );
}
