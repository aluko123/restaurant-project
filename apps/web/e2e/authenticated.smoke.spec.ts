import { expect, test } from "@playwright/test";

const storageState = process.env.E2E_STORAGE_STATE?.trim();
const baseUrl = process.env.E2E_BASE_URL?.trim();

test.describe("@authenticated credential-dependent owner smoke", () => {
  test.skip(
    !storageState || !baseUrl,
    "Set E2E_BASE_URL and E2E_STORAGE_STATE for a real WorkOS owner session.",
  );
  test.use({ storageState });

  test("owner routes and primary forms render without mutating data", async ({ page }) => {
    await page.goto("/today");
    await expect(page.getByRole("heading", { name: "Today" })).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Parline sections" })).toBeVisible();

    await page.getByRole("button", { name: "Brief" }).click();
    await expect(page.getByRole("heading", { name: "Current week" })).toBeVisible();

    await page.getByRole("button", { name: "Invoices" }).click();
    await expect(page.locator("#invoice-file")).toHaveAttribute(
      "accept",
      "application/pdf,image/jpeg,image/png,image/webp",
    );

    await page.getByRole("button", { name: "Sales" }).click();
    await expect(page.getByRole("heading", { name: "Record the day" })).toBeVisible();
    await expect(page.locator("#sales-business-date")).toBeVisible();
    await expect(page.locator("#sales-csv-file")).toHaveAttribute("accept", ".csv,text/csv");

    await page.getByRole("button", { name: "Menu" }).click();
    await expect(page.getByRole("heading", { name: / menu$/i })).toBeVisible();
    await expect(page.getByRole("button", { name: "Add menu item" })).toBeVisible();

    await page.getByRole("button", { name: "Inventory" }).click();
    await expect(page.getByRole("heading", { name: / inventory$/i })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Add an item" })).toBeVisible();

    await page.getByRole("button", { name: "Losses" }).click();
    await expect(page.getByRole("heading", { name: "Waste & stockouts" })).toBeVisible();

    await page.getByRole("button", { name: "Settings" }).click();
    await expect(page.getByRole("heading", { name: "Restaurant settings" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Team access" })).toBeVisible();
  });
});
