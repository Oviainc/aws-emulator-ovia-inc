// SPDX-License-Identifier: CC0-1.0

//! Whether a submitted tax registration number satisfies the structure its
//! jurisdiction publishes for it.
//!
//! This is what validating a TRN can establish without the tax authority's
//! register: the authority publishes how a number of that kind is built — its
//! length, its character layout and, for many of them, a check digit computed
//! from the rest of the number — and a submitted number that does not satisfy
//! that published structure is not a number the authority ever issued. It is the
//! one question about the number itself that has an answer here, so it is the
//! question [`super::trn_validation_status`] asks when the validation a write
//! started falls due.
//!
//! A jurisdiction and registration type this module publishes NO structure for
//! is not judged: there is nothing to contradict the submitted number with, so
//! the validation finds no fault. Adding a jurisdiction is adding a row to
//! [`PUBLISHED_STRUCTURES`]; nothing else changes.

/// The structure one jurisdiction publishes for one registration type.
///
/// `country_code` is the ISO 3166-1 alpha-2 code of the jurisdiction the
/// registration is held in (the stored `legalAddress.countryCode`), and
/// `registration_type` one of the model's `TaxRegistrationType` values, so the
/// pair is what a stored record carries.
struct PublishedStructure {
    /// The jurisdiction whose authority publishes this structure.
    country_code: &'static str,
    /// The `TaxRegistrationType` whose numbers it describes.
    registration_type: &'static str,
    /// The prefix the authority writes such a number with, which a submitted
    /// number may carry or leave off (`DE123456789` and `123456789` are the same
    /// German VAT identification number). Empty where the authority publishes
    /// none.
    prefix: &'static str,
    /// Whether `number` — normalised by [`normalised`] and with [`Self::prefix`]
    /// removed — satisfies the structure.
    satisfied: fn(&str) -> bool,
}

/// Every tax registration number structure this service holds, one row per
/// jurisdiction and registration type.
///
/// Each row's source is the authority that issues the number, named in the
/// comment above it: the structure is the authority's, not this emulator's, and
/// a row states only what that authority publishes.
const PUBLISHED_STRUCTURES: &[PublishedStructure] = &[
    // Germany, Umsatzsteuer-Identifikationsnummer (Bundeszentralamt für
    // Steuern): the country prefix `DE` and nine digits, the last of which is a
    // check digit over the other eight computed with ISO 7064 MOD 11,10.
    PublishedStructure {
        country_code: "DE",
        registration_type: "VAT",
        prefix: "DE",
        satisfied: german_vat_number,
    },
    // France, numéro de TVA intracommunautaire (Direction générale des Finances
    // publiques): the country prefix `FR`, a two-digit key and the nine-digit
    // SIREN of the business, where the key is fixed by the SIREN.
    PublishedStructure {
        country_code: "FR",
        registration_type: "VAT",
        prefix: "FR",
        satisfied: french_vat_number,
    },
    // Brazil, Cadastro Nacional da Pessoa Jurídica (Receita Federal): fourteen
    // digits, the last two check digits over the twelve before them.
    PublishedStructure {
        country_code: "BR",
        registration_type: "CNPJ",
        prefix: "",
        satisfied: brazilian_cnpj_number,
    },
    // Brazil, Cadastro de Pessoas Físicas (Receita Federal): eleven digits, the
    // last two check digits over the nine before them.
    PublishedStructure {
        country_code: "BR",
        registration_type: "CPF",
        prefix: "",
        satisfied: brazilian_cpf_number,
    },
    // India, Permanent Account Number (Income Tax Department): ten characters —
    // five letters, four digits and a letter — and no published check digit.
    PublishedStructure {
        country_code: "IN",
        registration_type: "PAN",
        prefix: "",
        satisfied: indian_permanent_account_number,
    },
    // India, Goods and Services Tax Identification Number (Goods and Services
    // Tax Network): fifteen characters — a two-digit state code, the holder's
    // Permanent Account Number, an entity character, the letter `Z` and a
    // checksum character.
    PublishedStructure {
        country_code: "IN",
        registration_type: "GST",
        prefix: "",
        satisfied: indian_gst_number,
    },
    // Kenya, Personal Identification Number (Kenya Revenue Authority): eleven
    // characters — the taxpayer-class letter `A` or `P`, nine digits and a
    // letter — and no published check digit.
    PublishedStructure {
        country_code: "KE",
        registration_type: "TIN",
        prefix: "",
        satisfied: kenyan_pin_number,
    },
];

