import { build } from "esbuild";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const temporary = await mkdtemp(join(tmpdir(), "coucou-telegram-tests-"));

try {
  const fixture = join(temporary, "inline.mjs");
  await build({
    stdin: {
      contents: `
        export * from "./src/telegram/inline";
        export * from "./src/telegram/notifications";
        export { State } from "./src/core/state";
        export { Telegram } from "./src/telegram/api";
      `,
      resolveDir: root,
      loader: "ts",
    },
    outfile: fixture,
    bundle: true,
    format: "esm",
    platform: "node",
    logLevel: "silent",
  });
  const result = spawnSync(process.execPath, ["--test",
    join(root, "tests/telegram-inline.test.mjs"),
    join(root, "tests/telegram-notifications.test.mjs"),
  ], {
    stdio: "inherit",
    env: { ...process.env, TELEGRAM_TEST_FIXTURE: fixture },
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} finally {
  await rm(temporary, { recursive: true, force: true });
}
