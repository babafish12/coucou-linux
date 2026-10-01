import { build } from "esbuild";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm, readdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const temporary = await mkdtemp(join(tmpdir(), "coucou-island-tests-"));

try {
  const fixture = join(temporary, "island.mjs");
  await build({
    stdin: {
      contents: `
        export { Island } from "./src/island/island";
        export { IslandStateMachine } from "./src/island/fsm";
        export { handleHook } from "./src/island/hooks";
        export { BotEngine } from "./src/mochi/engine";
        export * from "./src/mochi/minibots";
        export { State } from "./src/core/state";
        export { Bridge } from "./src/core/bridge";
        export { Sound } from "./src/core/sound";
        export { UploadSeq } from "./src/upload/sequence";
        export { TelegramNotifications } from "./src/telegram/notifications";
      `,
      resolveDir: root,
      loader: "ts",
    },
    outfile: fixture,
    bundle: true,
    format: "esm",
    platform: "node",
    loader: { ".css": "empty" },
    logLevel: "silent",
  });
  const tests = (await readdir(join(root, "tests")))
    .filter((name) => /^(island-|mochi-performance|minibots-performance).*\.test\.mjs$/.test(name))
    .sort().map((name) => join(root, "tests", name));
  const result = spawnSync(process.execPath, ["--test", ...tests], {
    stdio: "inherit",
    env: { ...process.env, ISLAND_TEST_FIXTURE: fixture },
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} finally {
  await rm(temporary, { recursive: true, force: true });
}
