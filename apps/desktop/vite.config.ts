import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  // Tauri следит за выводом сам; фиксированный порт обязан совпадать с devUrl.
  clearScreen: false,
  server: {
    // 1422: соседи держат два порта ниже — star-trails 1420, video-editing-tool
    // 1421. Все три окна открыты одновременно достаточно часто, чтобы
    // столкновение выглядело поломкой того, что запустили вторым.
    port: 1422,
    strictPort: true,
    watch: {
      // Не следить за rust-частью: cargo пишет в target/ во время сборки,
      // и вотчер падает с EBUSY на занятых файлах.
      ignored: ["**/src-tauri/**"],
    },
  },
});
