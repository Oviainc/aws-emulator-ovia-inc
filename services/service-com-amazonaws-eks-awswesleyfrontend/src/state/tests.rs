// SPDX-License-Identifier: CC0-1.0

//! Unit cover for the rules this service's state module decides on its own: the
//! forms of the identifiers it mints and accepts, and the pass-role question.

use super::*;

/// The pass-role rule over the ARN forms it generalises over: which account and
/// partition the role belongs to, what the IAM ARN names, the role's path, and the
/// forms that are not IAM ARNs at all. The rows differ along exactly those axes.
///
/// Every failure is ONE answer — the role may not be passed — and the message says
/// which: the fixed cross-account sentence for a value that names no role of this
/// account (the recorded service answers it even for a value that is not an ARN), or
/// what else was wrong for an in-account IAM ARN.
#[test]
fn the_pass_role_rule_refuses_every_value_that_is_not_a_role_of_this_account() {
    let own = ::engine::emulator_account_id();
    /// What the row's message must be: the fixed sentence, a message naming the
    /// value, or nothing at all (the role may be passed).
    #[derive(Debug, PartialEq, Eq)]
    enum Expected {
        Passed,
        CrossAccount,
        Named,
    }
    let rows: Vec<(&str, String, Expected)> = vec![
        (
            "a role of this account",
            format!("arn:aws:iam::{own}:role/eksClusterRole"),
            Expected::Passed,
        ),
        (
            "a role of this account under a path",
            format!("arn:aws:iam::{own}:role/service-role/eksClusterRole"),
            Expected::Passed,
        ),
        (
            "a role of another account",
            "arn:aws:iam::111122223333:role/eksClusterRole".to_string(),
            Expected::CrossAccount,
        ),
        (
            "a role of another account under a path",
            "arn:aws:iam::210987654321:role/service-role/anything".to_string(),
            Expected::CrossAccount,
        ),
        (
            "a role of another partition",
            format!("arn:aws-cn:iam::{own}:role/eksClusterRole"),
            Expected::Named,
        ),
        (
            "an IAM user of this account, which no service assumes",
            format!("arn:aws:iam::{own}:user/operator"),
            Expected::Named,
        ),
        (
            "an IAM ARN of this account naming no role at all",
            format!("arn:aws:iam::{own}:role/"),
            Expected::Named,
        ),
        (
            "another vendor's ARN",
            format!("arn:aws:eks:us-west-2:{own}:cluster/other"),
            Expected::CrossAccount,
        ),
        (
            "an IAM ARN carrying a Region, which IAM ARNs do not",
            format!("arn:aws:iam:us-west-2:{own}:role/eksClusterRole"),
            Expected::CrossAccount,
        ),
        (
            "an account that is not a 12-digit account id",
            "arn:aws:iam::12345:role/short-account".to_string(),
            Expected::CrossAccount,
        ),
        (
            "an ARN that stops before its resource",
            format!("arn:aws:iam::{own}"),
            Expected::CrossAccount,
        ),
        (
            "a value that is not an ARN at all",
            "not-an-arn".to_string(),
            Expected::CrossAccount,
        ),
        ("the empty string", String::new(), Expected::CrossAccount),
    ];

    for (case, role_arn, expected) in rows {
        let refusal = pass_role_refusal(&role_arn);
        let actual = match refusal.as_deref() {
            None => Expected::Passed,
            Some(CROSS_ACCOUNT_PASS_ROLE_REFUSAL) => Expected::CrossAccount,
            Some(_) => Expected::Named,
        };
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {refusal:?} for {role_arn}"
        );
        if actual == Expected::Named {
            let message = refusal.clone().unwrap_or_default();
            assert!(
                message.contains(&role_arn),
                "case {case:?}: a refusal that is not the fixed sentence must name the \
                     value at fault, got {message:?}"
            );
        }
        // A well-formed IAM ROLE ARN of this account is always passable: the two
        // readings of that form in this crate cannot disagree.
        if is_iam_role_arn(&role_arn) && role_arn.contains(&format!(":{own}:")) {
            assert_eq!(
                pass_role_refusal(&role_arn).is_none(),
                emulator_partition() == role_arn.split(':').nth(1).unwrap_or_default(),
                "case {case:?}: {role_arn} is an IAM role ARN of this account, so only \
                     its partition may refuse it"
            );
        }
    }
}

/// IAM addresses a role by NAME, so the path is dropped — and a value that is not
/// an in-account role ARN names no role to ask IAM about.
#[test]
fn the_iam_role_name_is_the_last_segment_of_the_resource() {
    let own = ::engine::emulator_account_id();
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "the default path",
            format!("arn:aws:iam::{own}:role/Connector"),
            Some("Connector"),
        ),
        (
            "a role under a path",
            format!("arn:aws:iam::{own}:role/service-role/Connector"),
            Some("Connector"),
        ),
        (
            "a role under a nested path",
            format!("arn:aws:iam::{own}:role/team/dept/Connector"),
            Some("Connector"),
        ),
        (
            "a role of another account",
            "arn:aws:iam::111122223333:role/Connector".to_string(),
            None,
        ),
        ("not an ARN at all", "Connector".to_string(), None),
    ];
    for (case, role_arn, expected) in rows {
        let name = iam_role_name(&role_arn);
        assert_eq!(
            name, expected,
            "case {case:?}: expected {expected:?}, got {name:?} for {role_arn}"
        );
    }
}

