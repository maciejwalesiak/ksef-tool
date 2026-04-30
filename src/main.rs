use core::result::Result;
use invoice_gen::{
    fa_3::{
        builder::{BuyerBuilder, LineBuilder, SellerBuilder},
        models::{
            Annotations, BankAccount, Header, IdentificationData2, Invoice, InvoiceBody,
            InvoiceLine, Payment, PaymentTerm, Subject1, Subject2,
        },
    },
    shared::{CountryCode, CurrencyCode, TaxRate},
};
use log::error;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::{env, fs::File, io::Read, time::Duration};
use thiserror::Error;

#[derive(Debug, Deserialize)]
struct Address {
    country_code: CountryCode,
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

mod validation;

#[derive(Debug, Error)]
enum CurrencyExchangeRateError {
    #[error("currency exchange rate request error: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("{0} currency exchange rate is missing")]
    RateMissing(String),
    #[error("{0} currency exchange rate value is invalid")]
    InvalidRate(String),
}

#[derive(Debug, Error)]
enum ToolError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("currency exchange error: {0}")]
    CurrencyExchange(#[from] CurrencyExchangeRateError),
    #[error("validation errors: {0}")]
    Validation(validation::ValidationErrors),
    #[error("invoice generation error: {0}")]
    InvoiceGen(String),
}

/// Fetches the mid exchange rate for `currency_code` from the NBP API and returns it rounded to 4 decimal places.
///
/// The function queries the NBP exchangerates endpoint, parses the first returned `mid` rate, converts it to `Decimal`,
/// and rounds it to four decimal places using midpoint-away-from-zero rounding.
///
/// # Returns
///
/// `Ok(Decimal)` containing the mid exchange rate rounded to 4 decimal places, or `Err(CurrencyExchangeRateError)` if the HTTP request or JSON parsing fails, if no rate is present for the currency, or if the parsed rate is invalid (zero).
///
/// # Examples
///
/// ```no_run
/// use rust_decimal::Decimal;
/// // Assume `CurrencyCode::new("USD")` constructs a CurrencyCode; adapt to your crate's API as needed.
/// let code = CurrencyCode::new("USD");
/// match get_currency_exchange_rate(&code) {
///     Ok(rate) => println!("Rate: {}", rate), // rate is a Decimal rounded to 4 dp
///     Err(e) => eprintln!("Failed to fetch rate: {}", e),
/// }
/// ```
fn get_currency_exchange_rate_with_base(
    client: &reqwest::blocking::Client,
    currency_code: &CurrencyCode,
    base: &str,
) -> Result<Decimal, CurrencyExchangeRateError> {
    const MAX_RETRIES: u32 = 3;
    const BACKOFF_BASE_MS: u64 = 200;

    let url = format!(
        "{}/api/exchangerates/rates/A/{}/last/1/?format=json",
        base, currency_code,
    );

    for attempt in 0..MAX_RETRIES {
        match client.get(&url).send() {
            Ok(resp) => {
                // Inspect HTTP status before attempting to parse JSON
                match resp.error_for_status() {
                    Ok(success_resp) => match success_resp.json::<NbpResponse>() {
                        Ok(response) => {
                            let mid = response.rates.first().map(|r| r.mid).ok_or(
                                CurrencyExchangeRateError::RateMissing(
                                    currency_code.as_str().to_string(),
                                ),
                            )?;

                            // from_f64_retain returns Option<Decimal> when NaN/Inf; treat as invalid
                            let dec = Decimal::from_f64_retain(mid).ok_or(
                                CurrencyExchangeRateError::InvalidRate(
                                    currency_code.as_str().to_string(),
                                ),
                            )?;

                            let dec = dec.round_dp_with_strategy(
                                4,
                                rust_decimal::RoundingStrategy::MidpointAwayFromZero,
                            );

                            if dec == Decimal::ZERO {
                                return Err(CurrencyExchangeRateError::InvalidRate(
                                    currency_code.as_str().to_string(),
                                ));
                            }

                            return Ok(dec);
                        }
                        Err(e) => {
                            // JSON parsing error is not transient; return immediately
                            return Err(CurrencyExchangeRateError::RequestError(e));
                        }
                    },
                    Err(e) => {
                        let status = e.status();
                        if status == Some(reqwest::StatusCode::NOT_FOUND) {
                            return Err(CurrencyExchangeRateError::RateMissing(
                                currency_code.as_str().to_string(),
                            ));
                        }

                        let retryable = matches!(
                            status,
                            Some(
                                reqwest::StatusCode::TOO_MANY_REQUESTS
                                    | reqwest::StatusCode::INTERNAL_SERVER_ERROR
                                    | reqwest::StatusCode::BAD_GATEWAY
                                    | reqwest::StatusCode::SERVICE_UNAVAILABLE
                                    | reqwest::StatusCode::GATEWAY_TIMEOUT
                            )
                        );

                        if !retryable || attempt + 1 == MAX_RETRIES {
                            return Err(CurrencyExchangeRateError::RequestError(e));
                        }
                    }
                }
            }
            Err(e) => {
                // Network-level error: retry unless this was the last attempt
                if attempt + 1 == MAX_RETRIES {
                    return Err(CurrencyExchangeRateError::RequestError(e));
                }
                // otherwise sleep and retry
            }
        }

        if attempt + 1 < MAX_RETRIES {
            let backoff_ms = BACKOFF_BASE_MS * (1u64 << attempt);
            std::thread::sleep(Duration::from_millis(backoff_ms));
        }
    }

    // Should not be reachable because errors return early
    unreachable!("request loop exited without producing a result or error");
}