/// Whether the number of a registration of this type, held in this jurisdiction,
/// satisfies the structure that jurisdiction publishes for it.
///
/// True for a jurisdiction and type [`PUBLISHED_STRUCTURES`] holds no row for:
/// the submitted number is then contradicted by nothing this service holds.
pub fn satisfies_published_structure(
    country_code: &str,
    registration_type: &str,
    registration_id: &str,
) -> bool {
    let Some(structure) = PUBLISHED_STRUCTURES.iter().find(|structure| {
        structure.country_code.eq_ignore_ascii_case(country_code)
            && structure.registration_type == registration_type
    }) else {
        return true;
    };
    let normalised = normalised(registration_id);
    let number = normalised
        .strip_prefix(structure.prefix)
        .unwrap_or(&normalised);
    (structure.satisfied)(number)
}

/// The submitted number with the separators a holder writes it with removed and
/// its letters upper-cased, which is the form every published structure above is
/// stated in: the authorities print the same number with and without whitespace,
/// dots, slashes and hyphens (`12.345.678/0001-95`, `CHE-123.456.789`), and the
/// padding a submission carries around it is no part of the number either.
///
/// This is therefore what one number IS, separators and padding aside, so it is
/// also the form a stored number is compared in wherever the number itself is an
/// identity rather than merely a value — see
/// [`super::ComAmazonawsTaxsettingsTaxsettingsState::find_supplemental_authority`].
pub fn normalised(registration_id: &str) -> String {
    registration_id
        .chars()
        .filter(|character| {
            !character.is_whitespace() && !matches!(character, '.' | '-' | '/' | '_')
        })
        .flat_map(char::to_uppercase)
        .collect()
}

/// `number` as its digits, or `None` when it is not all digits.
fn digits(number: &str) -> Option<Vec<u32>> {
    number.chars().map(|character| character.to_digit(10)).collect()
}

/// Whether the character layout of `number` matches `layout`, one character of
/// `layout` per character of `number`: `A` a letter, `9` a digit, `*` either, and
/// any other character itself.
fn matches_layout(number: &str, layout: &str) -> bool {
    if number.chars().count() != layout.chars().count() {
        return false;
    }
    number
        .chars()
        .zip(layout.chars())
        .all(|(character, expected)| match expected {
            'A' => character.is_ascii_uppercase(),
            '9' => character.is_ascii_digit(),
            '*' => character.is_ascii_uppercase() || character.is_ascii_digit(),
            literal => character == literal,
        })
}

/// The check digit ISO 7064 MOD 11,10 publishes for `body`, the digits before
/// it.
///
/// The standard's recursion: starting from a product of 10, each digit is added
/// to the running product modulo 10 (a zero result standing for 10) and the sum
/// doubled modulo 11; the check digit is 11 less that product, with 10 written
/// as 0.
fn iso_7064_mod_11_10_check_digit(body: &[u32]) -> u32 {
    let mut product = 10;
    for digit in body {
        let sum = match (digit + product) % 10 {
            0 => 10,
            sum => sum,
        };
        product = (2 * sum) % 11;
    }
    (11 - product) % 10
}

/// The check digit the Brazilian registers publish for a weighted digit sum: the
/// sum modulo 11, subtracted from 11, with a remainder below 2 standing for a
/// check digit of 0.
fn brazilian_check_digit(weighted_total: u32) -> u32 {
    match weighted_total % 11 {
        remainder if remainder < 2 => 0,
        remainder => 11 - remainder,
    }
}

/// The CNPJ check digit of `body`: its digits weighted from the right, 2
/// upwards, cycling back to 2 after 9.
fn cnpj_check_digit(body: &[u32]) -> u32 {
    brazilian_check_digit(
        body.iter()
            .rev()
            .enumerate()
            .map(|(index, digit)| digit * (u32::try_from(index % 8).unwrap_or(0) + 2))
            .sum(),
    )
}

/// The CPF check digit of `body`: its digits weighted from the right, 2 upwards,
/// without cycling.
fn cpf_check_digit(body: &[u32]) -> u32 {
    brazilian_check_digit(
        body.iter()
            .rev()
            .enumerate()
            .map(|(index, digit)| digit * (u32::try_from(index).unwrap_or(0) + 2))
            .sum(),
    )
}

/// Whether every digit of `number` is the same digit, which both Brazilian
/// registers publish as never issued however its check digits come out.
fn all_one_digit(body: &[u32]) -> bool {
    body.first()
        .is_some_and(|first| body.iter().all(|digit| digit == first))
}

/// Germany's VAT identification number: nine digits closed by an ISO 7064 MOD
/// 11,10 check digit.
fn german_vat_number(number: &str) -> bool {
    let Some(digits) = digits(number) else {
        return false;
    };
    let Some((check, body)) = digits.split_last() else {
        return false;
    };
    body.len() == 8 && iso_7064_mod_11_10_check_digit(body) == *check
}

