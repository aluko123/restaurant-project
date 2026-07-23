import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { VitePWA } from "vite-plugin-pwa";

export default defineConfig({
  plugins: [
    react(),
    VitePWA({
      registerType: "autoUpdate",
      manifest: {
        name: "Parline — Restaurant Operations",
        short_name: "Parline",
        description: "Know what changed and protect the next shift with source-backed restaurant operations insights.",
        theme_color: "#173f35",
        background_color: "#f7f4ed",
        display: "standalone",
        start_url: "/",
      },
    }),
  ],
});
