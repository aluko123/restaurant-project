import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { InventoryWorkspace } from "./InventoryWorkspace";
import type { ApiRequest } from "./SalesWorkspace";

type Route = {
  method: string;
  path: string | RegExp;
  respond: (path: string, init?: RequestInit) => unknown;
};

function makeRequest(routes: Route[]) {
  return vi.fn(async <T,>(path: string, init?: RequestInit): Promise<T> => {
    const method = init?.method ?? "GET";
    const match = routes.find(
      (route) =>
        route.method === method &&
        (typeof route.path === "string" ? route.path === path : route.path.test(path)),
    );
    if (!match) throw new Error(`Unexpected request ${method} ${path}`);
    return match.respond(path, init) as T;
  });
}

const item = {
  id: "item-1",
  name: "Chicken Thighs",
  category: "Protein",
  countUnit: "lb",
  parLevel: "10",
  active: true,
  storageAreaId: null,
  storageAreaName: null,
  shelfOrder: 0,
  preferredSupplierId: null,
  preferredSupplierName: null,
  latestQuantity: null,
  previousQuantity: null,
  change: null,
  lastCountedAt: null,
  lowStock: false,
};

const startedCount = {
  id: "count-1",
  status: "draft",
  scope: "all",
  storageAreaIds: [],
  revision: 0,
  createdAt: "2026-08-21T09:00:00Z",
  updatedAt: "2026-08-21T09:00:00Z",
  completedAt: null,
  entries: [
    {
      id: "entry-1",
      inventoryItemId: "item-1",
      name: "Chicken Thighs",
      category: "Protein",
      countUnit: "lb",
      storageAreaName: null,
      storageAreaSort: 0,
      shelfOrder: 0,
      previousQuantity: null,
      quantity: null,
      skipped: false,
    },
  ],
};

const completedCount = {
  ...startedCount,
  status: "completed",
  revision: 2,
  completedAt: "2026-08-21T10:00:00Z",
  entries: [{ ...startedCount.entries[0], quantity: "6" }],
};

const guide = {
  id: "guide-1",
  sourceCountId: "count-1",
  status: "draft",
  revision: 0,
  createdAt: "2026-08-21T10:00:01Z",
  updatedAt: "2026-08-21T10:00:01Z",
  orderedAt: null,
  receivedAt: null,
  cancelledAt: null,
  linkedInvoiceId: null,
  linkedInvoiceSupplierName: null,
  linkedInvoiceDate: null,
  lines: [],
};

const countSummary = [
  {
    id: "count-1",
    status: "completed",
    scope: "all",
    revision: 2,
    createdAt: "2026-08-21T09:00:00Z",
    updatedAt: "2026-08-21T10:00:00Z",
    completedAt: "2026-08-21T10:00:00Z",
    entryCount: 1,
    countedCount: 1,
    skippedCount: 0,
    areaNames: null,
  },
];

function overviewRoutes(overrides: Route[] = []): Route[] {
  return [
    { method: "GET", path: "/v1/inventory-items", respond: () => [item] },
    { method: "GET", path: "/v1/inventory-counts/draft", respond: () => ({ count: null }) },
    { method: "GET", path: "/v1/order-guides/open", respond: () => null },
    { method: "GET", path: "/v1/storage-areas", respond: () => [] },
    { method: "GET", path: "/v1/suppliers", respond: () => [] },
    ...overrides,
  ];
}

async function renderOverview(request: ReturnType<typeof makeRequest>) {
  render(
    <InventoryWorkspace
      restaurant={{ role: "owner" }}
      request={request as unknown as ApiRequest}
    />,
  );
  expect(await screen.findByText("Chicken Thighs")).toBeInTheDocument();
}

describe("InventoryWorkspace count flow", () => {
  it("starts a count, saves the draft, completes it, and opens past counts", async () => {
    const request = makeRequest([
      ...overviewRoutes(),
      { method: "POST", path: "/v1/inventory-counts", respond: () => startedCount },
      {
        method: "PUT",
        path: /\/v1\/inventory-counts\/count-1$/,
        respond: (_path, init) => {
          const body = JSON.parse(String(init?.body ?? "{}")) as {
            entries?: { id: string; quantity: string | null; skipped: boolean }[];
          };
          return {
            ...startedCount,
            revision: 1,
            entries: startedCount.entries.map((entry) => {
              const submitted = body.entries?.find((x) => x.id === entry.id);
              return submitted
                ? { ...entry, quantity: submitted.quantity, skipped: submitted.skipped }
                : entry;
            }),
          };
        },
      },
      {
        method: "POST",
        path: "/v1/inventory-counts/count-1/complete",
        respond: (_path, init) => {
          const body = JSON.parse(String(init?.body ?? "{}")) as { revision: number };
          expect(body.revision).toBe(1);
          return completedCount;
        },
      },
      { method: "POST", path: "/v1/order-guides", respond: () => guide },
      {
        method: "GET",
        path: /\/v1\/inventory-counts\?/,
        respond: () => countSummary,
      },
    ]);

    await renderOverview(request);

    fireEvent.click(screen.getByRole("button", { name: "Start count" }));
    expect(
      await screen.findByRole("heading", { name: "Record a physical count" }),
    ).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Chicken Thighs, quantity in lb"), {
      target: { value: "6" },
    });

    fireEvent.click(screen.getByRole("button", { name: "Save draft" }));
    expect(await screen.findByText("Draft saved.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Review count" }));
    fireEvent.click(await screen.findByRole("button", { name: "Complete count" }));

    expect(await screen.findByText(/Inventory count completed/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Past counts" }));
    expect(await screen.findByRole("heading", { name: "Past counts" })).toBeInTheDocument();
    expect(await screen.findByText(/1 counted/)).toBeInTheDocument();
  });

  it("re-enables actions after a failed draft save instead of wedging busy state", async () => {
    const request = makeRequest([
      ...overviewRoutes(),
      { method: "POST", path: "/v1/inventory-counts", respond: () => startedCount },
      {
        method: "PUT",
        path: /\/v1\/inventory-counts\/count-1$/,
        respond: () => {
          throw new Error("This count changed on another device. Reload and try again.");
        },
      },
    ]);

    await renderOverview(request);

    fireEvent.click(screen.getByRole("button", { name: "Start count" }));
    expect(
      await screen.findByRole("heading", { name: "Record a physical count" }),
    ).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Chicken Thighs, quantity in lb"), {
      target: { value: "6" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save draft" }));

    expect(await screen.findByText(/changed on another device/)).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save draft" })).toBeEnabled();
    });
  });
});
