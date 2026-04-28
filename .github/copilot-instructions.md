# Copilot instructions for ksef-tool

## Quick commands
- Build: `cargo build`
- Run (dev): `cargo run -- /path/to/invoice_data.json` (binary expects a single argument: path to JSON)
- Run built binary: `./target/debug/ksef-tool /path/to/invoice_data.json`
- Tests (full): `cargo test`
- Run a single test: `cargo test test_name` (example: `cargo test test_address_deserialize_with_flat_number`)
- CI: GitHub Actions runs `cargo build --verbose` and `cargo test --verbose` (.github/workflows/rust.yml)

> Note: repository does not include a custom lint script; before committing changes run formatting and lints:
> - `cargo fmt --all -- --check`
> - `cargo clippy -- -D warnings`
> 
> Both commands must pass without errors or warnings. The repository CI enforces these checks.

## High-level architecture
- Single binary crate; entry point: `src/main.rs`.
- Purpose: convert a JSON invoice descriptor into KSeF FA(3) invoice XML using the `invoice-gen` crate and print XML to stdout.
- Main flow: parse JSON (serde) -> validate/extract invoice number -> optionally fetch currency rate from NBP (blocking reqwest, 5s timeout) if currency != PLN -> build `Invoice` via `invoice-gen` builders -> `to_xml()` -> stdout.
- Payment details, bank accounts and payment terms are mapped from optional `payment_details` in the JSON into `Payment`/`BankAccount`/`PaymentTerm` structures.
- Reverse-charge handling: buyer.address.country_code != "PL" toggles reverse-charge annotations and sets EU VAT fields.
- Tests: unit tests are colocated in `src/main.rs` under `#[cfg(test)]` and focus on JSON deserialization, validation helpers, and small edge cases.

## Key conventions (project-specific)
- CLI contract: exactly one argument required (path to JSON). Missing or invalid args cause the program to exit with code 1.
- Invoice number: treated as required for runtime execution; JSON may omit it for deserialization tests, but `main` enforces a non-empty trimmed value. There's a TODO to implement automatic incremental numbering.
- JSON schema highlights:
  - `currency` is required (string mapped to `CurrencyCode`);
  - `seller` must include `nip`, `name`, and `address` (address contains `country_code`, `street`, `building_number`, `city`, `postal_code`; `flat_number` is optional);
  - `positions` entries expect `name`, `count`, `price` (strings or numbers parsed by `rust_decimal`) and `tax_rate` (string values mapped to `TaxRate`, e.g. `"23"`, `"8"`, `"5"`, `"zw"`, `"np I"`).
- External API: currency exchange uses the NBP public API and is a network call; main uses a blocking client with 5s timeout. Avoid depending on this in unit tests.
- Tests are named plainly and run by `cargo test`; to run a single test, pass the test name exactly. Many tests target deserialization behavior rather than binary execution.
- The crate depends on a git-based `invoice-gen` crate (see Cargo.toml). Be mindful when switching network or offline builds.

## Where to look next
- `README.md` — usage example and sample invoice JSON.
- `Cargo.toml` — dependency list and versions.
- `.github/workflows/rust.yml` — CI steps.

---

If you'd like adjustments or want coverage for additional areas (examples, contributor workflow, or adding linting CI), say which area to expand.
