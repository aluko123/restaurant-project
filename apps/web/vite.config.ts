import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { VitePWA } from "vite-plugin-pwa";

export default defineConfig({
  plugins: [
    react(),
    VitePWA({
      registerType: "autoUpdate",
      manifest: {
        name: "Restaurant Daily Profit Copilot",
        short_name: "Restaurant Copilot",
        description: "Daily purchasing, prep, and profit actions for independent restaurants.",
        theme_color: "#173f35",
        background_color: "#f7f4ed",
        display: "standalone",
        start_url: "/",
      },
    }),
  ],
});
