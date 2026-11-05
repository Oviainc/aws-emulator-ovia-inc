// SPDX-License-Identifier: CC0-1.0

//! The unit tests of this module's own rules.

use super::*;

/// A plausible instant to create a set of credentials at, and the credentials
/// one is created with. The username is shaped like one the credential store
/// mints; the password stands in for the secret it mints.
const CREATED_AT_MS: i64 = 1_700_000_000_000;
const USERNAME: &str = "EMULATORASJ4K7QWRT2X";
const PASSWORD: &str = "a-minted-registry-password";
const REGION: &str = "us-east-1";

/// One store holding one set of registry log in credentials.
fn store_with_one_login() -> ComAmazonawsLightsailLightsail20161128State {
    let mut store = ComAmazonawsLightsailLightsail20161128State::default();
    store.insert_registry_login(
        REGION,
        StoredRegistryLogin::new(USERNAME.to_string(), PASSWORD, CREATED_AT_MS),
    );
    store
}

/// RULE: the CLOCK is what ends a set of container image registry log in
/// credentials — it is recognised for the documented 12 hours after it was
/// created and gone from the store from that instant on, whether or not
/// anybody called in between. The integration suite asserts the `expiresAt`
/// the caller is told; this asserts the instant itself, which no wall-clock
/// test can reach.
#[test]
fn a_registry_login_is_forgotten_once_the_clock_passes_its_expiry() {
    let expiry = CREATED_AT_MS + REGISTRY_LOGIN_LIFETIME_MS;
    // (case, the instant the clock has reached, whether the set logs in)
    let cases = [
        ("the instant it was created", CREATED_AT_MS, true),
        ("one millisecond before it expires", expiry - 1, true),
        ("the instant it expires", expiry, false),
        ("an hour after it expired", expiry + 60 * 60 * 1_000, false),
    ];
    for (case, at_ms, want) in cases {
        // A fresh store per case: the clock only ever moves forward, so each
        // instant is reached from the creation rather than from the last case.
        let mut store = store_with_one_login();
        store.advance_clock(at_ms);
        let found = store.registry_login(REGION, USERNAME, PASSWORD).is_some();
        assert_eq!(
            found, want,
            "case {case}: expected the set to log in = {want}, got {found}"
        );
        assert_eq!(
            store.registry_logins.contains_key(REGION),
            want,
            "case {case}: expected an expired set to be gone from the store, got \
                 {:?}",
            store.registry_logins.get(REGION).map(Vec::len)
        );
    }
}

/// RULE: the store's own per-region bound keeps the credentials it holds
/// finite: past [`MAX_REGISTRY_LOGINS_PER_REGION`] the OLDEST set is the one
/// dropped, and every younger set still logs in.
#[test]
fn the_oldest_registry_login_is_the_one_the_bound_drops() {
    let mut store = ComAmazonawsLightsailLightsail20161128State::default();
    // One set past the bound, each a millisecond younger than the last so
    // "oldest" is well defined.
    let total = MAX_REGISTRY_LOGINS_PER_REGION + 1;
    let username = |index: usize| format!("{USERNAME}{index}");
    for index in 0..total {
        store.insert_registry_login(
            REGION,
            StoredRegistryLogin::new(
                username(index),
                PASSWORD,
                CREATED_AT_MS + i64::try_from(index).expect("the index fits"),
            ),
        );
    }
    store.advance_clock(CREATED_AT_MS);

    assert_eq!(
        store.registry_logins.get(REGION).map(Vec::len),
        Some(MAX_REGISTRY_LOGINS_PER_REGION),
        "case the region at its bound: expected {MAX_REGISTRY_LOGINS_PER_REGION} sets"
    );
    assert!(
        store
            .registry_login(REGION, &username(0), PASSWORD)
            .is_none(),
        "case the oldest set: expected it to have been dropped"
    );
    for index in 1..total {
        assert!(
            store
                .registry_login(REGION, &username(index), PASSWORD)
                .is_some(),
            "case the set created {index} milliseconds in: expected it to still log in"
        );
    }
}

