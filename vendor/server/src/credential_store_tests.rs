// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::aws_credentials::{AWS_ACCESS_KEY, SECRET_ACCESS_KEY, SIGV4_SCHEME};

/// A kind whose first id is always `FIXED`, as a colliding random draw would be.
struct FixedFirstKind(std::sync::atomic::AtomicUsize);

impl sealed::Sealed for FixedFirstKind {}

impl CredentialKind for FixedFirstKind {
    fn scheme(&self) -> &'static str {
        "test#scheme"
    }
    fn mint(&self) -> MintedCredential {
        let n = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = if n == 0 {
            "FIXED".to_string()
        } else {
            format!("FRESH{n}")
        };
        let secrets = Secrets::from([("secret", format!("secret{n}"))]);
        MintedCredential { id, secrets }
    }
}

fn fixed_first_kind() -> &'static FixedFirstKind {
    Box::leak(Box::new(FixedFirstKind(std::sync::atomic::AtomicUsize::new(0))))
}

#[test]
fn a_created_credential_is_found_under_its_scheme_until_deleted() {
    let store = CredentialStore::default();
    let created = store.create(&AWS_ACCESS_KEY);
    let secret = store.secret_for(&created.id, SIGV4_SCHEME, SECRET_ACCESS_KEY);
    assert_eq!(
        secret.as_deref(),
        Ok(created.secret(SECRET_ACCESS_KEY).unwrap())
    );
    let other_scheme = store.secret_for(&created.id, "other#scheme", SECRET_ACCESS_KEY);
    assert_eq!(other_scheme, Err(ACCESS_DENIED));
    assert_eq!(store.delete(&created.id), Ok(()));
    let deleted = store.secret_for(&created.id, SIGV4_SCHEME, SECRET_ACCESS_KEY);
    assert_eq!(deleted, Err(ACCESS_DENIED));
    assert_eq!(store.delete(&created.id), Err(CredentialError::NotFound));
    let printed = format!("{created:?}");
    assert!(
        !printed.contains(created.secret(SECRET_ACCESS_KEY).unwrap()),
        "{printed}"
    );
}

#[test]
fn a_colliding_id_is_minted_again_instead_of_replacing_or_panicking() {
    let (store, kind) = (CredentialStore::default(), fixed_first_kind());
    store.records.write().insert(
        "FIXED".into(),
        CredentialRecord {
            scheme: "test#scheme",
            secrets: Secrets::from([("secret", "held".into())]),
        },
    );
    let created = store.create(kind);
    assert_eq!(created.id, "FRESH1");
    let held = store.secret_for("FIXED", "test#scheme", "secret");
    assert_eq!(held.as_deref(), Ok("held"));
}

#[test]
fn a_second_create_never_returns_an_existing_id() {
    let store = CredentialStore::default();
    let first = store.create(&AWS_ACCESS_KEY);
    let second = store.create(&AWS_ACCESS_KEY);
    assert_ne!(first.id, second.id);
    let found = store.secret_for(&first.id, SIGV4_SCHEME, SECRET_ACCESS_KEY);
    assert_eq!(
        found.as_deref(),
        Ok(first.secret(SECRET_ACCESS_KEY).unwrap())
    );
}
