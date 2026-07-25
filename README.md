# Parline

**Know what changed. Protect the next shift.**

Parline is a mobile-first restaurant operations app that helps independent restaurants turn invoices, inventory counts, and waste logs into a short list of daily actions and provide insights into their operations.

## Architecture

- `apps/web`: TypeScript, Clouflare Pages
- `apps/api`: Rust + Fly.io
- PostgreSQL: PlanetScale, Docker
- Authentication: WorkOS AuthKit
- Invoice objects: Cloudflare R2 bucket

## Prerequisites

- Rust 1.93+
- Node.js 20+
- Docker

## Local development

Start PostgreSQL:

```sh
docker compose up -d postgres
```

Start the API:

```sh
cp apps/api/.env.example apps/api/.env
cargo run -p restaurant-api
```

Start the web app in another terminal:

```sh
cp apps/web/.env.example apps/web/.env
npm install --prefix apps/web
npm run dev --prefix apps/web
```

The web app runs at `http://localhost:5173`; the API runs at `http://localhost:8080`.

To exercise real sign-in, create a WorkOS environment, enable Google OAuth and Magic Auth, and add `http://localhost:5173` as a redirect URI and allowed origin. Set the client ID in `VITE_WORKOS_CLIENT_ID` and use its client-specific signing-key URL for `WORKOS_JWKS_URL`:

```env
# apps/web/.env
VITE_WORKOS_CLIENT_ID=client_your_client_id

# apps/api/.env
WORKOS_ISSUER=https://api.workos.com/user_management/client_your_client_id
WORKOS_JWKS_URL=https://api.workos.com/sso/jwks/client_your_client_id
```

### Sales CSV v1

Owners and managers can preview and apply one complete business date from the Sales workspace. Download the in-app template or provide a UTF-8, comma-delimited CSV (optional UTF-8 BOM and standard quoted fields are supported):

```csv
business_date,item_name,quantity,item_code,net_sales,currency
2026-07-21,Chicken Taco,84,TACO-CHICKEN,1008.00,USD
2026-07-21,Chips and Salsa,31,,,
```

- Required headers: `business_date`, `item_name`, `quantity`.
- Optional headers: `item_code`, `net_sales`, `currency`. Header order may vary; unknown and duplicate headers are rejected.
- Every row must use the same ISO `YYYY-MM-DD` business date. Files are limited to 1 MiB and 2,000 data rows.
- Quantity must be greater than zero with at most 6 decimal places. Net sales, when reported, must be nonnegative with at most 4 decimal places and must include a three-letter currency.
- Menu matching uses only the trimmed, case-insensitive item name. Item codes are reference-only; there is no fuzzy or alias matching, and name collisions remain unmatched.
- Every unmatched row must be manually mapped or explicitly excluded. Reported currency must match the selected menu item's currency and is never guessed when missing.
- Applying creates or atomically replaces the canonical sales day using the revision shown in preview. If that day changes first, the apply is rejected and the preview must be refreshed.


## Individual verification commands

```sh
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
TEST_DATABASE_URL=postgres://restaurant:restaurant@localhost:5432/postgres \
  cargo test -p restaurant-api release_tests -- --ignored --test-threads=1
npm run check --prefix apps/web
npm run build --prefix apps/web
npm run test:e2e --prefix apps/web
```

### Flow

```text
feature branch  →  PR  →  Release gates (required)
                              ↓ merge
                           main updated
                              ↓
                    Release gates on main
                              ↓ success
                    Deploy staging (Fly + Pages)
```

```sh
git switch -c my-change
# …commit…
git push -u origin HEAD
gh pr create --fill
# wait for Release gates ✓, then:
gh pr merge --squash
```

After merge, check Actions → **Deploy staging**, then smoke `https://parline-api.fly.dev/health/ready` and `https://parline.pages.dev`.

Manual deploy without a new commit: Actions → **Deploy staging** → **Run workflow**.