/// RULE: only the credentials a set was created with log in to it, and only at
/// the registry of the region it was created for.
#[test]
fn only_the_credentials_a_registry_login_was_created_with_are_accepted() {
    let mut store = store_with_one_login();
    store.advance_clock(CREATED_AT_MS);

    // (case, region, username, password)
    let refused = [
        ("another region's registry", "us-west-2", USERNAME, PASSWORD),
        ("a username no set was created for", REGION, "AWS", PASSWORD),
        ("another password", REGION, USERNAME, "another-password"),
        ("the password of the username", REGION, USERNAME, USERNAME),
        ("no password at all", REGION, USERNAME, ""),
        ("a password one character short", REGION, USERNAME, &PASSWORD[..PASSWORD.len() - 1]),
    ];
    for (case, region, username, password) in refused {
        assert!(
            store.registry_login(region, username, password).is_none(),
            "case {case}: expected no set to be recognised"
        );
    }
    assert!(
        store
            .registry_login(REGION, USERNAME, PASSWORD)
            .is_some_and(|login| login.expires_at_ms
                == CREATED_AT_MS + REGISTRY_LOGIN_LIFETIME_MS),
        "case the credentials it was created with: expected the set to be recognised, \
             expiring 12 hours after it was created"
    );
}

/// RULE: what one region's registry holds is accounted for in BYTES — every
/// entry charged what it costs this process, content it already holds and a
/// tag it already has costing it nothing, and a fresh tag of a manifest it
/// already holds costing what that tag takes — so nothing a client repeats
/// grows it without bound.
#[test]
fn a_registry_accounts_for_every_byte_it_holds() {
    let mut registry = StoredImageRegistry::default();
    let layer = b"a layer of image content".to_vec();
    let manifest = br#"{"schemaVersion":2,"layers":[]}"#.to_vec();
    let media_type = "application/vnd.oci.image.manifest.v1+json";
    let layer_digest = registry_digest(&layer);
    // One entry's cost: its own bytes, the string it is keyed by, and the
    // fixed overhead every entry carries.
    let blob_bytes =
        REGISTRY_ENTRY_OVERHEAD_BYTES + layer_digest.len() + layer.len();

    assert_eq!(
        registry.bytes_held(),
        0,
        "case an empty registry: expected it to hold nothing"
    );
    let stored = registry.put_blob(layer.clone(), CREATED_AT_MS);
    assert_eq!(
        (stored, registry.bytes_held()),
        (RegistryWrite::Stored(layer_digest.clone()), blob_bytes),
        "case one pushed layer: expected its own bytes, its digest key and one entry's \
             overhead to be held under its own digest"
    );
    // The SAME bytes again are the blob that is already there.
    registry.put_blob(layer.clone(), CREATED_AT_MS + 1);
    assert_eq!(
        (registry.blobs.len(), registry.bytes_held()),
        (1, blob_bytes),
        "case the same layer pushed twice: expected one blob and no growth"
    );

    let digest = registry_digest(&manifest);
    let manifest_bytes = REGISTRY_ENTRY_OVERHEAD_BYTES
        + digest.len()
        + manifest.len()
        + "app".len()
        + media_type.len();
    let tag_bytes = |tag: &str| {
        REGISTRY_ENTRY_OVERHEAD_BYTES + "app".len() + tag.len() + digest.len()
    };
    for (case, tag, want_tags, want_bytes) in [
        (
            "the manifest under its first tag",
            "latest",
            1,
            blob_bytes + manifest_bytes + tag_bytes("latest"),
        ),
        (
            "the same manifest under the same tag again",
            "latest",
            1,
            blob_bytes + manifest_bytes + tag_bytes("latest"),
        ),
        (
            "the same manifest under a second tag",
            "v1",
            2,
            blob_bytes + manifest_bytes + tag_bytes("latest") + tag_bytes("v1"),
        ),
    ] {
        let stored = registry.put_manifest(
            "app",
            Some(tag),
            manifest.clone(),
            media_type,
            CREATED_AT_MS,
        );
        assert_eq!(
            stored,
            RegistryWrite::Stored(digest.clone()),
            "case {case}: expected it to be held under the digest of its own bytes"
        );
        assert_eq!(
            (registry.manifests.len(), registry.tags.len(), registry.bytes_held()),
            (1, want_tags, want_bytes),
            "case {case}: expected one manifest, {want_tags} tags and {want_bytes} bytes"
        );
    }
}

