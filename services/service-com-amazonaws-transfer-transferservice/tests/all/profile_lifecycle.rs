// SPDX-License-Identifier: CC0-1.0

//! The AS2 profile lifecycle: `CreateProfile` mints and persists, `DescribeProfile`
//! reports what was persisted.
//!
//! RULE: `CreateProfile` mints a fresh system-assigned `ProfileId` of the modelled
//! shape for EVERY call and stores the configuration the request carried, so
//! `DescribeProfile` answers that profile's own ARN and the stored `As2Id`,
//! `ProfileType`, `CertificateIds` and `Tags` unchanged — and a `ProfileId` the store
//! does not hold is answered with the modelled not-found error rather than a profile.
//!
//! The rows differ along what the rule generalises over: the `ProfileType` value, the
//! form and the length of `As2Id`, and the COUNT of `CertificateIds` and of `Tags`
//! (none, one, several), so an absent optional list and a populated one are both
//! covered.

use serde_json::{json, Value};

use crate::harness::{
    assert_members, is_modelled_profile_id, member_of, Emulator, ACCOUNT, CERTIFICATE_IDS, REGION,
};

/// One `CreateProfile` request, as the configuration it is built from.
struct Case {
    /// What this row varies, named in every assertion it makes.
    name: &'static str,
    /// `As2Id`: modelled `len 1..128`, pattern `^[\u{0020}-\u{007E}\s]*$`.
    as2_id: String,
    /// `ProfileType`: the modelled enum's `LOCAL` | `PARTNER`.
    profile_type: &'static str,
    certificate_ids: Vec<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

impl Case {
    /// The `CreateProfile` body: an EMPTY list is sent as no member at all, so the row
    /// exercises an absent optional list rather than an empty one.
    fn request(&self) -> Value {
        let mut body = json!({"As2Id": self.as2_id, "ProfileType": self.profile_type});
        let object = body.as_object_mut().expect("the request body is an object");
        if !self.certificate_ids.is_empty() {
            object.insert("CertificateIds".to_string(), json!(self.certificate_ids));
        }
        if !self.tags.is_empty() {
            object.insert("Tags".to_string(), self.tags_json());
        }
        body
    }

    /// `Tags` on the wire: a list of `{Key, Value}` structures.
    fn tags_json(&self) -> Value {
        Value::Array(
            self.tags
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        )
    }

    /// What `DescribeProfile` must report for the profile this row created. A `null`
    /// expectation means the member must be absent.
    fn expected_members(&self) -> Value {
        json!({
            "As2Id": self.as2_id,
            "ProfileType": self.profile_type,
            // Both are collections the profile always holds, so a profile created
            // without certificates or without tags reports them EMPTY, not absent.
            "CertificateIds": json!(self.certificate_ids),
            "Tags": self.tags_json(),
        })
    }
}

/// The table: every row differs from the others in the profile type, the form or
/// length of the AS2-name, or the number of certificate ids or tags it carries.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a LOCAL profile with nothing optional",
            as2_id: "local-party".to_string(),
            profile_type: "LOCAL",
            certificate_ids: Vec::new(),
            tags: Vec::new(),
        },
        Case {
            name: "a PARTNER profile with nothing optional",
            as2_id: "partner-party".to_string(),
            profile_type: "PARTNER",
            certificate_ids: Vec::new(),
            tags: Vec::new(),
        },
        Case {
            name: "the shortest As2Id the model allows (len 1)",
            as2_id: "A".to_string(),
            profile_type: "LOCAL",
            certificate_ids: Vec::new(),
            tags: Vec::new(),
        },
        Case {
            name: "an As2Id at the modelled length bound (len 128)",
            as2_id: "a".repeat(128),
            profile_type: "PARTNER",
            certificate_ids: Vec::new(),
            tags: Vec::new(),
        },
        Case {
            name: "an As2Id of punctuation the modelled pattern allows",
            as2_id: "AS2-From_<name>.v2+1/#9".to_string(),
            profile_type: "LOCAL",
            certificate_ids: Vec::new(),
            tags: Vec::new(),
        },
        Case {
            name: "one certificate id",
            as2_id: "one-cert".to_string(),
            profile_type: "PARTNER",
            certificate_ids: vec![CERTIFICATE_IDS[1]],
            tags: Vec::new(),
        },
        Case {
            name: "three certificate ids, order preserved",
            as2_id: "three-certs".to_string(),
            profile_type: "PARTNER",
            certificate_ids: CERTIFICATE_IDS.to_vec(),
            tags: Vec::new(),
        },
        Case {
            name: "one tag",
            as2_id: "one-tag".to_string(),
            profile_type: "LOCAL",
            certificate_ids: Vec::new(),
            tags: vec![("env", "prod")],
        },
        Case {
            name: "two tags and two certificate ids",
            as2_id: "tags and certs".to_string(),
            profile_type: "LOCAL",
            certificate_ids: vec![CERTIFICATE_IDS[0], CERTIFICATE_IDS[2]],
            tags: vec![("env", "test"), ("owner", "as2-team")],
        },
    ]
}

