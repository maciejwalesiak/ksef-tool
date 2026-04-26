use rust_decimal::Decimal;

#[derive(Debug)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

pub type ValidationErrors = Vec<ValidationError>;

fn add_err(errors: &mut ValidationErrors, path: impl Into<String>, message: impl Into<String>) {
    errors.push(ValidationError {
        path: path.into(),
        message: message.into(),
    });
}

// Validation runs after serde deserialization. It uses types from the parent module via `super::`.
pub fn validate_invoice_data(data: &super::InvoiceData) -> Result<(), ValidationErrors> {
    let mut errors: ValidationErrors = Vec::new();

    // number: if present, must not be empty when trimmed
    if data.number.as_ref().map(|s| s.trim().is_empty()).unwrap_or(false) {
        add_err(&mut errors, "number", "invoice number is empty");
    }

    // currency: basic sanity check (must be three-letter code)
    if data.currency.as_str().len() != 3 {
        add_err(&mut errors, "currency", "invalid currency code length");
    }

    // Seller and buyer
    for (which, subj) in [("seller", &data.seller), ("buyer", &data.buyer)] {
        if which == "seller" {
            if subj.nip.trim().is_empty() {
                add_err(&mut errors, format!("{}.nip", which), "nip is empty");
            } else if subj.address.country_code.as_str() == "PL" {
                // For PL require 10 digits
                let digits: String = subj.nip.chars().filter(|c| c.is_ascii_digit()).collect();
                if digits.len() != 10 {
                    add_err(
                        &mut errors,
                        format!("{}.nip", which),
                        "PL NIP must contain 10 digits",
                    );
                }
            }
        } else {
            // buyer: optional nip; if present and PL, validate digits
            if !subj.nip.trim().is_empty() && subj.address.country_code.as_str() == "PL" {
                let digits: String = subj.nip.chars().filter(|c| c.is_ascii_digit()).collect();
                if digits.len() != 10 {
                    add_err(
                        &mut errors,
                        format!("{}.nip", which),
                        "PL NIP must contain 10 digits",
                    );
                }
            }
        }

        if subj.name.trim().is_empty() {
            add_err(&mut errors, format!("{}.name", which), "name is empty");
        }

        // Address fields presence
        let addr = &subj.address;
        if addr.country_code.as_str().len() != 2 {
            add_err(
                &mut errors,
                format!("{}.address.country_code", which),
                "country_code must be two letters",
            );
        }
        if addr.street.trim().is_empty() {
            add_err(
                &mut errors,
                format!("{}.address.street", which),
                "street is empty",
            );
        }
        if addr.building_number.trim().is_empty() {
            add_err(
                &mut errors,
                format!("{}.address.building_number", which),
                "building_number is empty",
            );
        }
        if addr.city.trim().is_empty() {
            add_err(
                &mut errors,
                format!("{}.address.city", which),
                "city is empty",
            );
        }
        if addr.postal_code.trim().is_empty() {
            add_err(
                &mut errors,
                format!("{}.address.postal_code", which),
                "postal_code is empty",
            );
        }
    }

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

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
