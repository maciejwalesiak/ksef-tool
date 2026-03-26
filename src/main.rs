use invoice_gen::{
    fa_3::{
        builder::{BuyerBuilder, LineBuilder, SellerBuilder},
        models::{BankAccount, Header, Invoice, InvoiceBody, Payment, PaymentTerm},
    },
    shared::TaxRate,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::{env, fs::File, io::Read};

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
    period: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct InvoiceData {
    number: Option<String>,
    currency: String,
    seller: Subject,
    buyer: Subject,
    positions: Vec<Position>,
    payment_details: Option<PaymentDetails>,
}

const PAYMENT_METHOD_BANK_TRANSFER: u8 = 6;

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

    let now = chrono::Local::now();

    let invoice = Invoice {
        header: Header {
            system_info: None,
            ..Default::default()
        },
        subject1: SellerBuilder::new(&invoice_data.seller.nip, &invoice_data.seller.name)
            .set_address(
                &invoice_data.seller.address.country_code,
                &invoice_data.seller.address.street,
                &invoice_data.seller.address.building_number,
                invoice_data.seller.address.flat_number.as_deref(),
                &invoice_data.seller.address.city,
                &invoice_data.seller.address.postal_code,
            )
            .build(),
        subject2: BuyerBuilder::new(&invoice_data.buyer.nip, &invoice_data.buyer.name)
            .set_address(
                &invoice_data.buyer.address.country_code,
                &invoice_data.buyer.address.street,
                &invoice_data.buyer.address.building_number,
                invoice_data.buyer.address.flat_number.as_deref(),
                &invoice_data.buyer.address.city,
                &invoice_data.buyer.address.postal_code,
            )
            .build(),
        invoice_body: InvoiceBody {
            invoice_number: invoice_data.number.unwrap_or(
                // TODO: generate incremental number based on last value from given month stored in
                // db
                "XX/XX/XX".to_string(),
            ),
            issue_date: now.format("%Y-%m-%d").to_string(),
            currency_code: invoice_gen::shared::models::CurrencyCode::new(invoice_data.currency),
            lines: {
                invoice_data
                    .positions
                    .into_iter()
                    .map(|position| {
                        LineBuilder::new(
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
                                (now + chrono::TimeDelta::days(period))
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
            ..Default::default()
        },
        ..Default::default()
    };

    //println!("{invoice:?}");
    let xml = invoice.to_xml()?;
    println!("{xml}");

    Ok(())
}
