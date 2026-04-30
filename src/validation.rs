use rust_decimal::Decimal;
use std::fmt;

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct ValidationErrors(pub Vec<ValidationError>);

impl std::ops::Deref for ValidationErrors {
    type Target = Vec<ValidationError>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<ValidationError>> for ValidationErrors {
    fn from(v: Vec<ValidationError>) -> Self {
        ValidationErrors(v)
    }
}

impl<'a> IntoIterator for &'a ValidationErrors {
    type Item = &'a ValidationError;
    type IntoIter = std::slice::Iter<'a, ValidationError>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{}: {}", e.path, e.message)?;
        }
        Ok(())
    }
}

fn add_err(errors: &mut ValidationErrors, path: impl Into<String>, message: impl Into<String>) {
    errors.0.push(ValidationError {
        path: path.into(),
        message: message.into(),
    });
}

/// Validate Polish NIP using checksum algorithm.
/// Reject inputs that contain non-digit characters; only pure 10-digit strings
/// are considered valid.
pub fn is_valid_pl_nip(nip: &str) -> bool {
    // Only accept strings composed entirely of ASCII digits
    if !nip.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    let digits: Vec<u32> = nip.chars().map(|c| c.to_digit(10).unwrap()).collect();
    if digits.len() != 10 {
        return false;
    }

    let weights = [6u32, 5, 7, 2, 3, 4, 5, 6, 7];
    let sum: u32 = digits
        .iter()
        .take(9)
        .enumerate()
        .map(|(i, d)| d * weights[i])
        .sum();

    let checksum = sum % 11;
    if checksum == 10 {
        return false;
    }
    checksum == digits[9]
}

// Helper: check trimmed non-empty
fn check_non_empty(
    errors: &mut ValidationErrors,
    path: impl Into<String>,
    val: &str,
    msg: impl Into<String>,
) {
    if val.trim().is_empty() {
        add_err(errors, path, msg);
    }
}

// Helper: check country code length == 2 and uppercase ASCII letters
fn check_country_code(errors: &mut ValidationErrors, path: impl Into<String>, code: &str) {
    if code.len() != 2 || !code.chars().all(|c| c.is_ascii_uppercase()) {
        add_err(
            errors,
            path,
            "country_code must be two uppercase ASCII letters",
        );
    }
}

// Validation runs after serde deserialization. It uses types from the parent module via `super::`.
pub fn validate_invoice_data(data: &super::InvoiceData) -> Result<(), ValidationErrors> {
    let mut errors: ValidationErrors = ValidationErrors::default();

    // number: must be present and not empty when trimmed
    if data.number.is_none() {
        add_err(&mut errors, "number", "invoice number is missing");
    } else if data
        .number
        .as_ref()
        .map(|s| s.trim().is_empty())
        .unwrap_or(false)
    {
        add_err(&mut errors, "number", "invoice number is empty");
    }

    // currency: basic sanity check (must be three-letter code)
    if data.currency.as_str().len() != 3 {
        add_err(&mut errors, "currency", "invalid currency code length");
    }

    // Seller and buyer: use helper to avoid duplicating PL-NIP logic
    fn validate_subject(
        subj: &super::Subject,
        key: &str,
        is_seller: bool,
        errors: &mut ValidationErrors,
    ) {
        // NIP rules: seller NIP is required; buyer NIP optional
        if is_seller && subj.nip.trim().is_empty() {
            add_err(errors, format!("{}.nip", key), "nip is empty");
        }
        // If NIP present and country_code == "PL", validate checksum
        if !subj.nip.trim().is_empty()
            && subj.address.country_code.as_str() == "PL"
            && !is_valid_pl_nip(&subj.nip)
        {
            add_err(
                errors,
                format!("{}.nip", key),
                "PL NIP is invalid (checksum or length)",
            );
        }

        check_non_empty(errors, format!("{}.name", key), &subj.name, "name is empty");

        // Address fields presence
        let addr = &subj.address;
        check_country_code(
            errors,
            format!("{}.address.country_code", key),
            addr.country_code.as_str(),
        );
        check_non_empty(
            errors,
            format!("{}.address.street", key),
            &addr.street,
            "street is empty",
        );
        check_non_empty(
            errors,
            format!("{}.address.building_number", key),
            &addr.building_number,
            "building_number is empty",
        );
        check_non_empty(
            errors,
            format!("{}.address.city", key),
            &addr.city,
            "city is empty",
        );
        check_non_empty(
            errors,
            format!("{}.address.postal_code", key),
            &addr.postal_code,
            "postal_code is empty",
        );
    }

    validate_subject(&data.seller, "seller", true, &mut errors);
    validate_subject(&data.buyer, "buyer", false, &mut errors);

    // Positions: require at least one
    if data.positions.is_empty() {
        add_err(
            &mut errors,
            "positions",
            "positions array must contain at least one position",
        );
    }

    for (i, pos) in data.positions.iter().enumerate() {
        let ppath = format!("positions[{}]", i);
        if pos.name.trim().is_empty() {
            add_err(
                &mut errors,
                format!("{}.name", ppath),
                "position name is empty",
            );
        }
        if pos.count <= Decimal::ZERO {
            add_err(&mut errors, format!("{}.count", ppath), "count must be > 0");
        }
        if pos.price < Decimal::ZERO {
            add_err(
                &mut errors,
                format!("{}.price", ppath),
                "price must be >= 0",
            );
        }
        // tax_rate is parsed by serde into TaxRate; no extra check here
    }

    // Payment details
    if let Some(pd) = &data.payment_details {
        if pd.account_number.trim().is_empty() {
            add_err(
                &mut errors,
                "payment_details.account_number",
                "account_number is empty",
            );
        }
        if let Some(swift) = pd.swift.as_ref() {
            let s = swift.trim();
            let len = s.len();
            if !(len == 8 || len == 11) {
                add_err(
                    &mut errors,
                    "payment_details.swift",
                    "SWIFT must be 8 or 11 characters if present",
                );
            }
            if !s.chars().all(|c| c.is_ascii_alphanumeric()) {
                add_err(
                    &mut errors,
                    "payment_details.swift",
                    "SWIFT contains invalid characters",
                );
            }
        }
        // period is u16 by type; no additional check required here
    }

    if errors.0.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