/// Create the profile `case` describes and return its `ProfileId`.
///
/// The create-and-assert-200 walk itself is [`Emulator::create_profile`], shared with
/// every other profile suite; this adds only the modelled-id check and names the case.
async fn create_profile(emulator: &Emulator, case: &Case) -> String {
    let profile_id = emulator.create_profile(case.request()).await;
    assert!(
        is_modelled_profile_id(&profile_id),
        "case {:?}: expected a ProfileId matching ^p-([0-9a-f]{{17}})$, got {profile_id:?}",
        case.name
    );
    profile_id
}

/// RULE: each create mints a modelled `ProfileId` and persists its configuration, and
/// `DescribeProfile` reports that configuration under the profile's own ARN.
#[tokio::test]
async fn created_profiles_are_described_from_stored_state() {
    let emulator = Emulator::new();
    let cases = cases();

    // Create every row first, so each describe below reads a store holding all of
    // them: a profile must be reported from its OWN record, not from the last write.
    let mut created: Vec<(&Case, String)> = Vec::new();
    for case in &cases {
        let profile_id = create_profile(&emulator, case).await;
        created.push((case, profile_id));
    }

    for (case, profile_id) in &created {
        let answer = emulator
            .call("DescribeProfile", json!({"ProfileId": profile_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected DescribeProfile 200, got {} with body {}",
            case.name, answer.status, answer.body
        );

        let profile = answer.get("Profile");
        assert_members(case.name, profile, &case.expected_members());

        let reported_id = member_of(profile, "ProfileId").as_str().unwrap_or_default();
        assert_eq!(
            reported_id, profile_id,
            "case {:?}: expected the described ProfileId to be {profile_id:?}, got {reported_id:?}",
            case.name
        );

        let expected_arn = format!("arn:aws:transfer:{REGION}:{ACCOUNT}:profile/{profile_id}");
        let arn = member_of(profile, "Arn").as_str().unwrap_or_default();
        assert_eq!(
            arn, expected_arn,
            "case {:?}: expected Arn {expected_arn:?}, got {arn:?}",
            case.name
        );

        // A read may not change what a later caller sees: the same describe answers
        // the same body.
        let again = emulator
            .call("DescribeProfile", json!({"ProfileId": profile_id}))
            .await;
        assert_eq!(
            again.body, answer.body,
            "case {:?}: expected a repeated DescribeProfile to answer the same body, got {} then {}",
            case.name, answer.body, again.body
        );
    }

    // Every minted id is distinct: a profile is addressed by its own identifier.
    let mut ids: Vec<&str> = created.iter().map(|(_, id)| id.as_str()).collect();
    ids.sort_unstable();
    let minted = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "expected {minted} distinct ProfileIds, got {ids:?}"
    );
}

/// RULE (minting): two creates carrying the SAME configuration are two independent
/// profiles, each describable on its own.
#[tokio::test]
async fn identical_create_requests_mint_independent_profiles() {
    let emulator = Emulator::new();

    for case in cases() {
        let first = create_profile(&emulator, &case).await;
        let second = create_profile(&emulator, &case).await;
        assert_ne!(
            first, second,
            "case {:?}: expected two creates of the same configuration to mint two ids, got {first:?} twice",
            case.name
        );

        for profile_id in [&first, &second] {
            let answer = emulator
                .call("DescribeProfile", json!({"ProfileId": profile_id}))
                .await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected DescribeProfile 200 for {profile_id}, got {} with body {}",
                case.name, answer.status, answer.body
            );
            assert_members(case.name, answer.get("Profile"), &case.expected_members());
        }
    }
}

