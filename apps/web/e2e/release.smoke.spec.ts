import { expect, test } from "@playwright/test";

const releaseRoutes = [
  "/",
  "/today",
  "/brief",
  "/invoices",
  "/sales",
  "/menu",
  "/inventory",
  "/losses",
  "/settings",
];

for (const route of releaseRoutes) {
  test(`unconfigured authentication is safe at ${route}`, async ({ page }) => {
    const apiRequests: string[] = [];
    const pageErrors: string[] = [];
    page.on("request", (request) => {
      if (request.url().includes("/v1/")) apiRequests.push(request.url());
    });
    page.on("pageerror", (error) => pageErrors.push(error.message));

    const response = await page.goto(route);

    expect(response?.status()).toBe(200);
    await expect(page).toHaveTitle("Parline — Know what changed. Protect the next shift.");
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Know what changed.");
    await expect(page.getByRole("button", { name: /Start with Parline/i })).toBeDisabled();
    await expect(page.getByText("Dallas pilot · 01").first()).toBeVisible();
    expect(apiRequests).toEqual([]);
    expect(pageErrors).toEqual([]);
  });
}

test("landing shell remains usable at a kitchen-phone viewport", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");

  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await expect(page.getByText("01 · Snap invoices")).toBeVisible();
  await expect(page.getByText("02 · Count what matters")).toBeVisible();
  await expect(page.getByText("03 · Work the brief")).toBeVisible();
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
  ).toBe(true);
});
