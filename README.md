# ksef-tool
CLI tool to generate KSeF FA(3) invoice XML from a JSON invoice descriptor; supports seller/buyer data, line items, currency, optional payment details and payment-term days; outputs XML to stdout.

requirements: rust development environment and tools installed, including cargo. Before committing changes, ensure code is formatted and lint-free by running:

- cargo fmt --all -- --check
- cargo clippy -- -D warnings

Both commands must pass without errors or warnings.

build: ```cargo build```

run: ```./target/debug/ksef-tool </path/to/invoice_data.json>```

Logging:

- ksef-tool uses the log crate with env_logger. The default log level is "info" if RUST_LOG is not set. To control verbosity set RUST_LOG. Examples:
  - `RUST_LOG=debug ./target/debug/ksef-tool /path/to/invoice_data.json`
  - `RUST_LOG=error ./target/debug/ksef-tool /path/to/invoice_data.json`

The RUST_LOG environment variable controls which log events are emitted.

Example of invoice descriptor json:  

```json
{
  "number": "FV-01-01-26",
  "currency": "PLN",
  "seller": {
    "nip": "1234567890",
    "name": "company name",
    "address": {
      "country_code": "PL",
      "street": "ulica",
      "building_number": "1",
      "flat_number": "2",
      "city": "Warszawa",
      "postal_code": "00 - 000"
    }
  },
  "buyer": {
    "nip": "0987654321",
    "name": "firma z o.o.",
    "address": {
      "country_code": "PL",
      "street": "inna ulica",
      "building_number": "11",
      "city": "Warszawa",
      "postal_code": "00 - 003"
    }
  },
  "positions": [
    {
      "name": "Produkt najlepszy",
      "count": 16,
      "price": 19.00,
      "tax_rate": "23"
    }
  ],
  "payment_details": {
    "bank_name": "Bank Hajsowy",
    "account_number": "10 10 1010 1010101",
    "swift": "SIWFTW",
    "period": 5
  }
}
```

Validation

ksef-tool performs post-deserialization validation of the input JSON and will fail with a non-zero exit code if violations are found. Validation focuses on presence, basic formats and cross-field business rules. Typical rules:

- number: optional when deserializing, but main requires a non-empty trimmed invoice number before generating XML.
- currency: must be a 3-letter currency code (e.g., PLN, EUR). Non-PL currencies trigger an exchange-rate lookup at runtime.
- seller: nip required; if seller.address.country_code == "PL" seller.nip must contain 10 digits.
- buyer: nip is optional; if present and country_code == "PL" it must contain 10 digits.
- address: country_code (2 letters), street, building_number, city, postal_code must be present (postal_code format is not enforced by default).
- positions: must contain at least one position; each position requires name, count > 0 and price >= 0; tax_rate must be one of supported tags (parsed by the existing TaxRate enum).
- payment_details (optional): account_number required when present; swift must be 8 or 11 alphanumeric characters if provided; period is an unsigned 16-bit value.

Example validation error output (stderr):

```
Validation error at positions: positions array must contain at least one position
Validation error at seller.nip: PL NIP must contain 10 digits
```

Configuration

Future work may add toggles for stricter, country-specific validations (e.g., postal code regex for PL). Current behavior keeps validation conservative to avoid rejecting valid international inputs.
