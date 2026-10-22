import { createServer } from "vite";

const server = await createServer({
  server: { middlewareMode: true },
  appType: "custom",
  logLevel: "error",
});

try {
  const mod = await server.ssrLoadModule("/src/lib/viewport.test.ts");
  await mod.runViewportTests();
  console.log("viewport tests passed");
} finally {
  await server.close();
}