/// RULE (not found): a well-formed `ProfileId` the store does not hold is answered
/// with the service's generic not-found error, whose `Message` carries no identifier.
///
/// The rows differ in WHERE in the id space the probe sits, and in whether any profile
/// exists at all.
#[tokio::test]
async fn describing_a_profile_the_store_does_not_hold_answers_not_found() {
    let emulator = Emulator::new();

    // Syntactically valid `ProfileId`s (`^p-([0-9a-f]{17})$`) that no create minted:
    // the low end of the id space, a mid value, and the high end.
    const NEVER_MINTED: [&str; 3] = [
        "p-00000000000000000",
        "p-0000000000000dead",
        "p-fffffffffffffffff",
    ];

    let mut messages: Vec<(String, String)> = Vec::new();

    for store in ["an empty store", "a store holding one profile"] {
        if store == "a store holding one profile" {
            create_profile(&emulator, &cases()[0]).await;
        }
        for absent in NEVER_MINTED {
            assert!(
                is_modelled_profile_id(absent),
                "the probe id {absent:?} must satisfy ProfileId's modelled pattern"
            );
            let case = format!("DescribeProfile for {absent} against {store}");
            let answer = emulator
                .call("DescribeProfile", json!({"ProfileId": absent}))
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {}",
                answer.body
            );
            let resource_type = member_of(&answer.body, "ResourceType")
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                resource_type, "Profile",
                "case {case:?}: expected ResourceType \"Profile\", got {resource_type:?}"
            );
            let resource = member_of(&answer.body, "Resource").as_str().unwrap_or_default();
            assert_eq!(
                resource, absent,
                "case {case:?}: expected the requested id in Resource, got {resource:?}"
            );
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !message.is_empty(),
                "case {case:?}: Message is modelled required, got body {}",
                answer.body
            );
            assert!(
                !message.contains(absent),
                "case {case:?}: expected no identifier in Message, got {message:?}"
            );
            messages.push((case, message));
        }
    }

    // One fixed message for the resource type, whatever the probe.
    let (first_case, first) = messages.first().expect("every row produced a message");
    for (case, message) in &messages {
        assert_eq!(
            message, first,
            "case {case:?}: expected the same fixed Profile message as case {first_case:?} \
             ({first:?}), got {message:?}"
        );
    }
}

/// RULE (ARN): the profile's ARN carries the region the caller signed the create for,
/// and the describe reports that stored ARN whatever region it is read from.
#[tokio::test]
async fn a_profiles_arn_carries_the_region_it_was_created_in() {
    let emulator = Emulator::new();

    // Rows: regions in three partitions' naming styles, none of them the handler's
    // fallback default.
    for region in ["eu-west-2", "ap-southeast-1", "us-west-2"] {
        let case = cases().remove(0);
        let answer = emulator
            .call_in(region, "CreateProfile", case.request())
            .await;
        assert_eq!(
            answer.status, 200,
            "region {region}: expected CreateProfile 200, got {} with body {}",
            answer.status, answer.body
        );
        let profile_id = answer
            .get("ProfileId")
            .as_str()
            .unwrap_or_else(|| panic!("region {region}: no ProfileId in {}", answer.body))
            .to_string();

        let expected_arn = format!("arn:aws:transfer:{region}:{ACCOUNT}:profile/{profile_id}");
        // Read from a DIFFERENT region than the create: the ARN is the stored one.
        for reader in [region, REGION] {
            let described = emulator
                .call_in(reader, "DescribeProfile", json!({"ProfileId": &profile_id}))
                .await;
            assert_eq!(
                described.status, 200,
                "region {region} read from {reader}: expected 200, got {} with body {}",
                described.status, described.body
            );
            let arn = member_of(described.get("Profile"), "Arn")
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                arn, expected_arn,
                "region {region} read from {reader}: expected Arn {expected_arn:?}, got {arn:?}"
            );
        }
    }
}
