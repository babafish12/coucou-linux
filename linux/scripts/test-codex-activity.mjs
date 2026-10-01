import { build } from "esbuild";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const temporary = await mkdtemp(join(tmpdir(), "coucou-codex-tests-"));
try {
  const fixture = join(temporary, "codex.mjs");
  await build({
    stdin: {
      contents: `
        export * from "./src/codex/activity";
        export { buildCodex } from "./src/views/codex";
        export { State } from "./src/core/state";
      `,
      resolveDir: root, loader: "ts",
    },
    outfile: fixture, bundle: true, format: "esm", platform: "node",
    loader: { ".css": "empty" }, logLevel: "silent",
  });
  const result = spawnSync(process.execPath, ["--test", join(root, "tests/codex-activity.test.mjs")], {
    stdio: "inherit", env: { ...process.env, CODEX_ACTIVITY_TEST_FIXTURE: fixture },
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} finally {
  await rm(temporary, { recursive: true, force: true });
}