/// The seeded calendar's own invariants: every published date parses, the rows
/// are ordered and unique, and each row's dates run release -> end of standard
/// support -> end of extended support. A typo in [`VERSION_CALENDAR`] shows up
/// here rather than as a mis-sorted page or a status nobody can explain.
#[test]
fn calendar_rows_are_well_formed() {
    let calendar = seed_cluster_versions();
    assert_eq!(
        calendar.len(),
        VERSION_CALENDAR.len(),
        "case row count: expected every published row to be seeded, got {}",
        calendar.len()
    );
    let mut seen: Vec<&str> = Vec::new();
    for row in &calendar {
        let case = row.page_key();
        assert!(
            row.release_date < row.end_of_standard_support,
            "case {case}: standard support must end after the release, got {:?} and {:?}",
            row.release_date,
            row.end_of_standard_support
        );
        assert!(
            row.end_of_standard_support < row.end_of_extended_support,
            "case {case}: extended support must end after standard support, got {:?} and \
                 {:?}",
            row.end_of_standard_support,
            row.end_of_extended_support
        );
        assert!(
            !seen.contains(&row.cluster_version.as_str()),
            "case {case}: the calendar lists this version twice, rows so far {seen:?}"
        );
        assert_eq!(
            row.kubernetes_patch_version(),
            format!("{}.0", row.cluster_version),
            "case {case}: expected the initial patch level of its own minor version, got {}",
            row.kubernetes_patch_version()
        );
        seen.push(row.cluster_version.as_str());
    }
    assert!(
        seen.windows(2).all(|pair| pair[0] > pair[1]),
        "case ordering: the calendar is published newest version first, got {seen:?}"
    );
}

/// Exactly one row is the default version, it is the version `CreateCluster`
/// creates when the caller names none — so the catalog and the cluster slice
/// cannot disagree about the default — and it is the newest published row but
/// one, which is the rule [`DEFAULT_KUBERNETES_VERSION`] states.
#[test]
fn exactly_one_row_is_the_default_version() {
    let calendar = seed_cluster_versions();
    let defaults: Vec<&str> = calendar
        .iter()
        .filter(|row| row.default_version)
        .map(|row| row.cluster_version.as_str())
        .collect();
    assert_eq!(
        defaults,
        vec![DEFAULT_KUBERNETES_VERSION],
        "case default row: expected exactly [{DEFAULT_KUBERNETES_VERSION}], got {defaults:?}"
    );
    let position = calendar
        .iter()
        .position(|row| row.default_version)
        .expect("the calendar marks one row as the default");
    assert_eq!(
        position, 1,
        "case default row position: the default is the newest published version but one, \
             got position {position} of {}",
        calendar.len()
    );
}

/// The published access-policy catalogue is sorted, unique, and every entry's
/// ARN round-trips back to the entry's own name — which is what makes a name
/// `ListAccessPolicies` reports usable as an `AssociateAccessPolicy`
/// `policyArn`.
#[test]
fn the_access_policy_catalogue_is_sorted_and_round_trips() {
    assert!(
        ACCESS_POLICY_NAMES.windows(2).all(|pair| pair[0] < pair[1]),
        "case ordering: the catalogue must be sorted and unique, got {ACCESS_POLICY_NAMES:?}"
    );
    for name in ACCESS_POLICY_NAMES {
        let arn = access_policy_arn(name);
        assert_eq!(
            arn,
            format!("{ACCESS_POLICY_ARN_PREFIX}{name}"),
            "case {name}: expected the service-owned ARN form, got {arn}"
        );
        assert_eq!(
            access_policy_name(&arn),
            Some(*name),
            "case {name}: its own ARN must resolve back to it, got {:?}",
            access_policy_name(&arn)
        );
    }
    // And an ARN whose name the catalogue does not publish resolves to nothing,
    // so `AssociateAccessPolicy` cannot accept a policy that does not exist.
    for absent in [
        "arn:aws:eks::aws:cluster-access-policy/AmazonEKSNoSuchPolicy",
        "arn:aws:eks::aws:cluster-access-policy/",
        "arn:aws:iam::aws:policy/AmazonEKSViewPolicy",
    ] {
        assert_eq!(
            access_policy_name(absent),
            None,
            "case {absent}: expected no catalogue entry, got {:?}",
            access_policy_name(absent)
        );
    }
}

