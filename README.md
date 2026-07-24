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

For the **staging MVP**, point Fly `DATABASE_URL` at PlanetScale's **direct TLS** endpoint on port `5432` only. The API still runs SQLx migrations on boot against that same URL. Split pooler vs migrator URLs later if you need them.

## Staging deploy (MVP)

Target stack: **API on Fly.io**, **Postgres on PlanetScale**, **web on Cloudflare Pages**, plus existing **R2**, **WorkOS**, and **Gemini**.

### Prerequisites

- [flyctl](https://fly.io/docs/flyctl/install/) installed and `fly auth login`
- PlanetScale **Postgres** database (not MySQL)
- Cloudflare account with Pages + R2
- WorkOS AuthKit client (Google OAuth + Magic Auth)
- Paid Gemini API key for real invoices
- Local secrets from `apps/api/.env` / `apps/web/.env` as a reference (never commit them)

### 1. PlanetScale

1. Create a Postgres database (prefer a region near Fly, e.g. US East for `iad`).
2. Copy the **direct** connection string on port **`5432`** with TLS.
3. Do **not** use the PgBouncer `:6432` URL for this MVP while migrations run on API boot.

**SQLx / Rust note:** PlanetScale's default string often includes `sslrootcert=system`. That is a libpq shortcut; SQLx treats it as a **file path** and the API fails with `No such file or directory (os error 2)`. For this app use one of:

```text
# simplest (works with sqlx + rustls webpki roots)
postgres://USER:PASS@HOST:5432/DB?sslmode=require

# or verify against the container CA bundle (Dockerfile installs ca-certificates)
postgres://USER:PASS@HOST:5432/DB?sslmode=verify-full&sslrootcert=/etc/ssl/certs/ca-certificates.crt
```

Drop `sslrootcert=system`. If the password has special characters, URL-encode them.

### 2. Fly API

Config lives in [`fly.toml`](fly.toml) (port `8080`, `/health/live` + `/health/ready`, **`min_machines_running = 1`** so invoice/menu extraction workers keep running).

```sh
# First time only — creates the app if needed; adjust app name/region in fly.toml first
fly apps create parline-api   # skip if the app already exists
fly deploy                    # builds apps/api/Dockerfile from repo root
```

Set secrets (use your real values; leave `WEB_ORIGIN` until the Pages URL exists, or set a temporary value and update later):

```sh
fly secrets set \
  DATABASE_URL='postgres://USER:PASS@HOST:5432/DB?sslmode=require' \
  WEB_ORIGIN='https://YOUR_PAGES_HOST' \
  WORKOS_ISSUER='https://api.workos.com/user_management/client_YOUR_CLIENT_ID' \
  WORKOS_JWKS_URL='https://api.workos.com/sso/jwks/client_YOUR_CLIENT_ID' \
  R2_ACCOUNT_ID='...' \
  R2_ACCESS_KEY_ID='...' \
  R2_SECRET_ACCESS_KEY='...' \
  R2_BUCKET='...' \
  GEMINI_API_KEY='...' \
  GEMINI_MODEL='gemini-3.5-flash'
# optional team invites:
# fly secrets set WORKOS_API_KEY='sk_...'
```

Verify:

```sh
curl -sS https://parline-api.fly.dev/health/live
curl -sS https://parline-api.fly.dev/health/ready
```

Both should return `{"status":"ok"}`. Use your real Fly hostname if the app name differs.

### 3. Cloudflare Pages (web)

1. Create a Pages project (Git-connected or direct upload via Wrangler).
2. Build settings:
   - Root directory: `apps/web`
   - Build command: `npm ci && npm run build`
   - Output directory: `dist`
3. Build environment variables:
   - `VITE_API_URL=https://parline-api.fly.dev` (no trailing slash)
   - `VITE_WORKOS_CLIENT_ID=client_...`
4. SPA routing is already handled by `apps/web/public/_redirects`.
5. Deploy and note the HTTPS origin (e.g. `https://parline-xxxx.pages.dev`).

### 4. Align origins (CORS + WorkOS)

1. In WorkOS, add the Pages origin as an allowed origin and redirect URI (and AuthKit invitation accept URL if invites are enabled).
2. Set Fly CORS to the **exact** Pages origin (scheme + host, **no trailing slash**):

```sh
fly secrets set WEB_ORIGIN='https://YOUR_PAGES_HOST'
```

3. Confirm `WORKOS_ISSUER` / `WORKOS_JWKS_URL` match the same client ID as `VITE_WORKOS_CLIENT_ID`.

### 5. Smoke checklist

1. `GET /health/live` and `GET /health/ready` → ok  
2. Open the Pages URL → sign in (Google or Magic Auth)  
3. Complete owner onboarding  
4. Upload a sample invoice → extraction reaches needs-review or ready  
5. Open the signed invoice URL  
6. Hit one owner path (Today or Settings)  

Optional authenticated Playwright smoke (storage state is credential material; never commit it):

```sh
E2E_BASE_URL=https://YOUR_PAGES_HOST \
E2E_STORAGE_STATE="$PWD/apps/web/.auth/owner.json" \
npm run test:e2e:authenticated --prefix apps/web
```

### Staging notes

- Extraction and menu-import workers run **inside** the API process. Keep **`min_machines_running = 1`** so jobs are not delayed until the next HTTP request.
- First Docker build on Fly is slow (Rust release compile); later deploys are faster with cache.
- Invoice “today” is the restaurant’s IANA timezone (Settings), not UTC.