/// RULE: a registry entry is charged what it costs this process, so the
/// per-region byte bound bounds the ENTRY COUNT too — a client pushing
/// distinct one-byte blobs is refused after at most
/// `MAX_REGISTRY_BYTES_PER_REGION / REGISTRY_ENTRY_OVERHEAD_BYTES` of them,
/// rather than after the hundred million the content alone would admit.
#[test]
fn a_registry_charges_every_entry_its_own_footprint() {
    // Distinct one-byte blobs: the cheapest content a client can push, and
    // each one a digest of its own, so none of them is the blob that is
    // already there.
    let digest_len = registry_digest(&[0]).len();
    for count in [1usize, 2, 8, 64] {
        let mut registry = StoredImageRegistry::default();
        for index in 0..count {
            let content = vec![u8::try_from(index).expect("the table stays under 256")];
            assert_eq!(
                registry.put_blob(content.clone(), CREATED_AT_MS),
                RegistryWrite::Stored(registry_digest(&content)),
                "case {count} one-byte blobs: expected blob {index} to be held"
            );
        }
        let want = count * (REGISTRY_ENTRY_OVERHEAD_BYTES + digest_len + 1);
        assert_eq!(
            (registry.blobs.len(), registry.bytes_held()),
            (count, want),
            "case {count} one-byte blobs: expected each to be charged its digest key and \
                 its own overhead, not just its one byte"
        );
    }

    // The bound that follows: every entry costs at least the per-entry
    // overhead, so one region's registry holds at most this many entries
    // however small the content pushed into them is.
    let ceiling = MAX_REGISTRY_BYTES_PER_REGION / REGISTRY_ENTRY_OVERHEAD_BYTES;
    assert!(
        ceiling <= 1_048_576,
        "expected the per-region bound to admit at most a million entries whatever their \
             content, got {ceiling}"
    );
}

/// RULE: the store's own per-region bound keeps the blob uploads in flight
/// finite: past [`MAX_REGISTRY_UPLOADS_PER_REGION`] the OLDEST session is
/// the one dropped, and every younger session is still there to send to.
#[test]
fn the_oldest_blob_upload_is_the_one_the_bound_drops() {
    let mut registry = StoredImageRegistry::default();
    // One session past the bound, each a millisecond younger than the last
    // so "oldest" is well defined.
    let total = MAX_REGISTRY_UPLOADS_PER_REGION + 1;
    let session = |index: usize| format!("session-{index:04}");
    for index in 0..total {
        registry.start_upload(
            session(index),
            "app",
            CREATED_AT_MS + i64::try_from(index).expect("the index fits"),
        );
    }

    assert_eq!(
        registry.uploads.len(),
        MAX_REGISTRY_UPLOADS_PER_REGION,
        "case the region at its bound: expected {MAX_REGISTRY_UPLOADS_PER_REGION} sessions"
    );
    assert_eq!(
        registry.append_to_upload(&session(0), "app", b"bytes for a dropped session"),
        UploadAppend::Unknown,
        "case the oldest session: expected it to have been dropped"
    );
    for index in 1..total {
        assert_eq!(
            registry.append_to_upload(&session(index), "app", b"ab"),
            UploadAppend::Appended { held: 2 },
            "case the session started {index} milliseconds in: expected it to take bytes"
        );
    }
}

