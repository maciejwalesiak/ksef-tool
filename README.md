# ksef-tool
A tool for generating ksef invoice FA (3) xml file based on input invoice descriptor json.

requirements: rust development environment and tools installed, including cargo

build: ```cargo build```

run: ```./target/debug/ksef-tool </path/to/invoice_data.json>```

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
