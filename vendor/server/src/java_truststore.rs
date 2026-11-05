// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! The Java trust store the emulator publishes next to `ca.pem`.
//!
//! JSSE (the TLS stack every JVM HTTP client uses, including all AWS SDK for Java
//! HTTP clients) never reads a PEM bundle from an environment variable, so
//! `AWS_CA_BUNDLE` and the shared config's `ca_bundle` do nothing for Java. What a
//! JVM does read, with no code, is a trust store named by the
//! `javax.net.ssl.trustStore` system property, which `JAVA_TOOL_OPTIONS` sets for
//! every JVM a shell starts:
//!
//! ```text
//! JAVA_TOOL_OPTIONS="-Djavax.net.ssl.trustStore=<emulator home>/<port>/truststore.p12 -Djavax.net.ssl.trustStoreType=PKCS12"
//! ```
//!
//! The store holds exactly the certificate in `ca.pem` and nothing else. That
//! property REPLACES the JDK's own `cacerts` rather than adding to it, so a JVM
//! pointed at this file trusts the emulator only: its other HTTPS calls (real AWS
//! included) fail. A JVM that needs both merges `ca.pem` into a copy of its own
//! `cacerts` with `keytool -importcert` instead.
//!
//! Format: a PKCS#12 (RFC 7292) file with one unencrypted SafeContents of
//! certificate bags and no MAC. With no MAC and no encryption there is no password
//! to agree on: the JDK loads it with no `trustStorePassword` (JDK 8u301, 11.0.12,
//! 12 and later), and also when one is given. Each bag carries the JDK's trusted
//! certificate attribute, without which the JDK's PKCS12 key store skips a
//! certificate that has no private key.

use yasna::models::ObjectIdentifier;
use yasna::{DERWriter, Tag};

/// The file name published in the emulator's `<home>/<port>/` folder.
pub const FILE_NAME: &str = "truststore.p12";

/// The alias the emulator certificate is stored under.
pub const EMULATOR_ALIAS: &str = "aws-emulator";

const ID_DATA: &[u64] = &[1, 2, 840, 113549, 1, 7, 1];
const CERT_BAG: &[u64] = &[1, 2, 840, 113549, 1, 12, 10, 1, 3];
const X509_CERTIFICATE: &[u64] = &[1, 2, 840, 113549, 1, 9, 22, 1];
const FRIENDLY_NAME: &[u64] = &[1, 2, 840, 113549, 1, 9, 20];
/// The JDK's "trusted key usage" bag attribute (`ORACLE_TrustedKeyUsage`).
const JDK_TRUSTED_KEY_USAGE: &[u64] = &[2, 16, 840, 1, 113894, 746875, 1, 1];
/// `anyExtendedKeyUsage`, the value `keytool -importcert` stores in that attribute.
const ANY_EXTENDED_KEY_USAGE: &[u64] = &[2, 5, 29, 37, 0];

/// A trust store holding only `emulator_cert_der`, the certificate in `ca.pem`.
pub fn emulator_only(emulator_cert_der: &[u8]) -> Vec<u8> {
    pkcs12(&[(EMULATOR_ALIAS.to_string(), emulator_cert_der)])
}

/// A PKCS#12 file of trusted certificate entries, `(alias, DER certificate)`.
pub fn pkcs12(entries: &[(String, &[u8])]) -> Vec<u8> {
    let safe_contents = yasna::construct_der(|w| {
        w.write_sequence_of(|w| {
            for (alias, der) in entries {
                cert_bag(w.next(), alias, der);
            }
        })
    });
    let authenticated_safe = yasna::construct_der(|w| {
        w.write_sequence_of(|w| data_content_info(w.next(), &safe_contents))
    });
    yasna::construct_der(|w| {
        w.write_sequence(|w| {
            w.next().write_u8(3);
            data_content_info(w.next(), &authenticated_safe);
        })
    })
}

fn oid(arcs: &[u64]) -> ObjectIdentifier {
    ObjectIdentifier::from_slice(arcs)
}

/// `ContentInfo { id-data, [0] EXPLICIT OCTET STRING }`.
fn data_content_info(w: DERWriter, content: &[u8]) {
    w.write_sequence(|w| {
        w.next().write_oid(&oid(ID_DATA));
        w.next()
            .write_tagged(Tag::context(0), |w| w.write_bytes(content));
    })
}