/// RULE: a label's version is one past the highest that label's registered
/// images carry — 1 for the first, 2 for the next, and counted per label —
/// which is what keeps the name of every image a service holds distinct.
#[test]
fn a_registered_image_takes_the_next_version_of_its_label() {
    let mut service = StoredContainerService {
        name: "svc".to_string(),
        arn: String::new(),
        created_at_ms: CREATED_AT_MS,
        location: StoredLocation {
            region_name: REGION.to_string(),
            availability_zone: Some(format!("{REGION}a")),
        },
        tags: Vec::new(),
        power: "nano".to_string(),
        power_id: "nano-1".to_string(),
        scale: 1,
        state: CONTAINER_SERVICE_READY,
        is_disabled: false,
        domain_token: "token".to_string(),
        principal_arn: String::new(),
        public_domain_names: BTreeMap::new(),
        ecr_image_puller_role_active: false,
        deployments: VecDeque::new(),
        next_deployment_version: 1,
        images: Vec::new(),
        owed: VecDeque::new(),
    };

    // (the label registered, the name the service must assign)
    let cases = [
        ("web", ":svc.web.1"),
        ("web", ":svc.web.2"),
        ("api", ":svc.api.1"),
        ("web", ":svc.web.3"),
        ("api", ":svc.api.2"),
    ];
    for (label, want) in cases {
        let registered = service.register_image(label, "sha256:abc", CREATED_AT_MS);
        assert_eq!(
            registered.name(&service.name),
            want,
            "case registering {label:?}: expected the name {want}"
        );
    }
    let held: Vec<String> = service
        .images
        .iter()
        .map(|image| image.name(&service.name))
        .collect();
    assert_eq!(
        held,
        cases.iter().map(|(_, name)| (*name).to_string()).collect::<Vec<_>>(),
        "case the whole registry: expected the images in registration order"
    );
}

