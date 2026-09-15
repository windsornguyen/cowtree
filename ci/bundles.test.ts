import assert from "node:assert/strict";
import { copyFile, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

type ActionResult = Readonly<{
  status: number | null;
  stdout: string;
  stderr: string;
  output: string;
}>;

async function invoke(
  action: string,
  input: Readonly<Record<string, string>>,
): Promise<ActionResult> {
  const directory = await mkdtemp(join(tmpdir(), "cowtree-action-"));
  try {
    const module = join(directory, "action.mjs");
    const output = join(directory, "output");
    await writeFile(output, "");
    await copyFile(`.github/actions/ci/${action}/dist/index.js`, module);
    const result = spawnSync(process.execPath, [module], {
      cwd: directory,
      encoding: "utf8",
      timeout: 10_000,
      env: { PATH: process.env.PATH, GITHUB_OUTPUT: output, ...input },
    });
    if (result.error) throw result.error;
    const contents =
      action === "resolve-vouch-pr" && result.status === 0 ? await readFile(output, "utf8") : "";
    return {
      status: result.status,
      stdout: result.stdout,
      stderr: result.stderr,
      output: contents,
    };
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test("bundled eligibility action runs without source files or node_modules", async () => {
  const accepted = await invoke("require-vouch", { INPUT_STATUS: "vouched" });
  assert.equal(accepted.status, 0, accepted.stderr);
  assert.match(accepted.stdout, /Vouch result: vouched/);
  const rejected = await invoke("require-vouch", { INPUT_STATUS: "unvouched" });
  assert.equal(rejected.status, 1);
  assert.match(rejected.stdout + rejected.stderr, /Vouch denied contributor access/);
});

test("bundled resolver reads generated input names and writes the PR output", async () => {
  const result = await invoke("resolve-vouch-pr", {
    INPUT_EVENT: "pull_request_target",
    "INPUT_EVENT-PR": "7",
    INPUT_REPOSITORY: "windsornguyen/cowtree",
    INPUT_SHA: "a".repeat(40),
  });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.output, /number<<[^\n]+\n7\n/);
});
