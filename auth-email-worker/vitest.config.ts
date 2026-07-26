import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

export default defineConfig({
  resolve: {
    alias: {
      "cloudflare:email": fileURLToPath(
        new URL("./test/email-mock.ts", import.meta.url).href,
      ),
      "cloudflare:workers": fileURLToPath(
        new URL("./test/workers-mock.ts", import.meta.url).href,
      ),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
  },
});