/// France's VAT number: a two-digit key over the nine-digit SIREN that follows
/// it, where the published key of a SIREN `s` is `(12 + 3 * (s mod 97)) mod 97`.
fn french_vat_number(number: &str) -> bool {
    if !matches_layout(number, "99999999999") {
        return false;
    }
    let (key, siren) = number.split_at(2);
    let (Ok(key), Ok(siren)) = (key.parse::<u64>(), siren.parse::<u64>()) else {
        return false;
    };
    key == (12 + 3 * (siren % 97)) % 97
}

/// Brazil's CNPJ: twelve digits closed by two check digits, the first over the
/// twelve and the second over the thirteen before it.
fn brazilian_cnpj_number(number: &str) -> bool {
    brazilian_number(number, 12, cnpj_check_digit)
}

/// Brazil's CPF: nine digits closed by two check digits, read the same way as
/// [`brazilian_cnpj_number`] but with that register's own weights.
fn brazilian_cpf_number(number: &str) -> bool {
    brazilian_number(number, 9, cpf_check_digit)
}

/// Whether `number` is `body_length` digits closed by the two check digits
/// `check_digit` computes for them — the first over the body, the second over
/// the body and the first check digit.
fn brazilian_number(
    number: &str,
    body_length: usize,
    check_digit: fn(&[u32]) -> u32,
) -> bool {
    let Some(digits) = digits(number) else {
        return false;
    };
    if digits.len() != body_length + 2 || all_one_digit(&digits) {
        return false;
    }
    check_digit(&digits[..body_length]) == digits[body_length]
        && check_digit(&digits[..=body_length]) == digits[body_length + 1]
}

/// India's Permanent Account Number: five letters, four digits and a letter.
fn indian_permanent_account_number(number: &str) -> bool {
    matches_layout(number, "AAAAA9999A")
}

/// India's GST identification number: a two-digit state code, a Permanent
/// Account Number, an entity character, `Z` and a checksum character.
fn indian_gst_number(number: &str) -> bool {
    matches_layout(number, "99AAAAA9999A*Z*")
}

/// Kenya's Personal Identification Number: the taxpayer-class letter `A` or `P`,
/// nine digits and a letter.
fn kenyan_pin_number(number: &str) -> bool {
    matches_layout(number, "A999999999A") && number.starts_with(['A', 'P'])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each published structure, with a number the authority's own rule admits
    /// and one it does not, so no row can pass by admitting everything.
    #[test]
    fn each_published_structure_admits_its_own_numbers_and_refuses_others() {
        // (country, type, admitted, refused). The admitted numbers carry the
        // check digits their authority's rule computes; the refused ones differ
        // from an admitted one only in that digit, or in the layout.
        let rows: &[(&str, &str, &str, &str)] = &[
            ("DE", "VAT", "DE136695976", "DE123456789"),
            ("DE", "VAT", "136695976", "13669597"),
            ("FR", "VAT", "FR15345678901", "FR12345678901"),
            ("BR", "CNPJ", "12.345.678/0001-95", "12.345.678/0001-96"),
            ("BR", "CNPJ", "11222333000181", "11111111111111"),
            ("BR", "CPF", "11144477735", "11144477736"),
            ("IN", "PAN", "ABCDE1234F", "ABCD12345F"),
            ("IN", "GST", "29ABCDE1234F1Z5", "29ABCDE1234F1X5"),
            ("KE", "TIN", "P051234567M", "X051234567M"),
        ];
        for (country, registration_type, admitted, refused) in rows.iter().copied() {
            assert!(
                satisfies_published_structure(country, registration_type, admitted),
                "case {country}/{registration_type}: {admitted:?} satisfies the published \
                 structure, got false"
            );
            assert!(
                !satisfies_published_structure(country, registration_type, refused),
                "case {country}/{registration_type}: {refused:?} does not satisfy the published \
                 structure, got true"
            );
        }
    }

    /// A jurisdiction and type no row covers is not judged, whatever the number
    /// looks like.
    #[test]
    fn a_structure_this_service_does_not_hold_judges_nothing() {
        for (country, registration_type, number) in [
            ("CH", "VAT", "CHE-123.456.789"),
            ("CH", "VAT", "not-a-number"),
            ("DE", "TIN", "123456789"),
            ("MY", "NRIC", "800101011234"),
        ] {
            assert!(
                satisfies_published_structure(country, registration_type, number),
                "case {country}/{registration_type}: {number:?} is judged by no structure this \
                 service holds, got false"
            );
        }
    }

    /// The German check digit is the one the standard's own worked example
    /// publishes, so the recursion above is the standard's and not a guess.
    #[test]
    fn the_german_check_digit_is_the_published_one() {
        for (body, check) in [
            ([1, 3, 6, 6, 9, 5, 9, 7], 6),
            ([1, 2, 3, 4, 5, 6, 7, 8], 8),
        ] {
            let computed = iso_7064_mod_11_10_check_digit(&body);
            assert_eq!(
                computed, check,
                "case {body:?}: expected check digit {check}, got {computed}"
            );
        }
    }
}
