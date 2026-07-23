import { defineConfig } from "@playwright/test";

const externalBaseUrl = process.env.E2E_BASE_URL?.trim();
const localBaseUrl = "http://127.0.0.1:4173";

export default defineConfig({
  testDir: "./e2e",
  outputDir: "test-results",
  reporter: process.env.CI ? "github" : "list",
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  use: {
    baseURL: externalBaseUrl || localBaseUrl,
    trace: "retain-on-failure",
  },
  webServer: externalBaseUrl
    ? undefined
    : {
        command: "npm run preview -- --host 127.0.0.1 --port 4173",
        url: localBaseUrl,
        reuseExistingServer: false,
      },
});
