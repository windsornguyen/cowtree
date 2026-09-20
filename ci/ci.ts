import { always, defineMatrix, expr, job, workflow } from "@dedalus-labs/hollywood";
import { readFileSync } from "node:fs";
import { checkout, run, setupNode, uv } from "./steps.ts";
import { nativeFilesystems } from "./native.ts";
import { uses } from "@dedalus-labs/hollywood";

const python = defineMatrix({
  os: ["ubuntu-latest", "macos-latest"],
  "python-version": ["3.10", "3.11", "3.12", "3.13", "3.14"],
});
const setupUv = { uses: uv.uses } as const;
const atlasRevision = readFileSync("tools/atlas-revision.txt", "utf8").trim();
if (!/^[a-f0-9]{40}$/.test(atlasRevision)) throw new Error("Atlas revision must be a full Git commit");
const atlasPath = expr<string>("format('{0}/.tools/atlas', github.workspace)");

export const ci = workflow(
  {
    name: "CI",
    on: { push: { branches: ["main"] }, pull_request: {} },
    permissions: { contents: "read" },
    jobs: {
      workflows: job({
        name: "Generated workflows",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 10,
        steps: [
          checkout,
          setupNode,
          run("Install workflow dependencies", "npm", ["ci", "--ignore-scripts"]),
          run("Check typed workflows and generated files", "npm", ["run", "ci", "check"]),
        ],
      }),
      schema: job({
        name: "Declarative schema",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 15,
        env: {
          COWTREE_ATLAS: atlasPath,
          GOTOOLCHAIN: "local",
          ATLAS_NO_UPDATE_NOTIFIER: "1",
          ATLAS_NO_UPGRADE_SUGGESTIONS: "1",
        },
        steps: [
          checkout,
          {
            ...checkout,
            name: "Check out Atlas Community source",
            with: {
              repository: "ariga/atlas",
              ref: atlasRevision,
              path: ".tools/atlas-source",
              "persist-credentials": false,
            },
          },
          {
            uses: "actions/setup-go@924ae3a1cded613372ab5595356fb5720e22ba16",
            with: {
              "go-version": "1.26.5",
              "cache-dependency-path": ".tools/atlas-source/**/go.sum",
            },
          },
          {
            ...run("Build Atlas Community", "go", [
              "build", "-mod=readonly", "-trimpath", "-o",
              atlasPath, ".",
            ]),
            "working-directory": ".tools/atlas-source/cmd/atlas",
          },
          uv,
          run("Install Python test dependencies", "uv", ["sync", "--locked", "--group", "test"]),
          run("Check generated SQLite schema", "uv", [
            "run", "python", "scripts/schema.py", "check", "--atlas", ".tools/atlas",
          ]),
          run("Test declarative schema generation", "uv", [
            "run", "pytest", "-q", "tests/test_schema_generation.py",
          ]),
        ],
      }),
      lint: job({
        name: "Lint",
        "runs-on": "ubuntu-latest",
        steps: [
          checkout,
          uv,
          run("Install Python", "uv", ["python", "install", "3.10"]),
          run("Install dependencies", "uv", [
            "sync",
            "--locked",
            "--group",
            "lint",
            "--group",
            "bench",
          ]),
          run("Ruff lint", "uv", ["run", "ruff", "check", "."]),
          run("Ruff format", "uv", ["run", "ruff", "format", "--check", "."]),
          run("Ty typecheck", "uv", ["run", "ty", "check"]),
        ],
      }),
      test: job({
        name: "Test",
        "runs-on": python.os,
        // Override the local development pin for both sync and run.
        env: { UV_PYTHON: python["python-version"] },
        strategy: { "fail-fast": false, matrix: python },
        steps: [
          checkout,
          uv,
          run("Install Python", "uv", ["python", "install", python["python-version"]]),
          run("Install dependencies", "uv", ["sync", "--locked", "--group", "test"]),
          run("Verify selected Python", "uv", [
            "run", "python", "-c",
            "import os, sys; actual = f'{sys.version_info.major}.{sys.version_info.minor}'; print(sys.version); assert actual == os.environ['UV_PYTHON'], actual",
          ]),
          run("Run tests", "uv", ["run", "pytest"]),
        ],
      }),
      build: job({
        name: "Build",
        "runs-on": "ubuntu-latest",
        steps: [
          checkout,
          uv,
          run("Install Python", "uv", ["python", "install", "3.10"]),
          run("Build package", "uv", ["build"]),
        ],
      }),
      filesystems: job({
        name: "Native filesystem matrix",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 15,
        steps: [checkout, setupUv, uses(nativeFilesystems, { with: {} })],
      }),
      windows: job({
        name: "Windows ReFS native cloning",
        "runs-on": "windows-2025",
        "timeout-minutes": 15,
        steps: [
          checkout,
          setupUv,
          run("Install Python", "uv", ["python", "install", "3.10"]),
          run("Install test dependencies", "uv", ["sync", "--locked", "--no-default-groups", "--group", "test"]),
          run("Check ReFS isolation and NTFS refusal", "pwsh", [
            "-NoProfile", "-File", "scripts/test_windows.ps1",
          ]),
        ],
      }),
      "windows-arm": job({
        name: "Windows ARM64 Dev Drive cloning",
        "runs-on": "windows-11-arm",
        "timeout-minutes": 15,
        env: { UV_PYTHON: "3.14" },
        steps: [
          checkout,
          setupUv,
          run("Install Python", "uv", ["python", "install", "3.14"]),
          run("Install test dependencies", "uv", ["sync", "--locked", "--no-default-groups", "--group", "test"]),
          run("Check Dev Drive isolation and NTFS refusal", "pwsh", [
            "-NoProfile", "-File", "scripts/test_windows.ps1", "-DevDrive",
          ]),
        ],
      }),
      specification: job({
        name: "Workspace protocol model",
        "runs-on": "ubuntu-24.04",
        "timeout-minutes": 15,
        steps: [
          checkout,
          setupUv,
          {
            uses: "actions/setup-java@de7274f081f381c8f8158605e0321c36c376e2e6",
            with: { distribution: "temurin", "java-version": "17" },
          },
          run("Check safety, witnesses, and fencing mutation", "uv", [
            "run",
            "--locked",
            "--no-default-groups",
            "python",
            "scripts/check_specs.py",
            "--cache",
            expr<string>("format('{0}/cowtree-tla', runner.temp)"),
          ]),
          {
            name: "Save checker evidence",
            if: always(),
            uses: "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            with: {
              name: "workspace-protocol-evidence",
              "if-no-files-found": "error",
              path: ["java-version.txt", "*/tlc.log", "*/counterexample.json", "*/*.tla", "*/*.cfg"]
                .map((path) => `\u0024{{ runner.temp }}/cowtree-tla/${path}`)
                .join("\n"),
            },
          },
        ],
      }),
    },
  },
  { filename: "ci.yml" },
);
