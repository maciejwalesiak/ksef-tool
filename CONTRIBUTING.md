CONTRIBUTING

Thanks for contributing! To keep the codebase consistent and CI green, follow these guidelines before committing:

1. Install the Rust toolchain (rustup) and ensure rustfmt and clippy are installed:

   rustup component add rustfmt clippy

2. Run the formatting and lint checks locally before committing:

   cargo fmt --all -- --check
   cargo clippy -- -D warnings

3. Run the test suite:

   cargo test

4. (Recommended) Install pre-commit hooks to automatically run these checks on commit:

   - Install pre-commit (pip): pip install pre-commit
   - From the repository root, install the hooks: pre-commit install
   - To run hooks on all files (e.g., CI check): pre-commit run --all-files

The repository includes a .pre-commit-config.yaml that runs cargo fmt (check mode), cargo clippy (treat warnings as errors) and cargo test. Commits will be blocked locally if any of these checks fail.

If you can't run the hooks locally (CI-only workflows), ensure your PR passes CI which enforces the same checks.