fn get_currency_exchange_rate_with_client(
    client: &reqwest::blocking::Client,
    currency_code: &CurrencyCode,
) -> Result<Decimal, CurrencyExchangeRateError> {
    let base = std::env::var("NBP_API_BASE").unwrap_or_else(|_| "https://api.nbp.pl".to_string());
    get_currency_exchange_rate_with_base(client, currency_code, &base)
}

fn get_currency_exchange_rate(
    currency_code: &CurrencyCode,
) -> Result<Decimal, CurrencyExchangeRateError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    get_currency_exchange_rate_with_client(&client, currency_code)
}

/// Reads invoice data from a JSON file path given as the sole command-line argument, constructs an Invoice
/// (including optional currency exchange rate lookup for non-PLN currencies and reverse-charge handling),
/// serializes the invoice to XML, and prints the XML to stdout.
///
/// The program expects exactly one argument: the path to a JSON file containing `InvoiceData`.
/// If the argument is missing or invalid invoice data is encountered (for example missing invoice number,
/// I/O errors, JSON deserialization errors, exchange-rate fetch failures, or XML serialization errors),
/// the function returns an error or exits with code 1 when the argument count is incorrect.
///
/// # Examples
///
/// ```no_run
/// // Run the compiled binary with a path to an invoice JSON file:
/// // cargo run -- /path/to/invoice_data.json
/// ```
///
/// Returns:
/// - `Ok(())` on successful processing and printing of the generated XML.
/// - `Err(...)` if any I/O, deserialization, exchange-rate retrieval, or XML serialization error occurs.
fn main() -> Result<(), ToolError> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        error!("Usage: {} </path/to/invoice_data.json>", args[0]);
        std::process::exit(1);
    }

    let file_path = &args[1];
    let mut file = File::open(file_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let invoice_data: InvoiceData = serde_json::from_str(&contents)?;
    // println!("{:?}", invoice_data);

    // Validate the invoice data for required fields and business rules before building the Invoice
    if let Err(errors) = validation::validate_invoice_data(&invoice_data) {
        for err in &errors {
            error!("Validation error at {}: {}", err.path, err.message);
        }
        return Err(ToolError::Validation(errors));
    }

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
        if invoice_data.buyer.address.country_code.as_str() != "PL" {
            (
                Some(invoice_data.buyer.address.country_code.as_str().to_string()),
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
            taxpayer_prefix: buyer_eu_code.is_some().then_some(
                invoice_data
                    .seller
                    .address
                    .country_code
                    .as_str()
                    .to_string(),
            ),
            ..SellerBuilder::new(&invoice_data.seller.nip, &invoice_data.seller.name)
                .set_address(
                    invoice_data.seller.address.country_code.as_str(),
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
                    invoice_data.buyer.address.country_code.as_str(),
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
    let xml = invoice
        .to_xml()
        .map_err(|e| ToolError::InvoiceGen(e.to_string()))?;
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
                "nip": "8567346215",
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
                "nip": "1765432897",
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
        assert_eq!(addr.country_code.as_str(), "PL");
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
            "nip": "8567346215",
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
        assert_eq!(subject.nip, "8567346215");
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
        assert_eq!(data.seller.nip, "8567346215");
        assert_eq!(data.buyer.nip, "1765432897");
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
                "nip": "8567346215",
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
                "nip": "1765432897",
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
                "nip": "8567346215",
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
                "nip": "1765432897",
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
        assert_eq!(addr.country_code.as_str(), "de");
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

    #[test]
    fn test_validation_accepts_full_invoice() {
        let data: InvoiceData =
            serde_json::from_str(full_invoice_json()).expect("should deserialize");
        assert!(validation::validate_invoice_data(&data).is_ok());
    }

    #[test]
    fn test_validation_rejects_empty_positions() {
        let json = r#"{
            "number": "FV-EMPTY",
            "currency": "PLN",
            "seller": {
                "nip": "8567346215",
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
                "nip": "1765432897",
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
        assert!(validation::validate_invoice_data(&data).is_err());
    }

    #[test]
    fn test_validation_allows_buyer_empty_nip() {
        let json = r#"{
            "number": "FV-01",
            "currency": "PLN",
            "seller": {
                "nip": "8567346215",
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
                "nip": "",
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
                {"name": "Item", "count": "1", "price": "10.00", "tax_rate": "23"}
            ]
        }"#;
        let data: InvoiceData = serde_json::from_str(json).expect("should deserialize");
        assert!(validation::validate_invoice_data(&data).is_ok());
    }

    #[test]
    fn test_validation_allows_non_pl_seller_nip_of_different_length() {
        let json = r#"{
            "number": "FV-INT",
            "currency": "PLN",
            "seller": {
                "nip": "ABC123",
                "name": "Intl Seller",
                "address": {
                    "country_code": "DE",
                    "street": "Some Str",
                    "building_number": "9",
                    "city": "Berlin",
                    "postal_code": "10115"
                }
            },
            "buyer": {
                "nip": "1765432897",
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
                {"name": "Item", "count": "1", "price": "10.00", "tax_rate": "23"}
            ]
        }"#;
        let data: InvoiceData = serde_json::from_str(json).expect("should deserialize");
        // seller NIP is required but non-PL NIP length is not enforced
        assert!(validation::validate_invoice_data(&data).is_ok());
    }

    #[test]
    fn test_validation_rejects_invalid_swift_and_empty_account_number() {
        let json = r#"{
            "number": "FV-ERR",
            "currency": "PLN",
            "seller": {
                "nip": "8567346215",
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
                "nip": "1765432897",
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
                {"name": "Item", "count": "1", "price": "10.00", "tax_rate": "23"}
            ],
            "payment_details": {
                "bank_name": "Bank",
                "account_number": "",
                "swift": "BAD-SW!FT",
                "period": 10
            }
        }"#;
        let data: InvoiceData = serde_json::from_str(json).expect("should deserialize");
        // empty account_number and invalid SWIFT should fail validation
        assert!(validation::validate_invoice_data(&data).is_err());
    }

    // -------------------------------------------------------------------------
    // Validation: PL NIP checksum
    // -------------------------------------------------------------------------

    fn pl_nip_check_digit(digits9: &str) -> u32 {
        let weights = [6u32, 5, 7, 2, 3, 4, 5, 6, 7];
        let sum: u32 = digits9
            .chars()
            .zip(weights.iter())
            .map(|(c, w)| c.to_digit(10).unwrap() * (*w))
            .sum();
        let checksum = sum % 11;
        if checksum == 10 { 0 } else { checksum }
    }

    #[test]
    fn test_validation_rejects_invalid_pl_nip_checksum() {
        let json = r#"{
            "number": "FV-NIP-ERR",
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
                "nip": "1765432897",
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
                {"name": "Item", "count": "1", "price": "10.00", "tax_rate": "23"}
            ]
        }"#;
        let data: InvoiceData = serde_json::from_str(json).expect("should deserialize");
        assert!(validation::validate_invoice_data(&data).is_err());
    }

    #[test]
    fn test_validation_accepts_valid_pl_nip_checksum() {
        let digits9 = "856734621";
        let check = pl_nip_check_digit(digits9);
        let full = format!("{}{}", digits9, check);

        let seller = serde_json::json!({
            "nip": full,
            "name": "Seller",
            "address": {
                "country_code": "PL",
                "street": "S",
                "building_number": "1",
                "city": "C",
                "postal_code": "00-000"
            }
        });

        let buyer = serde_json::json!({
            "nip": "1765432897",
            "name": "Buyer",
            "address": {
                "country_code": "PL",
                "street": "B",
                "building_number": "2",
                "city": "D",
                "postal_code": "11-111"
            }
        });

        let json_value = serde_json::json!({
            "number": "FV-NIP-OK",
            "currency": "PLN",
            "seller": seller,
            "buyer": buyer,
            "positions": [
                {"name": "Item", "count": "1", "price": "10.00", "tax_rate": "23"}
            ]
        });

        let json = serde_json::to_string(&json_value).unwrap();
        let data: InvoiceData = serde_json::from_str(&json).expect("should deserialize");
        assert!(validation::validate_invoice_data(&data).is_ok());
    }

    // -------------------------------------------------------------------------
    // Currency exchange rate fetching (uses injectable base URL via NBP_API_BASE)
    // -------------------------------------------------------------------------

    #[test]
    fn test_get_currency_exchange_rate_success() {
        use httpmock::MockServer;
        let server = MockServer::start();

        let body = r#"{ "table": "A", "currency": "US DOLLAR", "code": "USD", "rates": [{ "no": "001/A/NBP/2026", "effectiveDate": "2026-01-01", "mid": 4.1234 }] }"#;
        let m = server.mock(|when, then| {
            when.method("GET")
                .path("/api/exchangerates/rates/A/USD/last/1/")
                .query_param("format", "json");
            then.status(200)
                .header("Content-Type", "application/json")
                .body(body);
        });

        let currency_code: CurrencyCode = serde_json::from_str("\"USD\"").unwrap();
        let client = reqwest::blocking::Client::builder().build().unwrap();
        let rate =
            get_currency_exchange_rate_with_base(&client, &currency_code, &server.base_url())
                .unwrap();
        assert_eq!(rate.to_string(), "4.1234");

        m.assert();
    }

    #[test]
    fn test_get_currency_exchange_rate_missing() {
        use httpmock::MockServer;
        let server = MockServer::start();

        let body = r#"{ "table": "A", "currency": "US DOLLAR", "code": "USD", "rates": [] }"#;
        let m = server.mock(|when, then| {
            when.method("GET")
                .path("/api/exchangerates/rates/A/USD/last/1/")
                .query_param("format", "json");
            then.status(200)
                .header("Content-Type", "application/json")
                .body(body);
        });

        let currency_code: CurrencyCode = serde_json::from_str("\"USD\"").unwrap();
        let client = reqwest::blocking::Client::builder().build().unwrap();
        let err = get_currency_exchange_rate_with_base(&client, &currency_code, &server.base_url())
            .unwrap_err();
        match err {
            CurrencyExchangeRateError::RateMissing(code) => assert_eq!(code, "USD"),
            _ => panic!("expected RateMissing"),
        }

        m.assert();
    }

    #[test]
    fn test_is_valid_pl_nip_rejects_formatted_inputs() {
        // Formatted inputs containing spaces or dashes should be rejected
        assert!(!validation::is_valid_pl_nip("856 734 6215"));
        assert!(!validation::is_valid_pl_nip("856-734-6215"));
    }

    #[test]
    fn test_is_valid_pl_nip_rejects_non_digits() {
        assert!(!validation::is_valid_pl_nip("85A7346215"));
    }

    #[test]
    fn test_get_currency_exchange_rate_zero_is_invalid() {
        use httpmock::MockServer;
        let server = MockServer::start();

        let body = r#"{ "table": "A", "currency": "US DOLLAR", "code": "USD", "rates": [{ "no": "001/A/NBP/2026", "effectiveDate": "2026-01-01", "mid": 0.0 }] }"#;
        let m = server.mock(|when, then| {
            when.method("GET")
                .path("/api/exchangerates/rates/A/USD/last/1/")
                .query_param("format", "json");
            then.status(200)
                .header("Content-Type", "application/json")
                .body(body);
        });

        let currency_code: CurrencyCode = serde_json::from_str("\"USD\"").unwrap();
        let client = reqwest::blocking::Client::builder().build().unwrap();
        let err = get_currency_exchange_rate_with_base(&client, &currency_code, &server.base_url())
            .unwrap_err();
        match err {
            CurrencyExchangeRateError::InvalidRate(code) => assert_eq!(code, "USD"),
            _ => panic!("expected InvalidRate"),
        }

        m.assert();
    }
}
