# Restaurant Daily Profit Copilot

A mobile-first restaurant operations app that helps independent restaurants turn invoices, inventory counts, and waste logs into a short list of daily actions.

## Architecture

- `apps/web`: React + TypeScript + Vite PWA, intended for Cloudflare Pages
- `apps/api`: safe Rust + Axum API, intended for Fly.io
- PostgreSQL: local Docker for development and PlanetScale Postgres in production
- Authentication: WorkOS AuthKit with Google OAuth and Magic Auth
- Invoice objects: private Cloudflare R2 bucket (integration follows the first upload flow)

WorkOS establishes identity. The API and PostgreSQL remain the source of truth for restaurants, memberships, and the `owner`, `manager`, and `staff` roles.

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

## Verification

```sh
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check --prefix apps/web
npm run build --prefix apps/web
```

## Production database connections

Use PlanetScale's PgBouncer endpoint on port `6432` for API traffic. Use a direct TLS-verified connection on port `5432` for SQLx migrations. Give the runtime and migration processes separate least-privilege database roles.
