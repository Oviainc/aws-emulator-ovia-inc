// SPDX-License-Identifier: CC0-1.0

//! The ARN of the Amazon Web Services Secrets Manager secret an Amazon EVS request
//! named.
//!
//! A connector's credential secret is named on input by EITHER form — `secretIdentifier`
//! is "The ARN or name of the Amazon Web Services Secrets Manager secret", and the
//! modelled `SecretIdentifier` pattern spells both alternatives — while the member that
//! reports it back, `Connector$secretArn`, is "The Amazon Resource Name (ARN) of" that
//! secret. The input is therefore an identifier and the output is an ARN, so the
//! identifier is RESOLVED to an ARN before it is stored, and no connector ever reports a
//! bare name.
//!
//! An identifier that already is an ARN names the secret the caller meant and is reported
//! unchanged — rewriting it would report a different secret. A NAME is resolved the way
//! every other ARN of this emulator is built, from the Region the request was addressed to
//! and the account ([`::engine::emulator_arn`], as [`super::environment_arn`] uses it):
//! `arn:aws:secretsmanager:<region>:<account>:secret:<name>`. That is the PARTIAL-ARN form
//! of a secret — the ARN without the hyphen and six random characters Secrets Manager
//! appends when it creates one — which is a form Secrets Manager itself accepts as a
//! `SecretId`, so the value a connector reports is an ARN the caller can use directly.
//!
//! Resolution is a pure function of the request: it reads no store, writes nothing and
//! takes no lock, so one name reports one ARN on every call and for every operation.
//! Whether a secret by that name exists at all is a different question — refusing an
//! identifier that names no secret belongs to the validation of these operations' inputs,
//! not to the value this member reports.

/// Whether `secret_identifier` is in the ARN form of the modelled `SecretIdentifier`
/// rather than the bare-name form.
///
/// The pattern's two alternatives are `arn:aws:secretsmanager:<region>:<account>:secret:…`
/// and a name drawn from `[a-zA-Z0-9/_+=.@!-]`, which admits no colon — so a value
/// carrying an ARN's prefix is the ARN alternative and every other value is a name. The
/// test is the prefix and not the whole ARN grammar, because a value that already carries
/// one is reported as the caller wrote it: nesting it inside a newly built ARN would name
/// nothing.
fn is_arn_form(secret_identifier: &str) -> bool {
    secret_identifier.starts_with("arn:")
}

/// The ARN of the secret `secret_identifier` names, for `Connector$secretArn`: the
/// identifier itself when it already is an ARN, and otherwise the ARN of that name in
/// `region` and this account.
pub fn resolve_secret_arn(region: &str, secret_identifier: &str) -> String {
    if is_arn_form(secret_identifier) {
        return secret_identifier.to_string();
    }
    ::engine::emulator_arn(
        "secretsmanager",
        region,
        &format!("secret:{secret_identifier}"),
    )
}