/// RULE: a deployment that steps down when the clock activates the next one
/// stays one that PREVIOUSLY RAN, so the containers it declared stay ones
/// the service is running or previously ran, while a deployment that never
/// became active — one still waiting on the clock, or one replaced before it
/// activated — launched nothing and declares none of them. A deployment the
/// [`MAX_DEPLOYMENTS_PER_SERVICE`] bound dropped is forgotten with its
/// containers.
#[test]
fn a_superseded_deployment_is_kept_as_one_that_previously_ran() {
    let mut service = StoredContainerService {
        name: "svc".to_string(),
        arn: String::new(),
        created_at_ms: CREATED_AT_MS,
        location: StoredLocation {
            region_name: REGION.to_string(),
            availability_zone: Some(format!("{REGION}a")),
        },
        tags: Vec::new(),
        power: "nano".to_string(),
        power_id: "nano-1".to_string(),
        scale: 1,
        state: CONTAINER_SERVICE_READY,
        is_disabled: false,
        domain_token: "token".to_string(),
        principal_arn: String::new(),
        public_domain_names: BTreeMap::new(),
        ecr_image_puller_role_active: false,
        deployments: VecDeque::new(),
        next_deployment_version: 1,
        images: Vec::new(),
        owed: VecDeque::new(),
    };

    // Each deployment declares one container named after it, so which
    // deployment a container came from is unambiguous.
    let container_of = |version: i32| format!("app-{version}");
    let deployment = |version: i32| StoredDeployment {
        version,
        state: DEPLOYMENT_STATE_ACTIVATING,
        activated: false,
        containers: BTreeMap::from([(
            container_of(version),
            Container {
                image: Some("nginx:latest".to_string()),
                ..Default::default()
            },
        )]),
        public_endpoint: None,
        created_at_ms: CREATED_AT_MS,
    };

    // Three deployments past the bound, so what the bound drops is covered
    // as well as what it keeps.
    let total = i32::try_from(MAX_DEPLOYMENTS_PER_SERVICE + 3)
        .expect("the case count fits a deployment version");
    for version in 1..=total {
        service.record_deployment(deployment(version));
        service.owed.push_back(OwedPhase {
            at_ms: CREATED_AT_MS,
            state: CONTAINER_SERVICE_RUNNING,
            activates_next_deployment: true,
        });
        service.advance(CREATED_AT_MS);

        let ran = service.deployments_that_ran().count();
        let want = usize::try_from(version)
            .expect("the version fits a count")
            .min(MAX_DEPLOYMENTS_PER_SERVICE);
        assert_eq!(
            ran, want,
            "case {version} deployments activated: expected {want} of them still counted as \
                 having run, got {ran}"
        );
        // Every deployment still held answers for its own container,
        // whether it is the active one or one that ran before.
        for held in service
            .deployments_that_ran()
            .map(|deployment| deployment.version)
            .collect::<Vec<i32>>()
        {
            let container = container_of(held);
            assert!(
                service.has_run_container(&container),
                "case {version} deployments activated: expected {container:?} of the \
                     deployment {held} the store still holds to be one that ran, got the \
                     containers {:?}",
                service.containers_that_ran()
            );
        }
    }

    assert_eq!(
        service
            .current_deployment()
            .map(|active| (active.version, active.state)),
        Some((total, DEPLOYMENT_STATE_ACTIVE)),
        "case the newest deployment: expected it active, got {:?}",
        service.current_deployment()
    );
    let not_inactive: Vec<i32> = service
        .deployments
        .iter()
        .filter(|held| held.version != total && held.state != DEPLOYMENT_STATE_INACTIVE)
        .map(|held| held.version)
        .collect();
    assert!(
        not_inactive.is_empty(),
        "case every superseded deployment: expected each marked \
             {DEPLOYMENT_STATE_INACTIVE}, got {not_inactive:?} that are not"
    );
    // The oldest deployments are the ones the bound forgot, so their
    // containers are no longer ones this service ran.
    let bound = i32::try_from(MAX_DEPLOYMENTS_PER_SERVICE)
        .expect("the bound fits a deployment version");
    for forgotten in 1..=(total - bound) {
        let container = container_of(forgotten);
        assert!(
            !service.has_run_container(&container),
            "case the deployment {forgotten} the bound dropped: expected {container:?} to \
                 be forgotten with it, got the containers {:?}",
            service.containers_that_ran()
        );
    }
    let newest = container_of(total);
    assert!(
        service.containers_that_ran().contains(&newest.as_str()),
        "case the containers that ran: expected {newest:?} among them, got {:?}",
        service.containers_that_ran()
    );

    // A deployment still WAITING on the clock, and one replaced while it was
    // still waiting, both launched nothing — asserted here, where the clock
    // is explicit and no activation can race the check.
    let replaced_while_waiting = total.saturating_add(1);
    let waiting = total.saturating_add(2);
    service.record_deployment(deployment(replaced_while_waiting));
    service.record_deployment(deployment(waiting));
    for (case, version) in [
        ("a deployment still waiting on the clock", waiting),
        (
            "a deployment replaced before it ever activated",
            replaced_while_waiting,
        ),
    ] {
        let container = container_of(version);
        assert!(
            !service.has_run_container(&container),
            "case {case}: expected {container:?} not to be one that ran, got the containers \
                 {:?}",
            service.containers_that_ran()
        );
        assert!(
            service
                .deployments_that_ran()
                .all(|ran| ran.version != version),
            "case {case}: expected it outside the deployments that ran, got {:?}",
            service
                .deployments_that_ran()
                .map(|ran| ran.version)
                .collect::<Vec<i32>>()
        );
    }
}

/// RULE: the access settings this store names are the modelled
/// `ResourceBucketAccess` enum's own members, so the generated value list
/// stays the one source for what the wire accepts and these constants only
/// say which member each one is.
#[test]
fn each_named_bucket_access_setting_is_a_member_of_the_modelled_enum() {
    let generated = crate::ops::types::RESOURCE_BUCKET_ACCESS_VALUES;
    for (case, named) in [
        ("the setting that grants access", RESOURCE_BUCKET_ACCESS_ALLOW),
        ("the setting that revokes it", RESOURCE_BUCKET_ACCESS_DENY),
    ] {
        assert!(
            generated.contains(&named),
            "case {case}: expected {named:?} to be one of the modelled values \
                 {generated:?}"
        );
    }
    assert_ne!(
        RESOURCE_BUCKET_ACCESS_ALLOW, RESOURCE_BUCKET_ACCESS_DENY,
        "case the two settings: expected them to name different members"
    );
    assert_eq!(
        generated.len(),
        2,
        "case the modelled enum: expected the two members these constants cover, got \
             {generated:?}"
    );
}
