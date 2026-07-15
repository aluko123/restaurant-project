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

To exercise real sign-in, create a WorkOS environment, enable Google OAuth and Magic Auth, add `http://localhost:5173` as a redirect URI and allowed origin, then set `VITE_WORKOS_CLIENT_ID`. Without it, the web app runs in an explicit unconfigured-auth state.

## Verification

```sh
cargo fmt --all --check
cargo check --workspace
npm run check --prefix apps/web
npm run build --prefix apps/web
```

## Production database connections

Use PlanetScale's PgBouncer endpoint on port `6432` for API traffic. Use a direct TLS-verified connection on port `5432` for SQLx migrations. Give the runtime and migration processes separate least-privilege database roles.