/// `SafeBag { certBag, [0] EXPLICIT CertBag, SET OF Attribute }`.
fn cert_bag(w: DERWriter, alias: &str, der: &[u8]) {
    w.write_sequence(|w| {
        w.next().write_oid(&oid(CERT_BAG));
        w.next().write_tagged(Tag::context(0), |w| {
            w.write_sequence(|w| {
                w.next().write_oid(&oid(X509_CERTIFICATE));
                w.next()
                    .write_tagged(Tag::context(0), |w| w.write_bytes(der));
            })
        });
        w.next().write_set_of(|w| {
            w.next().write_sequence(|w| {
                w.next().write_oid(&oid(FRIENDLY_NAME));
                w.next().write_set_of(|w| w.next().write_bmp_string(alias));
            });
            w.next().write_sequence(|w| {
                w.next().write_oid(&oid(JDK_TRUSTED_KEY_USAGE));
                w.next()
                    .write_set_of(|w| w.next().write_oid(&oid(ANY_EXTENDED_KEY_USAGE)));
            });
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use yasna::BERReader;

    /// Reads a PKCS#12 written by [`pkcs12`] back into `(alias, DER)` pairs,
    /// checking every structural field on the way, and that there is no MAC.
    fn read_back(p12: &[u8]) -> Vec<(String, Vec<u8>)> {
        let authenticated_safe = yasna::parse_der(p12, |r| {
            r.read_sequence(|r| {
                assert_eq!(r.next().read_u8()?, 3);
                let content = read_data_content_info(r.next())?;
                // No third element: no MacData, so no password.
                assert!(r.read_optional(|r| r.read_der()).unwrap().is_none());
                Ok(content)
            })
        })
        .expect("PFX");
        let safe_contents = yasna::parse_der(&authenticated_safe, |r| {
            r.read_sequence(|r| read_data_content_info(r.next()))
        })
        .expect("AuthenticatedSafe");
        yasna::parse_der(&safe_contents, |r| {
            r.collect_sequence_of(|r| {
                r.read_sequence(|r| {
                    assert_eq!(r.next().read_oid()?, oid(CERT_BAG));
                    let der = r.next().read_tagged(Tag::context(0), |r| {
                        r.read_sequence(|r| {
                            assert_eq!(r.next().read_oid()?, oid(X509_CERTIFICATE));
                            r.next().read_tagged(Tag::context(0), |r| r.read_bytes())
                        })
                    })?;
                    let mut alias = None;
                    let mut trusted = false;
                    r.next().read_set_of(|r| {
                        r.read_sequence(|r| {
                            let id = r.next().read_oid()?;
                            if id == oid(FRIENDLY_NAME) {
                                r.next().read_set_of(|r| {
                                    alias = Some(r.read_bmp_string()?);
                                    Ok(())
                                })
                            } else {
                                assert_eq!(id, oid(JDK_TRUSTED_KEY_USAGE));
                                r.next().read_set_of(|r| {
                                    assert_eq!(r.read_oid()?, oid(ANY_EXTENDED_KEY_USAGE));
                                    trusted = true;
                                    Ok(())
                                })
                            }
                        })
                    })?;
                    assert!(trusted, "every entry must carry the JDK trusted attribute");
                    Ok((alias.expect("friendlyName"), der))
                })
            })
        })
        .expect("SafeContents")
    }

    fn read_data_content_info(r: BERReader) -> yasna::ASN1Result<Vec<u8>> {
        r.read_sequence(|r| {
            assert_eq!(r.next().read_oid()?, oid(ID_DATA));
            r.next().read_tagged(Tag::context(0), |r| r.read_bytes())
        })
    }

    #[test]
    fn round_trips_aliases_and_certificates() {
        let entries = vec![
            ("a".to_string(), &b"\x30\x03\x02\x01\x01"[..]),
            ("b".to_string(), &b"\x30\x00"[..]),
        ];
        let back = read_back(&pkcs12(&entries));
        assert_eq!(
            back,
            vec![
                ("a".to_string(), b"\x30\x03\x02\x01\x01".to_vec()),
                ("b".to_string(), b"\x30\x00".to_vec())
            ]
        );
    }

    #[test]
    fn holds_only_the_emulator_certificate() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let back = read_back(&emulator_only(cert.cert.der()));
        assert_eq!(
            back,
            vec![(EMULATOR_ALIAS.to_string(), cert.cert.der().to_vec())]
        );
    }

    /// When a JDK is on the box, `keytool` (the JDK's own PKCS12 reader) must list
    /// every entry as a trusted certificate with NO password. Skipped without a JDK.
    #[test]
    fn keytool_lists_every_entry_as_trusted_without_a_password() {
        let keytool = std::env::var_os("JAVA_HOME")
            .map(|home| std::path::PathBuf::from(home).join("bin").join("keytool"))
            .filter(|path| path.exists());
        let Some(keytool) = keytool else {
            eprintln!("skipped: no JAVA_HOME/bin/keytool");
            return;
        };
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let dir = std::env::temp_dir().join(format!("java-truststore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, emulator_only(cert.cert.der())).unwrap();
        let out = std::process::Command::new(keytool)
            .args(["-list", "-storetype", "PKCS12", "-keystore"])
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "keytool failed: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.contains("Your keystore contains 1 entry"),
            "{stdout}"
        );
        assert_eq!(stdout.matches("trustedCertEntry").count(), 1, "{stdout}");
        assert!(stdout.contains(&format!("{EMULATOR_ALIAS},")), "{stdout}");
    }
}