/// The support status is a function of the clock and the row's two
/// end-of-support dates, in both modelled spellings.
#[test]
fn support_status_follows_the_clock() {
    let row = ClusterVersionRecord {
        cluster_version: "9.99".to_string(),
        cluster_type: CLUSTER_TYPE_EKS.to_string(),
        default_version: false,
        release_date: calendar_instant("2030-01-01"),
        end_of_standard_support: calendar_instant("2031-03-01"),
        end_of_extended_support: calendar_instant("2032-03-01"),
    };
    let day = Duration::from_secs(24 * 60 * 60);
    let cases = [
        ("the day before release", row.release_date - day, false, SupportStatus::StandardSupport),
        ("the release day", row.release_date, true, SupportStatus::StandardSupport),
        (
            "the day standard support ends",
            row.end_of_standard_support,
            true,
            SupportStatus::ExtendedSupport,
        ),
        (
            "the day before standard support ends",
            row.end_of_standard_support - day,
            true,
            SupportStatus::StandardSupport,
        ),
        (
            "the day extended support ends",
            row.end_of_extended_support,
            true,
            SupportStatus::Unsupported,
        ),
        (
            "a year after extended support ends",
            row.end_of_extended_support + 365 * day,
            true,
            SupportStatus::Unsupported,
        ),
    ];
    for (case, now, released, expected) in cases {
        assert_eq!(
            row.is_released(now),
            released,
            "case {case}: expected is_released {released}, got {}",
            row.is_released(now)
        );
        let actual = row.support_status(now);
        assert_eq!(
            actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            SupportStatus::from_version_status(actual.version_status()),
            Some(actual),
            "case {case}: the VersionStatus token must round-trip, got {}",
            actual.version_status()
        );
        assert_eq!(
            SupportStatus::from_cluster_version_status(actual.cluster_version_status()),
            Some(actual),
            "case {case}: the ClusterVersionStatus token must round-trip, got {}",
            actual.cluster_version_status()
        );
    }
}

/// The tag commits to the PAIR `(listing, last_returned)`, not to their
/// concatenation: two pairs whose concatenations are equal must still get
/// different tags, or one listing's tag could be spliced onto the other
/// listing's payload. This is the assertion the length prefix in
/// `page_token_tag` exists for — it fails on a digest of the bare
/// concatenation — and the rows are the colliding pairs the empty
/// [`CLUSTERS_LISTING`] creates for each of the other scopes.
#[test]
fn a_page_tag_separates_the_listing_from_the_key() {
    let colliding_pairs = [
        (
            (CLUSTERS_LISTING, "cluster-versionsmy-cluster"),
            (CLUSTER_VERSIONS_LISTING, "my-cluster"),
        ),
        (
            (CLUSTERS_LISTING, "access-policiesAmazonEKSViewPolicy"),
            (ACCESS_POLICIES_LISTING, "AmazonEKSViewPolicy"),
        ),
        (
            (CLUSTERS_LISTING, "cluster-versions"),
            (CLUSTER_VERSIONS_LISTING, ""),
        ),
    ];

    for ((left_listing, left_key), (right_listing, right_key)) in colliding_pairs {
        // The rows really are the ambiguous case: concatenating each pair gives
        // the same bytes, which is what an unprefixed digest would hash.
        assert_eq!(
            format!("{left_listing}{left_key}"),
            format!("{right_listing}{right_key}"),
            "case {left_listing:?}/{left_key:?}: the row must be a concatenation collision"
        );
        let left = page_token_tag(left_listing, left_key);
        let right = page_token_tag(right_listing, right_key);
        assert_ne!(
            left, right,
            "case {left_listing:?}/{left_key:?} vs {right_listing:?}/{right_key:?}: the tag \
                 must separate the listing from the key, got {left:?} for both"
        );
    }
}

/// A listing resumes from its own token and from no other listing's, over a
/// table that spans every scope this service mints tokens under.
#[test]
fn a_page_token_is_scoped_to_the_listing_that_minted_it() {
    let cases = [
        (CLUSTERS_LISTING, "my-cluster"),
        (ACCESS_POLICIES_LISTING, "AmazonEKSViewPolicy"),
        (CLUSTER_VERSIONS_LISTING, DEFAULT_KUBERNETES_VERSION),
        // Keys that carry another scope's token as a prefix, so the payload a
        // listing reads back is not confused with the scope it was minted
        // under.
        (CLUSTERS_LISTING, "cluster-versionsmy-cluster"),
        (CLUSTERS_LISTING, "access-policiesAmazonEKSViewPolicy"),
    ];

    for (listing, key) in cases {
        let token = encode_scoped_page_token(listing, key);
        assert_eq!(
            decode_scoped_page_token(listing, &token),
            Some(key.to_string()),
            "case {listing:?}/{key:?}: a listing must resume from its own token"
        );
        for other in [
            CLUSTERS_LISTING,
            ACCESS_POLICIES_LISTING,
            CLUSTER_VERSIONS_LISTING,
        ] {
            if other == listing {
                continue;
            }
            let resumed = decode_scoped_page_token(other, &token);
            assert_eq!(
                resumed, None,
                "case {listing:?}/{key:?}: expected listing {other:?} to refuse this token, \
                     got {resumed:?}"
            );
        }
    }
}
