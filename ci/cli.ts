import { readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { nodeExec, type Command } from "@dedalus-labs/hollywood";

const hollywood = (args: readonly string[]): Command => ({
  file: "node",
  args: ["node_modules/@dedalus-labs/hollywood/dist/cli.js", ...args],
});
const generate = hollywood([
  "generate",
  "ci/ci.ts",
  "ci/metadata.ts",
  "ci/vouch.ts",
  "ci/native.ts",
  "ci/vouch-actions.ts",
  "--source-root",
  "ci",
]);
const bundle = hollywood(["build", "--actions-dir", ".github/actions/ci"]);

async function run(commands: readonly Command[]): Promise<void> {
  for (const { file, args } of commands) {
    const result = await nodeExec(file, args);
    process.stdout.write(result.stdout);
    process.stderr.write(result.stderr);
  }
}

export async function main(command: string | undefined): Promise<void> {
  switch (command) {
    case "generate":
      await run([generate, bundle]);
      return;
    case "check": {
      const tests = (await readdir("ci"))
        .filter((file) => file.endsWith(".test.ts"))
        .map((file) => `ci/${file}`);
      await run([
        { file: "node", args: ["node_modules/typescript/bin/tsc", "--noEmit"] },
        { file: "node", args: ["--test", ...tests] },
        generate,
        bundle,
        {
          file: "git",
          args: ["diff", "--exit-code", "--", ".github/workflows", ".github/actions/ci"],
        },
      ]);
      const untracked = await nodeExec("git", [
        "ls-files",
        "--others",
        "--exclude-standard",
        "--",
        ".github/workflows",
        ".github/actions/ci",
      ]);
      if (untracked.stdout.length > 0)
        throw new Error(`Generated files must be committed:\n${untracked.stdout}`);
      return;
    }
    default:
      throw new Error("Usage: npm run ci <generate|check>");
  }
}
if (process.argv[1] === fileURLToPath(import.meta.url)) await main(process.argv[2]);
