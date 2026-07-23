# Parline

**Know what changed. Protect the next shift.**

Parline is a mobile-first restaurant operations app that helps independent restaurants turn invoices, inventory counts, and waste logs into a short list of daily actions.

## Architecture

- `apps/web`: React + TypeScript + Vite PWA, intended for Cloudflare Pages
- `apps/api`: safe Rust + Axum API, intended for Fly.io
- PostgreSQL: local Docker for development and PlanetScale Postgres in production
- Authentication: WorkOS AuthKit with Google OAuth and Magic Auth
- Invoice objects: private Cloudflare R2 bucket, uploaded through the authenticated API

WorkOS establishes identity. The API and PostgreSQL remain the source of truth for restaurants, memberships, and the `owner`, `manager`, and `staff` roles.

Owner-only team invitations use the application-wide WorkOS AuthKit Invitation API, without WorkOS organizations. Configure AuthKit's default invitation URL and email template, then set the optional server-side `WORKOS_API_KEY` to enable them. When it is absent, the API still starts, settings report invitations as disabled, and invitation mutations return `503`. After acceptance and sign-in, `GET /v1/me` lazily grants the PostgreSQL role only when the verified WorkOS user, accepted invitation subject, provider email, and exact normalized local email all match. Invitation tokens and accept URLs are never stored.

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

WorkOS AuthKit SPA access tokens use a client-specific issuer and do not include an `aud` claim. The API binds tokens to this application through both the exact client-specific issuer and JWKS URL, and still requires an RS256 signature, expiration, subject, and session ID. Without the web client ID, the landing preview stays in an explicit unconfigured-auth state and does not call protected APIs. Live WorkOS validation requires credentials and is not covered by the local verification commands below.

### Private invoice storage

Create a private Cloudflare R2 bucket and an R2 API token with object read/write access to that bucket. Set `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, and `R2_BUCKET` in `apps/api/.env`. These values are required when the API starts; do not commit them. Browser CORS is not needed because uploads go through the API, and originals are opened with five-minute signed URLs.

### Invoice extraction

Set `GEMINI_API_KEY` and optionally `GEMINI_MODEL` (default: the stable `gemini-3.5-flash` model). Use a **paid Gemini API tier for real invoices**: Google states that free-tier content may be used to improve its products. The API sends private R2 bytes directly to Gemini, records the configured model and token usage, and runs a bounded, durable PostgreSQL-backed worker. Ambiguous network failures can be retried because exactly-once provider calls are not possible; stored results and line items are idempotently replaced.

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

## Release gates

Run the complete local release gate from the repository root:

```sh
./scripts/release-gate.sh
```

The gate starts the Compose PostgreSQL service only when port 5432 is unavailable, installs locked web dependencies and Chromium, and runs Rust formatting, compilation, Clippy, unit tests, PostgreSQL release tests, TypeScript checks, a production build, and credential-free Playwright smoke tests. The PostgreSQL tests create uniquely named disposable databases, test all current migrations (including the latest forward migration), upgrade from the immediately previous migration with existing data, and drop only databases they created.

API release coverage uses signed RS256 requests through the real Axum router for two tenants and the `owner`, `manager`, and `staff` boundaries. The financial brief and invoice brief remain owner-only. Credential-free browser coverage visits every direct release path, including `/settings` and `/sales`, at desktop and 390px widths to verify the explicit unconfigured-auth fallback makes no protected API calls; it does not exercise authenticated workspaces.

Set `TEST_DATABASE_URL` to use another PostgreSQL server whose user can create and drop disposable databases. Set `RELEASE_GATE_EXTERNAL_POSTGRES=1` for a server managed outside Compose. CI runs the same gate from [`.github/workflows/release-gates.yml`](.github/workflows/release-gates.yml).

### Credential-dependent WorkOS smoke

The default gate does not fake a WorkOS session or add an auth bypass. For an authenticated owner smoke against a deployed test environment, save a short-lived Playwright storage state after interactive sign-in, then run:

```sh
mkdir -p apps/web/.auth
npm exec --prefix apps/web -- playwright codegen \
  --save-storage="$PWD/apps/web/.auth/owner.json" \
  https://your-test-environment.example.com/today

E2E_BASE_URL=https://your-test-environment.example.com \
E2E_STORAGE_STATE="$PWD/apps/web/.auth/owner.json" \
npm run test:e2e:authenticated --prefix apps/web
```

The ignored storage state is credential material: never commit it or use it against production. The authenticated smoke is read-only and remains outside credential-free CI.

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

## Production database connections

Use PlanetScale's PgBouncer endpoint on port `6432` for API traffic. Use a direct TLS-verified connection on port `5432` for SQLx migrations. Give the runtime and migration processes separate least-privilege database roles.
