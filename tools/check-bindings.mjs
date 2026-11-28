import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("../", import.meta.url));
const bindings = join(repo, "src", "bindings");
const generated = mkdtempSync(join(tmpdir(), "ddreams-bindings-"));

function run(command, args, env = process.env) {
  const result = spawnSync(command, args, { cwd: repo, env, stdio: "inherit" });
  if (result.error) throw result.error;
  return result.status ?? 1;
}

try {
  process.exitCode = run(
    "cargo",
    ["test", "--offline", "--manifest-path", "src-tauri/Cargo.toml", "export_bindings_"],
    {
      ...process.env,
      TS_RS_EXPORT_DIR: generated,
    },
  );
  if (process.exitCode === 0) {
    const filesIn = (directory) => new Set(readdirSync(directory).filter((name) => name.endsWith(".ts")));
    const readBinding = (directory, name) => readFileSync(join(directory, name), "utf8").replaceAll("\r\n", "\n");
    const expected = filesIn(generated);
    const actual = filesIn(bindings);
    for (const name of new Set([...expected, ...actual])) {
      if (expected.has(name) && actual.has(name) && readBinding(generated, name) === readBinding(bindings, name))
        continue;
      console.error(`Binding drift: src/bindings/${name}`);
      process.exitCode = 1;
    }
    process.exitCode = run("git", ["diff", "--exit-code", "--", "src/bindings"]) || process.exitCode;
  }
} finally {
  rmSync(generated, { recursive: true, force: true });
}
