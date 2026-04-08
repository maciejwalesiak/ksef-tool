use core::result::Result;
use invoice_gen::{
    fa_3::{
        builder::{BuyerBuilder, LineBuilder, SellerBuilder},
        models::{
            Annotations, BankAccount, Header, IdentificationData2, Invoice, InvoiceBody,
            InvoiceLine, Payment, PaymentTerm, Subject1, Subject2,
        },
    },
    shared::{CurrencyCode, TaxRate},
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::{env, fs::File, io::Read, time::Duration};
use thiserror::Error;

#[derive(Debug, Deserialize)]
struct Address {
    country_code: String,
    street: String,
    building_number: String,
    flat_number: Option<String>,
    city: String,
    postal_code: String,
}

#[derive(Debug, Deserialize)]
struct Subject {
    nip: String,
    name: String,
    address: Address,
}

#[derive(Debug, Deserialize)]
struct Position {
    name: String,
    count: Decimal,
    price: Decimal,
    tax_rate: TaxRate,
}

#[derive(Debug, Deserialize)]
struct PaymentDetails {
    bank_name: String,
    account_number: String,
    swift: Option<String>,
    period: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct InvoiceData {
    number: Option<String>,
    currency: CurrencyCode,
    seller: Subject,
    buyer: Subject,
    positions: Vec<Position>,
    payment_details: Option<PaymentDetails>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NbpRate {
    #[allow(dead_code)]
    no: String,
    #[allow(dead_code)]
    effective_date: String,
    mid: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NbpResponse {
    #[allow(dead_code)]
    table: String,
    #[allow(dead_code)]
    currency: String,
    #[allow(dead_code)]
    code: String,
    rates: Vec<NbpRate>,
}
const PAYMENT_METHOD_BANK_TRANSFER: u8 = 6;
const REVERSE_CHARGE_SET: u8 = 1;
const REVERSE_CHARGE_UNSET: u8 = 2;

#[derive(Debug, Error)]
enum CurrencyExchangeRateError {
    #[error("currency exchange rate request error")]
    RequestError(#[from] reqwest::Error),
    #[error("{0} currency exchange rate is missing")]
    RateMissing(CurrencyCode),
    #[error("{0} currency exchange rate value is invalid")]
    InvalidRate(CurrencyCode),
}

fn get_currency_exchange_rate(
    currency_code: &CurrencyCode,
) -> Result<Decimal, CurrencyExchangeRateError> {
    let url = format!(
        "http://api.nbp.pl/api/exchangerates/rates/A/{}/last/1/?format=json",
        currency_code,
    );

    let response: NbpResponse = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(url)
        .send()?
        .json()?;

    let rate = response
        .rates
        .first()
        .map(|rate| {
            Decimal::from_f64_retain(rate.mid)
                .unwrap_or(Decimal::ZERO)
                .round_dp_with_strategy(4, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
        })
        .ok_or(CurrencyExchangeRateError::RateMissing(
            currency_code.clone(),
        ));

    if let Ok(rate) = rate
        && rate == Decimal::ZERO
    {
        return Err(CurrencyExchangeRateError::InvalidRate(
            currency_code.clone(),
        ));
    }

    rate
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} </path/to/invoice_data.json>", args[0]);
        std::process::exit(1);
    }

    let file_path = &args[1];
    let mut file = File::open(file_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let invoice_data: InvoiceData = serde_json::from_str(&contents)?;
    // println!("{:?}", invoice_data);

    // TODO: generate incremental number based on last value from given month stored in db
    // if invoice number is not explicitly set
    let invoice_number = invoice_data
        .number
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Missing required field: number",
            )
        })?;
    let currency_code = invoice_data.currency;
    let currency_rate = if currency_code.as_str() != "PLN" {
        let rate = get_currency_exchange_rate(&currency_code)?;
        Some(rate)
    } else {
        None
    };

    let (buyer_eu_code, buyer_eu_vat_no, reverse_charge) =
        if invoice_data.buyer.address.country_code != "PL" {
            (
                Some(invoice_data.buyer.address.country_code.clone()),
                Some(invoice_data.buyer.nip.clone()),
                REVERSE_CHARGE_SET,
            )
        } else {
            (None, None, REVERSE_CHARGE_UNSET)
        };

    let now = chrono::Local::now().date_naive();
    let invoice = Invoice {
        header: Header {
            system_info: None,
            ..Default::default()
        },
        subject1: Subject1 {
            taxpayer_prefix: buyer_eu_code
                .is_some()
                .then_some(invoice_data.seller.address.country_code.clone()),
            ..SellerBuilder::new(&invoice_data.seller.nip, &invoice_data.seller.name)
                .set_address(
                    &invoice_data.seller.address.country_code,
                    &invoice_data.seller.address.street,
                    &invoice_data.seller.address.building_number,
                    invoice_data.seller.address.flat_number.as_deref(),
                    &invoice_data.seller.address.city,
                    &invoice_data.seller.address.postal_code,
                )
                .build()
        },
        subject2: Subject2 {
            identification_data: Some(IdentificationData2 {
                name: Some(invoice_data.buyer.name.clone()),
                nip: buyer_eu_vat_no
                    .is_none()
                    .then_some(invoice_data.buyer.nip.clone()),
                eu_code: buyer_eu_code,
                eu_vat_number: buyer_eu_vat_no,
                ..Default::default()
            }),
            ..BuyerBuilder::new(&invoice_data.buyer.nip, &invoice_data.buyer.name)
                .set_address(
                    &invoice_data.buyer.address.country_code,
                    &invoice_data.buyer.address.street,
                    &invoice_data.buyer.address.building_number,
                    invoice_data.buyer.address.flat_number.as_deref(),
                    &invoice_data.buyer.address.city,
                    &invoice_data.buyer.address.postal_code,
                )
                .build()
        },
        invoice_body: InvoiceBody {
            invoice_number,
            issue_date: now.format("%Y-%m-%d").to_string(),
            currency_code,
            lines: {
                invoice_data
                    .positions
                    .into_iter()
                    .map(|position| InvoiceLine {
                        currency_rate,
                        ..LineBuilder::new(
                            &position.name,
                            position.count,
                            position.price,
                            position.tax_rate,
                        )
                        .build()
                    })
                    .collect()
            },
            payment: invoice_data.payment_details.map(|payment_details| Payment {
                bank_accounts: vec![BankAccount {
                    account_number: payment_details.account_number,
                    bank_name: Some(payment_details.bank_name),
                    description: None,
                    own_account: None,
                    swift: payment_details.swift,
                }],
                paid: None,
                payment_date: None,
                partial_payment_flag: None,
                partial_payments: Vec::default(),
                payment_terms: payment_details
                    .period
                    .map(|period| {
                        vec![PaymentTerm {
                            date: Some(
                                (now + chrono::TimeDelta::days(i64::from(period)))
                                    .format("%Y-%m-%d")
                                    .to_string(),
                            ),
                            description: None,
                        }]
                    })
                    .unwrap_or_default(),
                payment_method: Some(PAYMENT_METHOD_BANK_TRANSFER),
                other_payment: None,
                payment_description: None,
                factor_bank_accounts: Vec::default(),
                discount: None,
                payment_link: None,
                ip_ksef: None,
            }),
            annotations: Annotations {
                reverse_charge,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    //println!("{invoice:?}");
    let xml = invoice.to_xml()?;
    println!("{xml}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Helper: build the minimum-valid full JSON for InvoiceData
    // -------------------------------------------------------------------------
    fn full_invoice_json() -> &'static str {
        r#"{
            "number": "FV-01-01-26",
            "currency": "PLN",
            "seller": {
                "nip": "1234567890",
                "name": "Seller Corp",
                "address": {
                    "country_code": "PL",
                    "street": "Main St",
                    "building_number": "1",
                    "flat_number": "2",
                    "city": "Warsaw",
                    "postal_code": "00-000"
                }
            },
            "buyer": {
                "nip": "0987654321",
                "name": "Buyer Ltd",
                "address": {
                    "country_code": "PL",
                    "street": "Second Ave",
                    "building_number": "10",
                    "city": "Krakow",
                    "postal_code": "30-300"
                }
            },
            "positions": [
                {
                    "name": "Widget",
                    "count": "2",
                    "price": "19.00",
                    "tax_rate": "23"
                }
            ],
            "payment_details": {
                "bank_name": "Bank PKO",
                "account_number": "10 1010 1010 1010",
                "swift": "PKOPPLPW",
                "period": 14
            }
        }"#
    }

    // -------------------------------------------------------------------------
    // Address deserialization
    // -------------------------------------------------------------------------

    #[test]
    fn test_address_deserialize_with_flat_number() {
        let json = r#"{
            "country_code": "PL",
            "street": "Ulica",
            "building_number": "5",
            "flat_number": "3A",
            "city": "Warsaw",
            "postal_code": "00-001"
        }"#;
        let addr: Address = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(addr.country_code, "PL");
        assert_eq!(addr.street, "Ulica");
        assert_eq!(addr.building_number, "5");
        assert_eq!(addr.flat_number, Some("3A".to_string()));
        assert_eq!(addr.city, "Warsaw");
        assert_eq!(addr.postal_code, "00-001");
    }

    #[test]
    fn test_address_deserialize_without_flat_number() {
        let json = r#"{
            "country_code": "DE",
            "street": "Berliner Str",
            "building_number": "42",
            "city": "Berlin",
            "postal_code": "10115"
        }"#;
        let addr: Address = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(addr.flat_number, None);
        assert_eq!(addr.city, "Berlin");
    }

    #[test]
    fn test_address_deserialize_null_flat_number() {
        let json = r#"{
            "country_code": "PL",
            "street": "Kwiatowa",
            "building_number": "7",
            "flat_number": null,
            "city": "Gdansk",
            "postal_code": "80-001"
        }"#;
        let addr: Address = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(addr.flat_number, None);
    }

    #[test]
    fn test_address_deserialize_missing_required_field_fails() {
        // Missing "city"
        let json = r#"{
            "country_code": "PL",
            "street": "Kwiatowa",
            "building_number": "7",
            "postal_code": "80-001"
        }"#;
        let result: Result<Address, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "expected error when required field is missing"
        );
    }

    // -------------------------------------------------------------------------
    // Subject deserialization
    // -------------------------------------------------------------------------

    #[test]
    fn test_subject_deserialize_valid() {
        let json = r#"{
            "nip": "1234567890",
            "name": "Test Company",
            "address": {
                "country_code": "PL",
                "street": "Test St",
                "building_number": "1",
                "city": "Warsaw",
                "postal_code": "00-001"
            }
        }"#;
        let subject: Subject = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(subject.nip, "1234567890");
        assert_eq!(subject.name, "Test Company");
    }

    #[test]
    fn test_subject_deserialize_missing_nip_fails() {
        let json = r#"{
            "name": "Test Company",
            "address": {
                "country_code": "PL",
                "street": "Test St",
                "building_number": "1",
                "city": "Warsaw",
                "postal_code": "00-001"
            }
        }"#;
        let result: Result<Subject, _> = serde_json::from_str(json);
        assert!(result.is_err(), "expected error when nip is missing");
    }

    // -------------------------------------------------------------------------
    // Position deserialization
    // -------------------------------------------------------------------------

    #[test]
    fn test_position_deserialize_tax_rate_23() {
        let json = r#"{"name": "Item A", "count": "1", "price": "100.00", "tax_rate": "23"}"#;
        let pos: Position = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pos.name, "Item A");
        assert_eq!(pos.tax_rate, TaxRate::Rate23);
    }

    #[test]
    fn test_position_deserialize_tax_rate_8() {
        let json = r#"{"name": "Item B", "count": "2", "price": "50.00", "tax_rate": "8"}"#;
        let pos: Position = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pos.tax_rate, TaxRate::Rate8);
    }

    #[test]
    fn test_position_deserialize_tax_rate_5() {
        let json = r#"{"name": "Item C", "count": "3", "price": "10.00", "tax_rate": "5"}"#;
        let pos: Position = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pos.tax_rate, TaxRate::Rate5);
    }

    #[test]
    fn test_position_deserialize_tax_rate_zw() {
        let json = r#"{"name": "Item D", "count": "1", "price": "0.01", "tax_rate": "zw"}"#;
        let pos: Position = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pos.tax_rate, TaxRate::Zw);
    }

    #[test]
    fn test_position_deserialize_tax_rate_np_i() {
        let json = r#"{"name": "Item E", "count": "1", "price": "200.00", "tax_rate": "np I"}"#;
        let pos: Position = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pos.tax_rate, TaxRate::NpI);
    }

    #[test]
    fn test_position_deserialize_invalid_tax_rate_fails() {
        let json = r#"{"name": "Bad Item", "count": "1", "price": "10.00", "tax_rate": "INVALID"}"#;
        let result: Result<Position, _> = serde_json::from_str(json);
        assert!(result.is_err(), "expected error for invalid tax rate");
    }

    #[test]
    fn test_position_deserialize_decimal_count_and_price() {
        let json = r#"{"name": "Fractional", "count": "2.5", "price": "9.99", "tax_rate": "23"}"#;
        let pos: Position = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pos.count.to_string(), "2.5");
        assert_eq!(pos.price.to_string(), "9.99");
    }

    // -------------------------------------------------------------------------
    // PaymentDetails deserialization
    // -------------------------------------------------------------------------

    #[test]
    fn test_payment_details_full() {
        let json = r#"{
            "bank_name": "PKO Bank",
            "account_number": "10 2030 4050 6070",
            "swift": "PKOPPLPW",
            "period": 30
        }"#;
        let pd: PaymentDetails = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pd.bank_name, "PKO Bank");
        assert_eq!(pd.account_number, "10 2030 4050 6070");
        assert_eq!(pd.swift, Some("PKOPPLPW".to_string()));
        assert_eq!(pd.period, Some(30));
    }

    #[test]
    fn test_payment_details_without_optional_fields() {
        let json = r#"{
            "bank_name": "Bank XYZ",
            "account_number": "00 0000 0000 0000"
        }"#;
        let pd: PaymentDetails = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pd.swift, None);
        assert_eq!(pd.period, None);
    }

    #[test]
    fn test_payment_details_missing_required_bank_name_fails() {
        let json = r#"{"account_number": "00 0000 0000 0000"}"#;
        let result: Result<PaymentDetails, _> = serde_json::from_str(json);
        assert!(result.is_err(), "expected error when bank_name is missing");
    }

    #[test]
    fn test_payment_details_period_max_u16() {
        let json = r#"{
            "bank_name": "Bank",
            "account_number": "0000",
            "period": 65535
        }"#;
        let pd: PaymentDetails = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pd.period, Some(65535));
    }

    // -------------------------------------------------------------------------
    // InvoiceData deserialization
    // -------------------------------------------------------------------------

    #[test]
    fn test_invoice_data_full_deserialization() {
        let data: InvoiceData =
            serde_json::from_str(full_invoice_json()).expect("should deserialize");
        assert_eq!(data.number, Some("FV-01-01-26".to_string()));
        assert_eq!(data.currency, CurrencyCode::new("PLN"));
        assert_eq!(data.seller.nip, "1234567890");
        assert_eq!(data.buyer.nip, "0987654321");
        assert_eq!(data.positions.len(), 1);
        assert!(data.payment_details.is_some());
    }

    #[test]
    fn test_invoice_data_without_payment_details() {
        let json = r#"{
            "number": "FV-02-01-26",
            "currency": "EUR",
            "seller": {
                "nip": "1111111111",
                "name": "Seller",
                "address": {
                    "country_code": "PL",
                    "street": "A",
                    "building_number": "1",
                    "city": "City",
                    "postal_code": "00-000"
                }
            },
            "buyer": {
                "nip": "2222222222",
                "name": "Buyer",
                "address": {
                    "country_code": "PL",
                    "street": "B",
                    "building_number": "2",
                    "city": "Town",
                    "postal_code": "11-111"
                }
            },
            "positions": [
                {"name": "Svc", "count": "1", "price": "500.00", "tax_rate": "23"}
            ]
        }"#;
        let data: InvoiceData = serde_json::from_str(json).expect("should deserialize");
        assert!(data.payment_details.is_none());
        assert_eq!(data.currency, CurrencyCode::new("EUR"));
    }

    #[test]
    fn test_invoice_data_without_number() {
        let json = r#"{
            "currency": "PLN",
            "seller": {
                "nip": "1111111111",
                "name": "S",
                "address": {
                    "country_code": "PL",
                    "street": "A",
                    "building_number": "1",
                    "city": "C",
                    "postal_code": "00-000"
                }
            },
            "buyer": {
                "nip": "2222222222",
                "name": "B",
                "address": {
                    "country_code": "PL",
                    "street": "B",
                    "building_number": "2",
                    "city": "D",
                    "postal_code": "11-111"
                }
            },
            "positions": [
                {"name": "X", "count": "1", "price": "10.00", "tax_rate": "23"}
            ]
        }"#;
        let data: InvoiceData =
            serde_json::from_str(json).expect("should deserialize without number");
        assert_eq!(data.number, None);
    }

    #[test]
    fn test_invoice_data_multiple_positions() {
        let json = r#"{
            "number": "FV-99",
            "currency": "PLN",
            "seller": {
                "nip": "1234567890",
                "name": "Seller",
                "address": {
                    "country_code": "PL",
                    "street": "S",
                    "building_number": "1",
                    "city": "C",
                    "postal_code": "00-000"
                }
            },
            "buyer": {
                "nip": "0987654321",
                "name": "Buyer",
                "address": {
                    "country_code": "PL",
                    "street": "B",
                    "building_number": "2",
                    "city": "D",
                    "postal_code": "11-111"
                }
            },
            "positions": [
                {"name": "Item 1", "count": "1", "price": "10.00", "tax_rate": "23"},
                {"name": "Item 2", "count": "5", "price": "2.50", "tax_rate": "8"},
                {"name": "Item 3", "count": "10", "price": "1.00", "tax_rate": "5"}
            ]
        }"#;
        let data: InvoiceData = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(data.positions.len(), 3);
        assert_eq!(data.positions[0].tax_rate, TaxRate::Rate23);
        assert_eq!(data.positions[1].tax_rate, TaxRate::Rate8);
        assert_eq!(data.positions[2].tax_rate, TaxRate::Rate5);
    }

    #[test]
    fn test_invoice_data_missing_currency_fails() {
        let json = r#"{
            "number": "FV-01",
            "seller": {
                "nip": "1111111111",
                "name": "S",
                "address": {
                    "country_code": "PL",
                    "street": "A",
                    "building_number": "1",
                    "city": "C",
                    "postal_code": "00-000"
                }
            },
            "buyer": {
                "nip": "2222222222",
                "name": "B",
                "address": {
                    "country_code": "PL",
                    "street": "B",
                    "building_number": "2",
                    "city": "D",
                    "postal_code": "11-111"
                }
            },
            "positions": []
        }"#;
        let result: Result<InvoiceData, _> = serde_json::from_str(json);
        assert!(result.is_err(), "expected error when currency is missing");
    }

    // -------------------------------------------------------------------------
    // Invoice number validation logic
    // -------------------------------------------------------------------------

    fn extract_invoice_number(number: Option<String>) -> Result<String, std::io::Error> {
        number
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Missing required field: number",
                )
            })
    }

    #[test]
    fn test_invoice_number_valid() {
        let result = extract_invoice_number(Some("FV-01-01-26".to_string()));
        assert_eq!(result.unwrap(), "FV-01-01-26");
    }

    #[test]
    fn test_invoice_number_trimmed() {
        let result = extract_invoice_number(Some("  FV-01  ".to_string()));
        assert_eq!(result.unwrap(), "FV-01");
    }

    #[test]
    fn test_invoice_number_none_fails() {
        let result = extract_invoice_number(None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_invoice_number_empty_string_fails() {
        let result = extract_invoice_number(Some("".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn test_invoice_number_whitespace_only_fails() {
        let result = extract_invoice_number(Some("   ".to_string()));
        assert!(result.is_err(), "whitespace-only number should fail");
    }

    #[test]
    fn test_invoice_number_tab_whitespace_only_fails() {
        let result = extract_invoice_number(Some("\t\n ".to_string()));
        assert!(result.is_err(), "tab/newline whitespace should fail");
    }

    #[test]
    fn test_invoice_number_trimmed_non_empty_succeeds() {
        // A number that has whitespace but non-empty content after trim
        let result = extract_invoice_number(Some("\nFV-100\t".to_string()));
        assert_eq!(result.unwrap(), "FV-100");
    }

    // -------------------------------------------------------------------------
    // Constant PAYMENT_METHOD_BANK_TRANSFER
    // -------------------------------------------------------------------------

    #[test]
    fn test_payment_method_bank_transfer_constant() {
        assert_eq!(PAYMENT_METHOD_BANK_TRANSFER, 6u8);
    }

    // -------------------------------------------------------------------------
    // Edge cases / regression tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_invoice_data_empty_positions_array() {
        let json = r#"{
            "number": "FV-EMPTY",
            "currency": "PLN",
            "seller": {
                "nip": "1234567890",
                "name": "Seller",
                "address": {
                    "country_code": "PL",
                    "street": "S",
                    "building_number": "1",
                    "city": "C",
                    "postal_code": "00-000"
                }
            },
            "buyer": {
                "nip": "0987654321",
                "name": "Buyer",
                "address": {
                    "country_code": "PL",
                    "street": "B",
                    "building_number": "2",
                    "city": "D",
                    "postal_code": "11-111"
                }
            },
            "positions": []
        }"#;
        let data: InvoiceData = serde_json::from_str(json).expect("should deserialize");
        assert!(data.positions.is_empty());
    }

    #[test]
    fn test_invoice_data_null_number_becomes_none() {
        let json = r#"{
            "number": null,
            "currency": "PLN",
            "seller": {
                "nip": "1111111111",
                "name": "S",
                "address": {
                    "country_code": "PL",
                    "street": "A",
                    "building_number": "1",
                    "city": "C",
                    "postal_code": "00-000"
                }
            },
            "buyer": {
                "nip": "2222222222",
                "name": "B",
                "address": {
                    "country_code": "PL",
                    "street": "B",
                    "building_number": "2",
                    "city": "D",
                    "postal_code": "11-111"
                }
            },
            "positions": []
        }"#;
        let data: InvoiceData = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(data.number, None);
        // Validate that null number → validation error
        let number_result = extract_invoice_number(data.number);
        assert!(number_result.is_err(), "null number should fail validation");
    }

    #[test]
    fn test_position_zero_price_is_valid() {
        let json = r#"{"name": "Free Item", "count": "1", "price": "0", "tax_rate": "23"}"#;
        let pos: Position = serde_json::from_str(json).expect("should deserialize zero price");
        assert_eq!(pos.price.to_string(), "0");
    }

    #[test]
    fn test_address_country_code_preserved_as_provided() {
        // The Address struct stores country_code as a plain String (not normalized)
        let json = r#"{
            "country_code": "de",
            "street": "Str",
            "building_number": "1",
            "city": "Berlin",
            "postal_code": "10001"
        }"#;
        let addr: Address = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(addr.country_code, "de");
    }

    #[test]
    fn test_payment_details_period_boundary_zero() {
        let json = r#"{
            "bank_name": "Bank",
            "account_number": "0000",
            "period": 0
        }"#;
        let pd: PaymentDetails = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(pd.period, Some(0));
    }
}
