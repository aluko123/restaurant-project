#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export TEST_DATABASE_URL=${TEST_DATABASE_URL:-postgres://restaurant:restaurant@127.0.0.1:5432/postgres}

if [[ -z ${RELEASE_GATE_EXTERNAL_POSTGRES:-} ]]; then
  if ! (echo >/dev/tcp/127.0.0.1/5432) >/dev/null 2>&1; then
    docker compose --project-directory "$ROOT" -f "$ROOT/compose.yaml" up -d --wait postgres
  fi
fi

cargo fmt --manifest-path "$ROOT/Cargo.toml" --all --check
cargo check --manifest-path "$ROOT/Cargo.toml" --workspace
cargo clippy --manifest-path "$ROOT/Cargo.toml" --workspace --all-targets -- -D warnings
cargo test --manifest-path "$ROOT/Cargo.toml" --workspace
TEST_DATABASE_URL="$TEST_DATABASE_URL" cargo test \
  --manifest-path "$ROOT/Cargo.toml" \
  -p restaurant-api release_tests -- --ignored --test-threads=1

npm ci --prefix "$ROOT/apps/web"
if [[ ${CI:-} == true ]]; then
  npm exec --prefix "$ROOT/apps/web" -- playwright install --with-deps chromium
else
  npm run install:e2e --prefix "$ROOT/apps/web"
fi
npm run check --prefix "$ROOT/apps/web"
npm run test:e2e --prefix "$ROOT/apps/web"
